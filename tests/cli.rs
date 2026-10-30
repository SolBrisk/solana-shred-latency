use std::{
    net::UdpSocket,
    process::Command,
    thread,
    time::{Duration, Instant},
};

fn packet(index: u32) -> Vec<u8> {
    let mut bytes = vec![0; 1203];
    bytes[..64].fill(7);
    bytes[64] = 0x96;
    bytes[65..73].copy_from_slice(&42u64.to_le_bytes());
    bytes[73..77].copy_from_slice(&index.to_le_bytes());
    bytes[77..79].copy_from_slice(&1u16.to_le_bytes());
    bytes[79..83].copy_from_slice(&(index / 32 * 32).to_le_bytes());
    bytes[83..85].copy_from_slice(&1u16.to_le_bytes());
    bytes[86..88].copy_from_slice(&89u16.to_le_bytes());
    bytes[88] = 1;
    bytes
}

#[test]
fn udp_cli_matches_delayed_copies_and_rejects_occupied_ports() {
    let a = UdpSocket::bind("127.0.0.1:0").unwrap();
    let b = UdpSocket::bind("127.0.0.1:0").unwrap();
    let address_a = a.local_addr().unwrap();
    let address_b = b.local_addr().unwrap();
    let directory = std::env::temp_dir().join(format!(
        "solbrisk-shred-latency-fixture-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let config = directory.join("sources.json");
    std::fs::write(
        &config,
        serde_json::to_vec(&serde_json::json!({"sources":[
            {"name":"baseline","bind":address_a.to_string()},
            {"name":"candidate","bind":address_b.to_string()}
        ]}))
        .unwrap(),
    )
    .unwrap();
    let run = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_solana-shred-latency"));
        command.arg("--sources").arg(&config).args([
            "--duration",
            "2",
            "--warmup",
            "1",
            "--match-window",
            "1",
        ]);
        command
    };
    let occupied = run().output().unwrap();
    assert!(!occupied.status.success());
    assert!(String::from_utf8_lossy(&occupied.stderr).contains("Cannot exclusively bind"));
    drop((a, b));
    let child = run()
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let sender = thread::spawn(move || {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let start = Instant::now() + Duration::from_millis(200);
        for index in 0..180u32 {
            let deadline = start + Duration::from_millis(index as u64 * 25);
            thread::sleep(deadline.saturating_duration_since(Instant::now()));
            let bytes = packet(index);
            socket.send_to(&bytes, address_a).unwrap();
            thread::sleep(Duration::from_millis(4));
            // A changed relay proof must still match the same application data.
            let mut alternate = bytes;
            alternate[1100] ^= 1;
            socket.send_to(&alternate, address_b).unwrap();
        }
    });
    let output = child.wait_with_output().unwrap();
    sender.join().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["clean_run"], true);
    assert!(report["comparisons"][0]["matched"].as_u64().unwrap() > 20);
    assert!(
        report["comparisons"][0]["arrival_difference_us"]["p50"]
            .as_f64()
            .unwrap()
            > 0.0
    );
    assert_eq!(report["leader_signatures_verified"], false);
    std::fs::remove_dir_all(directory).unwrap();
}
