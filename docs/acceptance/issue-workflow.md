# Issue-workflow cutover evidence

The issue-workflow tools let an MCP client file a backlog graph, choose a Start
transition, edit prose, append comments, and manage existing labels and blockers.
The client makes each read and write as a separate call. Tines remains the
authority for permissions, graph constraints, revisions, consent, and dispatch.

**Status: HTTP acceptance passes; native acceptance is pending.** This record
belongs to tines-mcp/26 (`mcp-12`). It does not authorize a release or claim that
real shared-project consent has been verified.

## Revisions and evidence

Inspected on November 5, 2026:

- Adapter base: `0cbb9c3` (`dstoc/tines-mcp`).
- Reference source: `af6cb25febf1b45d7eff32251f78fcf6355aed4a`
  (`tbuckley/tines`). This is also the revision recorded in the native-shape
  fixtures. Those fixtures are source-derived examples, not captured live results.
- Native build tested: **none**.

Run the HTTP gate from the adapter root:

```sh
cargo test --locked --test issue_workflow_acceptance
```

The suite uses the production `/mcp` router and common dispatcher with a local
HTTP upstream. It records every outbound verb, URI, and JSON body, including
unexpected routes. Cancellation tests use the same common dispatcher directly
and hold the HTTP response until cancellation. They distinguish cancellation
before dispatch from cancellation after dispatch.

The tests cover:

- Four separate backlog creates forming a diamond DAG, with initial labels and
  known blocker IDs. Only a separate issue read and transition call select Start.
- Issue-content, relation, and transition response/receipt fixtures, including
  owner/member differences, native 204 deletions, and `blocked_by` direction.
- Permission, revision, artifact-gate, stale-label, and graph error forwarding,
  with no witness refresh, authority lookup, retry, or PATCH fallback. Native
  error messages retain the executor's existing 512-character bound.
- Zero requests for unadvertised fields; one request for failures and redirects
  across all ten issue-workflow tools. Every shipped old read, including issue
  aliases and both log modes, has zero-request validation and one-request native
  failure checks. Existing `read_tools` tests cover their success projections,
  query forwarding, and continuations.
- Unknown write outcomes for malformed success receipts and cancellation after
  dispatch. Cancellation before dispatch reports `not_attempted`.
- Exact P0 discovery definitions under both supported protocol revisions, using
  `tests/fixtures/issue-workflow-discovery.json`. Other domain modules can register
  independently; this fixture does not freeze the complete tool registry.
- Encoded immutable path IDs and literal related IDs in JSON bodies.

Existing comment HTTP tests already expect a direct POST and `meta.write`
outcomes. Existing old-read tests already count endpoint requests. This gate
requires no shared runtime activation change and does not gate artifact or
operational integration.

## Native verification to complete

Use a disposable native build of the inspected revision, with a fresh local D1
database. Keep production credentials and the supervisor run key out of it. The
reference `apps/web/e2e/server.sh` builds the worker, applies migrations, seeds
test identities, and starts Wrangler with `SHARED_EXECUTION:on`. It requires the
reference lockfile dependencies. Its seeded accounts are test-only. Do not use
the standard workflow as a presumed backlog workflow: its initial Open state is
active. Create a workflow with an explicit backlog state and a Start transition.

Record `GET /api/version`, the reference commit, and the adapter commit before
testing. Configure the adapter against the disposable worker. Use a recording
proxy or equivalent complete HTTP capture to count every adapter request; record
fixture setup and explicit inspection requests separately from each tool call.

Complete these checks through MCP:

1. Create a private fixture project, a backlog workflow, an existing label, and
   two independent named keys for the owner. Disable automatic execution or use
   isolated test runners. File root, left, right, and join issues in backlog.
   Give left/right the root blocker, and join both left/right blockers. Read the
   native graph and run list separately. Confirm all four are in backlog with
   no runs; there is no implicit Start or resume request.
2. Read the root and explicitly take its Start transition. Verify its new state.
   Read dispatch eligibility or runs separately if needed. Mutation acceptance
   alone does not prove that a run launched.
3. Edit the root title and description, then append literal Unicode/Markdown
   comment text. Read back separately. Confirm prose normalization is native,
   comment whitespace is preserved, and edits cannot force state.
4. Add an existing label, repeat the add, and remove it. Try a missing ID-shaped
   label. Add and remove blockers using both direction selectors and the returned
   link ID. Attempt a cycle and a duplicate link. Record native outcomes and
   unchanged graph state after rejected operations.
5. Set up a real shared project and member through native fixture setup. Record
   owner and member issue reads. Read owner consent separately: owner issue reads
   do not supply consent witnesses. Source member witnesses from `my_choice`.
   Use top-level `workflow_revision`, the exact transition ID, and all five
   `expected_*` witnesses for named-key transitions.
6. Change each witness independently after a preceding read: state, decision,
   workflow revision, consent revision, and consent epoch. Retry the stale
   invocation once as a deliberate test. Confirm `409 decision_refresh_required`
   and native details, unchanged issue/event/consent state, exactly one adapter
   POST, and no automatic refresh or fallback. Then perform a separate fresh
   read and a new explicit transition invocation.
7. Exercise browser-established personal consent in on, off, and unset states
   for owner and member. Confirm transitions preserve native permission receipts
   and do not grant personal execution permission. Establish consent with the
   native browser/session surface; MCP exposes no personal consent writer.
8. Repeat relevant reads/writes with a read-only key, a project-scoped key inside
   and outside its scope, an outsider, and a run key on its permitted issue and
   another issue. Verify native effective authority and denials, including the
   run-key denial from `get_issue_consent`. Do not substitute named-key behavior
   for run-key behavior. Record native state/events as well as API errors.

For each result, record tool inputs without secrets, HTTP method/path/count,
status, native error/details or receipt, and explicit before/after observations.
A failed write must not be assumed noncommitting from HTTP status alone. Preserve
unknown outcomes and inspect before deciding to repeat a mutation.

Once these native checks pass, add the tested build identity and evidence to this
record, complete the issue's review, and evaluate initial issue-workflow release
eligibility under the [approved proposal](../proposals/direct-api-mcp.md). P1/P2
integration gates remain independent.

## Environment blocker

On this runner, pnpm 10.33.0 installed successfully. Running
`pnpm install --frozen-lockfile` in the reference checkout failed with:

```text
ERR_PNPM_FETCH_403 GET https://registry.npmjs.org/magic-string/-/magic-string-0.30.21.tgz: Forbidden - 403
No authorization header was set for the request.
```

The attached `npm-dependencies` skill prohibits bypassing explicit access denials
with alternate registries, download tools, flags, or package managers. Docker and
Podman are absent, and no prebuilt native worker was available. A permitted
lockfile installation or an authorized disposable build is needed to finish the
native gate. No native test request was sent to the production Tines deployment.
