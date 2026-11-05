# Adapter image releases

The release workflow publishes one `linux/amd64` adapter image at
`ghcr.io/dstoc/tines-mcp:X.Y.Z`. It records the source commit and image digest,
then verifies signed provenance, signed release identity, anonymous pull, and
container runtime. Release Please maintains a reviewed version/changelog PR.
After that PR is separately authorized and merged, Release Please creates its
source tag and GitHub Release and passes the exact version and source SHA to
the publisher in the same run. A source release does not establish image acceptance.

The adapter runs as `10001:10001`. The tunnel client stays in a separate container.
There are no floating tags or CLI archives. Runtime acceptance establishes
liveness and clean shutdown; it does not establish live Tines permissions or
intended-client tunnel access. See the selected source commit's README for tools.
This guide describes the workflow, not evidence of a completed publication.

## First real release: 0.1.1

[Release PR #34](https://github.com/dstoc/tines-mcp/pull/34) prepares the first
`0.1.1` release. Keep it unmerged while tines-mcp/37's publishing change passes
independent review. Normal code review does not authorize a release. The separate
owner-only `0.1.0` bootstrap prerequisite from tines-mcp/31, /35, and /36 is
superseded by tines-mcp/37. No bootstrap dispatch, dummy package, or `v0.1.0`
publication is required.

The checked-in `0.1.0` manifest and `bootstrap-sha` (`2e2af44`) seed version and
changelog history only. They do not claim publication. The preparatory commit's
`Release-As: 0.1.1` body footer requested PR #34; do not add a persistent
`release-as` override or repeat preparation.

Before separately authorizing the first release PR merge, the owner must:

1. Verify the exact `dstoc/tines-mcp` container package is absent in an
   authoritative owner view. An inaccessible view or empty access-filtered
   namespace listing is insufficient.
2. Inspect earlier publication evidence and establish that no prior push remains
   unresolved. Missing logs or a missing digest do not prove no push occurred.
3. Exclude all other package creators and writers for the whole publication run.
4. Review the final release PR head, require CI and independent review, and check
   the synchronized `Cargo.toml`, `Cargo.lock`, `.release-please-manifest.json`,
   and `CHANGELOG.md`. Dependency identities must stay unchanged unless separately
   reviewed. Then separately authorize the release PR merge and its publication.

These are namespace and release-authorization checks. They create no image and
require no credential change. If the owner cannot establish them, do not release.
Proposal and implementation reviews do not establish these operational facts.

The helper derives creation eligibility only for a successful `main` push
handoff with `release_created=true`, exact tag/version `v0.1.1`/`0.1.1`, and
`GITHUB_RUN_ATTEMPT=1`. It independently verifies the source tag, ancestry, Cargo
manifest, lockfile, and release outputs. Callers cannot select a creation mode.
Manual dispatches, attempts after 1, and all other versions require positive
package access.

Only within that allowance can the exact authenticated package metadata route's
structured `404 Not Found`, without pagination, permit creation. The helper
records this as **unavailable**, not absent. It requires the same observation
immediately before its sole push. If the package becomes accessible before push,
it stops; restart through positive-access recovery. A package accessible at the
initial check follows ordinary identity validation and complete version
enumeration. If the version exists, it reuses that digest without replacement.

[GitHub can return 404 for insufficient permission](https://docs.github.com/en/rest/authentication/authenticating-to-the-rest-api#failed-login-limit).
A hidden existing package can return the identical 404. Successful Docker login
and mocked tests cannot resolve that ambiguity. The no-overwrite guarantee on
this path depends on the owner's authoritative absence check and exclusive
writers; GHCR provides no atomic conditional tag write in this flow. Unexpected
401/403/429/5xx, redirects, malformed or oversized responses, transport failures,
and version-list 404s never authorize creation.

After creation, positive package metadata, repository association, complete
version enumeration, and the returned digest are mandatory before signing.
[GitHub creates packages as private by default](https://docs.github.com/en/packages/working-with-a-github-packages-registry/working-with-the-container-registry).
The owner must separately [make the package public](https://docs.github.com/en/packages/learn-github-packages/configuring-a-packages-access-control-and-visibility)
if anonymous acceptance fails for visibility. Recover using the same signed
digest after that action. The workflow does not change visibility or credentials.
If its token cannot read the new package, investigate repository association and
Actions access; a 404 does not reopen creation. Remove the bounded creation
allowance through a reviewed follow-up after first publication.

## Later releases and authority

Use conventional commit subjects such as `fix:` and `feat:` on `main`. Release
Please updates one Rust release PR. Review the four version/changelog files
together and approve bot PR workflow runs when GitHub requires it. Require CI
and independent review before separately authorizing the release PR merge.
The workflow does not approve or auto-merge PRs. A PR-only update does not publish.
Do not dispatch a second publication for a successful run. Retain the accepted
digest record before deploying.

Repository policy must permit Actions to create pull requests and use the pinned
checkout, Rust toolchain, attestation, and artifact-upload actions. If policy
blocks them, ask the owner to resolve it. Do not introduce a PAT or change settings
from the workflow. Release Please uses ephemeral `GITHUB_TOKEN` with
`contents: write`, `issues: write`, and `pull-requests: write`; these capabilities
stay isolated from publication.

The publisher uses its ephemeral Actions token with `contents: read`,
`packages: write`, `attestations: write`, and `id-token: write`. Artifact storage
record creation is disabled for this personal-account repository. The workflow
needs no Tines token, PAT, `artifact-metadata` permission, or administration access.
Ephemeral authenticated Docker configuration is removed unconditionally.
Anonymous pulls use a separate temporary configuration without credentials.

For an established package, the job's token must read
`GET /users/dstoc/packages/container/tines-mcp`, validate its owner/repository
identity, and enumerate every active version page. Only complete enumeration
inside that positively accessible package can establish an absent version tag.
A 404 stops ordinary publication and recovery. Successful login does not prove
metadata or enumeration authority. Local fixtures do not establish live token
behavior; retain that evidence in the separately authorized first publication.

Restrict source-tag and package writers throughout every run. Workflow concurrency
serializes its own runs; external publishers can still race the immediate
pre-push and final checks. If other writers cannot be excluded, do not authorize
a release or dispatch. Request a reviewed authority change.

## Explicit recovery

Retain the existing source tag and GitHub Release. Record the full lowercase
40-character source SHA, which must match the tag and be an ancestor of the
selected `main` workflow commit. Never move a release tag. Source and workflow
commits can differ when recovering with a reviewed workflow fix.

The helper comes from its own `main` checkout and builds a separate exact source
checkout. Cargo metadata and the lockfile must match the tag. It builds once,
checks platform/user/labels and runtime locally, then pushes those same bytes once.

Use **Actions → release → Run workflow**, select `main`, and supply only the
existing tag and full expected source SHA:

```sh
gh workflow run release.yml --repo dstoc/tines-mcp --ref main \
  -f tag=v0.1.1 -f source_sha=REPLACE_WITH_FULL_SOURCE_SHA
```

This command can publish if the version is absent from an accessible package.
Run it only with separate publication/recovery authorization. There is no mode
or owner-confirmation input. Direct helper calls reject obsolete mode and
confirmation values.

An existing version selects its digest with zero builds, pushes, retags, or new
attestations. Both signed statements, source identity, OCI labels, anonymous pull,
and runtime must pass. An incomplete or conflicting image stops recovery.
A still-absent package cannot be created by manual dispatch or an ordinary rerun,
even when the first attempt failed before pushing. Preserve proof of no push
and request a narrowly reviewed workflow adjustment for another creation attempt.
Do not use the superseded `0.1.0` bootstrap. Release Please might not re-emit
release outputs on rerun; missing outputs confer no authority.

## Read and retain release evidence

Every run attempts to upload `adapter-release-<run-id>-<attempt>` with a 90-day
retention period. `record.json` and the job summary include the tag, version,
source SHA, orchestration SHA, run URL, actor, run attempt, creation eligibility,
initial/pre-push package observations, package ID, image digest, current operation, and acceptance result. Successful verification
also retains the CLI's verified provenance and identity output. Copy successful
records into durable release records before Actions artifacts expire. An
interrupted run may fail to upload evidence; treat missing evidence as uncertain.

The main stages are:

| Stage | Evidence and next action |
| --- | --- |
| `no_push_attempted` | This record has no push dispatch. Inspect all prior evidence. Recovery requires positive package access; a still-absent package needs a narrowly reviewed creation adjustment. |
| `push_attempted_outcome_uncertain` | A push was dispatched, but its result is not established. Investigate through positive-access recovery only. |
| `digest_observed` | A digest was selected or returned by a push. Publication can be partial; acceptance is incomplete. |
| `identity_verified` | Both signed statements match. Public pull/runtime or final consistency checks can still fail. |
| `public_runtime_acceptance_complete` | Anonymous digest pull, platform/user/labels, health 204, shutdown zero, and final registry/tag checks passed. |

A failure after a push can leave an image without one or both attestations.
Acceptance requires `validation: passed` and the final stage, not just a green
build or a digest. A workflow failure after acceptance, such as evidence upload
failure, still needs investigation and retention of the available record.

The custom predicate type is
`https://github.com/dstoc/tines-mcp/attestations/release-identity/v1`. Its verified
fields are `repository`, `tag`, `version`, `source_sha`, `workflow_sha`, and
`platform`. GitHub's generated build provenance identifies the workflow execution
commit; it does not by itself establish the separately checked-out source.
The helper verifies both statements cryptographically with `gh attestation
verify`, checks the subject digest and exact signer workflow on `main`, and binds
`workflow_sha` to the verified certificate's execution digest. It cross-checks
OCI labels. Labels and unverified bundles cannot authorize reuse.

To independently inspect the signed statements, authenticate to GHCR if your
client requires it, select the **recorded digest**, and run:

```sh
IMAGE=ghcr.io/dstoc/tines-mcp@sha256:REPLACE_WITH_RECORDED_DIGEST
SIGNER=https://github.com/dstoc/tines-mcp/.github/workflows/release.yml@refs/heads/main
gh attestation verify "oci://$IMAGE" --repo dstoc/tines-mcp \
  --bundle-from-oci --cert-identity "$SIGNER" --source-ref refs/heads/main \
  --predicate-type https://slsa.dev/provenance/v1 --format json
gh attestation verify "oci://$IMAGE" --repo dstoc/tines-mcp \
  --bundle-from-oci --cert-identity "$SIGNER" --source-ref refs/heads/main \
  --predicate-type https://github.com/dstoc/tines-mcp/attestations/release-identity/v1 \
  --format json
```

These commands verify signatures; also compare the verified statement fields to
the release record and source tag. Read `verificationResult.statement` and
`verificationResult.signature.certificate`, not an unverified bundle. Recovery
can use a newer workflow commit; the original signed workflow SHA must match its
certificate, not the recovery execution SHA. `scripts/release.py` applies these
checks during publication and recovery.

## Recover without replacing an image

A source GitHub Release or tag can exist even if the Release Please action fails
before returning outputs, or the publishing job fails or never starts. Inspect
those records and preserve them. Fix workflow problems through review, then use
the explicit recovery dispatch above with the same tag and source SHA. Disabling
the main-push Release Please job stops future automation; manual dispatch remains
available. Reverting automation cannot undo created source or image records.

Stop after any failure and inspect the record and exact package. Do not delete,
retag, overwrite, repeat a push, or sign an existing unverified image to make the
workflow green. Do not rebuild an existing version: mutable base images and
package repositories can change its bytes.

- **Before any push:** fix the source, workflow, or access problem through normal
  review. A changed release source needs a new version/tag. If the package is
  still absent, preserve proof of no push and request a narrowly reviewed
  creation adjustment. Ordinary reruns and dispatches cannot reopen creation.
- **Ambiguous push, post-push lookup failure, or missing evidence:** investigate
  as partial/uncertain publication. Recovery requires positive package access.
  A package 404 or denied enumeration requires owner investigation. Deletion,
  lost access, or a later 404 does not reopen creation.
- **Valid matching image, visibility-only failure:** the owner can make the
  package public. Dispatch recovery with the same tag/source. It verifies and
  tests the existing digest, with zero replacement writes or new attestations.
- **Incomplete attestations, conflicting source/labels/digest, or runtime
  failure:** investigate outside automated recovery. Automation will not add
  missing source attestations or repair registry content. Prefer a reviewed new
  patch version when identity cannot be established.
- **Moved source tag or package identity:** stop and resolve the integrity
  incident. The workflow does not move the tag back or repair a raced registry
  write. Use previously recorded digest pins for deployed instances.

## Deploy the accepted digest

Use the accepted record, not an assumed available version:

```sh
IMAGE=ghcr.io/dstoc/tines-mcp@sha256:REPLACE_WITH_RECORDED_DIGEST
podman pull --platform linux/amd64 "$IMAGE"
```

Replace `tines-mcp:local` with `"$IMAGE"` in the README's manual Podman commands.
Keep its loopback bind, explicit MCP configuration, and separate tunnel-client
container that joins the adapter's network namespace. Recreating the adapter
also requires recreating the tunnel client in the replacement namespace. The
adapter image contains no tunnel agent or process supervisor.

## Local verification

Run these checks from the repository root with Node.js 24 (or later), actionlint
1.7.12 (the CI pin), and the pinned Rust toolchain:

```sh
npm ci --prefix tests/release-please --ignore-scripts
npm test --prefix tests/release-please
python3 -m unittest discover -s tests/release -v
bash -n scripts/container-smoke.sh
actionlint -shellcheck= -pyflakes= .github/workflows/*.yml
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
docker build --file Containerfile --tag tines-mcp:ci .
bash scripts/container-smoke.sh tines-mcp:ci
```

Release tests mock registry/engine commands and test the actual shared shell
script. The Node suite runs Release Please 17.3.0 (bundled in the pinned action)
against a read-only fake GitHub client and evaluates actual workflow expressions.
It checks synchronized versions, unchanged dependencies, no-release updates,
malformed handoffs, skipped/failed jobs, first-attempt eligibility, and strict
recovery. These tests do not publish or establish live Actions-token authority. actionlint checks actual
workflow syntax and Actions contexts. CI keeps these checks, the Rust suite, and
the real container build/runtime check. A separately authorized
live publication must still establish GHCR access, both signed statements,
anonymous pull, and runtime acceptance.
