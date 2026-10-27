# tines-mcp

`tines-mcp` is a Rust service that will expose read-only Tines data to MCP clients. This bootstrap serves Streamable HTTP at `/mcp` and process liveness at `/healthz`. It currently advertises the MCP tools capability but registers no business tools.

## Run locally

The pinned Rust toolchain is selected by `rust-toolchain.toml`. Start the service with:

```sh
cargo run --locked
```

The server listens on `127.0.0.1:8080`. Check process liveness with:

```sh
curl -i http://127.0.0.1:8080/healthz
```

The health endpoint returns `204 No Content`. It does not contact Tines or validate an upstream token.

The MCP endpoint has no MCP-layer authentication. Keep it on loopback or place it behind a private network or tunnel that controls access. The default Host allowlist permits loopback hosts. Browser requests with an `Origin` header are rejected unless that exact origin is configured.

## Configuration

| Variable | Default | Meaning |
| --- | --- | --- |
| `MCP_BIND_ADDR` | `127.0.0.1:8080` | Listener address. |
| `MCP_ALLOWED_HOSTS` | `localhost,127.0.0.1,[::1]` | Comma-separated Host allowlist. Entries can include a port. |
| `MCP_ALLOWED_ORIGINS` | Empty | Comma-separated exact browser origins. An empty list rejects every present `Origin` header. |
| `TINES_BASE_URL` | Unset | HTTPS origin for Tines. Required when any Tines setting is supplied. |
| `TINES_API_TOKEN_FILE` | Unset | Path to a readable, non-empty token file. Set exactly one token source. |
| `TINES_API_TOKEN` | Unset | Token from the environment. Set exactly one token source. |
| `TINES_EXCLUDED_PROJECT_IDS` | Empty | Comma-separated immutable project IDs to exclude. |
| `TINES_WEB_URL` | `TINES_BASE_URL` | Optional HTTPS origin for browser links. |
| `TINES_ALLOW_HTTP` | `false` | Allows an HTTP Tines origin only when it resolves to loopback. |

Tines settings are validated when supplied, but this bootstrap does not make Tines requests or register upstream tools. It can start without Tines credentials. The service rejects malformed addresses, origins, token-source combinations, and non-loopback HTTP upstream URLs.

## Checks

Run the same checks as CI:

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```
