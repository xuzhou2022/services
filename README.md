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
| `/health`       | Alias for `/health/live`, kept for compatibility     |
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

1. A JSON body filler for error responses that arrive without one — see
   [Errors](#errors). Outermost, so it catches what the layers below
   synthesize.
2. `x-request-id` — reused if the client sends one, otherwise a fresh UUID.
3. Propagation of that ID onto the response.
4. `X-Content-Type-Options: nosniff`.
5. A `tracing` span carrying method, URI, and request ID, so log lines
   correlate with the header the client saw. Each response logs one line at
   `INFO` with status and latency, visible under the default filter.
6. A per-request timeout returning `408 Request Timeout`.
7. A panic catcher turning a panicking handler into `500` instead of a
   dropped connection.

Layers 3 and 4 sit above 6 and 7 on purpose, so the synthesized 408 and 500
carry the request ID and the header too — not just responses a handler
actually produced. Layer 1 has to be outermost for a duller reason: the panic
catcher changes the body type, so it only typechecks at the router boundary.

`Cache-Control: no-store` is applied separately, inside `routes()` rather than
in this stack, because it is a property of probe endpoints and not of
everything the service will serve.

Add routes in `routes()`; they inherit the whole stack. `apply_middleware`
is public so tests can wrap a router of their own, which is how the timeout
and panic cases are exercised.

## Errors

Every response is JSON, including the ones no handler produced:

```json
{"status": 404, "error": "Not Found"}
```

That covers `404`, `405`, the `408` from the request timeout, and the `500`
from a panicking handler. `error` comes from the status code's canonical
reason, so it cannot drift from the code it reports. All of them still carry
`x-request-id`, so a failed request is traceable from what the client saw.

The rule is enforced by a layer keyed on a missing content-type rather than
on a list of statuses, so a body-less layer added later is covered without
being enumerated here.

## CI

`.github/workflows/ci.yml` runs on every pull request and on pushes to
`main`, in three jobs: `fmt + clippy`, `test` (including doctests), and an
`msrv` build pinned to the `rust-version` declared in `Cargo.toml`. All of
them use `--locked`, so a stale `Cargo.lock` fails the build rather than
being silently updated.

## Adding a service

Create `crates/<name>/` with a `Cargo.toml` that inherits the workspace
fields. The `members = ["crates/*"]` glob picks it up automatically.
