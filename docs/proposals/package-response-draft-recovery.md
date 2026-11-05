# Proposal: Recover the existing v0.1.2 draft with the original workflow

## Motivation

Publish the existing v0.1.2 draft after normal implementation review and separate
owner authorization. Preserve release `435049346`, tag `v0.1.2`, source
`bf97287109eaab863d26ed82307d31db7c3c6a71`, and already-published v0.1.1.
This is proposal revision 3 for tines-mcp/40. It supersedes revision 2's disabled
alternative in response to scope-correction comment `cmt_9BfqQ0jOQWlbSP4E`.
It requests design review; it authorizes no live recovery or release mutation.

PR #39 merged the response correction and disabled preparation as
`6bb3fe5625c4b51746334e730864b0252286c066`. That does not complete this issue.
The next implementation should provide a working, narrowly bounded recovery
branch inside the existing helper, with no extra dispatch input or token grant.

## Problem statement

The failed release run rejected a structured metadata 404 because its message
was not the undocumented literal `Not Found`. The original message is unknown.
The merged fix validates a stripped nonempty string and retains exact route,
status, bounded strict JSON, documentation prefix and absent Link checks.
Keep that correction and sanitized snapshot v2; do not reopen unrelated parsing.
GitHub's [package reference](https://docs.github.com/en/rest/packages/packages#get-a-package-for-a-user)
specifies no stable error-message literal, and its
[troubleshooting guide](https://docs.github.com/en/rest/using-the-rest-api/troubleshooting-the-rest-api#404-not-found-for-an-existing-resource)
explains masked-resource 404s. A valid error shape establishes neither package
absence nor permission to publish.

The merged helper grants no unavailable-package creation authority. Its extra
incident selector rejects every recovery request, and its uncalled incident
module cannot execute recovery. The selector also changes `release.yml` relative
to the retained source, introducing a documented publication restriction.

### Evidence and limits

Independent read-only checks on November 5, 2026 establish:

- Draft `435049346` is unpublished, non-prerelease and mutable, with no assets,
  exact tag `v0.1.2` and the source SHA above. Independent `git ls-remote`
  confirms that tag and the v0.1.1 tag at
  `8853ca2e0af4ae78487443b3c6f0d9054c51ed66`.
- Original run `42010011054` is completed/failed, workflow `407725245`, number
  18, attempt 1, push/main, at the fixed source. Job `127186429900` failed in
  the source/registry/build/push step; later attestation and publication steps
  were skipped. Step metadata alone cannot prove no push.
- Artifact `13406950249` remains unexpired, named
  `adapter-release-diagnostics-42010011054-1`, size 819 bytes, associated with
  the original run/source. Its API-reported digest is
  `sha256:31c9fa47eb29b8dcfbef51358a45d206359fce40f21e8b5226177e1f14362356`.
  Earlier archive/log downloads on the runner returned Forbidden. They were
  not retried or bypassed. The runner has not downloaded or hashed those bytes.
- Scope-correction comment `cmt_9BfqQ0jOQWlbSP4E` reports that the supervisor
  read job logs through the authorized GitHub connector and observed
  `no_push_attempted`, `push_attempted=false`, a parsed 404 object,
  `message_exact=false`, and the other predicates passing. This is attributed
  authenticated log-inspection evidence, not a claim about downloaded archives.
- The original helper saves `push_attempted=true` immediately before its sole
  Docker push. The reported initial-authority failure precedes build/push.
- The complete release-workflow listing now has 19 runs. The later run
  `42016058890`, number 19, attempt 1, is the merge-triggered push at
  `6bb3fe5`; its release job was skipped. These are snapshots, not a continuing
  absence or exclusive-writer guarantee.
- That push also maintained Release Please PR
  [#40, v0.1.3](https://github.com/dstoc/tines-mcp/pull/40). It is an open candidate,
  not a selected replacement for this draft. Do not merge it as recovery for
  tines-mcp/40 or treat its presence as authorization for another version.

Douglas's reported package absence is dated owner evidence. It does not prove
present absence, exclude other writers, or authorize publication now.

## Proposal

### Restore the workflow and retain the existing interface

Restore `.github/workflows/release.yml` byte for byte from the fixed source.
Use only its existing `tag` and `source_sha` dispatch inputs on main. Keep all
existing job permissions, concurrency, actions, phases and final publication
ordering. Do not grant Actions read or add a workflow, environment or selector.
Remove the unused `scripts/release_incident.py` and its fixture-only admission
machinery; place the small called recovery checks in `scripts/release.py`.

The complete workflow comparison is:

| File | Fixed-source blob | Merged-main blob |
| --- | --- | --- |
| `.github/workflows/release.yml` | `e0889675fbfb7bd9663d7bf302b29520210eeb7b` | `a328d23116d2cdedbccc9395cdee1580d9d71b89` |
| `.github/workflows/test.yml` | `ccb5f1b8d2d7df9be7417ced80449430ef069557` | `ccb5f1b8d2d7df9be7417ced80449430ef069557` |

These are all workflow files in both revisions. Restoring the first blob makes
both complete trees identical, including path names and file modes. GitHub's
[update-release contract](https://docs.github.com/en/rest/releases/releases#update-a-release)
requires workflow-write authority when the retained target adds or modifies
workflow files relative to the default branch. Identical trees remove that
specific condition. This is a documented eligibility conclusion, not proof
that a future token can publish; there has been no live PATCH probe.

Before any recovery mutation or build, compare every workflow file at the
fixed source and executing workflow commit. Require the remote main SHA still
to equal the executing SHA, so the comparison also covers the current default
branch. Recheck before push, evidence upload and publication. Added, deleted,
changed, unreadable or stale workflow data stops the operation. Keep helper and
test changes outside `.github/workflows/`. A settings, policy or token denial
still stops safely without another credential or a repair attempt.

[Chime's pinned workflow](https://github.com/dstoc/chime/blob/7f5460dfa40497ef3b2aa00f529b7795542a4d54/.github/workflows/release.yml)
treats 404 as unavailable, rejects other API failures, enumerates accessible
versions and verifies provenance before digest reuse. Use that distinction and
existing-digest behavior. Retain this adapter's stronger metadata identity,
strict pagination, two signed statements and sanitized diagnostics. Do not copy
Chime's versions-route 404 fallback or raw-header logging.

### Admit only the exact existing draft

Normal push handoffs keep no unavailable-package allowance. The recovery branch
requires the fixed repository, workflow dispatch on main, attempt exactly `1`,
existing tag `v0.1.2`, fixed source and numeric release ID `435049346`. Validate
these through the existing complete authenticated release enumeration, direct
release GET, local/remote tag resolution, ancestry, separate source checkout,
Cargo manifest and lockfile checks. Require an unpublished, mutable,
non-prerelease draft with no acceptance evidence before any new image creation.
Other versions and drafts gain no allowance. Historical workflows cannot be
revoked by this patch; the owner must prohibit historical reruns and dispatches.

A positively observed existing version follows normal digest selection and both
attestation checks, with zero builds, pushes, retags or new attestations. Failure
to verify that digest never falls back to creation. A missing version, including
inside a positively accessible package, must pass the same exact-incident
creation guard below. This closes both creation branches.

### Consume creation authority once with an existing release-asset operation

Attempt 1 alone is insufficient: every fresh dispatch starts at attempt 1.
Use one fixed asset name on this draft, `adapter-recovery-42010011054.json`, to
consume the incident's creation opportunity. This is a proposed additional
durable asset, not the successful acceptance asset. It needs explicit proposal
approval because it adds a persistent write before image acceptance.

Before any incident build, completely enumerate assets. Any asset with that
name blocks creation, including a partial, malformed or mismatched asset. Do
not treat an existing matching marker as authority to build in a fresh process.
If absent and all preflight checks pass, POST the bounded JSON asset once using
`Releases.json` and the existing uploads host. Require HTTP 201, exact name,
uploaded state, size/digest and exact downloaded-byte readback. Pin its numeric
ID and bytes/digest in the local attempt record. Only this successful POST in
the current run may open creation; discovery of a marker cannot open it.

GitHub's [asset upload contract](https://docs.github.com/en/rest/releases/assets#upload-a-release-asset)
rejects an existing filename with HTTP 422. A failure can also leave a `starter`
asset. Stop on every upload failure or uncertain result; never retry, delete,
replace, rename or repair the marker. A successful upload followed by a local
save/readback failure consumes the opportunity without authorizing a build.
Recheck the pinned marker before push and later release writes. This uses the
same no-replacement upload/readback pattern already present for acceptance.

The marker contains only a fixed schema version, incident run/job/artifact
coordinates, the attributed no-push evidence reference, release/tag/source,
executing workflow SHA, run ID/attempt and actor. Its presence means an attempt
was admitted; it never means no push, image acceptance or publication. Preserve
it after success and failure. Its original bytes remain unchanged. Capture its
ID/digest in the diagnostic record and, for new successful incident acceptance,
an optional strictly validated recovery field in the existing allowlisted
evidence document. Old evidence remains readable and reused byte for byte.

The marker is required for the incident even if the namespace is accessible
but the version is missing. A rerun fails attempt validation; a fresh dispatch
finds the marker and cannot build. After any possible push, allow only the
existing positive-access signed-digest path. Hidden/missing results cannot
reopen creation. Losing or deleting evidence never restores authority; owner
preflight must preserve the marker and account for all later activity. An
administrator who changes code or deletes the marker is outside this trust
model, as with the existing source-tag and exclusive-writer requirements.

This replaces the future-run-number selection and archive reader. A simple
manual-only gate would permit repeated fresh dispatches after an uncertain
push; it is insufficient. A separate history scanner or creation database is
unnecessary when the existing draft already supports a non-replaceable asset.
The tradeoff is deliberate: a failure before push can consume the opportunity
and require another reviewed adjustment. There is no automatic reset.

### No-push evidence and separate owner authorization

This proposal requests accepting the supervisor's authenticated log inspection,
original identity metadata and verified source ordering as the baseline
no-push evidence for original attempt 1. Independent proposal review must
explicitly accept or reject that evidence basis. This changes revision 2's
mandatory machine archive-download gate; do not silently treat archive metadata
as downloaded bytes or retain an archive requirement that cannot be satisfied
under unchanged permissions. No new archive/log download or permission grant
is needed by the proposed helper. Denied downloads remain denied and preserved.
If the reviewer requires a further accessible log excerpt, obtain it through
permitted access before execution; do not manufacture or bypass evidence.

After implementation CI and independent review, the owner must separately:

1. Approve the accepted original no-push evidence and account for every later
   publication attempt and unresolved push. Recheck original attempt identity
   and review later runs; missing or contradictory evidence blocks execution.
2. Confirm authoritative current package absence if unavailable creation is
   needed. An access-filtered namespace listing or 404 is insufficient. If the
   namespace is accessible but the version is absent, confirm that exact image
   tag is absent and no earlier push remains unresolved.
3. Exclude package, image-tag, source-tag, release and asset writers for the
   entire operation. Preserve original incident evidence and any recovery
   marker. Confirm tag, draft, workflow comparison and token policy remain valid.
4. Explicitly authorize one manual dispatch from the exact reviewed main SHA,
   using only the existing fixed tag/source inputs, including its one marker
   upload, image publication, attestations, evidence retention and final draft
   publication. Record that decision on tines-mcp/40 before dispatch. Capture
   the executing actor/run in the marker; those fields record execution and do
   not cryptographically prove the external owner decision.

Proposal approval, implementation merge, successful login, a diagnostic flag or
an issue URL supplies none of those current operational facts. No live dispatch
command is part of this preparation. Before authorization, blockers are the new
proposal decision, reviewed implementation/CI, accepted no-push evidence basis,
fresh absence/exclusive-writer/later-activity checks and explicit execution
approval. Unchanged permissions and restored workflow equality avoid the two
specific gates introduced by revision 2; they do not guarantee live API success.

### Preserve image acceptance and publication last

Use the existing `publish`, `accept`, `retain` and `finalize` phases. Keep local
image/runtime checks, immediate pre-push registry/draft/tag/marker/workflow
rechecks and one push. Persist `push_attempted=true` before dispatching Docker.
Require the same unavailable observation at both checks when initially
unavailable; a newly accessible namespace stops that attempt. If initially
accessible, retain its package ID and require the version still to be absent.
After push require positive metadata, repository association, complete version
enumeration and the observed matching digest before either attestation.

Keep both signed statements and original signer/source verification, anonymous
digest pull and runtime checks, credential isolation/cleanup, durable evidence
readback and required Actions retention. Publish only this same draft last.
A final PATCH 403/404 preserves release/source/tag, marker, acceptance asset and
image digest, records uncertainty and stops without automatic retry or repair.
Published releases remain read-only. Preserve v0.1.1 without any backfill.

## Non-goals

- Live dispatch, retry, cancellation, image build/push/retag, tag/release/asset
  mutation, publication, deployment or credentials/settings changes here.
- A new release version, alternate token, Actions-read grant or workflow-write
  workaround. Do not move, delete or recreate the existing release or tag.
- General unavailable-package creation, automatic marker reset, a counter
  service, run-number scheduling or an unused recovery subsystem.
- Reconstructing the original message or claiming original ZIP bytes were read.

## Verification

Use fixtures; do not test authority with live release writes.

- Compare complete workflow path/mode/blob maps with the fixed source; assert
  byte-for-byte original `release.yml`, only the two original dispatch inputs,
  unchanged permissions and all existing phases/actions. Reject workflow
  additions/deletions, changed default main, denied reads and stale comparisons
  before marker upload, build or push.
- Preserve varied nonempty-message coverage and v2 sanitized diagnostics;
  401/403/429/5xx, redirects, bad/oversized JSON, bad documentation and Link,
  transport failures and every versions-route 404 still stop safely. Hidden and
  absent fixtures may have identical 404s; neither proves authorization.
- Old automatic handoffs, historical reruns, attempt 2, wrong source/tag/draft,
  another version and published releases never create through this branch.
  Exercise the actual restored YAML environment and both missing-version paths.
- A valid creation fixture performs one marker POST/readback, one build/push,
  positive post-push checks, both attestations, anonymous runtime, durable
  acceptance/readback, required Actions retention and final PATCH in order.
- Existing, duplicate, partial, malformed, replaced or deleted markers,
  401/403/404/422/5xx/transport errors, response loss and readback/save failure
  produce no push or fallback. After a marker is created, a fresh dispatch cannot
  build, even with missing diagnostics or a hidden package. Capture exact
  marker ID/bytes/digest through all later failure fixtures.
- Keep moved-tag/draft/package/local-image checks, persisted push uncertainty,
  missing-digest and post-push denial fixtures. Reused signed digests produce
  no image or attestation writes. Preserve old acceptance bytes; validate new
  optional recovery coordinates without copying arbitrary diagnostic data.
- Retain the final-PATCH 403/404 assertions for exact release/source/tag, asset
  IDs/bytes/digests, selected image digest, one PATCH and no retry/repair or
  second push. Verify v0.1.1 receives no writes.
- Run the existing Python, pinned Node updater/actual-workflow tests, shell and
  actionlint checks, plus unchanged CI Rust and real local container gates.
  Independent implementation review must inspect the final combined diff.

## Success criteria

- The exact existing draft has a usable reviewed recovery path through the
  existing tag/source inputs, with original workflow bytes and permissions.
- Separate owner authorization and fresh preflight are explicit, and no artifact
  metadata is represented as independently verified archive contents.
- One durable marker prevents reruns or fresh dispatches from renewing creation
  after an admitted attempt, including the accessible-package missing-version path.
- Existing-digest verification and every acceptance/publication safeguard remain
  effective. v0.1.1 and the v0.1.2 tag/source/release identity stay unchanged.
