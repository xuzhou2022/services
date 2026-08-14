# services

[![CI](https://github.com/xuzhou2022/services/actions/workflows/ci.yml/badge.svg)](https://github.com/xuzhou2022/services/actions/workflows/ci.yml)

A Cargo workspace for backend services.

## Status

Early. `api` serves a single `/health` route over axum; there is no domain
logic, persistence, or auth yet.

## Layout

```
Cargo.toml        # virtual workspace manifest
crates/
  common/         # shared types and helpers (library)
  api/            # entry-point service (lib + binary)
    src/lib.rs    # Config and router — where routes are added
    src/main.rs   # tracing setup, bind, graceful shutdown
    tests/        # route-level tests
rustfmt.toml
```

Shared settings (version, edition, license) live in `[workspace.package]`;
crates inherit them with `field.workspace = true`. Internal crates are wired
through `[workspace.dependencies]`, so `common.workspace = true` is all a
consumer needs.

## Getting started

Requires Rust 1.86 or newer (edition 2024).

```sh
cargo run -p api     # listens on 0.0.0.0:3000
cargo test           # run all workspace tests
cargo fmt --all
cargo clippy --all-targets -- -D warnings
```

```sh
curl localhost:3000/health
# {"status":"ok","name":"api","version":"0.1.0"}
```

## Configuration

| Variable | Default   | Notes                                          |
| -------- | --------- | ---------------------------------------------- |
| `HOST`   | `0.0.0.0` | IP address, not a hostname                     |
| `PORT`   | `3000`    | `0` binds an OS-assigned port                  |
| `RUST_LOG` | `info`  | Standard `tracing` env filter                  |

A variable that is set but unparseable is a startup error rather than a
silent fall back to the default. The service drains in-flight requests on
Ctrl-C or `SIGTERM`.

## CI

`.github/workflows/ci.yml` runs on every pull request and on pushes to
`main`, in three jobs: `fmt + clippy`, `test` (including doctests), and an
`msrv` build pinned to the `rust-version` declared in `Cargo.toml`. All of
them use `--locked`, so a stale `Cargo.lock` fails the build rather than
being silently updated.

## Adding a service

Create `crates/<name>/` with a `Cargo.toml` that inherits the workspace
fields. The `members = ["crates/*"]` glob picks it up automatically.
