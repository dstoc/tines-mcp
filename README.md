# tines-mcp

**tines-mcp** is a Rust service that lets an MCP client read Tines projects, issues, workflows, labels, activity, context, issue artifacts, and run data. It serves Streamable HTTP at **/mcp** and process liveness at **/healthz**.

## Available tools

The service provides nineteen read-only tools:

- **list_projects** and **get_project**
- **list_issues** and **get_issue**
- **list_issue_artifacts**, **get_issue_artifact**, and **read_artifact_content**
- **list_workflows** and **get_workflow**
- **list_labels**
- **list_events** for on-demand activity reads
- **list_context** and **get_context** for raw context items
- **get_issue_context** for effective context, including inherited parts
- **get_issue_journal** for the applicable guidance journal and its anchor
- **list_runs**, **get_run**, **get_run_log**, and **explain_dispatch** for run history, logs, and dispatch explanations

**get_issue** accepts either an issue ID or a project ID and positive issue number. Use the issue ID when it is available. The response includes the current project, so a moved issue is checked against its current project.

**list_issues** uses the global issue list and returns one native page. It omits descriptions by default. Set **descriptions=true** to include them. Continue only when **next_cursor** is not null, and keep the same filters on the next call. An empty page can still have a cursor when excluded projects accounted for every item on that page.

Issue filters include project ID, workflow, state, category, schedule, labels, search, readiness, done-state visibility, duplicate visibility, and archived-project visibility. Project and issue reads preserve the owner or shared-member response shape returned by Tines.

Artifact metadata and content use separate tools. **list_issue_artifacts** lists artifact metadata without fetching bytes. **get_issue_artifact** returns one artifact and its version history. **read_artifact_content** reads a selected version and, for folders, a selected file path. If you omit the version on the first read, the response includes the resolved version and artifact ID. Continue with those values, the same folder path, the returned SHA-256, and **next_offset**. Each call fetches and checks the bounded source again. The source limit is 1 MiB. Chunks default to 32 KiB; set **chunk_bytes** from 4 through 32768 for smaller chunks. Text chunks stay on UTF-8 boundaries. Binary chunks use base64. Link and PR artifacts remain references; the service does not follow their URLs or render artifact content.

**list_context** supports project, state, label, issue, kind, search, exact-scope, archive, and pagination filters. It returns raw items. **get_issue_context** returns Tines' assembled effective context; **get_issue_journal** resolves guidance for the issue and does not return chronological activity.

**list_events** reads one page of existing activity on demand. It accepts project and issue IDs, an open list of event type strings, a workflow state ID, an inclusive **since** bound, an exclusive **until** bound, and the native cursor. Bounds can be epoch milliseconds or ISO 8601 timestamps. Keep the filters unchanged when continuing. Activity traversal is live rather than transactional, and its cursor is not a durable replay checkpoint. An empty page can still have a cursor when excluded projects accounted for every event on that page.

### Runs, logs, and dispatch

**list_runs** returns one native page. It supports issue, project, runner, workflow, workflow-state, active-only, and pagination filters. Use immutable IDs. A workflow-state filter requires **workflow_id**. Set **active=true** to include only runs in `assigned`, `launching`, or `running`; omit it to include all statuses. Continue with **next_cursor** and keep the filters unchanged. The response preserves run **status** separately from **outcome**. Status describes run lifecycle; outcome is Tines' `advanced`, `stalled`, or `interrupted` judgment and can be null. There is no run-status filter.

**get_run** returns the run detail and captured log tail, including dropped-byte and retention metadata. It does not fetch the complete log. **explain_dispatch** is a separate read of Tines' eligibility and routing checks; it does not dispatch a run or determine issue acceptance.

**get_run_log** requires **mode** `tail` or `full`. It returns bounded UTF-8 chunks, with a 1 MiB source limit and a 32 KiB default chunk size. A nonzero **offset** requires **expected_sha256** from the previous response. Each call fetches and hashes the complete selected source again. Active logs can change between calls; a changed digest requires restarting at offset 0. Full mode preserves missing-log **404**, expired-log **410**, and temporarily unavailable **503** errors. It never substitutes the tail for an expired full log.

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

The MCP endpoint has no MCP-layer authentication. Keep it on loopback or place it behind a deliberately restricted private network or tunnel. Everyone who can reach the endpoint can use the configured Tines token through every available tool. Read-only access can still reveal sensitive data.

The default Host allowlist permits loopback hosts. Browser requests with an `Origin` header are rejected unless that exact origin is configured. Host and Origin checks limit unintended requests, but they are not authentication.

## Build and run with Podman

Build the image from the repository root:

```sh
podman build --file Containerfile --tag tines-mcp:local .
```

The build stage uses Rust 1.98.1 and `Cargo.lock`. The runtime image includes the CA certificate bundle used to verify HTTPS connections and runs the service as UID 10001. It contains no Tines token. The service is stateless and requires no writable data volume.

Podman reads `.dockerignore` when `.containerignore` is absent ([Podman build reference](https://docs.podman.io/en/stable/markdown/podman-build.1.html)). The ignore file limits the build context to the Cargo files, toolchain pin, and Rust source. Local environment files, Git data, and build output are excluded.

Set `TINES_API_TOKEN` in the host environment before running this command.

Publish the port only on host loopback. The process must bind to the container interface so the published port can reach it:

```sh
podman run --rm --stop-timeout 60 \
  --publish 127.0.0.1:8080:8080 \
  --env MCP_BIND_ADDR=0.0.0.0:8080 \
  --env MCP_ALLOWED_HOSTS=127.0.0.1:8080 \
  --env TINES_BASE_URL=https://tenant.tines.com \
  --env TINES_API_TOKEN \
  tines-mcp:local
```

The command publishes port 8080 on host loopback only. The Host allowlist contains that exact host and port. If an MCP client sends an `Origin` header, set `MCP_ALLOWED_ORIGINS` to that exact origin. The default empty list rejects every request that has an `Origin` header.

The process handles `SIGTERM` and stops accepting requests gracefully. The example allows up to 60 seconds for shutdown; the runtime timeout should be longer than the 45-second tool deadline if in-flight reads need to finish.

## Configuration

| Variable | Default | Meaning |
| --- | --- | --- |
| `MCP_BIND_ADDR` | `127.0.0.1:8080` | Listener address. |
| `MCP_ALLOWED_HOSTS` | `localhost,127.0.0.1,[::1]` | Comma-separated Host allowlist. Entries can include a port. |
| `MCP_ALLOWED_ORIGINS` | Empty | Comma-separated exact browser origins. An empty list rejects every present `Origin` header. |
| `TINES_BASE_URL` | Unset | Tines origin. HTTPS is required except for an explicitly enabled loopback HTTP origin. Required when any Tines setting is supplied. |
| `TINES_API_TOKEN_FILE` | Unset | Path to a readable, non-empty token file. Set exactly one token source. |
| `TINES_API_TOKEN` | Unset | Token from the environment. Set exactly one token source. |
| `TINES_EXCLUDED_PROJECT_IDS` | Empty | Comma-separated immutable project IDs to exclude. |
| `TINES_WEB_URL` | `TINES_BASE_URL` | Optional HTTPS origin for browser links. |
| `TINES_ALLOW_HTTP` | `false` | Allows an HTTP Tines origin only when it resolves to loopback. |

Tines settings are validated at startup. The service rejects malformed addresses, origins, token-source combinations, and non-loopback HTTP upstream URLs.

## Manual Podman deployment with Secure MCP Tunnel

[Secure MCP Tunnel](https://developers.openai.com/api/docs/guides/secure-mcp-tunnels) can connect supported OpenAI products to a private MCP server. The tunnel client is a separate process from `tines-mcp`; it makes an outbound connection to OpenAI and forwards MCP requests to the private server.

This manual setup shares the adapter's network namespace. `tines-mcp` starts first with Podman's normal outbound networking, then `tunnel-client` joins that namespace with `--network container:tines-mcp` ([Podman run reference](https://docs.podman.io/en/latest/markdown/podman-run.1.html)). Both processes can use `127.0.0.1` to reach each other. No host port is published. Any process with access to this shared namespace can call the adapter, which has no MCP-layer authentication; namespace sharing does not authenticate application requests.

The example pins the current official [`tunnel-client` release v0.0.18](https://github.com/openai/tunnel-client/releases/tag/v0.0.18), published October 23, 2026. Check the [release list](https://github.com/openai/tunnel-client/releases) before changing the pin. The image entrypoint and options below match the release's [container guide](https://github.com/openai/tunnel-client/blob/v0.0.18/docs/deployment/docker.md), [Dockerfile](https://github.com/openai/tunnel-client/blob/v0.0.18/Dockerfile), and [configuration reference](https://github.com/openai/tunnel-client/blob/v0.0.18/docs/configuration.md).

1. Use an existing tunnel and its ID. Before running these commands, set `TINES_API_TOKEN` and `CONTROL_PLANE_API_KEY` in your host environment. The runtime API key needs Tunnels Read and Use permissions; see the [Secure MCP Tunnel guide](https://developers.openai.com/api/docs/guides/secure-mcp-tunnels) and [`tunnel-client` onboarding guide](https://github.com/openai/tunnel-client/blob/v0.0.18/docs/onboarding.md) for setup.
2. Start `tines-mcp` first. It keeps the default loopback listener on port 8080, has normal outbound connectivity to Tines, and publishes no host port:

   ```sh
   podman run --detach --name tines-mcp --stop-timeout 60 \
     --env MCP_BIND_ADDR=127.0.0.1:8080 \
     --env MCP_ALLOWED_HOSTS=127.0.0.1:8080 \
     --env TINES_BASE_URL=https://tenant.tines.com \
     --env TINES_API_TOKEN \
     tines-mcp:local
   ```

   `MCP_ALLOWED_HOSTS` allows only the exact loopback host and port used by `MCP_SERVER_URL`. Origin validation remains enabled with the default empty allowlist, which rejects every present `Origin` header. Do not add a guessed origin. If an intended-client request is rejected because it carries an `Origin`, inspect the exact value in a controlled request and allow only that origin with `MCP_ALLOWED_ORIGINS` before repeating the read.
3. Start the tunnel client after the adapter. Set its health listener to loopback port 8081 so it does not collide with the adapter's port 8080. Replace `<tunnel-id>` with the existing tunnel ID:

   ```sh
   podman run --detach --name tunnel-client \
     --network container:tines-mcp \
     --env CONTROL_PLANE_API_KEY \
     --env CONTROL_PLANE_TUNNEL_ID="<tunnel-id>" \
     --env MCP_SERVER_URL=http://127.0.0.1:8080/mcp \
     --env HEALTH_LISTEN_ADDR=127.0.0.1:8081 \
     --env LOG_LEVEL=info \
     --env LOG_FORMAT=json \
     ghcr.io/openai/tunnel-client:v0.0.18
   ```

   This image starts `/usr/bin/tunnel-client run` by default. `MCP_SERVER_URL` points to the adapter over their shared loopback. `HEALTH_LISTEN_ADDR` also stays on loopback, and neither port is published. The tunnel client's default health port is 8080, so keep the explicit 8081 setting.

4. Inspect logs and process state without publishing ports:

   ```sh
   podman logs --follow tines-mcp
   podman logs --follow tunnel-client
   podman ps --filter name=tines-mcp
   podman ps --filter name=tunnel-client
   ```

   To request the adapter's `/healthz` from inside the shared namespace, use the BusyBox utility included in the pinned tunnel-client image:

   ```sh
   podman exec tunnel-client /bin/busybox wget -S -O /dev/null http://127.0.0.1:8080/healthz
   ```

   A `204 No Content` response means the adapter process is live. `/healthz` does not validate the Tines token or report tunnel connectivity. Check tunnel-client liveness and readiness, including at least one successful control-plane poll, with the release's health command:

   ```sh
   podman exec tunnel-client /usr/bin/tunnel-client health --port 8081 --require-control-plane-poll
   ```

   The command probes `/healthz`, `/readyz`, and the control-plane poll metric over loopback in the shared namespace. A passing health check confirms process/readiness signals and a successful poll; it does not prove that the intended MCP client can complete a tool call.

5. Connect the intended OpenAI MCP client through the tunnel, discover the tools, and make a safe read such as `list_projects` for an authorized Tines account. Only a successful read through that client verifies the full client-to-tunnel-to-adapter-to-Tines path, including the Tines token. Process liveness, a tunnel poll, and adapter health are not substitutes for this read.

To replace `tines-mcp`, stop and remove `tunnel-client` first, then stop and remove `tines-mcp`:

```sh
podman stop tunnel-client
podman rm tunnel-client
podman stop tines-mcp
podman rm tines-mcp
```

Start the replacement adapter and recreate `tunnel-client` with `--network container:tines-mcp`; the old client remains attached to the old namespace. The [Chime operator guide](https://github.com/dstoc/chime/blob/main/docs/operator-guide.md) is a reference for Podman's container namespace syntax. This walkthrough reverses its namespace ownership: `tines-mcp` owns the namespace and `tunnel-client` joins it. Chime's Tailscale setup, health behavior, secret handling, and Quadlet lifecycle do not apply here.

The tunnel controls access to its endpoint. It does not add authentication to this MCP server or reduce the permissions of its configured Tines token. Host and Origin validation are additional request checks, not an access-control boundary.

The local Rust tests use mock Tines endpoints and fixtures to check adapter configuration and read behavior. They need no live Tines token, tunnel, or MCP client. A deployment acceptance check requires a live tunnel and the intended MCP client to complete the safe read in step 5. This repository has not performed that end-to-end check.

## Runtime limits

- Each Tines tool call has a 45-second deadline and a 5-second connection timeout.
- The process admits up to four concurrent tool calls and eight concurrent upstream requests. It rate-limits calls to 10 per second with a burst of 20. Excess work is rejected instead of queued, and upstream requests are not retried automatically.
- MCP request bodies are limited to 64 KiB. Tines JSON responses are limited to 32 MiB; upstream error bodies are limited to 8 KiB.
- The complete serialized MCP tool result is limited to 512 KiB. Oversized results return an error instead of a truncated success.
- The service stores no Tines data, performs no background polling, and requires no writable data volume. `/healthz` reports process liveness only.

## Checks

Run the same checks as CI:

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```
