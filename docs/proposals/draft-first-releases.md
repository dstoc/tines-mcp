# Proposal: Publish releases after draft verification

## Motivation

For the next reviewed Rust version, prepare a GitHub draft and its exact source
tag, verify the adapter image, attach durable acceptance evidence, and publish
the draft last. This makes the workflow compatible with GitHub immutable releases.
It implements tines-mcp/38 without authorizing a live release or setting change.

## Problem statement

At main `8853ca2e0af4ae78487443b3c6f0d9054c51ed66`,
`.github/workflows/release.yml` runs Release Please before the image job.
Release Please creates a published GitHub release. Container build, push,
attestation, anonymous pull, and runtime acceptance happen afterward.
`scripts/release.py` saves acceptance records, but the workflow retains them only
in Actions artifacts for 90 days. A failed image job can therefore leave a
published release without an accepted image or durable acceptance evidence.

Read-only GitHub inspection on November 5, 2026 found release ID `434971264`,
`v0.1.1`, already published at `2026-11-05T09:02:00Z`, with `immutable:false`
and no assets. PR #34 is already merged; its source tag points to the main SHA
above. These observations supersede the prompt's assumption that #34 is open.
Preserve these resources. This change must not redraft, delete, republish, or
backfill that existing release.

### Verified upstream behavior

The pinned action
[`5c625bf` (4.4.1)](https://github.com/googleapis/release-please-action/blob/5c625bfb5d1ff62eadeeb3772007f7f66fdcf071/src/index.ts)
bundles release-please 17.3.0. Keep this pin. In that library:

- The [manifest schema](https://github.com/googleapis/release-please/blob/v17.3.0/schemas/config.json)
  supports `draft:true` and `force-tag-creation:true`. They are manifest
  configuration fields; do not invent action inputs.
- [Manifest.createRelease](https://github.com/googleapis/release-please/blob/v17.3.0/src/manifest.ts)
  forwards both options. The root action outputs include `id`, `draft`, `sha`,
  `tag_name`, `version`, and `release_created`. The last field means that a
  release object was created, including a draft.
- [GitHub.createRelease](https://github.com/googleapis/release-please/blob/v17.3.0/src/github.ts)
  creates `refs/tags/<tag>` at the release SHA before creating the draft when
  `forceTag` is set. It ignores every HTTP 422 from tag creation. Its returned
  `sha` is the release response's `target_commitish`, not proof of the tag target.
- Draft creation and later PR labeling/output delivery are separate operations.
  Duplicate-release handling does not promise to replay a successful handoff.
  Recovery must inspect existing resources explicitly.

[GitHub recommends](https://docs.github.com/en/code-security/concepts/supply-chain-security/immutable-releases)
creating a draft, attaching assets, then publishing. Publication locks the assets
and associated tag when immutability is enabled. That protection does not cover
GHCR image tags. The existing no-overwrite image policy remains necessary.

[Chime at 7f5460d](https://github.com/dstoc/chime/blob/7f5460dfa40497ef3b2aa00f529b7795542a4d54/.github/workflows/release.yml)
provides useful authenticated draft discovery, asset readback, and final
publication patterns. Keep this adapter's Python orchestration, pinned actions,
Rust strategy, explicit expected source SHA, and stronger registry authority
checks. Do not copy Chime's version-list 404 fallback or CLI archive machinery.

## Proposal

### Draft and source identity

Set `draft:true` and `force-tag-creation:true` for the existing root Rust package
in `release-please-config.json`. Keep the Rust strategy, bootstrap boundary,
version/changelog updates, and single main-push workflow.

Export the created release ID and draft flag alongside the existing outputs.
Automatic image work requires the current successful handoff, `draft=true`, a
positive release ID, stable tag/version agreement, and a full source SHA.
No-release PR updates must perform no image or release publication work.

Keep manual recovery inputs limited to the existing tag and full expected source
SHA. Authenticate and enumerate all release pages to find exactly one matching
release, then pin its numeric ID. Drafts are not discovered with the public
by-tag endpoint. Reject incomplete pagination, duplicate matches, inaccessible
responses, malformed identity, prereleases, and missing releases. Never infer
absence from a permission error or create a replacement release in recovery.

Before registry work, require the selected release's ID, exact tag, full
`target_commitish`, and expected source SHA to agree. Independently resolve both
local and remote tag commits, including annotated tags, and retain the ancestry,
separate source checkout, Cargo metadata, and Cargo.lock checks. A release's
`target_commitish` alone cannot authorize a build. A missing or mismatched tag
stops recovery; the helper never creates, moves, or repairs a source tag.

For a new draft require `draft=true`, `published_at=null`, and `prerelease=false`.
Record its ID and observed immutable flag. Re-read that same ID and recheck the
remote tag before image push, evidence upload, and publication. Unexpected
publication or identity change stops writes; it does not authorize a repair.

### Required ordering

Keep the current workflow concurrency group and `cancel-in-progress:false`.
Use this sequence in the release job:

1. Resolve the draft/release identity and exact existing source tag.
2. Validate source versions, package authority, and image policy. Build and test
   locally, then push once, or select a positively observed existing digest.
3. For newly pushed bytes only, create the existing signed provenance and exact
   source attestations with the pinned `actions/attest` action.
4. Verify both signed statements and their shared original workflow signer.
   Verify anonymous digest pull, OCI labels/platform/user, runtime health and
   shutdown, then recheck package identity, digest, and remote source tag.
5. Create or reuse the durable evidence asset described below and verify its
   downloaded bytes. Finish the required Actions evidence upload successfully.
6. Recheck the draft, source tag, package identity/digest, and durable asset.
   Publish only that numeric release ID with a PATCH setting `draft:false`.
   Verify the response and a fresh read show the same published release and tag.

The final publication request must be gated by successful acceptance and evidence
retention. No `always()` path or report command can authorize it. Diagnostic
reporting, Actions artifact retention on failure, and credential cleanup still
run after failures. No release mutation is permitted after publication; local
reporting and cleanup may continue.

Keep `contents:read` as the workflow default. The release job needs
`contents:write` to upload evidence and publish; use its authenticated client
for draft discovery as well. This is the only new token capability. Retain `packages:write`, `attestations:write`, and
`id-token:write`, with no issue/PR writes in this job. Release Please retains its
existing separate PR permissions. Keep the ephemeral `GITHUB_TOKEN`, disabled
storage-record creation, isolated Docker authentication, and unconditional
credential cleanup. Add no PAT, administration access, or credentials.

### Durable acceptance evidence

Attach one bounded JSON asset, `adapter-release-evidence-v1.json`, to the draft.
Use one file so interruption cannot leave a mixed set of acceptance files.
It contains a versioned schema and an explicit acceptance result, with:

- Repository, release ID, tag, version, source SHA, platform, package ID, digest,
  and digest-pinned image reference.
- The original image signer's workflow SHA and the acceptance workflow SHA,
  run URL/ID/attempt, actor attribution, creation eligibility, registry
  observations, and runtime acceptance result.
- Both verified attestation results currently retained as
  `verified-provenance.json` and `verified-identity.json`.

Build the asset from an allowlisted acceptance snapshot, not arbitrary process
state. It must contain no secrets and no claim that publication already happened.
Keep publication dispatch/outcome in the local record and Actions summary. An
immutable asset cannot be edited afterward to record publication success.

List assets completely and require zero or one matching name. On a draft, upload
only when absent, without overwrite or `--clobber`. Persist an upload-attempt
boundary before dispatch. After upload, re-enumerate and download the asset by
ID; check its uploaded state, size, digest, and exact bytes. Reject duplicate,
partial, oversized, malformed, or conflicting evidence without replacement.

On retry, a valid existing asset is the original acceptance snapshot. Validate
its schema, release/source/package/digest identity, acceptance fields, and
original signer against fresh live attestation and runtime verification. Reuse
its exact downloaded bytes; do not regenerate it with the recovery run's fields
and then overwrite it. A snapshot from a different acceptance execution is
expected; a different source, digest, package, or signer is a conflict. Verified
JSON stored in an asset is a record, not a replacement for current cryptographic
verification against OCI. Immediately before publishing, confirm the asset ID,
name, size, and downloaded content hash remain unchanged.

Keep 90-day Actions records for all attempts and diagnostics. The required
release asset supplies durable successful evidence independently of Actions
retention. Preserve Release Please's generated release notes; a separate mutable
notes section is unnecessary for this change.

### Recovery and failure behavior

| Observed state | Required behavior |
| --- | --- |
| Draft and exact tag, no version image | Existing positive-access publication rules apply. The bounded first-package exception below remains unchanged. |
| Draft and matching existing image | Zero builds, pushes, retags, or new attestations. Verify both existing signed statements and public runtime, retain/reuse evidence, then publish. |
| Draft, image pushed, attestation or public acceptance incomplete | Leave unpublished. Do not re-sign an existing unverified image. Inspect authority/visibility or choose a reviewed new patch version. |
| Draft, acceptance complete, evidence upload failed or interrupted | Leave unpublished. Retry verifies the existing image, discovers any completed asset upload, then reuses or uploads without replacement. |
| Draft, evidence retained, publication failed or response lost | Outcome is uncertain. Fresh discovery distinguishes the same draft from a published release; no blind repeat of the PATCH. |
| Already published, with valid matching evidence | Read-only verification of the source, retained evidence, existing digest, both statements, and runtime. Success is an already-published no-op. No builds or writes, including notes/assets/attestations. |
| Already published, evidence missing/conflicting | Stop before image or release writes. Report legacy publication or integrity conflict; never backfill, redraft, delete, or replace. Apply this to mutable and immutable releases alike. |
| Missing/duplicate draft, missing/moved tag, release identity change | Stop without registry writes. Preserve existing resources for reviewed operator investigation. |

A published release must never fall through to the build-if-version-absent path.
Missing positive-access package metadata, missing digest, or invalid attestations
fail its read-only verification. Publication reruns do not recreate deleted
resources. Repeated main-push runs without fresh action outputs confer no
publication authority; use the explicit authorized recovery path.

Preserve every tines-mcp/37 first-package safeguard: only the genuine successful
first-attempt `v0.1.1` handoff can receive its bounded unavailable-package
allowance, and it still needs owner absence/exclusive-writer authorization.
Reruns and manual dispatch require positive package access. A later 404 never
reopens creation after a possible push. The already published 0.1.1 observation
is not authorization to retry its first-package exception. Do not add another
creation path or revive the separate 0.1.0 bootstrap requirement.

GitHub draft creation, tag creation, asset upload, image push, and publication
are separate operations, not a transaction. Preserve pre-dispatch uncertainty
markers. Workflow concurrency excludes only this workflow's runs. Retain the
owner requirement to exclude external tag, release, asset, and package writers
throughout publication; preflight checks alone cannot close those races.

### Owner decisions and migration

After implementation CI and independent review, the owner can separately enable
**Settings → Releases → Enable release immutability** before authorizing the next
release. [GitHub documents](https://docs.github.com/en/code-security/how-tos/secure-your-supply-chain/establish-provenance-and-integrity/prevent-release-changes)
that this applies only to future releases. Do not change the setting in this
issue. Report the returned immutable flag after publication; the workflow works
with either setting and must not claim immutability from ordering alone.

Update `docs/releases.md` for the sequence, durable asset format, explicit
recovery, existing-release behavior, and remaining owner decisions. Keep package
visibility and repository Actions access decisions with the owner. Keep the
first-package authority checks, restricted writers, and separate authorization
for any release or dispatch. Preserve 0.1.1 as a legacy published release;
choose a separately reviewed future version for the first draft-first release.
No release/version PR is approved or merged during implementation verification.

## Non-goals

- Performing a release, creating live tags/drafts, publishing images, enabling
  immutability, changing credentials/settings, or deploying during this issue.
- Retrofitting existing published release evidence or modifying frozen assets.
- Automatically repairing tags, incomplete attestations, or conflicting images.
- Rebuilding existing versions, moving image tags, or adding latest aliases.
- Changing release versions, dependencies, architecture/platforms, or the native
  Rust Release Please strategy.
- Packaging the tunnel client with the adapter or changing tunnel deployment.

## Suggested implementation shape

Extend `scripts/release.py` with bounded authenticated release/asset reads and
narrow upload/PATCH operations for the fixed repository. Keep the existing
registry client and its authority rules separate. Extend `resolve` to select and
pin the release before image operations. Add explicit evidence-retention and
final-publication phases. Keep image publication and acceptance separate from
GitHub publication in records and diagnostics.

Update `.github/workflows/release.yml`, `release-please-config.json`,
`tests/release/test_release.py`, and `tests/release-please/`. Reuse the existing
actual-expression evaluator and fake command/API infrastructure. No production
dependency is required. Land only after proposal approval, implementation CI,
and independent review; do not release as part of landing.

## Verification

All verification in this issue uses fixtures, local repositories, or read-only
upstream inspection. Never create live release resources to test the workflow.

- Exercise release-please 17.3.0's manifest and real GitHub adapter with mocked
  HTTP. Assert config propagation, tag-create-before-draft ordering, exact SHA,
  draft output, and no publication request. Cover a pre-existing wrong tag,
  ignored tag 422, duplicate draft, labeling failure, and no-release PR update.
- Evaluate the actual workflow conditions and environment expressions for push,
  dispatch, skipped/failed/cancelled jobs, missing/invalid outputs, first-attempt
  creation eligibility, later reruns, forks, and other refs. Assert narrowed
  permissions and pinned actions.
- Drive the full release path against stateful fake APIs and command capture.
  Assert ordering from draft/tag validation through image push or reuse, both
  attestations, both verifications, anonymous pull/runtime, durable upload and
  byte readback, Actions retention, then the sole publication PATCH/readback.
  Test the actual workflow's step gates as well as Python phase behavior.
- Inject failure or interruption at every external boundary, including after
  push, each attestation, acceptance, upload dispatch/readback, Actions upload,
  publication dispatch, and successful publication before response delivery.
  Assert no early publication and safe fresh-run recovery without replacement.
- Test release and asset pagination/authority, duplicate identities/names,
  wrong source/ID, missing/moved/annotated tags, corrupt or partial asset uploads,
  evidence field/digest/signer conflicts, and assets from an older valid
  acceptance run. Published mutable and immutable fixtures must produce zero
  release/registry writes; legacy missing-evidence fixtures must stop.
- Retain all existing package authority, no-overwrite, exact-source, original
  signer, first-package, runtime, and Docker credential-isolation regressions.
  Verify zero new attestations or builds for every existing-image path.
- Run the Node Release Please tests, Python release suite, shell syntax checks,
  and actionlint 1.7.12 against the actual workflow files. Run the repository's
  locked Rust format/clippy/test and real container smoke checks required by CI.
  Local fixtures do not prove live Actions-token permissions or immutability;
  those remain checks for a separately authorized future release.

## Success criteria

- A new GitHub release remains a draft until exact-source image acceptance and
  durable evidence retention both succeed.
- Every failure before publication leaves an unpublished draft or reports a
  separately observed external publication; recovery never overwrites an image
  or asset and never repairs a frozen release.
- Already published releases are handled read-only, including legacy 0.1.1.
- Tests enforce complete workflow ordering, failures, authority, and recovery.
- Documentation identifies the owner setting changes without performing them.
