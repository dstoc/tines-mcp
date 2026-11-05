# Proposal: Append Tines issue comments through MCP

## Motivation

An MCP client that has read an issue should be able to append its findings to that issue. Add one tool, `append_issue_comment(issue_id, body)`, for this use case.

This proposal implements the direction Douglas approved for tines-mcp/12 on November 4, 2026: expose the tool normally and use the configured Tines token. The earlier investigation's enable flag, separate writer credential, project allowlist, and additional authorization framework are superseded. Those choices are settled and are not review questions in this proposal.

## Problem statement

At adapter commit `0fd267c5728185eec08e3deb5336059009b4ceea`, `src/read_tools.rs` registers nineteen tools through `ReadTools`. All are annotated read-only. `src/tines.rs` supports fixed GET routes, bounded responses, deadlines, cancellation, and native API error decoding. It already disables redirects and automatic reqwest retries.

Appending needs a fixed POST route and accurate write outcomes. A timeout, cancelled request, or failed response read cannot establish that no comment was created. The outer `TinesClient::run_tool` cancellation and deadline branches can drop the operation, so changing only the HTTP helper would leave misleading errors.

The reference repository at `af6cb25febf1b45d7eff32251f78fcf6355aed4a` implements `POST /api/v1/issues/{id}/comments` in `apps/web/src/routes/api/v1/issues/[id]/comments/+server.ts`. It accepts `{ "body": "..." }` and returns the native comment with HTTP 201. `createComment` in `apps/web/src/lib/server/api/issues.ts` checks native write permissions, inserts the comment and event, then reads back the comment. A failure after insertion can therefore occur even when the API returns an error. There is no comment idempotency key or expected-project request field.

## Proposal

### Tool and validation

Register `append_issue_comment` beside the existing tools. Require exactly two string arguments: immutable `issue_id` and `body`. Reject unknown arguments, missing fields, nulls, invalid types, invalid path segments, and a body containing only whitespace using existing argument and validation conventions. Do not accept issue numbers, project names, author overrides, or comment IDs as input.

Preserve the body verbatim, including leading whitespace, Markdown, quotes, and newlines. Match the API's nonblank check and maximum of 100,000 JavaScript UTF-16 code units; do not introduce the investigation's suggested 8 KiB body limit. The existing 64 KiB serialized MCP request limit remains in force and ordinarily imposes a smaller effective body size. Do not raise global request or result limits for this feature.

Set `readOnlyHint=false`, `destructiveHint=false`, `idempotentHint=false`, and `openWorldHint=true`. Append-only creation does not replace or delete existing data. The description must state that repeating the call can create another comment and that uncertain outcomes require checking the issue before deciding whether to append again. Keep every existing read tool's annotations intact.

### Issue identity and native permissions

Use the existing configured client, base URL, and token. Reuse the issue-child lookup pattern in `artifact_issue_link`: fetch the issue by immutable ID, verify its returned ID, and obtain the issue browser link when metadata is available. Extract a small shared helper so the comment handler uses the same identity check and link construction.

If the lookup fails or returns a missing or mismatched issue ID, return the existing error and send no POST. Project attribution is optional link metadata. Resolve it from either the flat owner shape or nested shared-member shape; missing attribution must not prevent an append. Native Tines permissions remain authoritative for the POST; propagate denials without credential discovery or extra permission probes.

PR #13 removed the adapter exclusion policy. This revision follows Douglas's November 5 direction to simplify the comment path against that removal. Do not restore ownership checks, project filters, or a replacement authorization framework. Do not perform a post-write lookup that could obscure an otherwise valid creation receipt.

### Request and receipt

Add a fixed `IssueComments` route to `Route`, with encoded issue-ID path segments. Add a JSON POST method to `TinesToolCall` that shares request admission, deadlines, authentication, bounded body reading, safe logging, and upstream error decoding with GET. Generalize only the private request plumbing needed for these two methods; do not expose arbitrary HTTP access as an MCP tool.

Send exactly one POST with only `{ "body": body }`. Keep `.retry(reqwest::retry::never())` and `Policy::none()`. Do not retry on transport errors, timeout, cancellation, HTTP errors, invalid JSON, missing receipt fields, redirects, or result-size errors. Do not add background completion, readback polling, or automatic duplicate detection.

Validate that the successful native result is an object with a nonempty comment `id` and the requested `issue_id`. Return the native object in `ResultEnvelope.data` and the verified issue link in `meta.links`, using the existing structured-content and JSON-text representations. Preserve native fields and timestamps. Link to the containing issue; do not invent a comment fragment URL. Apply the existing serialized result cap.

### Errors and uncertain outcomes

Use existing MCP tool execution errors, including `isError=true`, native status/code, safe details, diagnostic IDs, and bounded messages. Do not turn upstream 401 or 403 into MCP authentication challenges. Do not log or include the submitted body in adapter-generated errors.

Add a small optional write-outcome detail to the existing error envelope, rather than a separate error protocol. For this tool, use `error.details.write_outcome` with these values:

| Value | Meaning |
| --- | --- |
| `not_attempted` | Local validation, issue identity, configuration, admission, cancellation, or deadline failure occurred before POST dispatch. |
| `unknown` | POST execution began, but the adapter has no valid creation receipt or definitive native rejection. A comment may exist. |
| `created` | A valid creation receipt was obtained, but a later adapter failure prevented returning the ordinary success result. Preserve the comment ID when available. |

Preserve native API error status, code, and details. For a definitive native validation or permission rejection, explain that the API rejected the request. Treat ambiguous server errors conservatively as `unknown`: the reference handler can fail during readback after committing. A response lost after dispatch, malformed or oversized success body, invalid receipt, timeout, and cancellation after dispatch must likewise state that creation may have occurred. The message must direct the caller to inspect the issue before deciding whether to repeat the append. Never claim exactly-once delivery or roll back by deleting a comment.

Track dispatch and receipt state per tool call. Mark dispatch immediately before polling HTTP execution, after local checks and request construction. Both the outer `run_tool` cancellation/deadline branches and inner request errors must use this state, so they cannot disagree about whether a write was attempted. Record a validated receipt before result serialization. Keep read-tool errors unchanged. A cancelled MCP connection may not receive an error; cancellation still must not cause a second POST, and documentation must explain that losing the client response does not cancel a committed append.

## Non-goals

- Editing or deleting comments; other write tools.
- Enable flags, separate credentials or endpoints, a new allowlist, or permission-discovery machinery.
- Caller identity propagation, deployment permission changes, or a new authorization framework.
- Idempotency keys, reconciliation services, persistent operation state, or exactly-once guarantees.
- Upstream API changes or changes in the reference repository.

## Suggested implementation shape

1. Extend the fixed routes and bounded HTTP plumbing in `src/tines.rs`. Add request-local write outcome tracking and error details. Cover deadline and cancellation behavior before wiring the tool.
2. Add the typed append arguments, tool definition, and dispatch arm in `src/read_tools.rs`. Reuse the issue-child identity and link helper. Keep the existing module/type name for this change to avoid an unrelated rename.
3. Add `tests/write_tools.rs` with a local upstream that records method, path, JSON payload, and request count. Extend the existing discovery assertions in `tests/read_tools.rs` and `tests/protocol.rs` to distinguish nineteen read tools from the new append tool.
4. Update `README.md` to describe the twenty tools, configured-token write authority, limits, and uncertain-outcome handling. Keep the original read-only proposal as historical design context and link this extension where needed.

No new dependencies or deployment configuration are needed. The feature uses the existing client and transport limits.

## Verification

- Successful HTTP 201 returns the native comment ID and fields, equivalent structured/text envelopes, and an issue link. Assert exactly one POST, the immutable issue-ID path, configured-token use, and byte-for-byte body preservation.
- Missing, null, unknown, mistyped, invalid-ID, blank-body, and API-length-boundary inputs fail under the stated conventions. Check Unicode and escaped JSON within the unchanged 64 KiB transport limit. Verify no POST after local rejection.
- Owner and shared-member issues append through the configured client. Missing optional project metadata omits the link without blocking creation. Parent lookup failures and missing or mismatched issue identity send no POST. Parent 401, 403, and 404 responses retain their native status and code.
- Native 401, 403, 404, 422, 429 with Retry-After, and 5xx retain safe native error information. No case triggers a second POST. A simulated commit followed by a 500 reports uncertainty.
- A server that records the append and then drops or stalls its response yields `unknown`, with exactly one POST. Exercise response-body failure, malformed/oversized success JSON, mismatched receipt identity, cancellation, and deadline expiry. Separately verify pre-dispatch cancellation/admission failures as `not_attempted` and a valid receipt followed by a result-limit failure as `created` with its comment ID.
- Repeating two successful tool calls results in two creation requests; no deduplication or exactly-once promise is implied.
- Both supported protocol revisions discover twenty tools. Assert all four append annotations and retain the nineteen read tools' existing annotations. Keep the existing read behavior and tests intact.
- Run `cargo fmt --all -- --check`, `cargo clippy --locked --all-targets -- -D warnings`, and `cargo test --locked`, matching the repository CI. Use isolated local fixtures; do not test by appending to unrelated live issues.

## Success criteria

- One normally exposed append tool creates comments using the configured token and immutable issue IDs.
- Issue identity checks and native permission failures remain effective, with no adapter project policy or added authorization system.
- Every invocation sends at most one POST and reports uncertainty after ambiguous completion.
- Native creation receipts, issue links, accurate annotations, and the existing read suite pass the stated checks.

## Review

Proposal v1 was approved in tines-mcp/12 on November 4, 2026 (UTC). The implementation keeps native rejection errors without a write-outcome field, matches JavaScript whitespace validation, and preserves creation receipts through result-size errors. The package description and README describe the append tool alongside the existing read tools.

Douglas requested this revision on November 5, 2026 (UTC), after PR #13 landed. It removes the superseded exclusion preflight and keeps useful issue identity checks, native permissions, append semantics, annotations, and no automatic retries.
