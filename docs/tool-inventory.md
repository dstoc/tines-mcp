# Tool inventory

The adapter advertises 41 implemented tools: 25 reads and 16 mutations.
All endpoints below are relative to `/api/v1`. Each data invocation prepares
one fixed native request and projects its response locally. Discovery and local
validation make no upstream requests. Pagination, authority reads, witness
refreshes and later status checks require separate client calls.

The complete discovery contract is pinned in
[`full-surface-discovery.json`](../tests/fixtures/full-surface-discovery.json).
It stores each description, input schema and annotation, plus the common output
schema once. `tests/full_surface.rs` reconstructs the complete definitions and
compares both supported protocol revisions. The fixture uses the default 2 MiB
request budget; upload and context descriptions reflect the configured budget.

## Reads

Reads have `readOnlyHint=true`, `destructiveHint=false`, `idempotentHint=true`
and `openWorldHint=true`. Repeated reads can observe changed native data.
Missing, null, redacted and unknown native fields retain their meaning.

| Tool | Native request |
| --- | --- |
| `list_projects` | `GET /projects` |
| `get_project` | `GET /projects/{project_id}` |
| `list_issues` | `GET /issues` |
| `get_issue` | `GET /issues/{issue_id}` |
| `get_issue` | `GET /projects/{project_id}/issues/{number}` |
| `list_issue_artifacts` | `GET /issues/{issue_id}/artifacts` |
| `get_issue_artifact` | `GET /issues/{issue_id}/artifacts/{name}` |
| `read_artifact_content` | `GET /issues/{issue_id}/artifacts/{name}/content` |
| `list_workflows` | `GET /workflows` |
| `get_workflow` | `GET /workflows/{workflow_id}` |
| `list_labels` | `GET /labels` |
| `list_events` | `GET /events` |
| `list_context` | `GET /context` |
| `get_context` | `GET /context/{context_id}` |
| `get_issue_context` | `GET /issues/{issue_id}/context` |
| `get_issue_journal` | `GET /issues/{issue_id}/journal` |
| `list_runs` | `GET /runs` |
| `get_run` | `GET /runs/{run_id}` |
| `get_run_log` | `GET /runs/{run_id}` |
| `get_run_log` | `GET /runs/{run_id}/log` |
| `explain_dispatch` | `GET /issues/{issue_id}/dispatch` |
| `get_current_api_key_authority` | `GET /api-keys/current` |
| `get_issue_consent` | `GET /issues/{issue_id}/my-consent` |
| `list_runners` | `GET /runners` |
| `get_runner` | `GET /runners/{runner_id}` |
| `get_supervisor_queue` | `GET /supervisor/queue` |
| `get_supervisor_settings` | `GET /supervisor/settings` |

`get_issue` supports two mutually exclusive selectors: `issue_id`, or
`project_id` with a positive `number`. `get_run_log(mode=tail)` reads run detail;
`mode=full` reads only the full-log endpoint. Full-log errors never trigger a
tail fallback. Artifact content reads do not resolve identity or version through
metadata. Omitted `version` selects current at that request.

`get_current_api_key_authority` reports the configured server token's effective
authority. It is never an automatic preflight. `get_issue_consent` observes
native consent; run keys can be denied this read. There is no consent grant tool.
Only `get_supervisor_queue` accepts a project filter; runner and supervisor
settings reads do not. Guidance journal reads return context, not activity logs.

## Mutations

Mutations have `readOnlyHint=false`, `idempotentHint=false` and
`openWorldHint=true`. All use the configured server token and native permissions.
The destructive hint is conservative; annotations do not grant authority.

| Tool | Native request | `destructiveHint` |
| --- | --- | --- |
| `create_issue` | `POST /projects/{project_id}/issues` | `false` |
| `edit_issue` | `PATCH /issues/{issue_id}` | `true` |
| `append_issue_comment` | `POST /issues/{issue_id}/comments` | `false` |
| `transition_issue` | `POST /issues/{issue_id}/transition` | `true` |
| `add_issue_labels` | `POST /issues/{issue_id}/labels` | `true` |
| `remove_issue_label` | `DELETE /issues/{issue_id}/labels/{label_id}` | `true` |
| `add_issue_link` | `POST /issues/{issue_id}/links` | `true` |
| `remove_issue_link` | `DELETE /issues/{issue_id}/links/{link_id}` | `true` |
| `put_issue_artifact` | `PUT /issues/{issue_id}/artifacts/{name}` | `true` |
| `reaffirm_issue_artifact` | `POST /issues/{issue_id}/artifacts/{name}/reaffirm` | `false` |
| `upload_issue_artifact_file` | `PUT /issues/{issue_id}/artifacts/{name}/file` | `true` |
| `create_context` | `POST /context` | `false` |
| `edit_context` | `PATCH /context/{context_id}` | `true` |
| `append_context` | `POST /context/{context_id}/append` | `false` |
| `resume_issue` | `POST /issues/{issue_id}/resume` | `true` |
| `cancel_run` | `POST /runs/{run_id}/cancel` | `true` |

Issue creation accepts initial labels and dependencies in one POST. An active
initial state can become eligible for dispatch. Use a known backlog state to
file without starting. Prose edits cannot change workflow or state and have no
native compare-and-swap witness. Labels must already exist; blocker links use
native graph checks. Link and label removal expect native HTTP 204.

`transition_issue` selects a native action or exact transition ID; it never
falls back to a state PATCH. For shared named-key decisions, use the exact
transition ID and all five expected witnesses: state ID, decision revision,
workflow revision, consent revision and consent epoch. The workflow witness is
`IssueDetail.workflow_revision`. Owner issue reads omit consent witnesses; read
`get_issue_consent` separately. Member reads can expose `my_choice` witnesses.
A stale rejection requires an explicit new read before a new decision. Personal
execution consent remains a browser/session operation.

`put_issue_artifact` creates text, link or PR artifacts and updates metadata.
Text accepts at most 262144 UTF-8 bytes. Metadata-only updates omit payload
fields and leave versions and freshness unchanged. Artifact type is immutable.
Content writes append versions even when bytes match; the native cap is 50.
`reaffirm_issue_artifact` copies the current payload into a fresh version,
not a caller-selected historical version. Neither tool has a conditional-write
witness. Returned current-version fields can include concurrent writes.
`upload_issue_artifact_file` sends inline UTF-8 or canonical padded standard
base64 to the raw file endpoint. It never opens filenames, fetches URLs or
resolves attachment handles. Descriptive metadata needs a separate explicit call.

Context creation accepts prompt, skill, repo or env items. Omitting every scope
dimension creates global context. Edit raw context IDs, not effective bundles.
Omitted PATCH fields remain unchanged; explicit null clears nullable fields.
Skill files replace the set. Supply observed `expected_version` for edit/append
conflict detection; omission retains native last-write-wins behavior. Append is
for prompt text only. Secret env values retain native write-only behavior.

`resume_issue` clears native parking/attempt state; it does not resume a harness
session or guarantee dispatch. `cancel_run` targets the supplied run only and
can report accepted cancellation while status remains active and outcome null.
Use a separate `get_run` call to observe eventual state.

## Results and limits

Results contain native `data`, optional `error` and adapter `meta`, through both
structured content and equivalent JSON text. Native status, bounded error
code/message/details and Retry-After survive when available. A write reports
`not_attempted`, `rejected`, `confirmed` or `unknown` in `meta.write.outcome`.
HTTP status alone cannot prove rejection. A fully validated success that only
exceeds the output limit stays successful with `data:null`, a compact receipt
and an omission notice. Inspect native state before repeating an uncertain
mutation. Cancellation and response loss do not roll back writes.

The serialized request budget is configurable (2 MiB default, 36 MiB maximum).
Base64 expansion, JSON escaping and the JSON-RPC envelope count. The decoded
native file cap is 25 MiB. Separate limits remain: 32 MiB success JSON, 1 MiB
native error body, 1 MiB inline content/log source, 32 KiB returned content chunk
and 512 KiB complete serialized MCP result. A file accepted for upload can be
too large for inline reading. Larger adapter budgets cannot enlarge client or
tunnel budgets. See [operator configuration](../README.md#configuration).

There are no generic HTTP, consent-write, usage/accounting, project/workflow
administration, label-creation, runner-administration, bulk orchestration,
remote-import, folder-upload, automatic retry or background-polling tools.
Returned prose, context, files and error details are data, not instructions.
