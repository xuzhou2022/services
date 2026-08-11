# services

A Cargo workspace for backend services.

## Status

Early scaffold. The workspace builds and tests clean, but there is no real
service logic yet — `api` is a runnable stub.

## Layout

```
Cargo.toml        # virtual workspace manifest
crates/
  common/         # shared types and helpers (library)
  api/            # entry-point service (binary)
rustfmt.toml
```

Shared settings (version, edition, license) live in `[workspace.package]`;
crates inherit them with `field.workspace = true`. Internal crates are wired
through `[workspace.dependencies]`, so `common.workspace = true` is all a
consumer needs.

## Getting started

Requires Rust 1.86 or newer (edition 2024).

```sh
cargo run -p api     # api v0.1.0 starting
cargo test           # run all workspace tests
cargo fmt --all
cargo clippy --all-targets -- -D warnings
```

## Adding a service

Create `crates/<name>/` with a `Cargo.toml` that inherits the workspace
fields. The `members = ["crates/*"]` glob picks it up automatically.
