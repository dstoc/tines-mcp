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

- Native context conflicts, current-item details, and unchanged version witnesses.
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

## Provision the disposable native fixture

The two `native_` tests are ignored by default. They mutate fixture context,
unpark an issue, and cancel a run. They require a disposable local Tines server at
reference commit `af6cb25febf1b45d7eff32251f78fcf6355aed4a`. They reject remote
URLs and check `/api/version` before exercising tools. They never use the
supervisor's `TINES_API_KEY` or `TINES_API_URL`.

Use the reference repository's existing setup. From its root:

```sh
pnpm install --frozen-lockfile
PATH="$PWD/node_modules/.bin:$PATH" bash apps/web/e2e/server.sh
```

The server script builds the worker, recreates `.wrangler-e2e`, applies the native
D1 migrations, and seeds the test accounts. Its default address is
`http://127.0.0.1:8788`. Do not run it against a retained test database. See the
reference repository's `apps/web/e2e/README.md` for setup requirements.

Use that environment's existing API/browser and D1 fixture mechanisms to prepare
these objects before running the native tests. Keep setup outside MCP mutation
calls. Do not change the native authorization implementation to make a test pass.

| Environment variable | Required fixture |
| --- | --- |
| `MCP14_NATIVE_BASE_URL` | Local server origin, such as `http://127.0.0.1:8788` |
| `MCP14_NATIVE_OWNER_TOKEN` | Seeded owner's full-authority test key |
| `MCP14_NATIVE_PROJECT_TOKEN` | Same owner's delegated key: `projects.access=write`, scope only the test project; `workspace=none`, `control_plane=none` |
| `MCP14_NATIVE_RUN_TOKEN` | Unexpired, unrevoked issue-scope run key bound to the test project and run-key issue; native workspace/control-plane read authority |
| `MCP14_NATIVE_PROJECT_ID` | Test project owned by that owner |
| `MCP14_NATIVE_OTHER_PROJECT_ID` | A second project owned by the same owner, outside the delegated key's scope |
| `MCP14_NATIVE_RUN_KEY_ISSUE_ID` | Issue to which the issue-scope run key is bound |
| `MCP14_NATIVE_PARKED_ISSUE_ID` | Separate issue in the test project with `needs_attention=true` and `attempt_count>0` |
| `MCP14_NATIVE_CANCEL_RUN_ID` | Separate active `launching` or `running` run in the test project, owned by the same owner |
| `MCP14_NATIVE_RUNNER_ID` | Existing runner visible to the owner |

A delegated key can be created through native `POST /api/v1/api-keys` with this
permissions object. Keep the returned key in the environment, not in an issue
comment or committed fixture:

```json
{
  "version": 1,
  "projects": { "access": "write", "scope": ["<test-project-id>"] },
  "workspace": "none",
  "control_plane": "none"
}
```

The reference fixture `RUNROW` in `apps/web/e2e/constants.mjs` supplies an
issue-scoped run key and its bound issue. Verify its project ID before choosing
the test project and delegated scope. Its completed display run is unsuitable
as the cancellation target.

For pending cancellation, use a shared project and a distinct running run. At the
pinned revision, `requestSharedRunCancellation` records the request while the run
can remain active. Keep the run-key fixture distinct from the cancellation target:
cancellation revokes the target run's credentials. A real local fixture runner
must poll, stop its harness, and acknowledge cancellation. The native operational
test makes up to thirty explicit follow-up `get_run` calls, one second apart,
and requires eventual `canceled` status. It fails if the runner does not finish.
This test polling is outside the adapter. Each read is a separate tool invocation.

Use the native runner setup in `apps/web/e2e/runner.spec.ts` as the existing
fixture pattern. For the parked issue, observe `explain_dispatch` before and after
resume and record whether it was eligible, assigned, or still waiting. The test
checks that parking and attempts clear; it does not require a launch from the
resume response. Do not use the issue running this supervisor task as a fixture.

## Run the native checks

Set the table's variables through the local fixture setup. Then, from the adapter
repository:

```sh
export MCP14_NATIVE_DISPOSABLE=yes
cargo test --locked --test context_recovery_acceptance native_ -- --ignored --test-threads=1
```

Missing configuration fails the tests. It does not skip them. Reset the disposable
environment before repeating the operational test because its parked issue and
cancellable run have changed. Destroy the disposable environment after the run;
the context test intentionally leaves its uniquely named test items there.

The context scenario creates project-scoped guidance, edits it, sends stale PATCH
and append witnesses, compares each native conflict's current item, and reads back
the unchanged item. It then appends with the current version. It verifies denials
for global scope, another project, moving into global scope, and env writes
without control-plane write. It also verifies own-issue context creation and
project-context denial with an issue-scoped run key.

The operational scenario reads the runner registry, one runner, the project queue,
and settings. It checks native denials with the project-only key and settings
redaction with the run key. It verifies that denied resume calls preserve parking,
then resumes with the owner key. It checks cancellation denials, cancellation
acceptance, separate lifecycle reads, eventual cancellation, and rejection of an
already-ended run. Returned native status/outcome fields remain observations.

## Acceptance record

Record the adapter commit, reference commit, `/api/version` identity, exact test
command, results, credential permission shapes (never tokens), and the native
resume/cancellation observations in the issue thread. Include request-count
results from the executor suite. A passing mock test does not establish native
permissions or dispatch semantics.

The initial implementation run on 2026-11-05 could not start the disposable
reference server. Its `pnpm install --frozen-lockfile` failed with
`ERR_PNPM_FETCH_403` for
`https://registry.npmjs.org/prettier-plugin-tailwindcss/-/prettier-plugin-tailwindcss-0.8.1.tgz`.
No alternate registry, dependency downgrade, production recovery mutation, or
native acceptance claim was substituted. Both native tests remain unrun.
The context/recovery cutover gate remains incomplete until an operator provisions
the fixture and records passing native checks and dispatch observations.
