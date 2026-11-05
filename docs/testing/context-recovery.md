# Context and operational recovery acceptance

This gate checks that context writes and recovery tools preserve native Tines
results. API acceptance and run lifecycle are separate observations. A confirmed
`resume_issue` does not prove dispatch. A confirmed `cancel_run` can return a
running run with a null outcome.

The normative contracts are in [the approved proposal](../proposals/direct-api-mcp.md).
The integration gate is `mcp-14`, tracked as `tines-mcp/28`.

## Run the executor checks

From the adapter repository:

```sh
cargo test --locked --test context_recovery_acceptance
```

These tests use the production tool dispatcher, executor, and result serializer
against local HTTP servers. The transport test also calls `/mcp` for both
supported protocol revisions. No Tines credentials are needed.

The catch-all server records every request, including unexpected routes. Success
fixtures verify method, URL, body, configured bearer token, native data, and
receipts. Other checks cover:

- Representative native context conflicts, current-item details, and unchanged
  version witnesses forwarded from fixtures.
- Permission and run-ended errors without an authority preflight.
- Omitted fields, explicit PATCH nulls, and encoded path/query selectors.
- Zero requests for invalid input or cancellation before dispatch.
- One request for each dispatched write, including redirects, API failures,
  invalid receipts, cancellation, and lost or truncated recovery responses.
- A running cancellation receipt followed by a separate `get_run` invocation.
- Bounded conflict details and compact confirmed receipts for large results.

Socket tests inspect the intended mutation request and watch for an additional
connection after failure or cancellation. They do not simulate Tines authority,
parking, dispatch, or runner acknowledgement.

Context PATCH `version_conflict` is classified as `rejected` using the existing
endpoint-specific proof. Context append conflicts remain `unknown`: these tests
do not add a noncommitment rule. Neither classification permits an automatic retry.

## Evidence boundary for this issue

Douglas's 2026-11-05 direction in `tines-mcp/28` supersedes the separate native
service requirement in the original proposal, source plan, and earlier handoff.
Those documents remain historical design records. This gate uses executed adapter
HTTP/protocol tests plus inspection of existing upstream tests and source at a
recorded revision. Building or running another Tines service is not required.

Do not retry or bypass the denied package fetch, provision another service, mutate
production context/recovery state, or change credentials or settings for this
issue. Optional, already-accessible read-only smoke checks cannot prove mutation,
authorization, dispatch, or recovery lifecycle semantics. No live smoke check was
performed for this evidence record.

The eleven runnable adapter tests verify forwarding and request counts against
representative native fixtures. They do not execute native authorization or native
state transitions. In particular, a fixture returning `version_conflict` proves
that the adapter preserves the conflict and does not retry; it does not prove a
real Tines database rejected the stale write.

## Upstream evidence at the pinned revision

The reference checkout was inspected at
`af6cb25febf1b45d7eff32251f78fcf6355aed4a`. The links below pin that revision.
These are existing upstream tests and source, not tests rerun in this workspace.
Upstream tests use their own database fixtures, synthetic actors, and mocked
components; their coverage does not establish an adapter-to-live-service pass.

| Contract | Inspected upstream evidence | What it supports |
| --- | --- | --- |
| Context conflicts and disclosure | [Context handler tests](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/routes/api/v1/context/%5Bid%5D/server.test.ts#L129) | Exported PATCH/append handlers reject unauthorized writes before disclosing current data; stale witnesses return the actor-visible current item; commit-race cases preserve competing versions and do not write events for rejected edits. |
| Scope and run-key authority | [Permission tests](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/permissions.test.ts#L29), [context requirements](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/permissions.ts#L317), and [env context tests](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/context.test.ts#L1336) | Tests cover independent domains, missing authority, run-project ceilings, own-issue context mutations and env-write fencing. Source maps project/issue anchors to project authority, global/state/label anchors to workspace authority, and env writes to additional control-plane authority. |
| Resume acceptance and dispatch signal | [Engine resume tests](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/supervisor/engine.test.ts#L1480), [issue dispatch-effect tests](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/issues.test.ts#L553), and [resume source](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/issues.ts#L2680) | Tests cover clearing parking/attempts, idempotent event behavior, signaling after success/no-op, and no signal on a failed durable batch. Source requires project and control-plane write. A signal does not prove a runner launch. |
| Cancellation acceptance and errors | [Run API tests](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/runs.test.ts#L24), [engine cancel tests](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/supervisor/engine.test.ts#L1412), and [request wrapper](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/runs.ts#L713) | Tests cover a winning cancellation, already-ended rejection, failed-batch silence, and fake-adapter cancellation. Source requires control-plane and project write and reads run detail after acceptance. |
| Pending cancellation and acknowledgement | [Runner protocol test](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/runner-protocol.test.ts#L285) and [shared cancellation source](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/supervisor/engine.ts#L1712) | The test keeps an admitted shared run running until a matching cleanup acknowledgement; missing/wrong tokens do not settle it. Source records cancellation and revokes the target run's keys before terminal status. This does not exercise an actual harness stopping. |
| Operational reads | [Runner source](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/runners.ts), [settings authority](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/supervisor.ts#L172), [settings route redaction](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/routes/api/v1/supervisor/settings/%2Bserver.ts#L7), and [queue source](https://github.com/tbuckley/tines/blob/af6cb25febf1b45d7eff32251f78fcf6355aed4a/apps/web/src/lib/server/api/supervisor.ts#L415) | Source supplies native authority checks, project filtering for the queue, and run-key settings redaction. Adapter fixtures verify preservation of these shapes; this row is source inspection, not a live authority test. |

The [Typecheck and unit tests job](https://github.com/tbuckley/tines/actions/runs/38700810624/job/116308261464)
passed at this exact commit, including `pnpm test`. GitHub job metadata was checked
on 2026-11-05. The [overall workflow](https://github.com/tbuckley/tines/actions/runs/38700810624)
was not fully green: Playwright shard 2 failed. This upstream CI result supports
the inspected unit-test evidence; it is not complete end-to-end acceptance or a
local native-test pass.

## Two retained native scenarios: ignored and unrun

The following tests remain compiled and ignored in
`tests/context_recovery_acceptance.rs` as historical scenario specifications.
They are outside the revised completion gate and were not run in either issue
implementation run. There is no instruction to provision or execute them for
this issue. Their presence and successful compilation supply no native pass.

| Ignored test | Intended observations that remain unverified through the adapter against native Tines |
| --- | --- |
| `native_context_versions_scopes_and_run_key_authority` | Real create/edit/append version conflicts and unchanged readback; project/global scope denials; env writes without control-plane authority; own-issue versus project context with an issue-scoped run key. |
| `native_operational_permissions_resume_and_cancellation_observations` | Runner/queue/settings permissions and redaction; denied resume preserving parking; accepted resume clearing parking; dispatch explanation; cancellation denials and acceptance; separate run reads reaching eventual cancellation; already-ended rejection. |

The earlier run could not install the reference server's locked dependencies:
`pnpm install --frozen-lockfile` returned `ERR_PNPM_FETCH_403` for
`prettier-plugin-tailwindcss@0.8.1`. That failed setup is history, not a remaining
installation task. No alternate registry, dependency change, service provision,
or production mutation was substituted.

## Interpret operational results

- Treat a validated success receipt as API acceptance. For `resume_issue`, inspect
  the returned parking/attempt fields. Acceptance does not promise eligibility,
  assignment, or launch; those depend on subsequent native scheduling.
- For `cancel_run`, preserve the returned status and outcome. Shared active runs
  can remain `launching` or `running` after acceptance. A null outcome does not
  prove cancellation failed or that the harness stopped.
- To observe later state, make a separate explicit `get_run` call. Dispatch
  explanation is likewise a separate explicit `explain_dispatch` call. The
  adapter does not poll, preflight authority, retry a write, or issue recovery
  calls after cancellation or response loss.
- An `unknown` write outcome after dispatch requires inspection before deciding
  whether to make a new mutation. A status code alone does not prove no write
  committed. Never interpret a pending cancellation as terminal completion.

These are interpretation rules for authorized operations, not directions to
perform production mutations for this issue.

## Acceptance record and residual gaps

Record the adapter commit, required check commands/results, eleven adapter-test
results, two ignored/unrun scenarios, pinned upstream revision, and cited upstream
CI result in the issue and PR. Keep credential values out of evidence.

The revised gate establishes adapter protocol behavior and records relevant native
implementation/test evidence. It leaves adapter-to-native authorization,
transactional conflict handling, actual resume-to-dispatch, runner/harness
shutdown, acknowledgement timing, and eventual lifecycle convergence untested
end to end. Native deployment identity was not checked. Upstream unit tests and
mock receipts do not close those gaps. These limitations must remain visible
when reviewing or releasing the context/recovery surface.
