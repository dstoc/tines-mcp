# Issue-workflow cutover evidence

The issue-workflow tools let an MCP client file a backlog graph, choose a Start
transition, edit prose, append comments, and manage existing labels and blockers.
The client makes each read and write as a separate call. Tines remains the
authority for permissions, graph constraints, revisions, consent, and dispatch.

**Status: adapter HTTP acceptance passes; upstream semantics are mapped below.**
This record belongs to tines-mcp/26 (`mcp-12`). The issue's November 5, 2026
superseding testing direction permits this gate to proceed to review without a
separate native service. It replaces the disposable-build requirement in the
[historical proposal](../proposals/direct-api-mcp.md) and
[issue plan](../plans/direct-api-mcp-issues.md) for this adapter issue. Those
artifacts remain unchanged. Review can establish initial issue-workflow cutover
eligibility; this record does not claim a live native or deployment pass.

## Revisions and evidence

Inspected on November 5, 2026:

- Adapter base: `0cbb9c3` (`dstoc/tines-mcp`); acceptance implementation:
  `b02dd3ffb7062a7072957467787353d23caf6d4c` (PR #29). This follow-up changes
  only this evidence document.
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

The original 12 acceptance tests remain unchanged. On November 5, 2026,
`cargo fmt --all -- --check`, `cargo clippy --locked --all-targets -- -D warnings`,
and `cargo test --locked` passed with the installed Rust toolchain: **242 tests**
(the prior baseline had 230). No reference dependencies were fetched or native
service provisioned in this follow-up.

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
outcomes. `tests/write_tools.rs` also verifies confirmed receipts with omitted
oversized output, bounded untrusted receipt IDs, and unknown outcomes after lost
or truncated responses. `src/tines.rs` tests the complete serialized result cap,
escaping-heavy content, streamed body limits, cancellation and deadlines. These
checks remain in the 242-test result. Existing old-read tests already count
endpoint requests. This gate requires no shared runtime activation change and
does not gate artifact or operational integration.

## Upstream semantic evidence

All source links below pin `af6cb25febf1b45d7eff32251f78fcf6355aed4a`.
These are inspected source assertions, supported by the upstream CI job described
below. They are not tests run through this adapter against Tines.

| Contract | Inspected upstream tests/source | Evidence and boundary |
| --- | --- | --- |
| Initial blocker graph | [issue-create-links.test.ts](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/issue-create-links.test.ts#L47) | Asserts orientations, response links, events and one dispatch signal; repeated edges and requested cycles leave no create-owned rows. Does not execute the four-issue backlog scenario through MCP. |
| Graph races | [issue-links-concurrency.test.ts](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/issue-links-concurrency.test.ts#L228) | Asserts delayed cycle rejection, disjoint four-node cycles, acyclic concurrent additions, blocked_by normalization, duplicate precedence and rollback of link/events. This is the SQLite test harness, not native D1 concurrency. |
| Prose and scoped writes | [issues.test.ts](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/issues.test.ts#L87), [sparse patch tests](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/issues.test.ts#L318) | Asserts scope filtering, independent write domains and preservation of a concurrent transition during a title-only patch. The adapter forwards prose; it does not supply concurrency witnesses for prose edits. |
| Label semantics | [labels.test.ts](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/labels.test.ts#L214), [label route tests](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/routes/api/v1/issues/[id]/labels/server.test.ts#L61) | Asserts idempotent add, stale-ID rejection, atomic rejection of mixed invalid input and absent-label 404. Route tests assert add/remove dispatch wiring and 204 deletion over the test DB. |
| State and workflow witnesses | [issue-transition-pins.test.ts](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/issue-transition-pins.test.ts#L66) | Asserts detail-only revision fields, stale state visits and workflow revisions, run-key supplied witnesses, and one event per committed move. These tests explicitly use a nonshared project. |
| Shared exact decisions | [member-decisions.test.ts](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/member-decisions.test.ts#L106), [exact decision test](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/member-decisions.test.ts#L254) | Asserts current membership and named-key project scope, exact five-witness transitions, stale replay rejection and no member execution admission; an owner run key can transition its own issue without a witness. |
| Personal consent | [personal-consent.test.ts](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/personal-consent.test.ts#L43) | Asserts browser-only choices, revisioned on/off updates, stale intent rejection, Bearer-plus-cookie denial and no implicit key grant; Done/reset tests prevent restored consent on reopen. |
| Shared owner/member checks | [transitionIssue / transitionMemberIssue](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/issues.ts#L2080), [my-consent route](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/routes/api/v1/issues/[id]/my-consent/+server.ts#L9) | Source compares all five expected witnesses before shared named-key transitions, rejects key consent writes, and limits implicit consent to browser sessions without Bearer input. The consent GET explicitly rejects run keys. This maps source behavior, not exhaustive live witness-race results. |
| Restricted keys and run binding | [permissions.test.ts](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/permissions.test.ts#L29), [run-contributor.test.ts](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/run-contributor.test.ts#L301) | Asserts independent permission domains, issue/project/workspace run ceilings and commit-time binding checks for update, transition, comment, label and link writes. Does not establish the deployed middleware/authentication behavior. |
| Shared dispatch admission | [shared-launch.test.ts](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/supervisor/shared-launch.test.ts#L90) | Asserts guarded material/key minting, guidance-race rebuilding, release after repeated losses and competing poll behavior. The adapter HTTP tests neither run the supervisor nor launch agents. |

The upstream [test DB harness](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/test-db.ts#L1)
applies real migrations to in-memory `node:sqlite`, implements transactional
`DB.batch`, and checks D1 parameter limits. This gives substantive SQL guard and
event evidence. It does not reproduce the worker, native D1 scheduling, network
failures, browser sessions, or a complete supervisor lifecycle.

For explicit shared-project decisions, use top-level `workflow_revision` and the
exact transition ID. Owner issue reads omit personal consent witnesses; obtain
`my_agents.revision` and `my_agents.epoch` with a separate `get_issue_consent`
call. Member reads expose `my_choice.revision` and `my_choice.epoch`. Supply all
five `expected_*` witnesses from those observations. A stale rejection does not
trigger a refresh: any new read and transition are separate client invocations.
Run keys follow their native exception and cannot read personal consent.

## Upstream CI and E2E limits

The [Typecheck and unit tests job](https://github.com/tbuckley/tines/actions/runs/38700810624/job/116308261464)
passed at the pinned reference revision. The inspected CI workflow runs `pnpm
check`, formatting and `pnpm test` in that job. GitHub run metadata was read in
this follow-up and confirms the revision and job results. This is external CI
evidence; the reference tests were not rerun locally.

The [overall run](https://github.com/tbuckley/tines/actions/runs/38700810624)
failed: Playwright shards 1 and 3 succeeded, shard 2 failed, and the aggregate
Playwright job failed. The issue's continuation instruction reports two failures:

- A run-contributor context-write socket hang-up. The inspected
  [run-contributor E2E](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/e2e/run-contributor.spec.ts#L24)
  covers native D1 contributor binding with live and raced comment, transition,
  context, journal, link, prose and create operations. Context writes belong to
  another adapter gate, but this failure limits confidence in the shared native
  run-key HTTP path used by issue writes too. A socket hang-up does not establish
  whether a write committed. The passing unit tests do not close this gap.
- Concurrent runner fencing returned 409. The inspected
  [runner-fencing E2E](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/e2e/runner-fencing.spec.ts#L19)
  expects both previously unseen concurrent poll IDs to succeed, then one
  follow-up to be fenced. It separately expects a stale predecessor to return
  409. The reported failure concerns concurrency at the native poll boundary;
  it prevents a claim of validated native runner ownership and dispatch. Runner
  polling is outside the issue-workflow adapter surface.

The failure details above come from the continuation instruction, checked for
relevance against source. Downloading failed-job logs returned Forbidden in this
run; no alternate download was attempted. The precise failing request, server
cause and commit outcome were not independently recovered. Do not label these
failures harmless, resolved, or a native lifecycle pass. Other upstream E2E
sources, including `member-decisions.spec.ts` and `api-keys.spec.ts`, are not
individual passing results inferred from the aggregate run.

## Residual native and deployment gaps

The HTTP gate executes the production MCP router, common dispatcher and HTTP
executor, with exact request/count assertions. Its upstream supplies fixtures;
it does not implement Tines authentication, storage, consent or dispatch.
In particular, the backlog diamond/Start test proves the client call sequence
and absence of hidden requests. It does not prove persistence of a native DAG,
absence of native runs before Start, or launch of a run after Start.

These checks remain unexercised through this adapter against native Tines:

- Persistent graph/label/prose/comment effects and event atomicity, including
  concurrent edits, cycle rejection and duplicate operations.
- Shared owner and member transitions after independent state, decision,
  workflow, consent-revision and consent-epoch changes; before/after state,
  events and consent observations after each rejection.
- Real read-only, project-scoped, outsider and run-key authentication and effective
  authority, including denied consent reads and revoked contributor bindings.
- Browser-established on/off/unset consent and lifecycle resets, without personal
  execution permission being granted by an MCP key.
- Native supervisor admission, runner fencing, no-run backlog behavior, actual
  execution after Start, and the intended deployed MCP client/transport path.

Any later native verification must record `GET /api/version`, source and adapter
commits, secret-free inputs, all HTTP requests per tool call, native errors or
receipts, and explicit before/after state. Fixture setup and inspection reads
must be counted separately. Preserve unknown write outcomes after network or
receipt failures; inspect state before deciding to repeat a mutation. Such work
requires separate authorization and is not a prerequisite added back to this
issue's revised review gate.

## Dependency-access history and review scope

In the preceding run, `pnpm install --frozen-lockfile` failed with
`ERR_PNPM_FETCH_403` for `magic-string@0.30.21`. The attached dependency skill
prohibits bypassing explicit access denials. This continuation did not retry or
bypass that fetch, provision Tines, or mutate production fixtures. The supervisor
issue comments and review transition are workflow bookkeeping.

The revised gate consists of the preserved 12 HTTP acceptance tests, exact P0
discovery fixture, normal Rust checks, this upstream evidence map, and independent
review. It can support the first issue-workflow surface after review under the
superseding direction. Artifact and operational integration remain independent;
P1/P2 completion and the full-surface gate do not delay this review. Deployment
acceptance and the residual native checks above remain distinct evidence gaps.
