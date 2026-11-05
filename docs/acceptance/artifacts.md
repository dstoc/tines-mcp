# Artifact acceptance record

The artifact adapter has automated executor, HTTP and pinned SDK checks in
[`tests/artifact_acceptance.rs`](../../tests/artifact_acceptance.rs). Native
artifact checks passed on 2026-11-05. This continuation preserves that recorded
25-request result; it does not rerun the native test. The intended OpenAI
client/tunnel and the isolated native gate scenario remain unverified.

Under the superseding direction on tines-mcp/27, adapter review can proceed with
the executor/HTTP coverage, recorded native SDK evidence and pinned upstream
coverage below. Building or running a separate Tines service is not required.
This direction does not establish an intended-client upload pass or waive other
acceptance requirements. Artifact release acceptance remains incomplete.

## Automated checks

Run the normal repository checks:

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

The suite verifies these observable contracts:

- Each valid invocation sends one request, including native error responses.
  Discovery, invalid transfer arguments and rejected ingress send none.
- JSON content revisions, metadata-only writes and current-content reaffirmation
  preserve the native response. Metadata and content reads are separate calls.
- UTF-8, all 256 byte values, empty files and named JSON bytes reach the raw
  endpoint exactly. Filename query encoding and Content-Type are preserved.
- Both declared and streamed bodies accept exactly 2 MiB and reject one byte
  more. A larger configured budget accepts larger uploads. Escaping and the
  JSON-RPC envelope count toward the request limit.
- A response below 512 KiB can exceed the complete serialized MCP result cap
  because text and structured content duplicate and escape the envelope.
  This stress case uses extension metadata; native responses omit document text.
  Reduction retains a compact confirmed receipt and an omission notice.
- Native rejection details, malformed success responses and cancellation retain
  conservative write outcomes. No failed or cancelled write is retried.
- Unsupported URL, path, attachment-handle and metadata arguments fail locally.
  A URL supplied as literal UTF-8 content is uploaded as those bytes.
- Discovery advertises strict schemas, mutation annotations and the configured
  upload budget. These expectations are pinned in the artifact acceptance
  fixture, independently of other feature modules.

## Native and client evidence

The native target was `https://tines.tbuckley.dev`, using disposable Backlog issue
[tines-mcp/32](https://tines.tbuckley.dev/issues/prj_yruhXD41qH2onQN2/32).
The issue was seeded with the existing Tines CLI, independently of MCP issue
creation, and closed as Obsolete after verification. It never entered an active
state. The final run used slot prefix `mcp-13-1793855334987` and counted 25
upstream requests, including two explicit native verification GETs. The reference source inspected was
`af6cb25febf1b45d7eff32251f78fcf6355aed4a`; the deployed revision was not reported.
The runner managed native authentication. No credential is stored in this record.

The native test uses the production router and executor. A test-only loopback
proxy counts every upstream request, including errors. Negotiation and discovery
make no native requests. Explicit native verification GETs are counted separately
from MCP calls.

The native checks verified:

- Identical text PUTs append versions 1 and 2. Revised text appends version 3.
- A metadata-only PUT retains the version, current-version fields and freshness.
- Reaffirmation appends version 4 with `reaffirmed_from=3`.
- Metadata history and selected historical/current content remain readable.
- Large proposal writes return compact native metadata and confirmed receipts
  without document text. Native immutable-type rejection and missing-artifact
  errors each send one request and preserve the native code and HTTP status.
- UTF-8 Markdown, binary values and an empty `application/json` file round-trip
  exactly. SHA-256 matches the complete source.
- A 1 MiB binary file can be uploaded and read through the bounded content tool.
  Its first chunk and whole-source digest match; a native GET also matches all
  bytes.
- A 1,638,400-byte (1600 KiB, about 1.6 MiB) binary file uploads under a 3 MiB
  request budget. The bounded read
  returns `upstream_body_too_large` with HTTP status 200. An explicit native GET
  matches all uploaded bytes. Raising the upload budget does not raise the read
  budget.

| Client path | UTF-8 tested | Binary tested | Tested decoded size | Status |
| --- | --- | --- | --- | --- |
| Pinned rmcp 3.8.0 SDK, loopback HTTP, mock Tines | 53-byte proposal | Caller-supplied base64 | 1 MiB with default budget; 1600 KiB with 3 MiB budget | Passed |
| Pinned rmcp 3.8.0 SDK, loopback HTTP, runner-authenticated native Tines | 53-byte proposal | All byte values, empty file, caller-supplied base64 | Up to 1600 KiB with 3 MiB budget | Passed |
| Intended OpenAI client through its tunnel | Not tested | Not tested | Unknown | Needs operator connection |

These are tested sizes, not measured maximum capabilities. No result establishes
support for opaque attachments, local paths or client-side binary conversion.
The fixture serializer produces about 1.40 MB for a 1 MiB base64 file and
2.18 MB for a 1600 KiB file; actual client envelopes can differ.

## Pinned upstream gate and freshness coverage

The following sources were inspected at
[`af6cb25febf1b45d7eff32251f78fcf6355aed4a`](https://github.com/tbuckley/tines/tree/af6cb25febf1b45d7eff32251f78fcf6355aed4a).
These are upstream test/source evidence, not a native gate scenario executed
through this adapter:

| Evidence | What it covers |
| --- | --- |
| [`artifacts.test.ts`, design loop](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/artifacts.test.ts#L244) | Service/database test: missing artifact returns `422 transition_requirements_unmet`; attachment permits transition; returning makes content stale; revision and reaffirmation each unblock; history remains readable and reaffirmed bytes match. |
| [`artifacts.test.ts`, narrowed requirements](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/artifacts.test.ts#L323) | Requirement checks distinguish missing artifacts and type/MIME mismatches, including MIME prefix matching. |
| [`artifacts.test.ts`, metadata and type validation](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/artifacts.test.ts#L650) | Slot type is immutable; incompatible payloads fail; metadata-only changes preserve the version count. |
| [`artifacts.spec.ts`](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/e2e/artifacts.spec.ts#L24) | Playwright scenario over native HTTP and local R2: missing gate, raw file upload, successful move, stale return, revision, reaffirmation and exact reaffirmed content. This describes inspected coverage, not a claim that this run executed it. |
| [`serializeArtifact`](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/artifacts.ts#L354) and [`checkRequirements`](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/artifacts.ts#L627) | Freshness compares current version creation time with state entry time. Requirements check presence, type, MIME prefix and freshness. |
| [Metadata-only upsert](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/artifacts.ts#L1307) and [`reaffirmArtifact`](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/artifacts.ts#L1595) | Metadata changes append no version and do not refresh content. Reaffirmation copies the current payload into a new version with `reaffirmed_from` and a new timestamp. |
| [Transition enforcement](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/issues.ts#L2473) | Unmet requirements reject before the state compare-and-swap. The artifact check and state write are not atomic; the source explicitly treats the gate as a process guard. |

At this revision, the [Typecheck and unit tests job](https://github.com/tbuckley/tines/actions/runs/38700810624/job/116308261464)
passed. The [overall run](https://github.com/tbuckley/tines/actions/runs/38700810624)
failed: Playwright shard 2 and the aggregate job failed. The issue's operator
reported a run-contributor context-write socket hang-up and concurrent-runner
fencing returning 409. This continuation confirmed job conclusions through the
GitHub API; downloading the aggregate job log returned Forbidden. It does not
claim that the entire upstream E2E suite, or any particular artifact E2E test,
passed. Upstream coverage does not establish deployed revision parity, adapter
transition dispatch, live gate permissions or intended-client transport.

## Repeat the native artifact test

First seed a disposable Backlog issue through the existing CLI or test setup.
Keep it out of active states. The test creates uniquely named artifacts and
leaves their history for inspection. Supply its immutable ID, a native origin,
and an authorized credential through the environment:

```sh
export TINES_ARTIFACT_TEST_URL=https://tenant.example
export TINES_ARTIFACT_TEST_ISSUE_ID=iss_disposable
# Set TINES_ARTIFACT_TEST_TOKEN through the operator's secret mechanism.
export TINES_ARTIFACT_TEST_REQUEST_BYTES=3145728
cargo test --locked --test artifact_acceptance \
  native_artifact_versions_and_sdk_upload_bytes -- --ignored --exact --nocapture
```

Omit `TINES_ARTIFACT_TEST_REQUEST_BYTES` to exercise the 2 MiB default. The native
test is ignored by the normal suite so CI never mutates an ambient account.

## Remaining gate and intended-client checks

The earlier run key rejected workflow creation with `run_key_forbidden`. No
custom workflow was created, and this continuation did not retry setup. Native
gate/freshness transitions through the adapter remain unexecuted. A separate
service is not required under the superseding direction. If an operator elects
to close this residual native gap, they can provision the disposable workflow in
[`artifact-workflow.json`](../../tests/fixtures/artifact-workflow.json), then seed
an issue in its Draft state. The fixture uses no active states and needs no
runner or consent changes. Apply it only to an authorized test account:

```sh
tines workflows create --no-prompts --file tests/fixtures/artifact-workflow.json
tines issues create <test-project> --workflow 'MCP artifact acceptance' \
  --title 'Disposable MCP artifact gate fixture' --state Draft
```

Use the production executor for these checks. Read the issue explicitly before
each transition and supply the native selector and required witnesses:

1. Attempt `Submit text` without an artifact. Preserve the native missing-gate
   details and prove that no attachment or retry occurs.
2. Upload a Markdown file to `proposal-wrong-type`. `Submit wrong type` must
   remain rejected because a file does not satisfy a text gate.
3. Create `proposal-text` as text with `text/plain`; observe the MIME mismatch.
   Revise it with `text/markdown`, then take `Submit text` to Review.
4. Return to Draft. The earlier content must be stale. A metadata-only PUT must
   leave its version and stale status unchanged. Reaffirm current content, then
   verify the new version, `reaffirmed_from`, freshness and successful submission.
5. Repeat a review round using revised text. Verify historical bytes separately
   from current content. Exercise `Submit file` with an actual Markdown upload.

The intended-client requirement needs operator access or a scoped decision.
Supply the intended OpenAI client/version and its configured MCP tunnel connection,
plus an authorized disposable issue and effective adapter request budget. No such
connection was supplied to this run. SDK evidence cannot fill this gap. If access
cannot be supplied, the requirement owner must decide how to handle the incomplete
acceptance; this record grants no waiver.

With that access, connect the intended OpenAI MCP client through its configured tunnel.
Upload a UTF-8 proposal and read its metadata and bytes back. Test caller-supplied
base64 at specific sizes if that client supports it. Record the client/version,
tunnel path, configured adapter budget, exact tested sizes and observed failures.
If binary transfer is unsupported, record that limitation. Do not fetch a URL,
resolve a local path or stage an upload to compensate.
