# Proposal: Correct package response validation and recover the exact 0.1.2 draft

## Motivation

Correct the brittle package error-message comparison and provide one reviewed
creation opportunity for the existing v0.1.2 draft. Preserve the combined
implementation of tines-mcp/38 and tines-mcp/39 at main
`bf97287109eaab863d26ed82307d31db7c3c6a71`. This proposal addresses tines-mcp/40.
Proposal revision 2 addresses the two authority gaps in the Request Changes
review. Approval permits only the implementation scope explicitly approved by
the reviewer. It does not authorize live recovery or a credential/settings change.
The exact-draft path remains disabled until both authority decisions below are
resolved and all preflight evidence is verified.

## Problem statement

`Registry.lookup` in `scripts/release.py` requires `message == "Not Found"`
when accepting an unavailable-package observation. The supplied independent
incident inspection reports HTTP 404, a parsed object, `message_exact=false`,
`documentation_prefix=true`, and `link_absent=true`. It reports
`stage=no_push_attempted` and `push_attempted=false`. The original message was
not retained. Do not guess its text or manufacture an exact incident fixture.

`dispatch` currently derives creation eligibility from a successful push
handoff, v0.1.2, and `GITHUB_RUN_ATTEMPT == "1"`. Manual dispatch and later
attempts require positive package access. Correcting the predicate alone cannot
safely authorize recovery of the existing draft. Neither changing the version
constant nor removing the attempt restriction solves that authority problem.

### Independently observed records and limits

Read-only inspection on November 5, 2026 verified:

- [Run 42010011054](https://github.com/dstoc/tines-mcp/actions/runs/42010011054)
  is completed/failed, event `push`, attempt 1, run number 18, workflow ID
  `407725245`, source `bf97287109eaab863d26ed82307d31db7c3c6a71`.
- Job `127186429900` failed in the combined source/registry/build/push step.
  Subsequent attestations, acceptance, evidence retention, and publication were
  skipped. Reporting, diagnostics retention, and credential cleanup succeeded.
  Step metadata alone does not prove that no push occurred inside the failed step.
- Authenticated GET of release `435049346` returns tag `v0.1.2`, that exact
  target SHA, `draft=true`, `published_at=null`, and no assets. The Git ref API
  and independent `git ls-remote` agree on the exact lightweight tag target.
- Artifact `13406950249` is unexpired, named
  `adapter-release-diagnostics-42010011054-1`, size 819 bytes. Its API-reported
  digest is `sha256:31c9fa47eb29b8dcfbef51358a45d206359fce40f21e8b5226177e1f14362356`.
  Download followed the normal GitHub endpoint and returned `Forbidden`.
  The normal job-log download also returned `Forbidden`. Neither denial was
  bypassed. These download denials were observed during proposal v1 and its
  review. Revision 2 rechecked artifact metadata, not denied downloads. Archive
  bytes, archive digest, and no-push contents remain independently unverified.
- Complete authenticated release-workflow listing contained 18 runs, with the
  incident newest during v1 inspection. Complete release listing then exposed
  only the v0.1.2 draft.
  GET by tag for v0.1.1 returned 404. This is an observation about access, not
  proof that v0.1.1 was deleted or permission to recreate it. Preserve the
  already-published v0.1.1 without any mutation.

These observations are snapshots. Revalidate identity and authority before any
separately authorized execution. Retain supplied incident facts separately from
verified downloads. An inaccessible record cannot satisfy a machine admission gate.

Revision 2 independently rechecked the run, release, artifact metadata and remote
tag above. Their listed identities and states still match. Earlier job, listing
and download observations remain attributed to v1 and its review.

### Official endpoint semantics

The official [user package endpoint reference](https://docs.github.com/en/rest/packages/packages#get-a-package-for-a-user)
documents the exact metadata route and a 200 success response. It does not
specify a stable literal message for its 404 error body. The official
[REST troubleshooting guide](https://docs.github.com/en/rest/using-the-rest-api/troubleshooting-the-rest-api#404-not-found-for-an-existing-resource)
explains that 404 can hide a resource when authentication is insufficient.
Consequently, neither a different message nor the old literal establishes
absence or publishing authority. A hidden package can satisfy the same local
error-shape checks as an absent package.

GitHub's [variables reference](https://docs.github.com/en/actions/reference/workflows-and-actions/variables)
specifies that run ID and run number remain unchanged on rerun, while run
attempt increases. A new manual dispatch has a fresh first attempt. An
attempt-number check alone cannot make repeated dispatches a single opportunity.

## Proposal

### Correct only the literal-dependent predicate

In `Registry.lookup`, replace exact message equality with a present string
whose stripped value is nonempty. This is a conservative local error-shape
requirement, not a newly claimed GitHub absence contract. Preserve:

- Authenticated GET to fixed TLS origin `api.github.com`, exact path
  `/users/dstoc/packages/container/tines-mcp`, and HTTP status exactly 404.
- Bounded strict JSON parsing, duplicate-key and invalid-constant rejection,
  object requirement, the existing string documentation URL/prefix check, and
  a truly absent Link header. An empty Link header still fails.
- Separate creation eligibility. Return an unavailable observation, never an
  assertion of absence or authorization. Do not classify message words or add
  a list of allowed English spellings.
- Positive-access package identity checks and complete active-version
  enumeration. A versions endpoint 404 remains an error in every mode.

Do not relax the documentation prefix in this issue; its incident predicate
passed. Missing, null, non-string, empty, or whitespace-only messages fail.
Unexpected error strings, including access-related text in a 404 object, cannot
prove authorization. Inside an authorized creation opportunity their ambiguity
remains the explicit owner assumption; outside it they always fail closed.
Because the original message is unknown, this proposal does not promise that
the exact incident body will pass the new string check.

Retain `message_exact` as historical diagnostic information. Add fixed boolean
or null fields `message_string` and `message_nonempty` to the metadata snapshot,
and increment that snapshot schema version. Keep the old incident snapshot
readable as version 1. Persist through the current synchronous atomic sink,
including rejection and transport/parse failure. No raw message, body, header,
URL, arbitrary response key, exception, or message hash enters diagnostics.
The production predicate and its diagnostic flags must use the same calculation.

### One incident, one creation opportunity

Remove the old automatic v0.1.2 creation allowance from the corrected workflow.
All normal push handoffs, ordinary dispatches, later attempts, other drafts,
and later versions retain strict positive-access behavior. No corrected helper
may reinterpret a replayed old handoff as fresh first-package authorization.
Already-deployed historical workflow bytes cannot be revoked by this patch;
the owner must not rerun or dispatch historical workflows.

Extend the existing `release.yml` dispatch with one optional incident selector,
accepting only literal `42010011054`. This selector requests validation; it does
not establish authority. Keep tag and expected source SHA inputs. Do not add a
generic force, skip-validation, bootstrap, or allow-404 switch.

The implementation must pin a single future run number in reviewed code for
workflow ID `407725245`. Select that concrete number during final implementation
review from a fresh run listing and the expected merge-triggered push. Do not
compute it as “current run” or “latest plus one” at execution, accept it as an
input, or leave a wildcard/default. If intervening activity consumes the number,
stop and seek another reviewed adjustment. The number is deliberately not
chosen in this proposal: implementation/review pushes can change the counter.

Creation requires all of these conditions before any build or push:

1. Event `workflow_dispatch`, `main`, fixed repository, attempt exactly 1,
   matching incident selector, and the reviewed run-number pin. Independently
   GET the current run and verify workflow ID, run number, run ID, attempt,
   event, branch, and head SHA against the executing workflow identity.
2. Exact release ID `435049346`, tag `v0.1.2`, version `0.1.2`, and source
   `bf97287109eaab863d26ed82307d31db7c3c6a71`. Select through complete authenticated
   release enumeration and validate the same numeric release directly. Require
   an unpublished non-prerelease draft and no preexisting acceptance asset for
   the creation branch. Verify local/remote tag and main ancestry normally.
3. Verified original no-push evidence, as specified below. No missing record,
   default false value, failed download, or supplied selector substitutes for it.
4. Fresh, separate owner/publication authorization for this exact workflow
   commit, run opportunity, draft, source, package and image tag, with owner
   absence and exclusive-writer checks completed. Record the authorization
   reference in allowlisted recovery evidence. A string reference records the
   operator decision; it does not cryptographically establish owner consent.
5. The ordinary initial registry observation and immediate pre-push recheck
   both pass the corrected unavailable-response validation. A package appearing
   after initial unavailability stops this attempt. A positively accessible
   package uses existing complete enumeration and digest selection rules.
6. Both authority decisions below are resolved. The fixed-source/default-workflow
   comparison and a reviewed permitted publication path are verified before
   build/push. Missing authority keeps the incident path disabled.

Pinning the workflow identity and one run number consumes the opportunity even
if that attempt fails before push. A rerun has attempt greater than 1; a new
dispatch has a different run number. Both fail the creation gate. Deleting a
run, losing access, or later deleting a package does not reopen it. Require the
owner to preserve workflow/run identity and evidence; this is not protection
against an administrator replacing the workflow itself. After resolution,
remove the incident-only branch through review.

This adds one conservative admission check, not a publication database or
history scanner. The tradeoff is intentional: unrelated workflow activity can
consume the planned number and require fresh review. Prefer that inconvenience
to an automatically reusable creation bypass. No new token or settings change
is authorized by this revision. The Actions permission decision below must be
resolved explicitly before enabling this admission path. A denied read stops it.

### Decision 1: Actions artifact download authority

The existing release job in `.github/workflows/release.yml` grants `contents`,
`packages`, `attestations`, and `id-token` write permissions. It omits `actions`.
Under GitHub's [workflow permission rules](https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax#permissions),
permissions omitted from an explicit map become `none`. The
[artifact download endpoint](https://docs.github.com/en/rest/actions/artifacts#download-an-artifact)
requires Actions read permission and does not document the public-resource
exception offered by some metadata endpoints. Successful public metadata reads
and Actions artifact upload do not establish archive download authority.

Request an explicit review decision between these alternatives:

- **Retain the current permission boundary (default).** Do not grant Actions
  permission. Implement the predicate correction and fixture-based recovery
  validation, but keep exact-incident machine recovery disabled. Do not select
  an executable creation opportunity or substitute a metadata-only gate.
- **Approve a narrowly scoped `actions: read` grant.** Add it only to the existing
  `release` job, for original-artifact and run/attempt verification. Do not add
  it at workflow level or to `release-please`; do not grant Actions write.
  GitHub job permissions are static: every invocation of that job receives the
  grant, even when the incident selector is absent. This is a real permission
  expansion and requires explicit approval; the selector does not scope the
  token. If that scope is unacceptable, use the default disabled alternative.

The grant is proposed for review, not applied or presumed approved here. It does
not resolve this runner's separate Forbidden result or prove that future
repository-token downloads will work. Permitted original-byte access must be
verified separately. Keep the byte gate unchanged on denial. Neither alternative
permits other credentials, a download bypass, dropping evidence checks, or a
reference string in place of original bytes. Reverting a future grant removes
future access, but cannot undo reads already performed.

### Original no-push evidence

Before enabling the live opportunity, an authorized reviewer must obtain the
original archive through permitted access and verify its exact ID, name, size,
API digest, run/attempt/source association, and downloaded archive digest. Retain
the sanitized original record and a review reference without response bodies
or credentials. The implementation must read the pinned original artifact through
the normal authenticated API, fail on denial, and validate the archive with
bounded size, entry count and decompression limits. Read the expected record
entry without extracting arbitrary paths; reject duplicates and unsafe entries.

Require explicit `push_attempted:false`, `stage:no_push_attempted`, no selected
digest, matching source/tag/release/run/attempt, incomplete acceptance, and an
initial-authority failure with the reported predicate snapshot. Inspect the
original code ordering to confirm the persisted pre-push marker precedes the
only push command. Reject contradictions and incomplete records. Authenticate
original run and job identity and enumerate its attempts; any later attempt
requires separate evidence and review, and blocks this exact opportunity until
resolved. A later attempt is not covered by attempt 1's no-push record.

The archive is not a namespace inventory. The owner must also account for all
later publication activity, unresolved pushes and external writers. Missing
historical evidence is a blocker. If the original bytes remain denied, prepare
and review code using fixtures, but do not enable or execute recovery. Do not
use alternative credentials or another download route to bypass the denial.

### Decision 2: Retained-target publication authority

`Release.finalize` uses `GITHUB_TOKEN` to PATCH this same release with
`{"draft":false}`. GitHub's [update-release contract](https://docs.github.com/en/rest/releases/releases#update-a-release)
requires workflow-write authority when the retained target's workflow files
differ as documented from the default branch. This applies even when the PATCH
does not change `target_commitish`. `GITHUB_TOKEN` cannot receive that authority.
The dispatch-input edit proposed here changes `release.yml` relative to the fixed
v0.1.2 source once merged. The existing warning in `docs/releases.md`, under
Owner settings and publication authority, therefore applies to this recovery.

Before any creation attempt is authorized, perform a read-only comparison of
all `.github/workflows/` files at the fixed source, the reviewed orchestration
commit, and the current default-branch commit. Record exact commit IDs and the
changed paths/blob identities, including additions/deletions. Revalidate the
comparison against the current default branch immediately before execution.
Missing comparison data or denied access blocks creation before build/push.
Do not use a live PATCH as an authority probe.

For the proposed workflow edit and unchanged source, the current repository-token
publication path is blocked. Owner consent and `actions: read` do not supply
workflow-write authority. Keep live recovery disabled unless a permitted path
for publishing this exact draft/source is separately identified, reviewed and
authorized before any creation attempt. This proposal does not supply that path
or authorize new credentials, a workflow-write grant, a settings change, or a
manual publication operation. Do not build/push first and leave this known
publication blocker for later.

The unresolved owner decision is how to obtain a permitted publication path
while preserving this exact release, tag and source, or to leave recovery
disabled. Surface that decision separately; do not retarget, recreate, delete,
move the tag, or silently choose another version. Preserve v0.1.1 untouched.
If authority later changes unexpectedly, a final PATCH 403/404 must preserve the
same target, tag, asset bytes/ID/digest and selected image digest, record the
publication uncertainty, and stop without automatic retry or repair.

### Owner preflight and publication authorization

After proposal approval, implementation CI and independent review, obtain a
separate explicit owner decision before merge/dispatch steps that could release:

- Verify authoritative owner visibility shows the exact package absent if the
  creation branch is needed. Access-filtered lists, a token's 404, and this
  incident diagnostic do not establish absence. If a package exists, use
  positive-access recovery or investigate; do not overwrite it.
- Exclude other package, image-tag, source-tag, release and asset writers for
  the entire operation. Confirm no unresolved earlier push or recovery attempt.
  Preserve the fixed workflow identity/counter and forbid historical reruns.
- Approve the exact reviewed workflow SHA, source SHA, draft ID, tag, selected
  run number and evidence pins. Confirm the planned counter still applies
  immediately before dispatch. Approval of this proposal is insufficient.
- Verify the draft/tag still match, original no-push bytes are readable and
  verified, and Actions-token reads work under the explicitly reviewed permission
  choice. Verify the fixed-source/default-workflow comparison and separately
  reviewed permitted publication path before build/push. Confirm durable
  evidence/attestation tools remain available. Report each denied read exactly.
- Decide package visibility/Actions association separately if required. The
  workflow must not change them. Anonymous runtime acceptance remains mandatory
  and failure leaves the release unpublished.

Without these facts, decline execution and retain the draft. Do not silently
choose a new version. A new patch version is a separate owner decision if this
narrow recovery cannot be authorized.

### Preserve the publication contract

Reuse `Release.publish`, `accept`, `retain`, and `finalize`. Do not duplicate the
build/push path. Keep exact-source checkout, Cargo version checks, local image
identity/runtime checks, pre-push tag/draft/registry checks, one push, and the
persisted uncertainty marker before dispatch. Require positive metadata and
matching digest after push. Any possible push permanently ends this allowance.

Existing images use zero builds, pushes, retags or new attestations. Missing or
conflicting signed evidence stops recovery. Keep both signed statements,
original signer/source verification, anonymous digest pull and runtime checks,
credential isolation and cleanup, complete asset enumeration, pinned evidence
readback, required Actions retention, and draft publication last. Never move,
delete or recreate this release or tag. Preserve already-published v0.1.1.

Keep sanitized failure diagnostics separate from durable successful acceptance.
Add only explicit incident/run/evidence/authorization coordinates to allowlisted
recovery and acceptance records; update strict validators coherently. Preserve
old valid evidence readability and exact asset-byte reuse. No diagnostic may
claim a successful push, absence, acceptance or publication from a request intent.

## Non-goals

- Live retry, cancellation, dispatch, build/push/retag, release/tag mutation,
  publication, deployment, credential change or settings change in this issue.
- Reconstructing the unretained message or proving absence from error text.
- General recovery creation, automatic counter renewal, historical rerun
  authorization, new release versions, or repair of published v0.1.1.
- Replacing denied downloads, repairing an ambiguous push, re-signing existing
  bytes, overwriting assets, or changing dependencies.
- Applying a token-permission expansion in this proposal revision. Only the
  explicit `actions: read` alternative is submitted for review; absent approval,
  retain existing permissions and keep machine recovery disabled.
- Solving workflow-write publication authority through other credentials,
  retargeting, release recreation, settings changes or a live PATCH probe.

## Suggested implementation shape

1. Obtain proposal review, especially for the new message-shape policy and the
   one-run-number admission tradeoff. Obtain an explicit Actions permission
   decision. Carry the unresolved publication-authority blocker forward; keep
   recovery disabled until a permitted path is reviewed separately.
2. Change the predicate and bounded diagnostics in `scripts/release.py`; retain
   negative fixtures and version-1 diagnostic parsing for the original incident.
3. Add the exact incident admission and read-only evidence checks to resolve,
   before registry/build work. With unresolved authority, keep the production
   incident path disabled and exercise it only with fixtures. Select a concrete
   run-number pin at final implementation review only when the authority and
   original-byte gates can be satisfied; otherwise retain no live opportunity.
   Reuse existing fixed-origin API patterns, but keep Actions/archive handling
   bounded and separate from package observations.
4. Update `.github/workflows/release.yml`, `tests/release/test_release.py`,
   `test_package_responses.py`, `test_draft_release.py`, and the actual workflow
   expression tests under `tests/release-please/`. Update `docs/releases.md` with
   the approval boundary and exact-draft procedure, without an executable live
   command that implies current authorization.
5. Obtain implementation CI and independent review. Only a separately authorized
   operator may conduct preflight and the pinned dispatch only after the Actions
   decision, verified original bytes and permitted publication path are resolved.
   Remove the incident-only creation branch after resolution through a follow-up
   review.

## Verification

Baseline: the combined implementation passed all 90 Python release tests during
v1 preparation and again during revision 2. No runtime code changed. Proposal checks do not establish live
Actions-token authority or validate inaccessible archive contents.

Implementation must demonstrate these observable outcomes using fixtures and
captured commands, with no live publication:

- Synthetic varied nonempty messages pass only the eligible exact-route 404
  shape. Null/missing/empty/blank/non-string messages fail. Keep hostile text
  out of stdout, report, record and durable evidence. Assert legacy
  `message_exact=false` can coexist with a passing new shape. Do not call the
  synthetic fixture the incident body.
- 401/403/429/5xx, redirects, wrong routes, empty/present Link, invalid JSON,
  duplicate fields, invalid constants, oversized bodies, bad documentation and
  transport failures yield zero builds/pushes. Every version-list 404 fails.
- Hidden-package and absent-package fixtures can return identical 404 bodies;
  they confer no authority without the explicit owner assumption. Existing
  package identity and all pagination checks remain effective.
- Exact recovery selection passes only for the pinned repository/workflow,
  dispatch/main/run number/attempt, incident, source and draft. Reject old push
  handoffs, original run reruns, attempt 2 of the selected dispatch, another
  first-attempt dispatch, changed source/draft/tag, later version, wrong workflow,
  missing/malformed API identity, denial, and caller-supplied counter overrides.
  Evaluate the actual YAML conditions and environment, not a copied expression.
- Assert permissions from the actual workflow YAML: the default disabled option
  retains the existing map; if explicitly approved, only the `release` job gains
  `actions: read`, with no Actions write or workflow-level expansion. Test missing
  permission and artifact/run/attempt 401/403/404/410 responses: zero builds,
  pushes and release mutations, no alternate credentials or metadata fallback.
- Compare fixed-source, reviewed and default workflow trees using read-only
  fixtures. Changed workflow files with only `GITHUB_TOKEN`, unknown authority,
  stale comparison or denied comparison reads block creation before build/push.
  Owner approval alone and Actions-read approval alone cannot make this pass.
- Retain and extend `test_documented_workflow_update_authority_denial_preserves_everything`
  in `tests/release/test_draft_release.py`: final PATCH 403/404 preserves exact
  source target, tag, release ID, acceptance asset ID/bytes/digest and image
  digest. Capture one failed PATCH, zero automatic retries/repair, no tag/release
  recreation and no second push. Retain sanitized diagnostics and uncertainty.
- Original artifact denial, disappearance, replacement, digest/size/run mismatch,
  duplicate/unsafe ZIP entries, decompression excess, missing fields, true or
  missing push marker, selected digest, contradictory stage, later original run
  attempt or unknown evidence stops before build/push. A later failure cannot
  fall back to the original first-attempt no-push proof.
- A validated opportunity plus two unavailable observations builds once and
  pushes once, then requires positive package identity/digest, both signed
  statements, anonymous runtime, durable readback, Actions retention and final
  draft publication in that order. Assert all preexisting failure boundaries.
- Package appearance, moved source tag, changed draft, local image change and
  pre-push runtime failure yield zero pushes. Post-marker interruption, push
  errors/missing digest and post-push denial retain uncertainty and never reopen
  creation. Existing digest recovery produces no image or attestation writes.
- Published releases remain read-only. Missing evidence or frozen/conflicting
  assets stop without repair. Verify that no path mutates v0.1.1.

Run Python release tests, pinned Node updater/expression tests, shell syntax
checks, and actionlint against both workflows. Keep CI's Rust fmt, locked clippy,
locked tests and real local container smoke checks. Report unavailable local
checks accurately. Do not use live image publication as a test or wait for CI
before the implementation's Request Review transition.

## Success criteria

- Error validation no longer depends on an undocumented message literal; all
  unrelated route/status/parser/identity safeguards remain enforced.
- Only the separately authorized exact draft/source/run opportunity can create
  after verified no-push evidence and owner absence/exclusive-writer checks.
- Reruns, new dispatches and ambiguous pushes cannot renew that opportunity.
- Existing digest acceptance, attestations, anonymous runtime, durable evidence
  and final publication ordering remain intact; v0.1.1 is untouched.
- Actual-workflow permission assertions and authority-denial fixtures pass.
  Unresolved Actions access or retained-target publication authority leaves the
  live incident path disabled before build/push, without losing original evidence.
- Tests and independent review cover the changed contract and explicit limits.
