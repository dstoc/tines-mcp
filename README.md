# tines-mcp

**tines-mcp** is a Rust service that lets an MCP client read Tines projects, issues, workflows, labels, and issue artifacts. It serves Streamable HTTP at **/mcp** and process liveness at **/healthz**.

## Available tools

The service provides ten read-only tools:

- **list_projects** and **get_project**
- **list_issues** and **get_issue**
- **list_issue_artifacts**, **get_issue_artifact**, and **read_artifact_content**
- **list_workflows** and **get_workflow**
- **list_labels**

**get_issue** accepts either an issue ID or a project ID and positive issue number. Use the issue ID when it is available. The response includes the current project, so a moved issue is checked against its current project.

**list_issues** uses the global issue list and returns one native page. It omits descriptions by default. Set **descriptions=true** to include them. Continue only when **next_cursor** is not null, and keep the same filters on the next call. An empty page can still have a cursor when excluded projects accounted for every item on that page.

Issue filters include project ID, workflow, state, category, schedule, labels, search, readiness, done-state visibility, duplicate visibility, and archived-project visibility. Project and issue reads preserve the owner or shared-member response shape returned by Tines.

Artifact metadata and content use separate tools. **list_issue_artifacts** lists artifact metadata without fetching bytes. **get_issue_artifact** returns one artifact and its version history. **read_artifact_content** reads a selected version and, for folders, a selected file path. If you omit the version on the first read, the response includes the resolved version and artifact ID. Continue with those values, the same folder path, the returned SHA-256, and **next_offset**. Each call fetches and checks the bounded source again. The source limit is 1 MiB. Chunks default to 32 KiB; set **chunk_bytes** from 4 through 32768 for smaller chunks. Text chunks stay on UTF-8 boundaries. Binary chunks use base64. Link and PR artifacts remain references; the service does not follow their URLs or render artifact content.

The service can start without Tines settings so clients can discover its tools. Calls that need Tines data require a server-side API token and **TINES_BASE_URL**.

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

Tines settings are validated at startup. The service rejects malformed addresses, origins, token-source combinations, and non-loopback HTTP upstream URLs.

## Checks

Run the same checks as CI:

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```
