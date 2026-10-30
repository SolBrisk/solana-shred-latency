# Validation

Checked on 2026-10-30 with Rust/Cargo 1.93.1 on Linux x86_64.

- 9 automated tests passed, including the CLI integration tests in `tests/`.
- `cargo fmt --check` passed.
- `cargo clippy --locked --all-targets -- -D warnings` passed.
- `cargo build --release --locked` passed.
- Release CLI `--help` and `--version` passed.

Dependency downloads were disabled locally with `--offline`; the committed lockfile
was honored. Tests use synthetic fixtures and local ephemeral sockets. No live
paid endpoint or production token was used. The declared minimum Rust 1.89 and
other operating systems were not tested with their own toolchains here.

Read the README and format/methodology documentation for supported inputs and
measurement/trust boundaries. Connection examples: https://solbrisk.com/docs.
