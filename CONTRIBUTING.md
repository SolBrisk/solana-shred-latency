# Contributing

Use Rust 1.89+ and keep the committed `Cargo.lock`. Before opening a pull request:

```bash
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --release --locked
```

Describe the concrete problem, resulting behavior and validation. Keep changes
focused. Socket tests use ephemeral loopback ports; they do not require a paid
subscription, access to a production server or firewall/service changes.

For parser or matcher changes, include a small synthetic regression case. Do not
commit tokens, private keys, production configuration, customer captures or real
account logs. Use public, redistributable fixtures only, with their provenance.
Do not upload credentials or private reports to public Issues. Report security
problems privately to support@solbrisk.com; include reproduction steps and omit
secrets. Subscription support: https://solbrisk.com/docs.

Contributions are accepted under this repository's MIT license. Dependencies
retain their own licenses; see THIRD-PARTY.md.
