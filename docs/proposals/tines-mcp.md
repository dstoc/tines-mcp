# Proposal: Read-only Tines MCP adapter

## Motivation

Provide direct, useful access to Tines from MCP clients without routing reads through another agent or requiring clients to understand its REST API. Clients should be able to explore a backlog, understand dependencies and decisions, inspect execution progress, and read the evidence produced by a run.

Keep Tines responsible for authorization and business semantics. Keep the adapter responsible for protocol translation, bounded retrieval, project selection, and accurate error reporting.

## Problem statement

The work is not simply to rename HTTP routes as tools. The adapter must preserve pagination and distinguish absence from unavailable information; handle different owner and shared-project response shapes; keep workflow state, dependency readiness, dispatch eligibility and execution outcome separate; and make large content retrievable without unbounded responses.

A noauth MCP service holding a private API token also creates an important access boundary: everyone able to call it receives the same upstream authority. Read-only does not mean public or non-sensitive.

Assume the dedicated Tines read-only token is sufficient. Do not build a permission-discovery framework or make permission validation a development prerequisite. When Tines refuses an operation, propagate the refusal accurately.

## Proposal

Build `tines-mcp` as a standalone Rust application using `rmcp`, exposing a fixed set of read-only tools over Streamable HTTP. Call explicitly selected Tines GET endpoints using one server-side token.

The initial release has no database, background polling, subscriptions, synchronization, command execution or business-data mutations. Its activity tool reads existing events on demand; it does not monitor them.

### Architecture and evidence baseline

Use one Rust package, with a library for testable application logic and a small binary entry point. Use `tokio`, `rmcp`, `axum`, `reqwest` with rustls, `serde`/`serde_json`, JSON-schema support compatible with `rmcp`, and `tracing`. Add small supporting dependencies only when required by an implemented feature.

Pin the initial SDK dependency to `rmcp` 3.8.0 and commit `Cargo.lock`. This reviewed release supports MCP 2026-07-28 and earlier protocol revisions. Pin a compatible stable Rust toolchain in `rust-toolchain.toml`, and use it in development, CI and container builds. Do not use the SDK's Git main branch.[^rmcp]

The Tines source baseline for this proposal is commit `af6cb25febf1b45d7eff32251f78fcf6355aed4a`, inspected on 27 October 2026. This identifies the reviewed implementation, not the version of an independently deployed server.

The request path is:

```text
MCP client -> private HTTP endpoint -> rmcp tool handler
           -> visibility checks -> shared Tines HTTP client -> Tines
```

Share connection pools and immutable configuration across handlers. Allow request-local memoization of parent lookups, but do not persist Tines data or cache authorization/visibility decisions across calls.

Do not invoke the Tines CLI, scrape its UI, access its database, or copy its business-rule implementation. A generic arbitrary-HTTP tool and mechanically exposing every GET route are both rejected: neither produces a sufficiently controlled or useful interface.

### Transport and client compatibility

Expose `/mcp` and a minimal `/healthz` endpoint. Health reports process liveness only; it does not poll Tines or assert that the token works.

Use `rmcp::transport::streamable_http_server::StreamableHttpService`. Prefer JSON responses for ordinary calls and disable legacy session allocation: use the reviewed SDK's `legacy_session_mode = false` and `json_response = true`. Retain its compatibility handling rather than writing JSON-RPC, initialization, discovery or version negotiation manually.[^transport]

Support current per-request discovery and the older initialization lifecycle through the SDK. Do not enable `stateless_protocol_metadata_required` globally: the reviewed SDK documents that this would reject ordinary requests from older clients. Test at least MCP 2026-07-28 and 2025-11-25. No application state may depend on a connection or session ID.[^transport]

Advertise tools only. Do not advertise resources, prompts, tasks, subscriptions, MCP Events, sampling or elicitation that the server does not implement. Annotate every business tool as read-only and non-destructive; these annotations describe behavior and are not its enforcement mechanism.

Keep the transport independent of any hosting provider. Use Secure MCP Tunnel as the first private remote-connection target, but run it separately from the adapter. Test the actual target MCP client and tunnel after the first useful read slice, not only at final integration. Official documentation supports this private connection pattern; it does not prove a particular deployment works.[^tunnel]

### Initial tool surface

Use explicit, typed tool inputs with descriptions explaining their semantics. Reject unknown arguments and invalid combinations rather than silently ignoring them. Do not multiplex unrelated domains behind a generic `action` parameter.

All upstream paths below are relative to `/api/v1` and use GET.

| Domain | Tools | Upstream coverage |
|---|---|---|
| Projects | `list_projects`, `get_project` | `/projects`, `/projects/{id}` |
| Issues | `list_issues`, `get_issue` | `/issues`, `/issues/{id}`, `/projects/{id}/issues/{number}` |
| Workflows | `list_workflows`, `get_workflow` | `/workflows`, `/workflows/{id}`, including the native project selector where applicable |
| Labels | `list_labels` | `/labels` |
| Context | `list_context`, `get_context`, `get_issue_context`, `get_issue_journal` | `/context`, `/context/{id}`, `/issues/{id}/context`, `/issues/{id}/journal` |
| Artifacts | `list_issue_artifacts`, `get_issue_artifact`, `read_artifact_content` | `/issues/{id}/artifacts`, `/issues/{id}/artifacts/{name}`, and its `/content` endpoint |
| Execution | `list_runs`, `get_run`, `get_run_log`, `explain_dispatch` | `/runs`, `/runs/{id}`, `/runs/{id}/log`, `/issues/{id}/dispatch` |
| Activity | `list_events` | `/events` |

This is 19 focused tools, covering multiple routes where that matches one user operation.

Use the global `/issues` route with its project filter for project-scoped lists. There is no need to expose a second list tool or use the project-specific list route merely because it exists.

`get_issue` accepts exactly one of `issue_id` or the pair `project_id` and positive integer `number`. Prefer IDs for project selectors; project names are display values, not identity or exclusion keys. Check the returned issue's current project as well as any project in its lookup address: an old issue address can survive a transfer.[^issue-address]

Issue detail already supplies comments and relationships. Do not fetch them again or create separate dependency tools. Keep effective context, artifacts, run history and activity separate; do not automatically build an expensive all-inclusive issue bundle.[^issue-shapes]

Expose supported filters explicitly. Important issue filters are project, workflow, state, category, schedule, labels, search, readiness, done visibility, duplicate visibility and archived status. Use native `brief=1` by default for issue lists, with an option to include descriptions. Preserve the upstream defaults of hiding duplicates and archived projects, and document the explicit complete-inventory combination: archived `all`, duplicate hiding disabled, and no done-state filter.[^issues]

For runs, start with issue, project, runner, workflow/state and active-run filters. Leave period accounting, cost evidence and accounting-specific cursors to the later usage feature. Do not invent an unsupported run-status filter. For events, expose project, issue, event types, state, `since` and `until`; accept event-type strings without freezing the adapter to a closed list of today's types.[^events]

### Data and semantic contracts

Preserve native Tines fields and upstream redaction indicators. Use small typed structures for identities, pagination and fields needed for policy checks; use JSON values for the rest. Do not recreate the entire Tines type system.

Owner and member responses are not interchangeable. For example, a shared-member issue has a different project/relationship representation and may provide a safe latest-run summary rather than contributor-private run detail. Recognize the supported shapes explicitly. A missing field is not an empty collection, `false`, or proof that no execution occurred.[^issue-shapes][^runs]

Keep the following distinctions in tool descriptions and responses:

- `state` versus `effective_state`; dependency information versus dispatch eligibility; an assigned or launching run versus a running one; run `status` versus `outcome`.
- Workflow definition revision versus issue decision revision and workflow decision revision; context-item version versus artifact version. None is a universal revision of a composite response.
- Journal guidance versus chronological activity. The journal tool resolves the applicable journal; `list_events` supplies history.

Do not recompute readiness, transitions, context inheritance, artifact freshness or dispatch decisions. Preserve Tines' explanations and any limitations of the returned projection. In particular, a completed or advanced run does not by itself establish that an issue's acceptance criteria are met.

For JSON tools, return a stable object envelope containing `data` and `meta`. `data` contains the upstream object or collection envelope, including `items` and `next_cursor`. `meta` carries an adapter observation timestamp, relevant links and explicit adapter omissions. Keep native timestamps unchanged; the observation timestamp is not an upstream modification time or snapshot revision.

Return that envelope in `structuredContent` and an equivalent serialized JSON text block for clients that use text results. Output schemas describe the adapter envelope without falsely requiring owner-only fields on member responses. Count both representations when enforcing the wire-size limit.[^tools]

Generate project, issue and workflow browser links only for verified routes. Prefer stable project IDs in issue addresses. If a run or artifact has no verified standalone UI route, link to its containing issue and label the link accordingly. Links may require the user's browser authentication; do not mint public links or embed credentials.[^issue-address]

### Pagination without lost records

Each paginated tool call fetches one upstream page. Return every allowed record from that page and preserve its upstream `next_cursor` unchanged. Do not fill a page by scanning ahead, cut a successful page to a second item limit, or manufacture cursors from the last visible record.

Exclusions can produce an empty page with a non-null cursor. This means continue, not end of collection. End of traversal is `next_cursor: null`, never `items.is_empty()` or a short page. An oversized result is an explicit failure, not a silently shortened page.

Validate page sizes against the endpoint's supported range. Keep cursors opaque, bound their input length, and pass them only as encoded query parameters. Document that callers must retain filters across continuation and restart when changing them. Cursors never bypass current visibility checks. Reject an immediately repeated continuation cursor as an upstream pagination error.

Known unpaginated collections are returned as complete collections, with `next_cursor: null` normalized where necessary. Do not pretend unsupported `limit` or `cursor` parameters work on them. A missing pagination field on an endpoint expected to paginate is a schema error, not evidence of completion.

There is no adapter-imposed maximum page count. Traversal is nevertheless live rather than transactional: changes during traversal, particularly collections ordered by mutable update timestamps, can affect what is observed. An event pagination cursor is not a durable replay checkpoint.[^issues][^events]

### Project exclusions: selection, not a second authorization system

Configure exclusions by immutable project ID. The generic default is an empty set. This deployment's configuration excludes the shared **Tines** project and does not exclude **tines-runner-rs**. Resolve the intended project during deployment; never perform a startup name-based guess or hardcode its ID in the application.

Exclusions remove project-owned records from discovery and block their direct retrieval through the adapter. Apply them to projects, issues, project/issue-scoped context, artifacts, runs, dispatch and activity. Reject an explicitly excluded project selector before querying it, and check returned attribution where lookup aliases or transfers can change the current project.

Use the object's project ID where available. For issue children, resolve their owning issue; for an unscoped run, use its `issue_id`, not the display project name. Reuse an explicitly checked request scope when the upstream endpoint enforces it, rather than resolving every row again. Memoize resolutions only within the current request. If a required ownership lookup fails, report the failure and do not guess or return a partial successful page. When no exclusions are configured, do not perform lookups needed solely for this filter.

For events, check both recorded event-project attribution and the current issue-project attribution where available. An event with either attribution excluded is omitted. Genuine workspace-level records without a project remain workspace-level; do not classify them from prose or names.

**This is deliberately not a confidentiality or recursive text-redaction guarantee.** Allowed issues can retain references to excluded issues; allowed comments, logs, artifacts and global guidance can mention excluded projects. Shared workflows, labels and workspace-wide counts are not automatically re-scoped to the filtered project set. Do not delete dependency information and thereby imply an issue is ready.

Direct expansion of an excluded reference is blocked. Stronger confidentiality must be enforced by Tines' authorization and documented projections, not by attempting to scrub arbitrary text in the adapter. This narrower, explicit contract keeps the adapter thin and avoids promising a security boundary it cannot provide.

### Artifacts, logs and bounded content

Keep metadata separate from bytes. Artifact reads support version selection and folder-file paths. Resolve an omitted version once, return the artifact ID and resolved version, and use that version for the content request. Recheck artifact identity after the byte fetch and fail if the name was deleted and recreated during the read. A link or PR artifact returns its reference; it is not an instruction to fetch an external URL. Folder content requires a selected path.[^artifact-content]

Use one bounded-content helper for `read_artifact_content` and `get_run_log`. Return the source identity, MIME type where known, encoding, total byte length, SHA-256 digest, byte offset, returned byte length, content, and `next_offset` or null.

Text chunks contain valid UTF-8. Generated boundaries must not split a code point; reject invalid caller-supplied boundaries. Binary chunks use explicitly marked base64. Do not render HTML, execute artifact code, parse arbitrary documents, or claim that a client will display binary data merely because it can retrieve the bytes.

A nonzero offset requires `expected_sha256`, the digest of the entire previously fetched byte sequence. Artifact continuation also requires the resolved version and expected artifact ID, so deletion and recreation under the same name cannot silently change identity. Re-fetch the bounded source and fail with `content_changed` if its identity or digest differs. This avoids joining chunks from different live versions without adding server-side snapshots or storage. Resolve and authorize the source again on every call. Digests identify content; they grant no access.

The inspected artifact and log handlers serve complete bodies, not byte-range APIs. The initial adapter therefore re-fetches a bounded body for each chunk and computes its digest before returning a slice. This costs additional bandwidth, so keep the default content-source limit small rather than repeatedly downloading multi-megabyte files hundreds of times. Do not advertise efficient random access or unlimited log streaming.[^artifact-content][^run-log]

`get_run` preserves Tines' log-tail and truncation metadata. `get_run_log` offers explicit `tail` and `full` modes; full reads are on demand. Active logs are best inspected via their tail because a changing full log can invalidate continuation. An expired full log remains an error; never substitute its tail while calling the response complete. Preserve full-log `410`, missing-log `404` and temporarily unavailable `503` distinctions.[^runs][^run-log]

Initial operator-configurable bounds are: 64 KiB per incoming MCP request, 32 MiB per decoded upstream JSON body, 1 MiB per fetched artifact/log content source, 512 KiB per fully serialized tool result, and 32 KiB of decoded content per chunk. Larger artifact/log sources return an explicit size error with metadata and an authenticated-browser link; raising this limit must account for the repeated-fetch cost. Enforce body limits while reading, not solely from `Content-Length`. Bound error bodies separately to 8 KiB. These are safety defaults, not protocol limits.

An oversized JSON result or unpaginated collection fails explicitly with the limit and a useful remedy, such as a smaller native list page or an operator-adjusted bound. Do not add a general JSON-fragment protocol, persistent blob cache or download service in the initial release. Complete pagination does not imply unlimited single-record or file sizes.

### Errors, deadlines and cancellation

Upstream API failures are MCP tool execution errors with `isError: true`, not successful empty results. Preserve the HTTP status, Tines error code, safe message and useful bounded details. Include the error source and safe `Retry-After` information. Distinguish upstream failures from local validation, visibility, timeout, transport and schema errors.[^tools]

An upstream `401` or `403` must not become an MCP HTTP authentication challenge: the MCP endpoint is noauth, and the failing credential is held by the server. Do not forward upstream cookies or authentication headers. An upstream `404` remains missing-or-inaccessible; do not infer which. A locally excluded resource produces an adapter `resource_unavailable` error, not a fabricated upstream response.

For malformed JSON, an unexpected success shape, or non-JSON error pages, preserve the status and return a safe diagnostic without dumping the response body. Do not use broad catch-and-return-empty handling. Preserve intentional empty or redacted successful responses from Tines; the adapter cannot recover permissions or facts the upstream response does not reveal.

Use a 5-second connection timeout and a 45-second overall tool deadline, including visibility lookups and body reads. Limit concurrent tool work to four calls and upstream requests to eight across the process. Rate-limit tool calls to ten per second with a burst of twenty by default. Reject excess admitted work rather than queuing it without bound; all internal waits remain inside the tool deadline. Honor cancellation signaled by the SDK and shutdown, and bound other interrupted requests by the deadline.

Do not automatically retry upstream failures initially. This makes latency and upstream request counts predictable and leaves rate-limit guidance visible to the caller. Callers can repeat safe reads deliberately; no stale-data fallback is provided.

### Security and configuration

**Noauth is appropriate here only inside a deliberately restricted access boundary.** Default to `127.0.0.1:8080`. A private interface or container listener requires explicit configuration and an operator-controlled network/tunnel boundary. TLS alone does not make a publicly reachable noauth service safe. Other processes inside the admitted boundary also have access.

Use the SDK's Host validation with a narrow configured allowlist. Enforce Origin validation even when the origin allowlist is empty: the reviewed `rmcp` default otherwise disables that check. Default to rejecting every present browser Origin while permitting requests without one; add exact trusted origins only when required. Keep CORS disabled by default. Host and Origin checks are defense in depth, not authentication.[^transport]

Use a fixed, validated Tines origin. Build paths from fixed route templates and encoded identifiers, and encode filters with a URL library. Disable upstream redirects. Never accept caller-supplied origins, arbitrary request paths, HTTP methods, authorization headers or callback URLs. Never automatically follow URLs found in returned content.

Use verified HTTPS upstream; permit HTTP to a loopback origin only when `TINES_ALLOW_HTTP=true` is explicitly set for local development. Store the token in a runtime-mounted secret file where practical, with an environment-variable alternative. Never accept it as a command-line argument or bake it into an image. Mark its HTTP header sensitive and do not log configuration dumps, request bodies, raw responses or artifact contents.

| Configuration | Meaning |
|---|---|
| `TINES_BASE_URL` | Required upstream origin; the adapter appends `/api/v1` |
| `TINES_API_TOKEN_FILE` or `TINES_API_TOKEN` | Exactly one server-side token source |
| `TINES_EXCLUDED_PROJECT_IDS` | Optional comma-separated immutable IDs; empty generic default |
| `TINES_WEB_URL` | Optional browser origin; defaults to the Tines origin |
| `TINES_ALLOW_HTTP` | Default false; permits loopback HTTP for development only |
| `MCP_BIND_ADDR` | Listener; defaults to `127.0.0.1:8080` |
| `MCP_ALLOWED_HOSTS`, `MCP_ALLOWED_ORIGINS` | Explicit transport allowlists with the defaults above |

Validate local configuration at startup and fail clearly without revealing secrets. Do not contact Tines at startup to discover permissions or create state. Token/configuration changes take effect after restart.

Log tool name, route template, duration, status and a local diagnostic identifier. Do not treat caller-provided client metadata as authenticated identity. Return private-data responses with `Cache-Control: no-store` and do not advertise business-result caching.

Treat all issue text, comments, guidance, logs and artifact content as untrusted data. Read-only access limits mutations by this server; it does not make retrieved instructions trustworthy or prevent a client from misusing data with other tools.

## Non-goals

No writes, command execution, API-key management, account administration, publishing, imports, invitations, runner registration/polling/completion/cancellation, or arbitrary HTTP proxying.

No MCP OAuth, per-client identities, recursive content-redaction engine, database, persistent cache, scheduler, background Tines polling, subscriptions, webhook delivery, legacy HTTP+SSE transport, or UI/resource server.

Usage analytics, schedules, supervisor queue/statistics, detailed runner/routing configuration and generated launch prompts remain later read-only extensions. Native Tines business logic and upstream authorization changes are not part of this adapter implementation.

## Verification

### Automated checks

CI uses a local mock HTTP server and non-sensitive fixtures. It requires no live Tines token, account or tunnel. Run formatting, Clippy with warnings denied, and tests with the committed lockfile.

Test every tool's schema, exact upstream method/path/query mapping, source-shape handling and result encoding. Record representative owner/member fixtures and verify that missing or redacted fields are not invented. Verify that ordinary issue reads do not fan out to comments, context, artifacts or runs unnecessarily.

Exercise pagination with equal timestamps, short pages, an excluded-only page followed by allowed records, empty pages with continuation, and traversal beyond 100 records. Compare all returned IDs with the expected set and assert that no successful call drops allowed records to satisfy a secondary size limit.

Exercise exclusions through direct IDs, project/number aliases, moved issues, context items, artifact versions/paths, runs and events. Test failed ownership resolution and ensure no name-based fallback. Also test the documented boundary: an allowed record may retain a cross-project reference, but retrieving the excluded target fails.

Exercise UTF-8 and binary chunk reconstruction, source changes between chunks, artifact version changes, expired logs, declared and undeclared oversized bodies, serialized-result bounds, timeouts and cancellation. Test upstream 401/403/404/410/422/429/5xx, malformed responses and token-bearing diagnostic strings.

### Protocol and deployment proof

Use both SDK client tests and independent HTTP-level protocol fixtures so a matching client/server SDK is not the only compatibility check. Verify modern discovery, legacy initialization, tool discovery, successful and failed calls, invalid origins/hosts, unsupported methods/versions, shutdown and reconnection after restart.

After the core read slice, connect the actual intended MCP client through Secure MCP Tunnel. Verify discovery, a real read, a multi-page issue traversal and a surfaced API failure. Record the client, SDK and tunnel versions used. Do not substitute an Inspector-only test for this proof.

The real-deployment read-only smoke test covers every included endpoint family using the supplied token, then checks representative questions: what is blocked, what changed, why an issue has not started, and what evidence a run produced. Use controlled fixtures for failure cases unavailable in the live account. Do not mutate production data to manufacture test conditions.

## Success criteria

A standard MCP client can connect without MCP-layer authentication and inspect Tines projects, issues, workflows, relationships, comments, context, artifacts, runs, dispatch and activity through the documented tools.

Pagination reaches every available matching record in a quiescent test dataset, including through excluded-only pages. Larger content is either retrieved through explicit bounded continuation or rejected with a truthful limit; nothing is silently presented as complete.

Project selection behaves consistently within its documented scope. Upstream failures and redactions remain visible, the adapter's upstream credential does not appear in client results or diagnostics, and the registry contains no business-data mutation tools.

The adapter restarts without lost application state because it has none to restore. It is usable through the intended private connection, has a passing required `test` check, and remains a small independently deployable Rust service.

## Suggested implementation shape

```text
tines-mcp/
├── Cargo.toml
├── Cargo.lock
├── rust-toolchain.toml
├── README.md
├── Containerfile
├── .github/workflows/test.yml
├── docs/proposals/tines-mcp.md
├── src/
│   ├── lib.rs
│   ├── main.rs
│   ├── config.rs
│   ├── server.rs
│   ├── tines.rs
│   ├── visibility.rs
│   ├── content.rs
│   └── tools/
└── tests/
    └── fixtures/
```

`tines.rs` owns the shared HTTP client, fixed route construction, bounded body reading and upstream error decoding. `visibility.rs` owns project-attribution checks. `content.rs` owns digest-checked chunking. Domain tool modules contain typed inputs and minimal adaptation, not duplicated HTTP or policy logic.

Maintain an internal route/filter manifest beside the tools and fixtures. Snapshot tool names, input schemas and descriptions as a compatibility test; keep it out of runtime tool output. Unknown fields can pass through ordinary data, but an unknown shape in a field needed for visibility or pagination fails explicitly.

Build a minimal non-root container with a verified TLS trust store, no embedded token and graceful signal handling. Document direct loopback operation and a private tunnel deployment. The container needs no writable data volume. Do not introduce a release-publishing pipeline, distributed service infrastructure or a multi-crate workspace without a separate need.

## Proposed implementation issue graph

These are planning IDs, not issues created by this proposal.

| ID | Deliverable and acceptance | Depends on |
|---|---|---|
| **MCP-01** | Bootstrap the single Rust package, pinned SDK/toolchain, validated local configuration, `/mcp`, `/healthz`, shutdown and baseline protocol tests. **Commit this complete approved proposal to `docs/proposals/tines-mcp.md`. Add `.github/workflows/test.yml` with workflow name `test` and one stable job/check named `test`, triggered by pushes and pull requests.** Run formatting, locked Clippy and locked tests. No live credentials or Tines integration required. | — |
| **MCP-02** | Shared Tines client, fixed route building, token-file/environment handling, bounded requests, deadlines, result/error envelopes and mock-server tests. Prove upstream authorization failures are tool errors, not MCP auth challenges. | MCP-01 |
| **MCP-03** | Project, issue, workflow and label tools; shared-owner shape handling; visibility helpers; native pagination. Complete the first real-client/tunnel proof and a multi-page issue read. | MCP-02 |
| **MCP-04** | Context and journal tools, scope resolution, effective-context handling and representative inherited/shared fixtures. | MCP-03 |
| **MCP-05** | Artifact metadata/version/path tools and shared digest-checked content helper, including binary and UTF-8 tests. | MCP-03 |
| **MCP-06** | Run, dispatch and log tools; contributor-scoped response handling, visibility resolution and log retention/change tests. | MCP-03, MCP-05 |
| **MCP-07** | Activity tool with filters, time bounds, historical/current attribution and excluded-page pagination tests. No polling worker. | MCP-03 |
| **MCP-08** | Containerfile, operator README, private deployment instructions, runtime bounds and shutdown verification. | MCP-03 |
| **MCP-09** | Cross-domain regression, independent protocol checks, complete read-only smoke test through the intended client/tunnel, and acceptance evidence. | MCP-04, MCP-06, MCP-07, MCP-08 |

For MCP-01, use `cargo fmt --all -- --check`, `cargo clippy --locked --all-targets -- -D warnings`, and `cargo test --locked`. The workflow uses read-only repository permissions, no production secrets, no `pull_request_target`, and no path filters that can leave the required check absent. Keep the externally visible check name exactly `test`.

Each issue adds its own focused tests and leaves the project runnable. MCP-09 completes cross-domain verification; it is not where testing first begins. Domain work after MCP-03 can overlap, except run-log delivery reuses the content helper from MCP-05.

## Future expansion

Add usage/evidence, schedules, supervisor diagnostics, runner metadata, routing rules or launch-prompt inspection only when a concrete read use case warrants them. Review their sensitive fields and native filtering before adding tools.

For event-driven follow-up, build on the existing event records where suitable. Tines already has an event insertion mechanism designed to commit with the mutation it describes; the missing work is not simply inventing an event table. A later design must establish push notification, retention, commit/replay ordering, durable delivery checkpoints and recovery guarantees. Do not treat the newest-first activity cursor as that contract.[^event-insert][^events]

MCP's core subscription mechanism and ChatGPT's MCP Events webhook integration are distinct. The latter currently specifies webhook subscription methods, callback verification, signing and durable subscription state. Its identity and delivery requirements need a separate design; noauth read access does not automatically solve subscription ownership. Test Events over the actual tunnel rather than inferring support from ordinary tool calls.[^mcp-events]

Prefer improving Tines itself when an adapter limitation becomes material: for example, authoritative project IDs on child records, granular large-response reads, conditional/range content retrieval, or a replayable push feed. Preserve the MCP tool contract if implementation later moves into Tines; do not freeze every incidental REST response field forever.

Authentication and writes remain separate proposals, not latent capabilities in this release.

[^rmcp]: [rmcp 3.8.0 release](https://github.com/modelcontextprotocol/rust-sdk/releases/tag/rmcp-v3.8.0), [versioned SDK README](https://github.com/modelcontextprotocol/rust-sdk/blob/rmcp-v3.8.0/README.md), and [workspace toolchain requirements](https://github.com/modelcontextprotocol/rust-sdk/blob/rmcp-v3.8.0/Cargo.toml).
[^transport]: [Versioned Streamable HTTP implementation and configuration](https://github.com/modelcontextprotocol/rust-sdk/blob/rmcp-v3.8.0/crates/rmcp/src/transport/streamable_http_server/tower.rs); [MCP 2026-07-28 Streamable HTTP specification](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/streamable-http).
[^tunnel]: [Secure MCP Tunnel](https://developers.openai.com/api/docs/guides/secure-mcp-tunnels) and [custom MCP server connection options](https://developers.openai.com/api/docs/guides/custom-mcp-server), reviewed 27 October 2026.
[^tools]: [MCP tools: structured results, schemas and errors](https://modelcontextprotocol.io/specification/2026-07-28/server/tools).
[^issues]: [Tines issue-list handler](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/routes/api/v1/issues/%2Bserver.ts).
[^issue-shapes]: [Owner issue-detail assembly](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/issues.ts) and [shared issue projections](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/shared-issues.ts).
[^issue-address]: [Issue UI address handling](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/routes/%28app%29/issues/%5Bproject%5D/%5Bnumber%5D/%2Bpage.server.ts).
[^runs]: [Run serialization, private and shared projections, and log metadata](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/runs.ts).
[^artifact-content]: [Artifact metadata and content implementation](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/artifacts.ts).
[^run-log]: [Full-run-log HTTP handler](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/routes/api/v1/runs/%5Bid%5D/log/%2Bserver.ts).
[^events]: [Filtered, paginated activity endpoint](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/routes/api/v1/events/%2Bserver.ts).
[^event-insert]: [Mutation-coupled event insertion](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/events.ts).
[^mcp-events]: [ChatGPT MCP Events contract](https://developers.openai.com/plugins/build/mcp-events), reviewed 27 October 2026.
