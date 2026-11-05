# Proposal: Fix attestation credential discovery and review digest-only recovery

## Motivation

Complete tines-mcp/41 in two reviewed stages: fix credential discovery for normal
releases, then decide how to accept the image already pushed by recovery run
`42022779198`. Proposal v1 received stage-1 approval in tines-mcp/41. This revision records
its helper-only implementation, following the owner's review alternative. The
incident acceptance contract remains unapproved. No live execution is authorized.
No workflow execution, image operation, attestation, release write, change to real
credentials or publication is part of this preparation.

Preserve draft `435049346`, tag `v0.1.2`, application source
`bf97287109eaab863d26ed82307d31db7c3c6a71`, package ID `16599548`, and image digest
`sha256:7c775b55c896df425c65c8a4f28d334f98a1d9148ac4d5697aa891e6946a86a3`.
The package ID, observed push digest and `push_attempted=true` are attributed to
this issue's independent job-log inspection. This runner could not independently
read package metadata or original diagnostics. This is a post-push incident;
none of tines-mcp/40's no-push creation allowance remains available.

## Problem statement

### Credential lookup fails after provenance creation

At current main `90f80fd541869b41cff38265054dadc2503f0ac5`,
`.github/workflows/release.yml` sets `DOCKER_CONFIG` to
`$RUNNER_TEMP/release-docker`. `Release.publish` in `scripts/release.py` initializes
that config without credential helpers and runs Docker login with the ephemeral
job token on stdin. The final `always()` step removes that directory.

The workflow pins `actions/attest` to
`1e69f48acb82d1966a394da916b4c1698aa569d6`. Its lockfile pins `@sigstore/oci` 0.7.2.
The shipped dependency's `getRegistryCredentials` reads
`os.homedir()/.docker/config.json`, ignoring `DOCKER_CONFIG` and credential-helper
settings. The [pinned action](https://github.com/actions/attest/blob/1e69f48acb82d1966a394da916b4c1698aa569d6/src/attest.ts)
calls `attest` before credential discovery and registry attachment. Consequently,
a registry failure does not imply that no GitHub-side provenance exists.

A local probe executed the shipped dependency modules and webpack runtime,
omitting action startup. With an empty default auth map and fixture credentials
only in `DOCKER_CONFIG`, it reproduced `No credentials found for registry ghcr.io`.
An isolated symlink at the default config path resolved the fixture credentials.
Restoring the original config and removing the fixture directory removed access.
The probe mocked only the fixture home directory, never changed `HOME`, and used
no real credentials. The shipped `dist/index.js` SHA-256 is
`3ca89e06ffcb09ff97e9b1633575865cad0b23310ac82176d72990d7554836b5`.
This proves dependency lookup behavior, not production bridge cleanup or a live
registry attachment.

### Observed incident evidence and access limits

Read-only checks on November 5, 2026 established:

- Run `42022779198` is completed/failed, workflow `407725245`, run number 21,
  attempt 1, manual dispatch on main at orchestration commit
  `90f80fd541869b41cff38265054dadc2503f0ac5`. Job `127229419087` succeeded at
  publish/select, failed at build-provenance attestation and skipped exact-source
  attestation, acceptance, evidence and publication. Cleanup succeeded.
- Release `435049346` remains draft, mutable and non-prerelease, with the fixed
  tag and application-source target. `git ls-remote` independently confirms the
  source tag and current main.
- The complete release-asset listing contains permanent marker
  `adapter-recovery-42010011054.json`, ID `713244917`, size 485, API-reported
  digest `sha256:1795b7e0415a39cd3f521db3419482c0ef625055923c797ad637ebb834a43f9f`.
  No acceptance asset was listed. The normal marker-byte download was Forbidden.
  This run has not downloaded or independently hashed the original marker bytes.
- Diagnostics artifact `13412352693`, named
  `adapter-release-diagnostics-42022779198-1`, is unexpired, size 1474, with
  API-reported archive digest
  `sha256:48881606b12a646b2d3e98a89b44c8efcd950120bac88047d7ca2d852560d945`.
  Its archive and the job-log downloads were Forbidden. Neither was retried.
- The authenticated package metadata and versions endpoints returned HTTP 403,
  `Resource not accessible by personal access token`. No alternate identity,
  credential or endpoint was used to bypass those denials. Current package/tag
  and registry-attestation state remain unverified here, not absent.
- The repository attestation API returned one bundle for the exact digest. Its
  decoded SLSA predicate identifies invocation
  `https://github.com/dstoc/tines-mcp/actions/runs/42022779198/attempts/1`, the
  release workflow on main and orchestration commit `90f80fd...`. It does not
  bind the separate application checkout by itself. No exact-source statement
  was returned in that inventory; this is not proof of registry-wide absence.
- Verification of the extracted bundle with `gh` 2.103.3, exact workflow identity,
  source ref and original signer/source digests stopped before verification with
  `no valid Sigstore verifiers could be initialized`. No trust-root substitution
  or blind retry occurred. The decoded claims are observations, not a completed
  cryptographic acceptance. The extracted bundle JSON has SHA-256
  `a5bde4db0be528a2ba041332e0bb58bf932602a7259306171da2d1952eaf06c5`;
  it is a serialization extracted from the API response, not original marker or
  diagnostics bytes. Preserve it for independent verification.

The existing `Release.accept`, `verified_identity` and `validate_evidence`
require both registry-backed statements to share the original signer commit.
A later custom statement cannot satisfy that rule without a reviewed policy
change. Ordinary existing-digest selection sets `new_image=false`, skips both
attestation actions and stops if either verification fails. Keep that default.

## Proposal

### 1. Add a temporary credential bridge for the pinned action

Keep the pinned action, token grants, isolated Docker login and anonymous-pull
configuration. Add a small helper under `scripts/` to expose the same job login
through the path actually read by the pinned dependency, only for the two
attestation steps. Use a symlink to the ephemeral `release-docker/config.json`;
do not copy job credentials into another persistent file, change `HOME`, use
PATs, configure credential helpers or merge credentials into the runner config.

Immediately before the attestation steps, prepare the bridge after successful
image selection. Prefer to prepare it only when `new_image=true`, since the
ordinary existing-digest path makes no attestation writes. The helper must:

1. Validate the fixed temporary paths, current UID, expected GHCR-only login
   config, regular-file ownership and absence of helper/header forwarding. Make
   the login directory mode 0700 and its config mode 0600 before exposure.
2. Inspect the actual default Docker config without following unexpected links.
   If an owned regular config already exists, move it into a separate mode-0700
   backup directory under `RUNNER_TEMP`, outside `release/` and artifact paths.
   Preserve its bytes, identity and mode. If absent, record absence. Never replace
   a foreign-owned file, symlink, special file or unsafe parent directory.
3. Persist bounded restoration metadata outside artifact paths before moving
   anything. Install the symlink exclusively; never truncate an existing path.
   A partial preparation must preserve or restore the original file before returning failure.
   Refuse a pre-existing bridge/backup or ambiguous state; do not adopt it.
4. Emit only fixed success/failure messages. Do not emit config contents, auth,
   passwords, tokens, command traces or backup contents.

Use the existing `release.py report` command in the `always()` report step to
restore the original default config and remove only its own exact symlink. If the helper
created an empty `.docker` directory, remove that directory only when empty.
Check path identity before unlinking or restoring. An unexpected replacement
must fail safely without deleting somebody else's file. Preserve an original
backup if restoration becomes unsafe, outside uploads, and report a fixed error.
In a finally/fallback path, remove the ephemeral login target regardless of
restoration or helper/checkout failure. No new job credential may remain readable
after cleanup. Preserve the original runner credentials rather than deleting
unknown files. Runner teardown remains the boundary for abrupt host loss, as in
the existing workflow.

This is a compatibility bridge for one pinned dependency, not a general
credential manager. The approved implementation uses existing helper callers
rather than new workflow steps. `Release.publish` prepares the bridge after
positive post-push checks, before emitting `new_image=true`. Reused digests emit `false` without
preparing a bridge. `main` handles report cleanup before loading a possibly
missing/corrupt record or writing a summary. Its `finally` removal runs even if
restoration fails. The existing final shell cleanup remains independent of
checkout, helper execution and `DOCKER_CONFIG` initialization. No workflow file,
permission or action pin changes.

The private journal is `$RUNNER_TEMP/release-attestation-bridge/state.json`,
mode 0600 in a mode-0700 directory. It contains path, run/attempt and device/inode/mode
identities only. An exclusive hard link preserves the original owned config,
then its original directory entry is removed. The symlink is staged privately,
its identity recorded, then installed exclusively. Restoration links the original
inode back without replacing a competing file. A cross-device link failure stops
safely before removing the original. Missing/ambiguous state is never adopted.

Preparation restores after caught failures. Cooperative cancellation can reach
the existing always-run report path. `always()` cannot guarantee execution after
hard cancellation, runner termination or host loss. Those outcomes depend on
GitHub-hosted ephemeral runner teardown; this design is not a safe credential
lifecycle for a persistent shared runner. No production cleanup test claims to
simulate host teardown. Private original-config backups must not be uploaded;
unsafe restoration leaves them for operator inspection rather than deleting them.

### 2. Keep creation closed and decide the truthful repair contract

The marker is permanent consumed authority. Pin and preserve its numeric ID,
original bytes and digest using permitted reads. Metadata hashes do not replace
byte verification. Preserve both original diagnostic sets. Any missing or
changed marker blocks recovery; it never opens a build path. No creation guard,
marker, package/image tag, source tag or release identity may be reset or changed.

The proposed recovery evidence contract is specific to this one incident:

- Cryptographically verify the **original** SLSA bundle against the exact digest,
  original release workflow identity, main ref, original orchestration SHA and
  original invocation. Verify transparency evidence and certificate trust using
  supported `gh attestation verify` behavior in a functioning trusted verifier.
  A parse, certificate decode or API success is insufficient.
- Through permitted positive reads, verify package ID/repository association,
  complete version inventory, exact image-tag/digest mapping, manifest bytes,
  platform and OCI source/revision/version labels. Inspect registry referrers
  and verify all candidate statements before classifying anything as missing.
  Pin observed existing statement/referrer identities and bytes. Stop on
  conflicts, changed digest, inaccessible state or unverifiable provenance.
- Bind the separate application source with the fixed source/tag, the reviewed
  original orchestration's separate checkout/build path, original run evidence
  and actual image labels. The SLSA dependency is the orchestration commit,
  not the application commit. Require independent review of this evidence basis
  before making a later source-verification claim; labels alone are insufficient.
- If the original verified provenance is absent only from the registry, attach
  its exact existing bundle bytes with the supported `@sigstore/oci` 0.7.2
  `attachArtifactToImage` API and the same media type/annotations used by the
  pinned action. Do not re-sign or regenerate SLSA provenance. A repair helper
  would need a reviewed lockfile for that actual library and tests through its
  public attachment API; do not extract the action's internal module in production.
  Check registry state first, attach only the missing bundle, verify readback
  through `--bundle-from-oci`, and stop on uncertainty without automatic retry.
  The actual library adds creation-time annotations and can update a digest-derived
  referrers-index tag when native referrers are unavailable. It sends If-Match only
  when the index read supplies an ETag; the pinned action normally supplies retry:3.
  Before approving repair, review the exact writes, preservation of all existing
  descriptors, response-loss/denial handling, explicit retry settings and exclusive
  writers through the public API. Attachment is not established to be append-only
  or byte-idempotent by its name.
- If no valid original exact-source statement exists, generate a **custom
  recovery-verification statement**, not build provenance. The pinned action
  supports `predicate-type` and `predicate-path`. Use a distinct predicate type,
  `https://github.com/dstoc/tines-mcp/attestations/release-recovery/v1`. Its fields
  must identify this fixed release/tag/application source/digest/package,
  original run/attempt/signer, original verified bundle hash, reviewed evidence
  basis, current verifier workflow/run and marker ID/hash. Its claim is that
  this later run verified retained evidence for an existing image. It does not
  claim that this run built that image or that its signer built the application.
- Accept that original provenance plus the later recovery statement only through
  an explicit exact-incident policy branch. Require the original signer to be
  `90f80fd541869b41cff38265054dadc2503f0ac5` and the recovery signer to be the
  independently reviewed executing main SHA. Verify each certificate, subject,
  predicate and cross-binding. Keep normal release identity v1, same-signer
  checks and existing-digest refusal unchanged for every other image.

This is a proposed change to the acceptance contract, not permission inferred
from a failed action. If reviewers require the original same-signer exact-source
statement, this incident cannot pass that contract unless an already-existing
valid original statement is found. A later run cannot manufacture that original
signer. Stop with that exact decision; never silently loosen the verifier.

After a separately approved repair, require registry-backed verification of
both accepted statements, anonymous digest pull, image identity and real runtime
acceptance. Retain bounded versioned recovery evidence in a distinct additive
asset, `adapter-release-recovery-evidence-v1.json`, with original and recovery
signers explicitly separated, both verified results, marker and bundle pins,
source binding, anonymous runtime result and acceptance-run attribution. Keep
ordinary `adapter-release-evidence-v1.json` bytes and schema unchanged. Upload
once, pin ID/size/hash and read back exact bytes; existing conflicting evidence
stops writes. Retain the required Actions evidence before final publication.
This recovery contract requires its own implementation review and CI; do not
land an uncalled recovery module or a selector that always rejects.

### 3. Preserve workflow equality; keep the recovery decision explicit

The complete workflow maps at current main and fixed application source match:

| Path (both mode 100644) | Blob in both revisions |
| --- | --- |
| `.github/workflows/release.yml` | `e0889675fbfb7bd9663d7bf302b29520210eeb7b` |
| `.github/workflows/test.yml` | `ccb5f1b8d2d7df9be7417ced80449430ef069557` |

These are all workflow files in both revisions. The helper-only implementation
preserves every path, mode and byte. A test inventories the complete working-tree
workflow directory and checks both Git blob identities. This corrects v1's claim
that the credential fix necessarily changes YAML. Stage 1 introduces no
workflow-difference publication restriction and needs no human publication
handoff solely because of this fix. Equality is not proof of all other live
publication authority; no PATCH or other authority probe was made.

GitHub's [update-release rule](https://docs.github.com/en/rest/releases/releases#update-a-release)
still matters if a later repair changes workflows. Keep `recovery_workflows`
and its current-main checks unchanged. Any later implementation must compare
all workflow paths, modes and blobs before live work. If it introduces a real
workflow difference, resolve workflow-write authority before live repair;
`GITHUB_TOKEN` cannot receive that permission. Human final publication is then
one decision to review, not a consequence assumed from v1. Do not change the
retained target/tag/draft or introduce alternate credentials.

**Exact decision needed before recovery implementation:** accept or reject the
fixed-incident original-SLSA plus later custom recovery-verification contract in
section 2, including its proposed source-evidence basis and distinct durable
asset. Labels alone are insufficient. If the original same-signer source rule
remains required, this digest is blocked unless an existing valid original
exact-source statement can be found. A later signer cannot manufacture it.

The minimum supported signing building block exists: the pinned action uses
public `@actions/attest.attest` with explicit subject, custom predicate type and
predicate, then `@sigstore/oci.attachArtifactToImage`. A later reviewed helper
could call those public APIs from the existing acceptance phase while the reused
image keeps `new_image=false` and skips both ordinary attestation steps. It must
never emit replacement build provenance. This is an implementation direction,
not an approved or currently implemented recovery path. Its exact dependency
pins, called integration, original-bundle attachment behavior, retry/uncertainty
handling and full workflow comparison require separate review. Production must
not extract modules from the action bundle; that is a test probe only.

Recommendation: review and ship stage 1. Keep the draft and consumed marker
fixed. Decide the two-signer/source-evidence policy only after permitted original
bytes, positive registry observations and functioning cryptographic verification
are available. Then review a called helper-only flow first, retaining workflow
equality and normal publication guards. If that cannot meet the reviewed
contract, surface the exact workflow/publication decision before live work.
No dormant repair helper or always-rejecting selector is part of stage 1.

### Sequencing and live approval

1. Review this proposal. Implement the credential bridge, tests and operator
   documentation on a conventional branch; run normal CI and obtain independent
   implementation review. Do not execute the incident workflow to test it.
2. Resolve the recovery policy, publication method and evidence/verifier blockers.
   Review the exact recovery implementation, dependency behavior, accepted
   workflow map and versioned evidence format before merging it. Ordinary
   existing-digest refusal stays in force until that review is complete.
3. Before any live repair, the owner must separately approve the exact main SHA,
   digest/release/source/marker pins, statement and attachment operations,
   evidence retention and publication method. Obtain fresh positive registry
   evidence, permitted original bytes, verified source/provenance, a complete
   account of later activity and exclusive writers. Stop on any uncertainty.
4. Perform only separately authorized missing-attestation operations. No build,
   push of an image, retag, overwrite, marker mutation or new build provenance.
   Registry attachment writes are explicit mutations requiring that approval.
5. Verify and retain all acceptance evidence, then publish the same draft last
   through the separately approved method. Confirm identity and
   immutable/publication state afterward; a denial stops without retry or repair.

## Non-goals

- Live execution, rerun, cancellation, dispatch, image or attestation writes,
  publication, deployments, settings changes or credentials changes in this run.
- Reusing no-push evidence as creation authority after the observed push.
- Removing or weakening ordinary existing-digest, same-signer or workflow guards.
- Replacing original bundles, marker, evidence, image bytes, tags or the release.
- New PATs, broader token permissions, trust-root substitutions, denied-read
  bypasses, a new version or changes to already-published v0.1.1.
- Claiming that downloaded API metadata proves original archive/marker bytes,
  cryptographic verification, current registry state or operational authority.

## Verification

The original preparation probe passed against the actual shipped bundle/runtime.
Stage 1 adds the lockfile-pinned public `@sigstore/oci` 0.7.2 reader to the existing
Node CI suite. Its integrity matches the pinned action lockfile. The same bridge
lifecycle test also passed locally through the hash-checked shipped bundle,
omitting only action startup. It proves lookup, restoration and removal with
fixture credentials, not registry attachment or live authorization.

The Python suite tests the actual filesystem operations and called publish/report
paths, including report startup before record loading. Existing HTTP/engine tests
isolate the bridge and retain their independent publication gates. The workflow
expression suite proves both attestation failures and cooperative cancellation
schedule report and the final independent cleanup. Abrupt cancellation/host loss
are documented limits, not simulated teardown guarantees.

After approval, implementation must verify:

- The real pinned dependency fails with credentials only in `DOCKER_CONFIG`,
  succeeds through the prepared bridge, and cannot recover the job credential
  after cleanup. Pin and check the shipped bundle or lockfile dependency identity;
  fail if the test harness cannot identify it. Do not mirror lookup in a mock.
- Existing owned default configs and absent configs restore correctly. Refuse
  unsafe links/owners/paths, pre-existing backup state and unsafe modes. Exercise
  failure at every move/symlink/state-write boundary, both attestation failures,
  skipped steps, cleanup failure and checkout failure. Assert exact restoration,
  no job credentials in logs/artifacts, and unconditional ephemeral-target removal.
- Actual workflow expressions invoke preparation only when needed, preserve both
  attestations and permissions, and always invoke cleanup. Ordinary reused digests
  make no new attestation/image writes; verification failure never builds.
- Exact incident recovery, if separately approved, verifies original and recovery
  signers differently only for the fixed subject. Reject other digests/releases,
  swapped/unverified bundles, wrong invocation/source/ref, changed marker bytes/ID,
  missing original evidence, unsafe source claims and stale workflow maps. Exercise
  the real pinned attachment API against a local fixture registry, including
  existing referrers, denied reads, response loss and post-write verification.
- Changed workflow trees stop the current incident path. A future approved repair
  flow must check its complete reviewed tree and unchanged default main before
  each mutation. Never fall back to image creation. Publication requires explicit
  approval, verified durable acceptance and fresh authority checks.
- Keep anonymous pull/runtime, complete registry enumeration, no-overwrite asset
  readback, required Actions retention and draft-publication-last tests. Human
  handoff cannot claim the release is published or substitute for these checks.
- Run the Python release suite, pinned Node updater/actual-workflow suite,
  actionlint 1.7.12, shell syntax, Rust fmt/clippy/locked tests and CI container
  gates from `docs/releases.md`. Use local fixtures for preparation; CI must not
  perform incident recovery. Independent review inspects the final combined diff.

## Success criteria

- Normal releases expose only ephemeral GHCR job credentials to the actual pinned
  dependency and restore the original default configuration on success/failure.
- Creation remains permanently closed for the incident; no code or plan treats
  the consumed marker as renewed authority.
- Existing provenance is preserved and independently verified before any repair;
  a later statement makes only a truthful source/evidence-verification claim.
- The acceptance-policy decision, evidence blockers and old-target publication
  decision are explicit before live work. No disabled-only recovery path lands.
- Live repair and publication remain separately authorized, with source/tag/digest
  binding, anonymous runtime acceptance, durable evidence and publication last.
