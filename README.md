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
cargo test --workspace --all-targets --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Those are the commands CI runs. `--locked` is the one that matters locally:
without it cargo quietly refreshes `Cargo.lock`, so a stale lockfile passes
here and fails in CI.

```sh
curl localhost:3000/health
# {"status":"ok","name":"api","version":"0.1.0"}
```

## Health endpoints

| Path            | Meaning                                              |
| --------------- | ---------------------------------------------------- |
| `/health`       | General status                                       |
| `/health/live`  | Liveness — a failure means restart the process       |
| `/health/ready` | Readiness — 503 once shutdown starts; stop routing   |

Keeping these separate matters during shutdown. On `SIGTERM` the service
withdraws readiness, then keeps serving for `SHUTDOWN_DRAIN_SECS` before it
stops accepting connections. In that window `/health/ready` answers 503 while
`/health/live` still answers 200, so a load balancer deregisters the instance
instead of an orchestrator restarting a process that is shutting down
normally. Without the drain window the flip would be invisible: the socket
closes immediately and probes get connection-refused rather than a 503.

## Configuration

| Variable               | Default   | Notes                            |
| ---------------------- | --------- | -------------------------------- |
| `HOST`                 | `0.0.0.0` | IP address, not a hostname       |
| `PORT`                 | `3000`    | `0` binds an OS-assigned port    |
| `REQUEST_TIMEOUT_SECS` | `30`      | Per-request deadline, in whole seconds |
| `LOG_FORMAT`           | `text`    | `text` for humans, `json` for aggregators |
| `SHUTDOWN_DRAIN_SECS`  | `5`       | Keep serving this long after readiness is withdrawn; `0` exits at once |
| `RUST_LOG`             | `info`    | Standard `tracing` env filter    |

A variable that is set but unparseable is a startup error rather than a
silent fall back to the default. The service drains in-flight requests on
Ctrl-C or `SIGTERM`.

`LOG_FORMAT=json` emits one JSON object per line, with the request ID inside
`span`, which is what a log aggregator wants:

```json
{"timestamp":"...","level":"DEBUG","message":"started processing request",
 "span":{"method":"GET","request_id":"json-trace","uri":"/health"}}
```

## Middleware

Every request passes through, outermost first:

1. `x-request-id` — reused if the client sends one, otherwise a fresh UUID.
2. Propagation of that ID onto the response.
3. `X-Content-Type-Options: nosniff`.
4. A `tracing` span carrying method, URI, and request ID, so log lines
   correlate with the header the client saw. Each response logs one line at
   `INFO` with status and latency, visible under the default filter.
5. A per-request timeout returning `408 Request Timeout`.
6. A panic catcher turning a panicking handler into `500` instead of a
   dropped connection.

Layers 2 and 3 sit above 5 and 6 on purpose, so the synthesized 408 and 500
carry the request ID and the header too — not just responses a handler
actually produced.

Add routes in `routes()`; they inherit the whole stack. `apply_middleware`
is public so tests can wrap a router of their own, which is how the timeout
and panic cases are exercised.

## CI

`.github/workflows/ci.yml` runs on every pull request and on pushes to
`main`, in three jobs: `fmt + clippy`, `test` (including doctests), and an
`msrv` build pinned to the `rust-version` declared in `Cargo.toml`. All of
them use `--locked`, so a stale `Cargo.lock` fails the build rather than
being silently updated.

## Adding a service

Create `crates/<name>/` with a `Cargo.toml` that inherits the workspace
fields. The `members = ["crates/*"]` glob picks it up automatically.
