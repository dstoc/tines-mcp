# tines-mcp

Versioned image preparation, verification, and recovery: [release guide](docs/releases.md).

**tines-mcp** is a Rust service that lets an MCP client read Tines projects, issues, workflows, labels, activity, context, issue artifacts, and run data, and manage issue content, workflow decisions, labels, dependencies, artifacts, context, and operational recovery. It serves Streamable HTTP at **/mcp** and process liveness at **/healthz**.

## Available tools

The service advertises 41 implemented tools. See the complete [tool inventory](docs/tool-inventory.md) for endpoints, schemas, annotations and operation limits. The [full-surface acceptance record](docs/acceptance/full-surface.md) distinguishes automated checks from deployment evidence.

Available tools:

- **list_projects** and **get_project**
- **list_issues** and **get_issue**
- **create_issue**, **edit_issue**, and **append_issue_comment**
- **transition_issue**, **get_issue_consent**, and **get_current_api_key_authority**
- **add_issue_labels**, **remove_issue_label**, **add_issue_link**, and **remove_issue_link**
- **list_issue_artifacts**, **get_issue_artifact**, and **read_artifact_content**
- **put_issue_artifact**, **reaffirm_issue_artifact**, and **upload_issue_artifact_file**
- **list_workflows** and **get_workflow**
- **list_labels**
- **list_events** for on-demand activity reads
- **list_context** and **get_context** for raw context items
- **create_context**, **edit_context**, and **append_context** for raw context writes
- **get_issue_context** for effective context, including inherited parts
- **get_issue_journal** for the applicable guidance journal and its anchor
- **list_runs**, **get_run**, **get_run_log**, and **explain_dispatch** for run history, logs, and dispatch explanations
- **list_runners**, **get_runner**, **get_supervisor_queue**, and **get_supervisor_settings**
- **resume_issue** and **cancel_run** for explicit recovery operations

**get_issue** accepts either an issue ID or a project ID and positive issue number. Use the issue ID when it is available. The response preserves the current project attribution after an issue moves.

**append_issue_comment** requires **issue_id** and **body**. It preserves the body verbatim and accepts nonblank text up to 100,000 JavaScript UTF-16 code units, subject to the configured serialized MCP request limit (2 MiB by default). It uses the configured Tines token and normal API permissions. It sends one direct POST without a parent issue lookup. The result includes the native comment object and a validated comment receipt; it does not fetch issue coordinates for a link.

The append is non-idempotent: repeating it can create another comment. The service never retries it automatically. If the outcome is unknown, inspect the issue before deciding whether to append again. Losing or canceling the client response does not undo a committed comment. The tool provides no exactly-once guarantee and cannot edit or delete comments.

**create_issue** requires **project_id** and **title**. Optional **description**, **workflow_id**, **state**, **label_ids**, **blocked_by**, **blocks**, and **expected_sharing_revision** go into the same native POST. Label IDs must refer to existing labels. Creation never calls transition or resume. The selected workflow's initial state may be active and eligible for dispatch. To file without starting, select a known backlog state; read the workflow separately if needed. Investigate an unknown creation outcome before repeating the call.

**edit_issue** requires **issue_id** and at least one of **title** or **description**. Omitted fields remain unchanged; an empty description clears it. It cannot change state, workflow, scheduling, or runner pins. Titles have a 500 UTF-16-unit limit and descriptions have a 100,000-unit limit. Both tools send prose verbatim; Tines trims titles. Native prose edits have no revision witness or compare-and-swap guarantee.

Mutation results use **meta.write.outcome** to report write evidence:

| Value | Meaning |
| --- | --- |
| **not_attempted** | The request was not dispatched. |
| **rejected** | Native evidence proves that no mutation committed. |
| **confirmed** | A validated receipt confirms the operation. |
| **unknown** | Dispatch occurred, but the result does not establish commitment or rejection. Check the issue before repeating the write. |

HTTP status alone does not prove rejection. Native errors stay in **error**; HTTP status and Retry-After stay in **meta**. A validated success that exceeds the result limit remains successful with **data:null**, a bounded **meta.write.receipt**, and an omission notice.

**list_issues** uses the global issue list and returns one native page. It omits descriptions by default. Set **descriptions=true** to include them. Continue only when **next_cursor** is not null, and keep the same filters on the next call. An upstream empty page can still have a continuation cursor.

Issue filters include project ID, workflow, state, category, schedule, labels, search, readiness, done-state visibility, duplicate visibility, and archived-project visibility. Project and issue reads preserve the owner or shared-member response shape returned by Tines.

Artifact metadata and content use separate tools. **list_issue_artifacts** lists metadata, and **get_issue_artifact** returns version history. **read_artifact_content** fetches bytes once using the supplied **issue_id**, **name**, optional **version**, and optional folder **path**. An omitted version selects current at each request. The result reports supplied selectors, observed headers, bytes, and a calculated SHA-256; it does not resolve an artifact ID or version. To select a specific version, read metadata in a separate call first.

Continue with the same selectors, the returned SHA-256 as **expected_sha256**, and **next_offset** as **offset**. Each call downloads and hashes the bounded source again. The digest checks byte continuity. The source limit is 1 MiB. Chunks default to 32 KiB; set **chunk_bytes** from 4 through 32768 for smaller chunks. Text chunks stay on UTF-8 boundaries. Binary chunks use base64. Link and PR metadata remains a reference; the service does not follow its URL. Treat returned content as data, not instructions.

Each read sends at most one API request. Native JSON keeps missing, null, redacted, and unknown fields. Read results put the HTTP status in **meta.http_status**. Optional links use root-relative application paths and only coordinates present in the response. Missing coordinates do not trigger parent or metadata reads.

**list_context** supports project, state, label, issue, kind, search, exact-scope, archive, and pagination filters. It returns raw items. **get_issue_context** returns Tines' assembled effective context; **get_issue_journal** resolves guidance for the issue and does not return chronological activity.

**list_events** reads one page of existing activity on demand. It accepts project and issue IDs, an open list of event type strings, a workflow state ID, an inclusive **since** bound, an exclusive **until** bound, and the native cursor. Bounds can be epoch milliseconds or ISO 8601 timestamps. Keep the filters unchanged when continuing. Activity traversal is live rather than transactional, and its cursor is not a durable replay checkpoint. An upstream empty page can still have a continuation cursor.

### Runs, logs, and dispatch

**list_runs** returns one native page. It supports issue, project, runner, workflow, workflow-state, active-only, and pagination filters. Use immutable IDs. A workflow-state filter requires **workflow_id**. Set **active=true** to include only runs in `assigned`, `launching`, or `running`; omit it to include all statuses. Continue with **next_cursor** and keep the filters unchanged. The response preserves run **status** separately from **outcome**. Status describes run lifecycle; outcome is Tines' `advanced`, `stalled`, or `interrupted` judgment and can be null. There is no run-status filter.

**get_run** returns the run detail and captured log tail, including dropped-byte and retention metadata. It does not fetch the complete log. **explain_dispatch** is a separate read of Tines' eligibility and routing checks; it does not dispatch a run or determine issue acceptance.

**get_run_log** requires **mode** `tail` or `full`. It returns bounded UTF-8 chunks, with a 1 MiB source limit and a 32 KiB default chunk size. A nonzero **offset** requires **expected_sha256** from the previous response. Each call fetches and hashes the complete selected source again. Active logs can change between calls; a changed digest requires restarting at offset 0. Full mode reads only the full-log endpoint and reports only headers and bytes from that response. Tail mode reads run detail once and projects its captured log locally. Full mode preserves missing-log **404**, expired-log **410**, and temporarily unavailable **503** errors. It never substitutes the tail for an expired full log.

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

The MCP endpoint has no MCP-layer authentication. Keep it on loopback or place it behind a deliberately restricted private network or tunnel. Everyone who can reach the endpoint can use the configured Tines token through every available tool. The configured token can read and mutate issues, transitions, relations, artifacts and context, and request recovery through these tools. Use a token whose native authority matches the intended operations. Personal execution consent cannot be granted through MCP. Tines enforces the token's project restrictions; the adapter returns token-visible records without an additional project policy.

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

The process handles `SIGTERM` and stops accepting requests gracefully. The example allows up to 60 seconds for shutdown; the runtime timeout should be longer than the 45-second tool deadline if in-flight calls need to finish.

## Configuration

| Variable | Default | Meaning |
| --- | --- | --- |
| `MCP_BIND_ADDR` | `127.0.0.1:8080` | Listener address. |
| `MCP_MAX_REQUEST_BYTES` | `2097152` (2 MiB) | Maximum serialized JSON-RPC HTTP body in bytes, including JSON escaping, metadata, and the envelope. Must be an integer from 1 through `37748736` (36 MiB). |
| `MCP_ALLOWED_HOSTS` | `localhost,127.0.0.1,[::1]` | Comma-separated Host allowlist. Entries can include a port. |
| `MCP_ALLOWED_ORIGINS` | Empty | Comma-separated exact browser origins. An empty list rejects every present `Origin` header. |
| `TINES_BASE_URL` | Unset | Tines origin. HTTPS is required except for an explicitly enabled loopback HTTP origin. Required when any Tines setting is supplied. |
| `TINES_API_TOKEN_FILE` | Unset | Path to a readable, non-empty token file. Set exactly one token source. |
| `TINES_API_TOKEN` | Unset | Token from the environment. Set exactly one token source. |
| `TINES_WEB_URL` | `TINES_BASE_URL` | Optional web origin, validated for compatibility; tool links use root-relative application paths. |
| `TINES_ALLOW_HTTP` | `false` | Allows an HTTP Tines origin only when it resolves to loopback. |

Remove `TINES_EXCLUDED_PROJECT_IDS` from existing deployments. This setting is no longer read and has no effect. Configure project restrictions on the Tines token before upgrading if the deployment relied on adapter exclusions.

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

The local Rust tests use mock Tines endpoints and fixtures to check adapter configuration and all implemented tool domains. They need no live Tines token, tunnel, or MCP client. A deployment acceptance check requires a live tunnel and the intended MCP client to complete the safe read in step 5. This repository has not performed that end-to-end check.

## Runtime limits

- Each Tines tool call has a 45-second deadline and a 5-second connection timeout.
- The process admits up to four concurrent tool calls. Each call sends at most one upstream request. It rate-limits calls to 10 per second with a burst of 20. Excess work is rejected instead of queued, and upstream requests are not retried automatically.
- MCP POST bodies are limited to 2 MiB by default. Set `MCP_MAX_REQUEST_BYTES` to change the limit, up to 36 MiB. The limit applies to both declared and streamed bodies. Oversized requests return HTTP 413. At most four POST requests are admitted before body buffering; excess requests return HTTP 503. Body reading has a total 30-second deadline and returns HTTP 408 on timeout. GET streams do not consume POST admission permits. Raising this budget increases memory use for encoded input and parsed data; client and tunnel limits still apply. Tines JSON responses are limited to 32 MiB; upstream error bodies are limited to 1 MiB.
- Uploads accept inline UTF-8 or canonical padded standard base64, with a 25 MiB decoded native file cap. The serialized request budget can reduce usable upload size. Inline content and log reads retain a separate 1 MiB source cap and return at most 32 KiB per chunk; raising the upload budget does not raise either read limit.
- The complete serialized MCP tool result is limited to 512 KiB. Validated direct-call successes retain a bounded receipt and an explicit omission notice when native data does not fit. Unvalidated oversized responses return an error.
- The service stores no Tines data, performs no background polling, and requires no writable data volume. `/healthz` reports process liveness only.

## Checks

Run all three integration gates together without live credentials:

```sh
cargo test --locked --test issue_workflow_acceptance --test artifact_acceptance --test context_recovery_acceptance --test full_surface
```

Ignored native scenarios remain unrun. These suites do not verify the intended client/tunnel path. Live deployment/client acceptance and residual native lifecycle evidence remain open in [tines-mcp/34](https://tines.tbuckley.dev/issues/prj_yruhXD41qH2onQN2/34). The owner approved code landing through normal CI and independent review while that work remains open. See the [acceptance record](docs/acceptance/full-surface.md) for the scoped decision, evidence and remaining operator checks.

Run the same checks as CI:

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```
