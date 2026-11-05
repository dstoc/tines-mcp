# Proposal: Remove adapter project exclusions

## Motivation

A token-authorized project must remain available through the MCP adapter. For example, when Tines returns two projects from `list_projects`, the adapter must return both. A direct read of either project must reach Tines, which decides whether the configured token can read it.

Douglas's direction in tines-mcp/13 makes the configured token's restrictions sufficient. Remove the adapter's project visibility policy without introducing another allowlist or permission system.

## Problem statement

At normal-base commit `7626a18f4de0572e0f4a70b676372a90753bcaa9`, `src/config.rs` parses and validates `TINES_EXCLUDED_PROJECT_IDS`. `src/tines.rs` stores the IDs in a set and exposes exclusion checks on each tool call. `TinesError::Visibility` produces a local `visibility` / `resource_unavailable` error without an upstream status.

`src/read_tools.rs` rejects explicit project selectors and filters returned projects, issues, events, context items, and runs. It also resolves issue ownership to enforce exclusions on issue children, artifact reads, effective context, journals, run details and logs, and dispatch explanations. These checks can hide records that Tines returned or stop an operation before its API endpoint is called.

The existing integration tests in `tests/read_tools.rs` configure exclusions through `configured_app` and assert these local rejections and filtered pages. `README.md` and `docs/proposals/tines-mcp.md` describe the policy. These expectations must change together.

## Proposal

Use the configured token and normal Tines API authorization for every existing tool. Keep the shared request client, route construction, validation unrelated to exclusions, response bounds, cancellation, deadlines, and explicit no-retry configuration.

### Configuration and errors

Remove the exclusion field, accessor, parsing, validation, and contribution to the configured/unconfigured decision from `src/config.rs`. Remove the exclusion set and methods from `src/tines.rs`. Remove `TinesError::Visibility` and its envelope mapping after removing its callers.

The removed environment variable has no effect, following the current behavior for unrecognized environment variables. Do not add a deprecated policy parser, replacement setting, or startup ownership discovery. Deployment instructions must tell operators to remove the old setting and configure the desired restrictions on their Tines token.

Preserve the shared client's upstream error handling. A refusal from a required upstream request retains its upstream status and error information within the existing bounded, sanitized envelope. Do not turn a Tines refusal into a local visibility error, successful empty result, or MCP authentication challenge.

### Reads and ordinary selection

Remove `reject_excluded`, `retain_visible_*`, context visibility helpers, and callers that exist only to enforce the exclusion policy. Remove issue ownership lookups that serve only those checks. Do not add requests to discover token permissions.

Return every item in each upstream page, preserving its native cursor and response projection. Preserve ordinary query selection: for example, `list_issues.project_id` still maps to the API's `project` query parameter. Keep archive, workflow, state, label, search, time, run-active, and other existing query semantics.

Keep useful identity checks, including requested versus returned context IDs, issue IDs, run IDs, and artifact issue/name/version/path identity. Keep checks needed to construct reliable links and bounded continuations. A field needed only to classify a record for exclusion must no longer be mandatory for that purpose.

Artifact and run helpers currently mix ownership checks with issue-link construction. Separate those responsibilities by subtraction: retain lookups needed for existing links and identity validation, and remove their policy branches. Preserve the current non-exclusion behavior for optional metadata: an unavailable optional run issue link can be omitted, while cancellation and timeout still propagate. Do not make optional link enrichment a new authorization gate. Artifact parent reads currently used for identity and links remain required, with upstream failures propagated normally.

Remove request-local parent-issue caching only if no remaining caller benefits from it. Do not remove shared request handling merely because the filter previously used it.

### Append-comment sequence

Implement this removal against the normal base, without importing open PR https://github.com/dstoc/tines-mcp/pull/12. The inspected base has nineteen read tools and no append-comment handler.

After this removal lands, tines-mcp/12 returns to implementation, rebases, and removes its comment ownership/exclusion gate against the simplified client. That issue must verify token-authorized comment creation and upstream denials. Preserve its append-only behavior, non-idempotent write annotations, input validation, and no automatic retries. This proposal adds no write operation.

### Documentation

Remove the exclusion configuration row and filtered-page explanations from `README.md`. Explain that endpoint callers receive the configured token's authority, with project restrictions enforced by Tines. Keep transport Host/Origin checks and private deployment guidance.

Update the original `docs/proposals/tines-mcp.md` sections that still prescribe project exclusions, local visibility errors, ownership filtering, and exclusion-specific acceptance tests. Keep legitimate API selection and pagination guidance. Link this proposal as the reason the earlier policy changed.

## Non-goals

- No replacement allowlist, alternate credentials, permission discovery, or authorization framework.
- No change to upstream permissions, API query semantics, transport authentication, or Host/Origin validation.
- No new tool, write operation, dependency, retry behavior, background work, or content redaction.
- No merge or implementation of PR #12 in the removal branch.

## Verification

Use the existing mock router and per-path response fixtures in `tests/read_tools.rs`.

- Replace filter-specific fixtures and assertions with coverage that preserves all token-visible projects and issue, event, context, and run rows, including historical and current project attributions and moved/shared-member issues.
- Assert direct project/issue reads and issue-child, artifact, context, journal, run/log, and dispatch reads reach their intended endpoints and succeed on authorized fixtures. Assert no extra ownership requests occur where ownership was needed only for filtering.
- Return upstream 401, 403, and 404 responses from affected required endpoints, including retained parent requests. Assert their statuses and upstream error source survive and no local visibility error or successful empty page appears.
- Preserve query-mapping assertions and cursor traversal, including an upstream empty page with a continuation. Keep identity mismatch, bounded content, changed digest, cancellation, and no-retry coverage.
- Remove the exclusion parameter from `configured_app`, obsolete config assertions, and tests whose only purpose is the removed policy. Verify config lookup no longer requests the removed variable, rather than maintaining an exclusion test configuration API.
- Search source, tests, examples, and operator documentation for obsolete exclusion configuration and enforcement. References explaining this removal may remain in this proposal.

Run the repository's required Rust checks:

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

Open the implementation PR with a conventional commit title and a tines-mcp/13 reference. The existing `.github/workflows/test.yml` must run its required `test` job, including container build, runtime UID, health, and shutdown checks. Do not wait for CI completion before the workflow's Request Review transition; report local results and any unavailable checks accurately.

## Success criteria

- No adapter project-exclusion setting, state, enforcement helper, filtering, or local visibility error remains.
- Token-visible records survive adapter reads; required upstream refusals retain their normal error semantics.
- API selection, useful identity validation, native pagination, bounded reads, and no-retry behavior remain intact.
- Deployment guidance describes token-based restrictions and no longer instructs operators to configure adapter project exclusions.
- Removal is independent of PR #12; its comment simplification remains explicitly assigned to tines-mcp/12.
