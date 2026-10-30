# Solana Shred Latency Compare

[简体中文](README.zh-CN.md) · [SolBrisk](https://solbrisk.com/) · [UDP connection guide](https://solbrisk.com/docs)

Compare matched native Solana shreds from **2–8 UDP feeds on one receiver**.
On Linux, timestamps come from **SO_TIMESTAMPNS software ingress**, reducing
receiver-thread scheduling bias. Reports show signed arrival differences,
first-arrival shares, p50/p95/p99, coverage and local queue/drop diagnostics.

Built by [SolBrisk](https://solbrisk.com/), with Raw Shred delivery from Frankfurt
and Amsterdam. Get a subscription at [solbrisk.com](https://solbrisk.com/) and
follow the [connection guide](https://solbrisk.com/docs) to configure your UDP
destinations. Other compatible native feeds work without changing the matcher.
No provider receives special treatment or an artificial time adjustment.

## Build

Rust 1.89+ and a C toolchain are required. Linux is recommended for kernel
timestamps and overflow counters. On other platforms the tool uses userspace
monotonic timestamps; that measurement includes socket queue/scheduling delays.

```bash
cargo build --release --locked
cp examples/sources.json sources.local.json
```

## Configure destinations

```json
{
  "sources": [
    {"name":"solbrisk-fra","bind":"0.0.0.0:21001"},
    {"name":"other-provider","bind":"0.0.0.0:21002"}
  ]
}
```

Configure each provider to deliver to your receiver's **public IP** and its own
port: `21001` or `21002` in this example. `0.0.0.0` is a local bind address, not
a subscription destination. Allow those UDP ports in your firewall. Optional
`"source_ip":"198.51.100.10"` restricts an input to a known sender; replace that
documentation address with a provider-confirmed IP or omit the field.

The program exclusively binds all ports before workers start. It never enables
SO_REUSEPORT, edits forwarding rules, modifies sysctls or stops existing services.
An occupied port is an error. Run on unused diagnostic ports if trading receivers
already occupy your normal ports. The examples do not require root.

SolBrisk verification packets print their code to stderr and are excluded from
measurement. You can verify destinations while it runs; then start a fresh timed
run after all feeds are active. Configuration files contain no credentials.

## Run

```bash
./target/release/solana-shred-latency \
  --sources sources.local.json --warmup 5 --duration 60 --output report.json
```

The first source is the baseline. Measurement starts after warmup and continues
for 60 seconds, followed by the default five-second matching drain. New keys are
admitted only during measurement; the drain accepts counterparts of admitted
keys. One JSON report is printed at completion. Logs go to stderr, and existing
output files are never overwritten. Repeat runs or swap the source order to
assess variation.

Data shreds are compared by default; `--include-coding` also includes coding
shreds. `--userspace-timestamps` explicitly selects application receive timing,
which must not be presented as the same methodology as kernel ingress timing.

## Read the report

`arrival_difference_us = candidate arrival - baseline arrival`:

- Positive: baseline arrived earlier. Negative: candidate arrived earlier.
- `baseline_first_percent` and `candidate_first_percent` are matched-event shares.
- Signed p50/p95/p99 describe paired differences, not absolute leader-to-receiver
  latency. `quantiles_sampled` says whether reservoir sampling was used.
- `*_without_match` is unmatched coverage, **not a verified network loss rate**.
- Linux `kernel_drop_count` is the most recent SO_RXQ_OVFL counter received on
  that socket, not a network loss count. Drops after the last received packet
  may not be observed. `null` means unavailable on the selected timing platform.
- Application queue drops, matching-capacity eviction, missing kernel stamps,
  receive errors or >5ms clock divergence mark `clean_run=false`. Missing matched
  samples and interruption do too. A nonzero exit code preserves the report.
- `clean_run=true` only means these local checks passed; it does not guarantee
  both sources delivered the full expected Solana stream or authenticate shreds.

See [METHODOLOGY.md](METHODOLOGY.md) before publishing comparisons. Keep receiver
location, source settings, duration, sample count and diagnostic fields alongside
the result. No example in this repository claims actual provider performance.

## Tuning and development

`--receive-buffer-mib` asks for socket memory; the OS may clamp it. The report
shows effective values. `--queue-capacity`, `--max-keys`, `--match-window` and
`--quantile-samples` control local capacity. There is no rate throttle or external
RPC polling. High queue lag or drop counters indicate a receiver-limited run.

```bash
cargo test --locked
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
```

Tests cover kernel timestamps on loopback, duplicate-aware matching, signed
differences, out-of-window events and bounded memory. MIT license;
[THIRD-PARTY.md](THIRD-PARTY.md). Questions:
[SolBrisk docs](https://solbrisk.com/docs) · support@solbrisk.com.

Validation details: [VALIDATION.md](VALIDATION.md). Contribution guide: [CONTRIBUTING.md](CONTRIBUTING.md).
