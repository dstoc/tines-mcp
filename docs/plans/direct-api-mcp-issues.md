# Implementation issue graph: Direct Tines API tools for MCP

**Specification:** [Proposal: Direct Tines API tools for MCP](../proposals/direct-api-mcp.md).

Repository: `dstoc/tines-mcp`. Each issue below references `docs/proposals/direct-api-mcp.md` as its normative specification. IDs are planning IDs, not filed GitHub issues.

## Scheduling model

The graph has one common predecessor, eleven independent implementation packages, three integration gates and one completion gate. Dependencies mean prerequisites for the stated deliverable, not relative importance.

Feature packages implement pure preparation/projection code and their tests against the contract from `mcp-00`. They do **not** claim HTTP-level integration before the relevant gate passes. This is why they can run in parallel with the executor. Shared-file ownership and registration interfaces are settled in the root task. Common infrastructure owns runtime dispatcher wiring; no feature gate is a hidden prerequisite for another gate.

The initial issue-workflow cutover is eligible after `mcp-12`. Artifacts are eligible after `mcp-13` and should be delivered next. Context/recovery are eligible after `mcp-14`. `mcp-15` consolidates full-surface acceptance; it must not delay a completed high-priority release. Each shipped surface uses the new direct-call implementation, with no partially implemented advertised tools.

Prioritize the issue-workflow critical path when workers are scarce. With more workers, start the artifact JSON and upload lanes immediately; they do not depend on issue editing. Operational reads and recovery writes are independent. Usage is explicitly deferred.

## Graph

```text
mcp-00  Shared contracts, module seams and fixture helpers
   |
   +-- mcp-01  Single-request executor and results -----------+
   +-- mcp-02  Ingress limits and admission ------------------+-- common integration inputs
   +-- mcp-03  Existing reads: direct-call conversion --------+
   |
   +-- mcp-04  Issue creation, prose editing and comments ----+--> mcp-12  Issue workflow gate
   +-- mcp-05  Transitions, authority and consent reads ------+
   +-- mcp-06  Existing labels and blocker links ------------+
   |
   +-- mcp-07  JSON artifact writes and reaffirmation -------+--> mcp-13  Proposal/artifact gate
   +-- mcp-08  File upload wrapper --------------------------+
   |
   +-- mcp-09  Context writes -------------------------------+--> mcp-14  Context/recovery gate
   +-- mcp-10  Runner and supervisor reads ------------------+
   +-- mcp-11  Resume and cancel ----------------------------+

mcp-12 + mcp-13 + mcp-14 --> mcp-15  Full-surface acceptance and documentation
                                      |
                                      +--> mcp-16  Usage reads (deferred; not a release gate)
```

All three integration gates additionally depend on `mcp-01`, `mcp-02` and `mcp-03`; the table below is authoritative.

| ID | Title | Priority | Depends on |
|---|---|---|---|
| mcp-00 | Establish direct-call contracts and parallel module boundaries | P0 | — |
| mcp-01 | Implement the sole upstream executor and outcome-safe results | P0 | mcp-00 |
| mcp-02 | Add bounded configurable MCP ingress | P0 | mcp-00 |
| mcp-03 | Convert existing reads to one-request plans | P0 | mcp-00 |
| mcp-04 | Add issue creation/editing and direct comment append | P0 | mcp-00 |
| mcp-05 | Add guarded transitions and authority/consent reads | P0 | mcp-00 |
| mcp-06 | Add existing-label and blocker-link operations | P0 | mcp-00 |
| mcp-07 | Add JSON artifact writes and reaffirmation | P1 | mcp-00 |
| mcp-08 | Add inline raw-file artifact uploads | P1 | mcp-00 |
| mcp-09 | Add native context create/edit/append wrappers | P2 | mcp-00 |
| mcp-10 | Add runner, queue and supervisor-settings reads | P2 | mcp-00 |
| mcp-11 | Add issue resume and specific-run cancellation | P2 | mcp-00 |
| mcp-12 | Verify and cut over the issue-workflow surface | P0 | mcp-01, mcp-02, mcp-03, mcp-04, mcp-05, mcp-06 |
| mcp-13 | Verify the proposal/artifact workflow and upload path | P1 | mcp-01, mcp-02, mcp-03, mcp-07, mcp-08 |
| mcp-14 | Verify context, permissions and operational recovery | P2 | mcp-01, mcp-02, mcp-03, mcp-09, mcp-10, mcp-11 |
| mcp-15 | Complete full-surface acceptance and operator documentation | P2 | mcp-12, mcp-13, mcp-14 |
| mcp-16 | Expose native usage reports | Deferred | mcp-15 |

## Issue definitions

### mcp-00 — Establish direct-call contracts and parallel module boundaries

```yaml
id: mcp-00
priority: P0
depends-on: []
proposal: docs/proposals/direct-api-mcp.md
```

**Scope:** Freeze the prepared-request, upstream-response, local-projection and result-envelope contracts. Establish the module interfaces listed in proposal §2, local limit access and the registration seams. Add pure fixture helpers that inspect a plan's method/segments/query/body and run its response projection. Record endpoint-specific success/receipt contracts, optional/null semantics and the tested noncommitting error evidence convention.

**Ownership:** `src/tools/mod.rs`, `src/tools/contract.rs`, module declarations, shared fixture types and a checked-in tool/endpoint contract fixture. Allocate later ownership now; endpoint additions should not require concurrent edits to one central route enum or dispatcher.

**Acceptance:** The crate compiles and pure fixtures run. Each module can supply tool definitions and a preparation function without touching another module. Preparation/projection have no authenticated client handle. There is no generic-HTTP MCP entry point, operation-list plan or second-request callback. Do not fold transport, all old-read rewrites or every schema into this root task.

### mcp-01 — Implement the sole upstream executor and outcome-safe results

```yaml
id: mcp-01
priority: P0
depends-on: [mcp-00]
proposal: docs/proposals/direct-api-mcp.md
```

**Scope:** Implement one execution of a prepared GET/POST/PATCH/PUT/DELETE; support empty, JSON and raw-byte bodies and JSON/bytes/204 responses. Centralize status/error parsing, cancellation/deadline handling, configured-token redaction, compact receipts, bounded native details and truthful write outcomes. Keep no-retry/no-redirect behavior and remove the now-unnecessary separate upstream fan-out allowance.

**Ownership:** `src/tines.rs`, executor tests and the common dispatcher-to-executor wiring in `src/tools/mod.rs` after `mcp-00`. Do not edit feature schemas or ingress middleware. Complete this shared wiring here, rather than assigning it to a feature integration gate.

**Acceptance:** A catch-all HTTP mock records one request for each dispatched plan and none for a locally rejected plan. Redirect, 429, timeout, malformed JSON, dropped response, oversized body and cancellation cases cannot issue another request. Confirmed-success output overflow returns a successful compact receipt. Native errors keep their codes and bounded details; classification does not assume every 4xx means no commit. Native `committed` evidence survives output reduction.

### mcp-02 — Add bounded configurable MCP ingress

```yaml
id: mcp-02
priority: P0
depends-on: [mcp-00]
proposal: docs/proposals/direct-api-mcp.md
```

**Scope:** Implement the 2 MiB default / 36 MiB maximum request budget and `MCP_MAX_REQUEST_BYTES`. Enforce a four-POST admission bound before body buffering, a 30-second body-reading deadline, and the existing execution bound. Preserve Host/Origin and HTTPS settings. Expose effective limits as local configuration to schema descriptions; no upstream discovery call.

**Ownership:** `src/config.rs`, `src/server.rs`, ingress/configuration tests. Coordinate only through the agreed executor deadline/cancellation contract.

**Acceptance:** Invalid configurations fail at startup. Both Content-Length and streamed/unknown-length requests are bounded. Oversized or admission-rejected requests make zero upstream requests. Excess uploads cannot allocate unbounded buffers; stalled body readers release permits. Long-lived SSE GETs do not consume POST body-admission permits. JSON escaping and metadata/envelope bytes count toward the actual wire limit.

### mcp-03 — Convert existing reads to one-request plans

```yaml
id: mcp-03
priority: P0
depends-on: [mcp-00]
proposal: docs/proposals/direct-api-mcp.md
```

**Scope:** Move existing read behavior into `src/tools/reads.rs`. Remove parent/link enrichment requests and artifact metadata-before/after fetches. Tail/full log selection chooses one endpoint before execution. Artifact content accepts native selectors and reports bytes/headers/digest without claiming resolved identity/version metadata. Keep useful filters, native pages, redactions and bounded content projection.

**Ownership:** `src/tools/reads.rs`, `src/content.rs` and read-specific tests. Remove obsolete read-handler glue within this scope; coordinate only the agreed common dispatcher boundary with `mcp-01`. No later feature gate owns shared activation code.

**Acceptance:** Every read builds one request. No expected-artifact-ID promise remains. Tests cover owner/member shapes, empty pages with cursors, optional/current versus selected artifact versions, folder paths, changing digests, UTF-8 boundaries, binary encoding, missing coordinates for links and full-log failures without tail fallback. Projection cannot call a client.

### mcp-04 — Add issue creation/editing and direct comment append

```yaml
id: mcp-04
priority: P0
depends-on: [mcp-00]
proposal: docs/proposals/direct-api-mcp.md
```

**Scope:** Implement `create_issue`, title/description-only `edit_issue` and direct `append_issue_comment`. Creation supports the proposal's native initial labels, blockers and sharing witness fields in its single POST. Reuse the shared label-ID validator from the contract, not another worker's implementation module.

**Ownership:** `src/tools/issues.rs` and issue-content tests.

**Acceptance:** Requests map exactly to the native endpoints. Creation accepts its documented initial workflow/state fields; editing rejects state, workflow-change and pin fields. Unadvertised fields and scheduling inputs are rejected. Create emits no transition or resume. Tool descriptions warn about active initial states. Prose is preserved; local lengths use the correct units. Fixtures cover create responses, comment identity, sparse edit bodies and minimal confirmed receipts. Native prose edits do not claim a revision guarantee they lack.

### mcp-05 — Add guarded transitions and authority/consent reads

```yaml
id: mcp-05
priority: P0
depends-on: [mcp-00]
proposal: docs/proposals/direct-api-mcp.md
```

**Scope:** Implement `transition_issue`, `get_current_api_key_authority` and `get_issue_consent`. Support exactly one action selector and all native witness fields. Document shared-project exact-ID requirements and source the workflow witness from the issue read. Omit browser-only consent-write inputs.

**Ownership:** `src/tools/transitions.rs` and transition/authority fixtures.

**Acceptance:** A transition plan contains only one POST, with witnesses forwarded unchanged. Owner/private and owner/member shared projections are accepted without normalizing away differences. Tests preserve `decision_refresh_required`, artifact-gate details and native consent errors. No workflow fetch, name-to-ID lookup, witness refresh, personal consent write or PATCH fallback is possible.

### mcp-06 — Add existing-label and blocker-link operations

```yaml
id: mcp-06
priority: P0
depends-on: [mcp-00]
proposal: docs/proposals/direct-api-mcp.md
```

**Scope:** Implement add/remove label and add/remove link tools. Label IDs use the native ID shape; multiple additions use the native labels array. Link additions expose `blocks` and `blocked_by`, with unambiguous source/target mapping. Link deletion uses the supplied link ID.

**Ownership:** `src/tools/relations.rs` and relation fixtures.

**Acceptance:** Names/malformed label IDs cannot accidentally reach native get-or-create semantics. Both link directions, encoded path components, singular deletions, native 204s and returned link IDs are tested. Native cycles/permission failures are forwarded, not locally reimplemented. No lookup, set-difference reconciliation, bulk loop or invented dispatch receipt is added.

### mcp-07 — Add JSON artifact writes and reaffirmation

```yaml
id: mcp-07
priority: P1
depends-on: [mcp-00]
proposal: docs/proposals/direct-api-mcp.md
```

**Scope:** Implement `put_issue_artifact` and `reaffirm_issue_artifact`, with native text/link/PR and metadata-only payloads. Return artifact identity and current-version metadata as a native observation, not as an invented conditional-write receipt.

**Ownership:** `src/tools/artifacts.rs` and JSON artifact tests.

**Acceptance:** Metadata-only updates do not invent content fields. PR URL/split-field forms and text/link fields are forwarded correctly. Tests cover type mismatch, version cap, repeated writes, reaffirmation and uncertain responses. No preflight, expected-version fiction, duplicate-content suppression or automatic retry is introduced.

### mcp-08 — Add inline raw-file artifact uploads

```yaml
id: mcp-08
priority: P1
depends-on: [mcp-00]
proposal: docs/proposals/direct-api-mcp.md
```

**Scope:** Implement `upload_issue_artifact_file`, UTF-8/base64 local conversion, bounded decode, filename query encoding and content-type handling. Consume the agreed local limit interface; do not modify ingress configuration or transport code.

**Ownership:** `src/tools/uploads.rs` and upload-plan tests.

**Acceptance:** The exact decoded bytes form one PUT body. Invalid base64, unsafe header values and over-native-limit content fail before dispatch. Filenames are not filesystem paths. The tool cannot read a local file, resolve an attachment handle, download a URL, stage chunks or perform an extra metadata write. Tests cover Unicode, empty/native-valid binary content, base64 boundaries and descriptive metadata remaining separate.

### mcp-09 — Add native context create/edit/append wrappers

```yaml
id: mcp-09
priority: P2
depends-on: [mcp-00]
proposal: docs/proposals/direct-api-mcp.md
```

**Scope:** Implement the proposal's typed context payloads and native scope fields, explicit-null semantics, `expected_version`, and the direct append endpoint. Keep artifact payloads on artifact tools and editing raw context distinct from reading effective bundles.

**Ownership:** `src/tools/context.rs` and context fixtures.

**Acceptance:** Missing and null fields serialize differently; empty text and omitted fields are not conflated. Skill files replace the set; prompt append uses native `text`, not GET plus PATCH. Conflict details/current versions survive projection. Scope omission/global behavior is documented explicitly. Secret or other context payloads are never logged. No journal lookup or find-or-create operation is hidden inside a write.

### mcp-10 — Add runner and supervisor reads

```yaml
id: mcp-10
priority: P2
depends-on: [mcp-00]
proposal: docs/proposals/direct-api-mcp.md
```

**Scope:** Implement `list_runners`, `get_runner`, `get_supervisor_queue` and `get_supervisor_settings`. Preserve native health, backoff, concurrency and queue details. Document the existing active-run read as the way to inspect claims.

**Ownership:** `src/tools/operations_read.rs` and operational-read fixtures.

**Acceptance:** One plan per endpoint; no synthetic fleet-health aggregation. Project filters are native query mappings. Unknown health fields survive. Permissions, redactions and native failures are not normalized into “offline” or “empty queue.” These reads do not depend on recovery writes.

### mcp-11 — Add issue resume and specific-run cancellation

```yaml
id: mcp-11
priority: P2
depends-on: [mcp-00]
proposal: docs/proposals/direct-api-mcp.md
```

**Scope:** Implement `resume_issue` and `cancel_run` as bodyless native POSTs. Keep recovery tools distinct from prose editing. Describe control-plane/project permissions, parked-state reset and eventual cancellation correctly.

**Ownership:** `src/tools/recovery.rs` and recovery fixtures.

**Acceptance:** Plans target exactly the supplied immutable IDs. Tests retain native status and outcome separately, including a successful cancellation response for a still-active run, already-ended rejection and no-op resume. No active-run lookup, wait loop, cancel/resume composition, force transition or claim-release shortcut exists.

### mcp-12 — Verify and cut over the issue-workflow surface

```yaml
id: mcp-12
priority: P0
depends-on: [mcp-01, mcp-02, mcp-03, mcp-04, mcp-05, mcp-06]
proposal: docs/proposals/direct-api-mcp.md
```

**Scope:** Exercise the complete issue-workflow slice through the common MCP/HTTP dispatcher. Replace issue-workflow tests that expected hidden requests; common dispatcher wiring is already supplied by `mcp-01`. Use a disposable native Tines build to verify real revision, permission, label and graph semantics. Record the inspected/native test revision.

**Ownership:** `tests/issue_workflow_acceptance.rs`, P0 discovery fixtures and initial-cutover documentation. Do not own shared runtime activation glue: artifact and operational integration gates must be executable independently of this issue.

**Acceptance:** File a small backlog DAG without starting it; explicitly start a chosen issue; edit prose; append a comment; manipulate labels and blockers; reject stale shared-project witnesses without a fallback. Restricted tokens and real shared-project consent state behave natively. All shipped old reads and issue writes satisfy zero-or-one request counting. This gate may release the first useful surface without waiting for P1/P2 completion.

### mcp-13 — Verify the proposal/artifact workflow and upload path

```yaml
id: mcp-13
priority: P1
depends-on: [mcp-01, mcp-02, mcp-03, mcp-07, mcp-08]
proposal: docs/proposals/direct-api-mcp.md
```

**Scope:** Exercise JSON artifact writes, file upload, revised versions, current-content reaffirmation and explicit metadata/content reads against the real executor and native Tines. Seed issues through existing test setup; do not depend on the MCP issue-create implementation just to obtain fixtures.

**Ownership:** `tests/artifact_acceptance.rs`, artifact discovery/size fixtures and the intended-client upload acceptance record.

**Acceptance:** The proposal workflow preserves native versions and gate behavior. Validate the 2 MiB default, configured larger budget, full serialized output cap, unknown write outcomes and compact success receipts. Prove exact raw file bytes. Test a UTF-8 proposal through the intended client; record that client's tested binary/size capability rather than assuming it. Unsupported file transfer does not trigger URL fetching or staging. Artifact implementation/integration can finish before `mcp-12`; delivery priority is not a blocker edge.

### mcp-14 — Verify context, permissions and operational recovery

```yaml
id: mcp-14
priority: P2
depends-on: [mcp-01, mcp-02, mcp-03, mcp-09, mcp-10, mcp-11]
proposal: docs/proposals/direct-api-mcp.md
```

**Scope:** Exercise context versions/scopes, runner and queue reads, parking/resume and cancellation through the executor and native Tines. Separate API mutation acceptance from observed run lifecycle.

**Ownership:** `tests/context_recovery_acceptance.rs` and operational scenario documentation.

**Acceptance:** Real context conflicts preserve current versions without retry. Project-scoped, missing-control-plane and run-key cases produce native outcomes. Cancellation acceptance can precede terminal status; follow-up inspection is a different MCP call. No authority preflight is hidden in a write. Mocks verify protocol behavior, while native tests verify authorization and dispatch semantics.

### mcp-15 — Complete full-surface acceptance and operator documentation

```yaml
id: mcp-15
priority: P2
depends-on: [mcp-12, mcp-13, mcp-14]
proposal: docs/proposals/direct-api-mcp.md
```

**Scope:** Reconcile the complete tool inventory, schemas, annotations, README/operator guidance and final discovery fixture. Audit the only network execution site and run all three acceptance suites together. Verify deployment settings and intended-client discovery.

**Ownership:** Final README and tool-inventory documentation, full-surface acceptance record. Feature teams should not compete to rewrite these shared files while their modules are in flight.

**Acceptance:** Only implemented tools are advertised. Every data invocation makes at most one request; no auth/preflight/enrichment/fallback paths bypass the executor. No token or payload content appears in diagnostics. The upload budget and separate read/output limits are documented. The deployed Tines revision and tested client path are recorded. This completion gate does not retroactively block earlier validated high-priority deliveries.

### mcp-16 — Expose native usage reports (deferred)

```yaml
id: mcp-16
priority: deferred
depends-on: [mcp-15]
proposal: docs/proposals/direct-api-mcp.md
```

**Scope:** Add `get_usage` over `GET /api/v1/usage` with native report modes, scope and filter fields. Keep server-calculated accounting, incomplete/unpriced coverage and response shapes intact.

**Acceptance:** One request; no run-page aggregation, repricing or rates mutation. Add endpoint/schema fixtures and an executor-level test. This issue is not a prerequisite for any initial feature or cutover gate.

## Common definition of done

Each implementation issue includes its own tests and tool descriptions. All schema changes agree with the normative proposal, runtime deserialization rejects unadvertised fields, and domain files do not access the authenticated client. Native error and response fixtures include revision/permission failures where relevant. Inputs and outputs are data, not instructions.

Integration gates count every outbound request, including error paths. A failed or cancelled write is never retried automatically. Approval of pure feature code is not a claim that live permissions or dispatch were verified; those checks belong to the explicitly dependent integration gates.

## mcp-00 implementation interface

The approved specification above remains normative. The contracts in
`src/tools/contract.rs` and the catalog in
`tests/fixtures/direct-api-contracts.json` provide the handoff to the implementation
packages. The catalog records native response types, status codes, receipt paths,
optional/null semantics, and pinned noncommitting error evidence. It is test data,
not a runtime route dispatcher or a claim of HTTP integration.

Each feature package owns its module and tests as allocated above. The static
`tools::MODULES` list already includes all nine domain modules. Within its own
file, a package supplies:

- `definitions(&LocalLimits) -> Vec<Tool>`, including only implemented tools.
- `prepare(name, arguments, &LocalLimits) -> Result<PreparedCall, LocalError>`.

Adding an endpoint to an allocated module requires no change to another domain
module or to a shared route enum. The dispatcher selects the module by its
advertised tool name. Unknown and multiply owned names cannot prepare a call.
The module must use closed request types with `deny_unknown_fields`; the shared
`arguments` helper returns a local error without exposing payload values.
Definitions and preparation receive the same effective local limits.

A domain module constructs an `Endpoint` with a fixed method/template and raw
segments, query pairs, one empty/JSON/byte body, a `ResponseContract`, and a pure
`Projection`. The executor must encode each path segment and query pair with
`Url`; the template is only a fixed log label. Raw bodies can supply Content-Type
but no arbitrary header map. No tool accepts generic HTTP operations.

The projector is a synchronous function pointer with JSON local context. It
receives only `UpstreamResponse`: status, observation time, selected observed
headers and a parsed JSON/byte/empty body. It validates the endpoint's receipt
before optional enrichment. Missing response IDs cannot be filled from inputs
as success evidence. Native response objects remain extensible; preserve missing
and null fields and owner/member differences. Expected 204s use empty receipts.
If a later projection step fails, `ProjectionError::with_receipt` preserves already
validated success evidence.

`ExecutionResult` distinguishes responses from `ExecutionFailure`, which retains
whether dispatch occurred, any observed status/Retry-After, and a bounded,
redacted error. mcp-01 owns execution, native error decoding, outcome
classification, conversion to the complete MCP result, and result budgeting.
The result contract puts adapter outcome/receipt information in `meta.write`,
separate from native `error.details`. A confirmed mutation can still have a
native error. A validated success reduced only for the MCP output budget has
`data:null`, an omission and its receipt, with no error.

`NoncommittingError` rules apply only to the endpoint that supplies them. A rule
must name the exact status/code and a pinned native regression test that proves
unchanged resource/event state. Record the same evidence in the catalog. Native
explicit commitment takes precedence. The initial evidence is for context PATCH
`409 version_conflict`; it does not classify every conflict or every 4xx. The
native source test was inspected for this contract, not run as a live integration
check.

Nullable PATCH fields use `PatchField<T>` with serde default and missing-field
omission. Optional non-null fields can use `deserialize_optional` with serde
default; annotate their schemars value type as well so their advertised schema
rejects null. The shared label-ID validator is available to both issue creation
and relation tools without a dependency between those modules.

Feature integration test files can use `mod support;` and
`support::direct_calls::{inspect, project}`. `PlanFixture` compares a single
method/template/segments/query/body. `ResponseFixture` runs the same projector
without a token, executor or HTTP runtime. Endpoint packages add their native
shape and strict-schema fixtures; integration gates count actual requests.

At mcp-00, feature modules advertise no tools and the existing runtime remains
active. mcp-01 owns common dispatcher/executor wiring. mcp-02 connects the local
limit value to configuration and ingress. The named integration gates own
cutover verification; pure fixture approval does not assert live permissions,
one-request runtime behavior or intended-client upload limits.
