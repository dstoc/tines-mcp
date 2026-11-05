# Proposal: Direct Tines API tools for MCP

## Motivation

Make routine issue relay, proposal-driven work, and operational recovery possible through `tines-mcp`: file milestone graphs, edit issue prose, take workflow transitions, manage labels and blockers, attach and revise proposals, update context, and recover or inspect running work.

Creation and starting are distinct actions. Tines remains the authority for authorization, workflow rules, revisions, artifact gates, versioning, and dispatch. MCP is an adapter, not another workflow engine.

## Evidence and scope

Repository heads inspected for this proposal on 5 November 2026:

- `dstoc/tines-mcp`: `a47366557255d74f40fe10cc7d57b624fcc94845`.
- `tbuckley/tines`: `af6cb25febf1b45d7eff32251f78fcf6355aed4a`.

The investigation covered the MCP transport, server, tool implementations and write tests; Tines request types, API client, issue/transition, label, context, artifact and run implementations; and MCP and reqwest documentation. These are source-level findings, not verification of the deployed Tines revision. No implementation or live mutation was performed. Request-size calculations and the issue dependency graph were checked independently.

The implementation work is defined in [the issue graph](../plans/direct-api-mcp-issues.md). This proposal is the replacement design; implement one final tool surface and cut over to it.

## Problem statement

The current adapter has nineteen read tools and one comment-write tool. Some handlers do more than their name suggests: the comment tool fetches the issue before posting; artifact reads fetch their parent issue; content reads fetch metadata before and after bytes; run reads fetch issue metadata for links. The existing comment tests explicitly expect two requests. These paths conflict with the required one-request boundary. [M1] [M2]

The current transport is specialized around GET and comment POST. It filters most nested error details, limits incoming MCP requests to 64 KiB, and has comment-specific write receipts. Its 512 KiB output limit counts both text and structured representations. Those implementation details need to change or be generalized, rather than copied into new handlers. [M3] [M4]

The Tines APIs already provide the requested mutations and companion reads. No Tines API additions are necessary. [T1] [T2]

## 1. Hard contracts

### One invocation, at most one request

A valid data operation sends exactly one HTTP request to one existing Tines endpoint. Local rejection, discovery and health checks send none.

The count includes retries, redirects, parent lookups, permission checks, automatic pagination, metadata enrichment, post-write reads and remote file retrieval. None may be hidden inside a tool call. Local JSON/base64 conversion, response projection, hashing and URL construction are permitted.

A native operation that accepts multiple labels or creates an issue with relationships remains one operation. Its internal database queries, dispatch effects and server-side conflict handling do not count as adapter requests. Conversely, an MCP batch helper that loops over issues is out of scope.

### Native semantics, explicit orchestration

Clients perform reads and writes as separate invocations. They supply immutable IDs and native revision witnesses where supported. The adapter does not refresh witnesses, repair permissions, select alternative transitions, infer consent, wait for a run, or retry an ambiguous write.

Local checks enforce input shape, safe transport construction and advertised resource limits. They do not reproduce Tines authorization or workflow evaluation. Typed request fields are closed; native response objects remain extensible so owner/member projections and future response fields are preserved.

## 2. Architecture

### Prepare, execute once, project

Use a small synchronous request-construction interface:

```text
prepare(tool_name, typed_arguments, local_limits)
    -> one PreparedCall or a local tool error

execute(PreparedCall, cancellation, deadline)
    -> one upstream response or a transport error

project(response, local_request_context)
    -> MCP result
```

`PreparedCall` describes a fixed endpoint, method, encoded path segments, query pairs, an empty/JSON/byte body, expected response kind, and local projection/receipt information. It contains no callback capable of issuing another request.

Only the executor owns the authenticated HTTP client. Preparation and projection receive neither a client nor a credential. The dispatcher calls the executor once. This makes the invariant structural; do not add a request-budget framework on top of freely network-capable handlers.

Keep the route allowlist internal and tool-specific. Callers cannot select arbitrary methods, hosts, paths or headers. Dynamic path segments and query values are encoded with the URL library. Raw uploads can set only their declared content type, not an arbitrary header map.

Use the existing dependency/toolchain pins. Retain reqwest's explicit `retry(never())`, disabled redirects, HTTPS configuration checks and sensitive authorization header. Disabling application retries alone is insufficient because reqwest also has protocol-level retry behavior. [M5] [R1]

### Modules and parallel ownership

Establish these boundaries before functional work starts:

```text
src/tools/mod.rs                 registration and prepare dispatcher
src/tools/contract.rs            PreparedCall, result and projection contracts
src/tools/reads.rs               existing reads
src/tools/issues.rs              create/edit/comment
src/tools/transitions.rs         transition plus authority/consent reads
src/tools/relations.rs           labels and blocker links
src/tools/artifacts.rs           JSON upsert and reaffirm
src/tools/uploads.rs             file upload argument conversion
src/tools/context.rs             context writes
src/tools/operations_read.rs     runners, queue and settings
src/tools/recovery.rs            resume and cancel
src/tines.rs                     sole upstream executor and error decoding
src/server.rs, src/config.rs     ingress admission, limits and configuration
```

The first issue defines and registers the module interfaces and provides pure request/response fixture helpers. Feature modules can then be implemented and tested without waiting for the executor implementation. The common infrastructure owns dispatcher-to-executor wiring; no feature-specific integration gate owns shared activation code. HTTP-level integration is an explicit later work package, not an implicit dependency of every wrapper. Each integration gate can assemble its completed modules with the common infrastructure without waiting for another feature gate.

No plugin registry, endpoint code generator, generic HTTP tool, persistent request ledger or new storage service is needed.

## 3. Issue workflow tools — first delivery priority

Paths in the tables are relative to `/api/v1`.

| Tool | Method and endpoint | Exposed input |
|---|---|---|
| `create_issue` | `POST /projects/{project_id}/issues` | `project_id`, `title`; optional `description`, `workflow_id`, `state`, `label_ids`, `blocked_by`, `blocks`, `expected_sharing_revision` |
| `edit_issue` | `PATCH /issues/{issue_id}` | `issue_id`; at least one of `title`, `description` |
| `transition_issue` | `POST /issues/{issue_id}/transition` | `issue_id`; exactly one of `action`, `transition_id`; native witness fields |
| `add_issue_labels` | `POST /issues/{issue_id}/labels` | `issue_id`, nonempty `label_ids` array, mapped to native `labels` |
| `remove_issue_label` | `DELETE /issues/{issue_id}/labels/{label_id}` | `issue_id`, `label_id` |
| `add_issue_link` | `POST /issues/{issue_id}/links` | `issue_id`, `kind: blocks\|blocked_by`, `related_issue_id`, mapped to native body `issue_id` |
| `remove_issue_link` | `DELETE /issues/{issue_id}/links/{link_id}` | `issue_id`, `link_id` |
| `append_issue_comment` | `POST /issues/{issue_id}/comments` | `issue_id`, `body`; direct POST without parent lookup |

These are existing Tines operations. Native initial labels and relationships may be included in a single creation request; the adapter must not turn them into follow-up requests. Scheduling, duplicate creation, multipart issue creation, runner pins and force-state edits are not part of this surface. [T1] [T3]

### Filing is not an automatic Start

`create_issue` never invokes transition, resume or dispatch. The native default is the chosen workflow's initial state, which may be active. Therefore, a caller asked to file without starting must select a known backlog state, obtaining it through a separate workflow read if necessary. Do not claim that omitting a Start call alone prevents dispatch. Do not invent a universal backlog state name or silently override the selected workflow. [T3]

For milestone graphs, create prerequisites in backlog and include already-known blocker IDs on subsequent creations. Independent nodes may be created concurrently; dependent nodes wait for their prerequisite IDs. The graph is not one transaction and unknown create outcomes must be investigated before another create.

### Transitions and witnesses

Expose these native fields without renaming or inventing revisions:

```text
expected_state_id
expected_decision_revision
expected_workflow_revision
expected_consent_revision
expected_consent_epoch
```

Keep their native optionality; recommend that callers send the applicable witnesses from their preceding read. Outside shared-project consent mode, Tines can resolve an action name directly and honors supplied state/decision/workflow witnesses.

For named-key calls in shared projects, both owner and member paths require an exact `transition_id` and all five witnesses. A human request such as “Request Changes” is resolved by the caller using the preceding issue read; the tool itself does not perform that read. Send `IssueDetail.workflow_revision`, not the workflow definition's `revision`. Preserve owner and member response shapes rather than assuming all witnesses live at the same JSON path. [T4] [T5]

Return `409 decision_refresh_required` and artifact-gate errors unchanged, including relevant details. Never retry with refreshed revisions or fall back to PATCH. Consent-writing fields (`allow_my_agents`, `disclosure_version`) are not exposed.

Native artifact checks are a process guard: Tines explicitly documents a race between checking artifacts and committing the transition. The adapter preserves those semantics and does not claim stronger transactional guarantees. [T4]

### Existing labels and explicit dependency direction

Accept label IDs matching the native `lbl_[0-9A-Za-z]{16}` shape. Arbitrary nonempty strings are insufficient: a misspelled label name can create a new label through the native API. A missing ID-shaped reference is rejected as stale instead. Perform no label lookup in the mutation. [T6]

For relationships, `blocks` means the addressed issue blocks `related_issue_id`; `blocked_by` reverses that direction. Removal uses the native link ID from a previous issue read or link response. There is no hidden search, bulk-delete loop or client-side graph validation. Tines checks accessibility and graph constraints. Link removal follows the native endpoint's semantics; the caller selects the intended blocking link.

### Dispatch reporting

A successful mutation means the API operation succeeded, not that a run started. Preserve native `active_run`, permission receipts, label results and run statuses only where returned. An admitted or assigned run is not necessarily running, and an existing active run is not evidence that this mutation started it.

Do not add a fabricated `run_started` boolean. `explain_dispatch`, `list_runs` and `get_run` remain explicit subsequent calls.

## 4. Artifacts — immediately next in delivery priority

| Tool | Method and endpoint | Contract |
|---|---|---|
| `put_issue_artifact` | `PUT /issues/{issue_id}/artifacts/{name}` | Typed native JSON fields for text/link/PR payloads and metadata-only updates |
| `reaffirm_issue_artifact` | `POST /issues/{issue_id}/artifacts/{name}/reaffirm` | Reaffirm the current content; no invented version precondition |
| `upload_issue_artifact_file` | `PUT /issues/{issue_id}/artifacts/{name}/file?filename=…` | Decode caller-supplied content locally and send raw bytes |

The JSON tool exposes `type`, `description`, `content`, `filename`, `content_type`, `url`, `title`, `pr_url`, `pr_repo_url` and `pr_number`, as applicable. Creation needs the native type/payload; metadata-only updates do not require a preflight. A raw file upload cannot silently add a description with a second call; use an explicit metadata-only PUT when needed. [T1] [T7]

Content writes and reaffirmation append versions; metadata-only updates do not. Repeating a PUT can therefore create another version. Artifact type is immutable. The native limit is 50 versions, 256 KiB per text document and 25 MiB per file. Return native version counts, current-version metadata and freshness; do not suppress identical content or synthesize idempotency. [T7] [T8]

Reaffirmation is explicitly “the current content still stands,” not “reaffirm the version I previously reviewed.” There is no expected-version input. Concurrent writers may also advance the current-version snapshot returned after a write; do not promise an adapter-issued, transaction-bound version receipt that Tines does not supply. [T8]

### Inline file contract and size policy

`upload_issue_artifact_file` accepts:

```text
issue_id, name, filename, content_type,
encoding: utf8 | base64,
content: string
```

UTF-8 is convenient for generated Markdown proposals. Base64 carries binary files. These are two local encodings of the same raw upload endpoint, not two upstream operations. Validate base64 and the native decoded byte cap before dispatch. Treat filenames as metadata, never local paths. Do not fetch URLs, open caller-supplied filesystem paths, accept an opaque attachment ID as bytes, or render content.

Replace the hard-coded 64 KiB ingress cap with `MCP_MAX_REQUEST_BYTES`, default **2 MiB**, with a hard maximum **36 MiB**. The entire serialized HTTP request, including the JSON-RPC envelope, counts. Advertise the configured cap in upload tool documentation generated from local settings.

A focused serialization check gave approximately 1.40 MB for a 1 MiB base64 file and 1.57 MB for a maximally JSON-escaped 256 KiB text payload. Both fit the default. A 25 MiB base64 file needs approximately 34.95 MB before large additional metadata and fits a 36 MiB budget with bounded arguments. The default deliberately does not promise the full native file range.

Enforce a four-request POST admission bound before buffering, a 30-second body-reading deadline, and the existing 45-second tool execution deadline with its 5-second connect timeout. Do not hold the POST-body admission permit for a long-lived SSE GET. Retain a separate four-call execution bound for direct/internal tool invocation; these bound different stages. Reject excess work rather than queue unbounded bodies. Upstream-request concurrency beyond the four calls is unnecessary under the one-request architecture.

The operator must size memory for encoded input, parsed strings, decoded bytes and response buffers, not just the configured wire cap. Increasing the adapter limit cannot increase a client's or tunnel's limit. No chunked-upload session, staging storage, presigned-upload API or URL-import service is included.

## 5. Context tools

| Tool | Method and endpoint |
|---|---|
| `create_context` | `POST /context` |
| `edit_context` | `PATCH /context/{context_id}` |
| `append_context` | `POST /context/{context_id}/append` |

Expose the native typed create/update fields, not an opaque HTTP body. Valid create kinds are prompt, skill, repo and env; artifacts use their own endpoints. Issue/project scope fields support the requested workflow, while other native scope dimensions retain their actual meaning. Tool descriptions must explicitly state that omitting all scope dimensions on creation creates global context. Scope selection is not a second permission system. [T9]

Preserve omitted versus explicit-null PATCH fields. Do not serialize missing optional fields as null. In Rust, use an explicit missing/null/value representation for nullable patch fields; a plain optional value that conflates missing and null is insufficient. Preserve text verbatim and distinguish UTF-8 byte caps from JavaScript UTF-16 length limits where local validation mirrors a native limit.

`edit_context` and `append_context` expose native `expected_version`. An omitted value retains native last-write-wins semantics; descriptions recommend supplying the observed version. Conflicts preserve the current item when it fits the result budget. Do not merge, refresh or retry automatically. Skill files replace the file set; append is for prompt items only and uses the existing atomic append endpoint. [T9]

Edit raw item IDs, not effective-context bundles. A journal read followed by an append is two explicit invocations. No find-or-create, append-by-scope, effective-context rewrite or implicit scope expansion is introduced.

Env payloads retain native secret/write-only behavior. Never log any context body, file content or secret value. The API, not the MCP adapter, decides whether the token may perform the context operation.

## 6. Recovery and companion reads

| Tool | Method and endpoint | Notes |
|---|---|---|
| `get_current_api_key_authority` | `GET /api-keys/current` | Effective authority of the configured server token |
| `get_issue_consent` | `GET /issues/{issue_id}/my-consent` | Native consent/readiness read; no grant operation |
| `list_runners` | `GET /runners` | Native health, backoff, concurrency and active-run counts |
| `get_runner` | `GET /runners/{runner_id}` | One runner |
| `get_supervisor_queue` | `GET /supervisor/queue` | Preserve native project filter and response |
| `get_supervisor_settings` | `GET /supervisor/settings` | Read enabled state and limits; no settings mutation |
| `resume_issue` | `POST /issues/{issue_id}/resume` | Clear parking/attempt state using the native operation |
| `cancel_run` | `POST /runs/{run_id}/cancel` | Target exactly the supplied run |

Authority and consent reads ship with transitions. The other reads and recovery tools form an independent lane. These endpoints already exist. [T1] [T2]

Resume does not resume a particular harness session or guarantee dispatch. Cancel may request cancellation rather than immediately release the claim; return the native run detail and inspect eventual status/outcome through subsequent calls. Do not implement cancel-current, cancel-and-wait, retry-and-resume, runner-token rotation or a claim-release shortcut. [T10] [T11]

Use existing `list_runs(runner_id, active=true)` to inspect claim holders; the active statuses are assigned, launching and running. No synthetic claims API or combined fleet-health aggregator is needed. [T7]

Permissions are operation-specific. For example, issue creation requires workspace read as well as project write; resume and cancellation require both project and control-plane write. API exposure does not grant those capabilities. Personal consent remains a browser/session operation; every MCP call acts as the configured token, not independently as the person using the chat client. [T3] [T10] [T11] [T12]

Usage is a deferred, non-blocking extension: a typed `get_usage` wrapper over `GET /usage`, preserving native modes, scope selectors, filters and totals. It must not calculate accounting by paging through runs. The initial delivery does not include rates, repricing or cost mutation tools. [T2]

## 7. Convert existing reads to the same boundary

Preserve useful read capabilities, not their hidden request chains.

- `list_issue_artifacts` and `get_issue_artifact` call their own endpoint once. `get_run` calls run detail once. Remove parent reads for links.
- `get_run_log(mode=tail)` reads run detail once and projects its log locally. `mode=full` reads the full-log endpoint once. Preserve full-log 404/410/503 errors; never fetch a tail as fallback. Full-mode metadata comes only from that response.
- `read_artifact_content` calls the content endpoint once. Pass native optional `version` and folder `path`. Omitted version means current at that request, not a resolved version known to MCP. Return only observed headers, supplied selectors, bytes and a calculated digest.
- Other reads keep one native page/request, native filters, owner/member field differences and redactions. Never synthesize missing fields or turn an empty page with a cursor into end-of-results.

Remove `expected_artifact_id` and the before/after metadata checks. The content response provides no artifact-ID or resolved-version header, so that guarantee cannot be implemented with one request. A caller wanting version selection first reads metadata explicitly. [T8]

Retain bounded content projection: a 1 MiB source cap, up to 32 KiB returned bytes per chunk, UTF-8 boundary handling or base64, and SHA-256 validation on continuations. Nonzero offsets require a previous digest and unchanged selectors. Each call downloads the bounded source again; this is not server-side range pagination. The digest checks byte continuity, not artifact identity or review-round freshness. Files accepted by a larger upload budget may remain too large for inline content reads; metadata and the native application remain available.

Generate links only from returned fields or already-supplied coordinates. Missing coordinates mean no link, not another read. No remote content URL is followed.

## 8. Results, errors and uncertain writes

### Result shape

Use one result envelope with native `data`, an optional native/adapter `error`, and `meta` containing HTTP status, observation time, omissions, optional locally constructed links and optional write information. Keep adapter metadata outside native `error.details`.

Return the envelope through structured content and a consistent text representation. Apply the **512 KiB serialized result cap to the actual complete MCP result**, including both representations. Preserve native JSON when it fits; do not silently clip a body or array and call it complete. MCP tool execution failures use `isError=true`; malformed protocol requests remain protocol errors. [M3] [P1]

### Write outcomes

| Outcome | Required evidence |
|---|---|
| `not_attempted` | The executor did not dispatch the request. |
| `rejected` | A native contract proves no mutation committed, such as an explicit `committed:false` or a tested, endpoint-specific pre-write rejection. |
| `confirmed` | A validated success receipt or expected 204 confirms the native operation, including a native no-op; an explicit native commitment can also establish the write part of a failing operation. |
| `unknown` | Dispatch occurred but the available evidence does not establish commitment or rejection. |

Never infer rejection merely from an HTTP 4xx: some APIs perform response reads after a mutation, and failures can occur then. Unclassified errors default to unknown. Preserve native commitment indicators separately from adapter classifications. A confirmed mutation accompanied by a genuine native error remains an error; confirmation does not erase a partial failure.

Extract a small receipt before optional enrichment or output serialization: returned resource identifiers and relevant native revision/status/current-version fields. Validate identifiers actually supplied by the response; do not present input IDs as proof of a successful create.

**A fully validated API success that only exceeds the MCP output budget remains `isError=false`.** Return `data:null`, the compact receipt and an explicit omission notice. Do not turn confirmed success into a retry-looking failure merely because the full issue/comment/context response is large. A malformed, oversized-before-validation or lost success response can still be unknown.

Timeouts, disconnections, 5xx responses, redirects and cancellation after dispatch never trigger another request. A cancelled client may receive no final outcome message. MCP cancellation is not rollback, and this stateless service provides no exactly-once guarantee or durable recovery receipt. [P2]

### Error fidelity and bounds

Preserve native HTTP status, code, message, `Retry-After` and bounded nested details, including revision conflicts, current context items, transition requirements, allowed transitions and permission errors. Replace the narrow detail-key allowlist with bounded JSON preservation plus redaction of the configured credential.

Use a 1 MiB error-body read cap and the normal 512 KiB MCP result cap. If details exceed the final budget, retain status/code/message, commitment/outcome evidence and receipt fields, and explicitly mark the details as omitted. Do not replace a recognizable native conflict with a generic size error. Retain the existing 32 MiB upstream success-JSON ceiling and bounded content-reading behavior. Response parsing and output limits must not cause retries.

## 9. Security and operational boundaries

All calls use the configured server-side token. Keep the current private-network/tunnel deployment boundary, Host/Origin checks, HTTPS restrictions and token redaction. Host/Origin validation and MCP annotations are not authentication. Anyone able to invoke this server can exercise the token through its advertised tools; do not expose it publicly without the existing intended access boundary. [M4] [M6]

Do not cache authority as a substitute for native checks or call the authority endpoint automatically before writes. Keep recovery tools visibly distinct in names and descriptions.

Set reads as read-only. Mark mutations as writes; use conservative non-idempotent hints for this release because an otherwise repeatable assignment can still affect dispatch or a changed resource. Mark edits, removals, transitions and recovery as potentially destructive; describe additive operations accurately. These hints assist the client but grant no permission. [P1]

Logs contain operation name, fixed route template, method, status, timing, request count and outcome classification, not authorization headers, request bodies, context values, file bytes or arbitrary native error bodies. Preserve untrusted source content as data; do not render it or execute remediation commands from error details.

## 10. Verification and success criteria

Verification is split across the issue graph, not deferred into one final testing task.

**Pure wrapper tests:** Every module has strict-schema tests, native field-mapping tests, omitted/null tests where relevant, and response fixtures for owner/member or redacted forms. These run without a real executor and enable parallel work.

**Executor and ingress tests:** A catch-all mock records every upstream request. Assert zero on local rejection and exactly one on valid calls, including 204, redirects, 429, timeout, malformed/error JSON, oversized bodies, cancellation and dropped-response paths. Verify no lower-level retries and no credential leakage. Test request budgets with and without Content-Length, admission before buffering, invalid base64, Unicode escaping and true serialized output sizes.

**Native semantic checks:** A disposable Tines environment verifies backlog creation, initial dependencies, stale labels, cycles, valid and invalid transitions, stale decision/workflow/consent witnesses, artifact gates and repeated versions, context conflicts, and recovery permissions. Mocked 403s prove forwarding, not actual authorization. Use existing Tines setup/browser mechanisms for consent fixtures; do not add MCP consent writes to make tests easier.

**Workflow acceptance:** Build a small milestone DAG without starting it; attach a proposal; start a chosen issue with current witnesses; add a review label; send work back; revise and reaffirm the proposal appropriately; update context; inspect a parked issue, resume it, cancel a specific run, and inspect its eventual status. Every arrow is a separate tool invocation. No test asserts that API mutation success guarantees launch.

**Cutover acceptance:** Discovery advertises the implemented final schemas and annotations, every advertised data tool satisfies the one-request invariant, and the intended MCP client can complete the proposal workflow. No native API changes, new storage or elevated token grants are required by the adapter.

## 11. Delivery and non-goals

Implement according to the [issue graph](../plans/direct-api-mcp-issues.md): one shared contract task, eleven parallel implementation packages, three independent integration gates and a final full-surface acceptance gate. Feature priority is not a dependency: artifact and operational work can start while issue editing is being built.

The initial issue-workflow cutover is eligible after `mcp-12`; artifact delivery is eligible after `mcp-13` and comes next in priority. Context and recovery follow their independent `mcp-14` gate. `mcp-15` consolidates the complete surface and documentation, but does not block earlier validated releases. Advertise only implemented tools in each delivered surface.

Prioritize issue-workflow validation, then proposal/artifact validation. Do not add artificial dependency edges from artifacts to issue editing, or from operational reads to recovery writes. Every issue owns distinct files and its own tests.

Excluded: new Tines APIs; generic HTTP access; force-state changes; project/workflow administration; label creation; runner registration/configuration/token rotation; consent grants; auto-start helpers; bulk graph orchestration; upload staging; remote file imports; artifact folders; automatic retries; exactly-once storage; background polling or subscriptions; and native workflow/authorization reimplementation.

## Remaining uncertainties

1. **Intended-client file transport limits.** The adapter can accept a configurable JSON payload, but the actual client/tunnel maximum and its ability to supply binary attachment bytes were not tested. UTF-8 proposals and caller-supplied base64 are the defined interface; opaque attachment IDs and local paths are not. The upload integration issue must verify that client path. This affects usable upload size, not the endpoint design.
2. **Deployed Tines revision.** The inspected repository contracts are pinned above; the live deployment was not verified. Integration uses a known disposable build of those contracts and records the deployed revision before cutover. Unsupported native operations fail transparently; the adapter never substitutes another endpoint.

No further product decision is needed to start the graph. Native limitations—no title/description compare-and-swap input, current-only reaffirmation, non-atomic artifact gates and mutation-versus-dispatch separation—are explicit constraints, not unresolved adapter design choices.

## Sources

[M1]: https://github.com/dstoc/tines-mcp/blob/a47366557255d74f40fe10cc7d57b624fcc94845/src/read_tools.rs
[M2]: https://github.com/dstoc/tines-mcp/blob/a47366557255d74f40fe10cc7d57b624fcc94845/tests/write_tools.rs
[M3]: https://github.com/dstoc/tines-mcp/blob/a47366557255d74f40fe10cc7d57b624fcc94845/src/tines.rs
[M4]: https://github.com/dstoc/tines-mcp/blob/a47366557255d74f40fe10cc7d57b624fcc94845/src/server.rs
[M5]: https://github.com/dstoc/tines-mcp/blob/a47366557255d74f40fe10cc7d57b624fcc94845/Cargo.toml
[M6]: https://github.com/dstoc/tines-mcp/blob/a47366557255d74f40fe10cc7d57b624fcc94845/README.md
[T1]: https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/packages/shared/src/client.ts
[T2]: https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/packages/shared/src/client.ts#L650-L765
[T3]: https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/issues.ts#L1300-L1445
[T4]: https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/issues.ts#L2040-L2545
[T5]: https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/packages/shared/src/types.ts
[T6]: https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/labels.ts
[T7]: https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/packages/shared/src/types.ts
[T8]: https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/artifacts.ts#L1400-L1830
[T9]: https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/context.ts
[T10]: https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/runs.ts#L460-L750
[T11]: https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/issues.ts#L2673-L2718
[T12]: https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/routes/api/v1/issues/[id]/my-consent/+server.ts
[P1]: https://modelcontextprotocol.io/specification/2025-11-25/server/tools
[P2]: https://modelcontextprotocol.io/specification/2025-11-25/basic/utilities/cancellation
[R1]: https://docs.rs/reqwest/0.13.5/reqwest/retry/fn.never.html
