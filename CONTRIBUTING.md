# Contributing

Thanks for helping. Start with [docs/development.md](docs/development.md): it
covers the code layout, running the tests and debugging on a tablet over Wi-Fi.

Before opening a pull request:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Two rules for the code:

- **Nothing hardcoded to one machine.** No fixed GPU, encoder, resolution or
  tablet model. Detect at runtime and let the user override.
- **Decisions in `core`, I/O in `host`.** Logic that decides something takes
  plain values and lives in `tabextend-core` with unit tests.

If you test on a distribution other than Fedora, please report the package
names that worked so [docs/setup.md](docs/setup.md) can be corrected.
