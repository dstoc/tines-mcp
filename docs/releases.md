# Adapter image releases

The release workflow publishes one `linux/amd64` adapter image at
`ghcr.io/dstoc/tines-mcp:X.Y.Z`. It records the source commit and image digest and
verifies signed provenance, signed release identity, and the container runtime.
It publishes no floating tags or CLI archives. Release Please maintains a reviewed
version/changelog PR and creates its source tag and GitHub Release after merge.
The same workflow run passes that exact version and source SHA to the existing
container publisher. A source GitHub Release does not establish image acceptance.
This guide describes the prepared workflow; it is not evidence that a version has been published.

The image runs the adapter as `10001:10001`. The tunnel client remains a separate
container. Release checks establish container liveness and clean shutdown, not
live Tines permissions, the final API inventory, or intended-client tunnel access.
See the selected source commit's README for its supported tools.

## Normal releases

Before the first normal release, the owner must complete the separate `0.1.0`
bootstrap below and establish package access. Normal automation cannot bootstrap.

1. Use conventional commit subjects on `main`, such as `fix:` or `feat:`. On each
   main push, Release Please creates or updates one release PR for the root Rust
   package. Review `Cargo.toml`, `Cargo.lock`, `.release-please-manifest.json`, and
   `CHANGELOG.md` together. Dependency versions must stay unchanged unless a
   separate reviewed change updates them.
2. Approve bot-created or updated PR workflow runs when GitHub shows **Approve
   workflows to run**. Review the final PR head after each bot update. Require
   normal CI and independent review before merging. There is no auto-merge or
   workflow self-approval.
3. Merge the reviewed release PR to authorize its normal release. Release Please
   creates stable `vX.Y.Z` source identity and changelog notes, then hands its
   `release_created`, `tag_name`, `version`, and `sha` outputs to the publishing
   job in the same run. A PR-only update does not publish. Automatic publication
   always uses `normal` mode and independently checks the tag, ancestry and Cargo
   versions. Do not dispatch a second publication for a successful run.
4. Check and retain the image acceptance record below before deploying its digest.
   If source release creation or publication fails, inspect the source records
   and use explicit normal recovery. Do not assume a rerun emits release outputs.

The checked-in `0.1.0` manifest and `bootstrap-sha` (`2e2af44`) seed version and
changelog history only. They do not claim that `0.1.0` is published. Release Please
uses its native single-crate Rust updater; no workspace or CLI archive flow runs.
`CHANGELOG.md` is created with the first release PR.

GitHub suppresses tag-push workflows for tags created with `GITHUB_TOKEN`. This
flow uses a same-run dependency, not a tag-triggered workflow or a second dispatch.
See [GitHub's token-trigger rules](https://docs.github.com/en/actions/how-tos/write-workflows/choose-when-workflows-run/trigger-a-workflow).

## Owner setup before publication

No real release, tag, release PR merge, or publishing dispatch belongs to
implementation or review of this automation.

Repository policy must permit GitHub Actions to create pull requests. An owner
can check **Settings → Actions → General → Workflow permissions → Allow GitHub
Actions to create and approve pull requests**. The workflow never approves PRs.
If policy blocks PR creation or required checks, ask the owner to resolve it;
do not introduce a PAT or change settings from the workflow. Policy permission
was not verified during preparation (the read-only API lookup returned 403).

The Release Please job uses only ephemeral `GITHUB_TOKEN` with `contents: write`,
`issues: write` (release labels), and `pull-requests: write`, as required by the
[pinned action](https://github.com/googleapis/release-please-action/tree/5c625bfb5d1ff62eadeeb3772007f7f66fdcf071).
These capabilities are isolated from the publishing job.

The repository must permit the pinned Release Please, checkout, Rust toolchain,
attestation, and artifact-upload actions. The publication job uses only its
ephemeral Actions `GITHUB_TOKEN`, with `contents: read`, `packages: write`, `attestations: write`, and
`id-token: write`. Repository policy must permit those capabilities and GitHub
artifact attestations. The workflow does not need a PAT, a Tines token,
`artifact-metadata` permission, or repository administration permission. Storage
record creation is explicitly disabled for this personal-account repository.
No workflow step changes credentials, visibility, or repository settings.

Restrict tag and package writers and arrange exclusive publication for the whole
run. The workflow serializes its own runs and checks the remote source tag and
registry immediately before pushing and at completion. GHCR has no conditional
tag write in this flow. An external publisher can race those checks. If other
writers cannot be excluded, **do not merge a release PR or dispatch**. Request a
reviewed authority change. Tag protections and digest pins remain necessary outside the workflow.

For an established package, this job's token must obtain the exact package at
`GET /users/dstoc/packages/container/tines-mcp`, including its repository
association, and enumerate **every active version page**. An access-filtered
namespace listing, anonymous reads, or the runner's token cannot replace these
checks. Authentication, transport, rate-limit, malformed-response, pagination,
and package identity errors stop publication. A 404 never means absence in
normal mode. A complete enumeration can establish that a tag is absent only
inside the positively accessible package.

GitHub normally creates a new package as private. After first creation, the owner
must explicitly make it public in the package settings. This is a separate
operator action. Anonymous digest pull must succeed before acceptance. If the
job cannot read the new package with its own token, investigate the package's
repository association and Actions access. A successful GHCR login does not
prove package read or enumeration authority. Record the actual token behavior
in the first separately authorized publication; local tests do not establish it.

## Manual source preparation for bootstrap or recovery

Normal releases use the release PR above. For the owner-only first `0.1.0`
publication, review the existing `0.1.0` Cargo versions and normal CI, choose the
exact reviewed source commit, and create an annotated `v0.1.0` tag for it as a
separately authorized owner action. This automation does not perform that setup.

For recovery, retain the existing source tag and GitHub Release. Record the full
lowercase 40-character source SHA. It must match the tag and be an ancestor of
the selected `main` workflow commit. Never move a release tag. Source and workflow
commits can differ when recovering with a reviewed workflow fix.

The workflow uses release helpers from its own `main` checkout. It checks out the
resolved source SHA separately and builds only that directory. Cargo metadata
and the lockfile must agree with the tag. The source container is built once,
loaded locally, and tested before the same image is pushed.

## Dispatch normal recovery for an established package

Use **Actions → release → Run workflow**, with `main` selected. Set:

- `tag`: the existing `vX.Y.Z` source tag.
- `source_sha`: its full expected source commit SHA.
- `mode`: `normal` (the default).
- `bootstrap_confirmation`: leave empty.

The equivalent explicit command is:

```sh
gh workflow run release.yml --repo dstoc/tines-mcp --ref main \
  -f tag=v0.1.0 -f source_sha=REPLACE_WITH_FULL_SOURCE_SHA -f mode=normal
```

If the tag is absent from a positively accessible package, the workflow builds,
checks, and pushes it once. If the tag already exists, it selects that digest
without rebuilding, pushing, retagging, or generating new attestations. Reuse
requires both valid signed statements, matching release fields and OCI labels,
and runtime acceptance. A different or incomplete image stops recovery.

## First publication only: owner-confirmed bootstrap

Bootstrap is a temporary exception for the currently nonexistent package. The
owner's November 5 observation is design evidence; it is **not** a fresh dispatch
confirmation. Bootstrap is limited to `v0.1.0` / `0.1.0` and owner `dstoc`, numeric
actor ID `539597`. Both the initiating and triggering actor must be `dstoc` and
the run attempt must be `1`. Do not use GitHub's rerun button for bootstrap.

Before **each** bootstrap dispatch, the owner must:

1. Inspect the exact `dstoc/tines-mcp` package while signed in as `dstoc` and
   confirm it is still absent. An inaccessible owner view is not sufficient.
2. Inspect earlier publication runs. Confirm that no prior push remains
   unresolved. Missing logs or a missing digest do not prove no push occurred.
3. Exclude all other package creators and writers for this attempt.
4. Bind the confirmation to the reviewed source SHA by entering this exact input
   on a new dispatch from `main`, with `mode=bootstrap` and `tag=v0.1.0`:

```text
CREATE ghcr.io/dstoc/tines-mcp:0.1.0 FROM <full-source-sha>; OWNER CHECKED PACKAGE ABSENT; NO UNRESOLVED PRIOR PUSH
```

Replace `<full-source-sha>` with the same SHA as `source_sha`. Do not add a newline
or spaces at either end. This input records the owner's fresh checks. It grants
no registry atomicity.

The workflow must authenticate successfully and receive a structured 404 from
the exact authenticated package route, both initially and immediately before
push. That response **does not prove absence**: an inaccessible existing package
can produce the same result. The fresh owner confirmation and reviewed exception
supply bootstrap authority. A visible package or any other result stops
bootstrap. It never switches automatically from normal to bootstrap.

After a push is attempted, even if Docker returns an error, bootstrap is closed
pending investigation. Use a **new normal dispatch** for recovery. Only proof
that the prior run never attempted a push permits a new bootstrap dispatch with
fresh checks. A created or possibly created package, later deletion, lost access,
or a later 404 does not reopen bootstrap. Once the package is established,
remove this option through a small reviewed follow-up, even if public acceptance
is still blocked. No persistent latch or additional credential is installed.

## Read and retain release evidence

Every run attempts to upload `adapter-release-<run-id>-<attempt>` with a 90-day
retention period. `record.json` and the job summary include the tag, version,
source SHA, orchestration SHA, run URL, mode, actor/confirmation, package ID,
image digest, current operation, and acceptance result. Successful verification
also retains the CLI's verified provenance and identity output. Copy successful
records into durable release records before Actions artifacts expire. An
interrupted run may fail to upload evidence; treat missing evidence as uncertain.

The main stages are:

| Stage | Evidence and next action |
| --- | --- |
| `no_push_attempted` | This run has not dispatched a push. Correct the failure; apply fresh bootstrap checks if still eligible. |
| `push_attempted_outcome_uncertain` | A push was dispatched, but its result is not established. Investigate in normal mode only. |
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
the explicit normal dispatch above with the same tag and source SHA. Disabling
the main-push Release Please job stops future automation; manual dispatch remains
available. Reverting automation cannot undo created source or image records.

Stop after any failure and inspect the record and exact package. Do not delete,
retag, overwrite, repeat a push, or sign an existing unverified image to make the
workflow green. Do not rebuild an existing version: mutable base images and
package repositories can change its bytes.

- **Before any push:** fix the source, workflow, or access problem through normal
  review. A changed release source needs a new version/tag. Bootstrap requires
  proof that no earlier push was attempted and all fresh owner checks.
- **Ambiguous push, post-push lookup failure, or missing evidence:** investigate
  as partial/uncertain publication. Use normal mode only. A package 404 or denied
  enumeration requires owner investigation, not another bootstrap.
- **Valid matching image, visibility-only failure:** the owner can make the
  package public. Dispatch normal mode with the same tag/source. It verifies and
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

Run these checks from the repository root with Node.js 24 (or later) and the pinned
Rust toolchain:

```sh
npm ci --prefix tests/release-please --ignore-scripts
npm test --prefix tests/release-please
python3 -m unittest discover -s tests/release -v
bash -n scripts/container-smoke.sh
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
malformed handoffs, skipped/failed jobs, and manual-only bootstrap. These tests do
not publish or establish live Actions-token authority. CI keeps the Rust suite and real container build/runtime check. A separately authorized
live publication must still establish GHCR access, both signed statements,
anonymous pull, and runtime acceptance.
