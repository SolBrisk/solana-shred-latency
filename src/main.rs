use anyhow::{bail, Context, Result};
use clap::Parser;
use serde::Deserialize;
use serde_json::json;
use solana_shred_latency::{
    compare::Comparison,
    udp,
    wire::{self, Shred},
};
use std::{
    collections::HashSet,
    io::Write,
    net::{IpAddr, SocketAddr, UdpSocket},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, TrySendError},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Parser)]
#[command(
    version,
    about = "Compare matched raw Solana shreds using exclusive UDP receivers"
)]
struct Cli {
    /// JSON sources; each source needs its own local UDP port.
    #[arg(long)]
    sources: PathBuf,
    #[arg(long, default_value_t = 60)]
    duration: u64,
    #[arg(long, default_value_t = 5)]
    warmup: u64,
    #[arg(long, default_value_t = 5)]
    match_window: u64,
    #[arg(long, default_value_t = 500_000)]
    max_keys: usize,
    #[arg(long, default_value_t = 100_000)]
    quantile_samples: usize,
    #[arg(long, default_value_t = 65536)]
    queue_capacity: usize,
    #[arg(long, default_value_t = 32)]
    receive_buffer_mib: usize,
    /// Include coding shreds; default compares data shreds only.
    #[arg(long)]
    include_coding: bool,
    /// Use userspace timing instead of Linux kernel software ingress stamps.
    #[arg(long)]
    userspace_timestamps: bool,
    /// Create a report file; refuses to overwrite an existing file.
    #[arg(long)]
    output: Option<PathBuf>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    sources: Vec<Source>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    name: String,
    bind: SocketAddr,
    #[serde(default)]
    source_ip: Option<IpAddr>,
}
#[derive(Default)]
struct Counters {
    packets: AtomicU64,
    bytes: AtomicU64,
    invalid: AtomicU64,
    control: AtomicU64,
    ignored_senders: AtomicU64,
    coding_skipped: AtomicU64,
    queue_dropped: AtomicU64,
    receive_errors: AtomicU64,
    kernel_drops: AtomicU64,
    timestamp_missing: AtomicU64,
    clock_steps: AtomicU64,
}
struct Arrival {
    source: usize,
    key: [u8; 32],
    at: Instant,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if cli.duration == 0
        || cli.match_window == 0
        || cli.max_keys == 0
        || cli.quantile_samples == 0
        || cli.queue_capacity == 0
        || cli.receive_buffer_mib == 0
        || cli.receive_buffer_mib > 1024
    {
        bail!("Durations and capacities must be positive; receive-buffer-mib must be <=1024");
    }
    let config: Config =
        serde_json::from_slice(&std::fs::read(&cli.sources).context("Cannot read sources JSON")?)
            .context("Invalid sources JSON")?;
    if !(2..=8).contains(&config.sources.len()) {
        bail!("Configure between 2 and 8 sources");
    }
    let mut names = HashSet::new();
    let mut binds = HashSet::new();
    for source in &config.sources {
        if source.name.is_empty()
            || !names.insert(source.name.clone())
            || !binds.insert(source.bind)
        {
            bail!("Source names and bind addresses must be non-empty and unique");
        }
    }
    let mut file = cli
        .output
        .as_ref()
        .map(|p| {
            std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(p)
        })
        .transpose()
        .context("Cannot create report file")?;
    // Bind every socket before starting any worker. Never enable SO_REUSEPORT.
    let mut sockets = Vec::new();
    let mut buffers = Vec::new();
    let clock = udp::Clock::default();
    let kernel = cfg!(target_os = "linux") && !cli.userspace_timestamps;
    for source in &config.sources {
        let socket = socket2::Socket::new(
            socket2::Domain::for_address(source.bind),
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )?;
        socket.set_recv_buffer_size(cli.receive_buffer_mib * 1024 * 1024)?;
        socket.bind(&source.bind.into()).with_context(|| {
            format!(
                "Cannot exclusively bind {}; select a free UDP port",
                source.bind
            )
        })?;
        socket.set_read_timeout(Some(Duration::from_millis(100)))?;
        buffers.push(socket.recv_buffer_size()?);
        let socket = UdpSocket::from(socket);
        udp::enable(&socket, kernel)?;
        sockets.push(socket);
    }
    let stop = Arc::new(AtomicBool::new(false));
    let signal = stop.clone();
    ctrlc::set_handler(move || signal.store(true, Ordering::Relaxed))?;
    let counters = (0..sockets.len())
        .map(|_| Arc::new(Counters::default()))
        .collect::<Vec<_>>();
    let (tx, rx) = mpsc::sync_channel(cli.queue_capacity);
    let mut tasks = Vec::new();
    for (id, socket) in sockets.into_iter().enumerate() {
        let source_ip = config.sources[id].source_ip;
        let name = config.sources[id].name.clone();
        let stats = counters[id].clone();
        let stop = stop.clone();
        let tx = tx.clone();
        let include_coding = cli.include_coding;
        let clock = clock.clone();
        tasks.push(thread::spawn(move || {
            let mut packet = [0u8; 65535];
            while !stop.load(Ordering::Relaxed) {
                match udp::receive(&socket, &mut packet, &clock, kernel) {
                    Ok(datagram) => {
                        let udp::Datagram {
                            len,
                            peer,
                            at,
                            overflow,
                            timestamp_missing,
                        } = datagram;
                        if timestamp_missing {
                            stats.timestamp_missing.fetch_add(1, Ordering::Relaxed);
                        }
                        if let Some(drops) = overflow {
                            stats.kernel_drops.store(drops as u64, Ordering::Relaxed);
                        }
                        if kernel && clock.stepped() {
                            stats.clock_steps.fetch_add(1, Ordering::Relaxed);
                        }
                        if source_ip.is_some_and(|ip| ip != peer.ip()) {
                            stats.ignored_senders.fetch_add(1, Ordering::Relaxed);
                            continue;
                        }
                        stats.packets.fetch_add(1, Ordering::Relaxed);
                        stats.bytes.fetch_add(len as u64, Ordering::Relaxed);
                        if let Some(code) = wire::verification(&packet[..len]) {
                            stats.control.fetch_add(1, Ordering::Relaxed);
                            eprintln!("{name} endpoint verification code: {code}");
                            continue;
                        }
                        let shred = match Shred::parse(&packet[..len]) {
                            Ok(s) => s,
                            Err(_) => {
                                stats.invalid.fetch_add(1, Ordering::Relaxed);
                                continue;
                            }
                        };
                        if shred.code && !include_coding {
                            stats.coding_skipped.fetch_add(1, Ordering::Relaxed);
                            continue;
                        }
                        match tx.try_send(Arrival {
                            source: id,
                            key: shred.identity(),
                            at,
                        }) {
                            Ok(()) => {}
                            Err(TrySendError::Full(_)) => {
                                stats.queue_dropped.fetch_add(1, Ordering::Relaxed);
                            }
                            Err(TrySendError::Disconnected(_)) => return,
                        }
                    }
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) => {}
                    Err(_) => {
                        stats.receive_errors.fetch_add(1, Ordering::Relaxed);
                        stop.store(true, Ordering::Relaxed);
                        break;
                    }
                }
            }
        }));
    }
    drop(tx);
    let started = Instant::now();
    let measurement_start = started + Duration::from_secs(cli.warmup);
    let measurement_end = measurement_start + Duration::from_secs(cli.duration);
    let drain_end = measurement_end + Duration::from_secs(cli.match_window);
    eprintln!(
        "All sockets ready; warmup={}s measurement={}s; first source is baseline",
        cli.warmup, cli.duration
    );
    let mut matcher = Comparison::new(
        config.sources.iter().map(|s| s.name.clone()).collect(),
        Duration::from_secs(cli.match_window),
        cli.max_keys,
        cli.quantile_samples,
    );
    let mut lag = 0u128;
    while !stop.load(Ordering::Relaxed) && Instant::now() < drain_end {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(event) => {
                lag = lag.max(
                    Instant::now()
                        .saturating_duration_since(event.at)
                        .as_micros(),
                );
                if event.at >= measurement_start
                    && (event.at <= measurement_end || matcher.contains(&event.key))
                {
                    matcher.observe(event.key, event.source, event.at);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let interrupted = stop.load(Ordering::Relaxed);
    stop.store(true, Ordering::Relaxed);
    for task in tasks {
        task.join()
            .map_err(|_| anyhow::anyhow!("Receiver thread panicked"))?;
    }
    let diagnostics=counters.iter().enumerate().map(|(i,s)|json!({"source":config.sources[i].name,"packets":s.packets.load(Ordering::Relaxed),"bytes":s.bytes.load(Ordering::Relaxed),"invalid_packets":s.invalid.load(Ordering::Relaxed),"verification_packets":s.control.load(Ordering::Relaxed),"ignored_sender_packets":s.ignored_senders.load(Ordering::Relaxed),"coding_packets_skipped":s.coding_skipped.load(Ordering::Relaxed),"application_queue_dropped":s.queue_dropped.load(Ordering::Relaxed),"receive_errors":s.receive_errors.load(Ordering::Relaxed),"effective_receive_buffer_bytes":buffers[i],"kernel_drop_count":if kernel{Some(s.kernel_drops.load(Ordering::Relaxed))}else{None},"missing_kernel_timestamps":s.timestamp_missing.load(Ordering::Relaxed),"clock_step_observations":s.clock_steps.load(Ordering::Relaxed)})).collect::<Vec<_>>();
    let mut report = matcher.report();
    let matched = report["comparisons"]
        .as_array()
        .unwrap()
        .iter()
        .all(|p| p["matched"].as_u64().unwrap_or(0) > 0);
    let clean = !interrupted
        && matched
        && matcher.capacity_evictions == 0
        && counters.iter().all(|s| {
            s.queue_dropped.load(Ordering::Relaxed) == 0
                && s.receive_errors.load(Ordering::Relaxed) == 0
                && s.kernel_drops.load(Ordering::Relaxed) == 0
                && s.timestamp_missing.load(Ordering::Relaxed) == 0
                && s.clock_steps.load(Ordering::Relaxed) == 0
        });
    report["tool"] = json!("solana-shred-latency");
    report["matching"]=json!("same leader signature, slot, index, type, version and application-bearing headers/payload; relay Merkle tail and repair nonce excluded");
    report["measurement_seconds_requested"] = json!(cli.duration);
    report["warmup_seconds"] = json!(cli.warmup);
    report["drain_seconds"] = json!(cli.match_window);
    report["socket_counters_include_warmup_and_drain"] = json!(true);
    report["receiver_diagnostics"] = json!(diagnostics);
    report["max_application_queue_lag_us"] = json!(lag);
    report["clean_run"] = json!(clean);
    report["interrupted"] = json!(interrupted);
    report["leader_signatures_verified"] = json!(false);
    report["clock"] = json!(if kernel {
        "Linux SO_TIMESTAMPNS software ingress, mapped from shared receiver CLOCK_REALTIME to Instant; >5ms clock steps invalidate run"
    } else {
        "userspace monotonic Instant immediately after recv_from; includes scheduling and socket queue delays"
    });
    let text = serde_json::to_string_pretty(&report)?;
    println!("{text}");
    if let Some(file) = file.as_mut() {
        writeln!(file, "{text}")?;
    }
    if !clean {
        bail!("No matched data or run has diagnostics; inspect report before interpreting speed");
    }
    Ok(())
}
