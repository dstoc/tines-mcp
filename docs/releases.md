# Adapter image releases

The release workflow publishes one `linux/amd64` adapter image at
`ghcr.io/dstoc/tines-mcp:X.Y.Z`. It records the source commit and image digest and
verifies signed provenance, signed release identity, and the container runtime.
It publishes no floating tags, CLI archives, or GitHub Releases. Merging the
workflow or creating a tag does not publish an image. This guide describes the
prepared workflow; it is not evidence that a version has been published.

The image runs the adapter as `10001:10001`. The tunnel client remains a separate
container. Release checks establish container liveness and clean shutdown, not
live Tines permissions, the final API inventory, or intended-client tunnel access.
See the selected source commit's README for its supported tools.

## Before publication

An operator must separately authorize each publication. Review the release PR and
its independent checks first. Do not dispatch during implementation or review.

The repository must permit the pinned checkout, Rust toolchain, attestation, and
artifact-upload actions. The publication job uses only its ephemeral Actions
`GITHUB_TOKEN`, with `contents: read`, `packages: write`, `attestations: write`, and
`id-token: write`. Repository policy must permit those capabilities and GitHub
artifact attestations. The workflow does not need a PAT, a Tines token,
`artifact-metadata` permission, or repository administration permission. Storage
record creation is explicitly disabled for this personal-account repository.
No workflow step changes credentials, visibility, or repository settings.

Restrict tag and package writers and arrange exclusive publication for the whole
run. The workflow serializes its own runs and checks the remote source tag and
registry immediately before pushing and at completion. GHCR has no conditional
tag write in this flow. An external publisher can race those checks. If other
writers cannot be excluded, **do not dispatch**; request a reviewed authority
change. Tag protections and digest pins remain necessary outside the workflow.

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

## Prepare a version and source tag

1. Review and merge a version change on `main`. Use a stable numeric version
   without leading zeroes, such as `0.1.0`. Set both `Cargo.toml` and the local
   `tines-mcp` entry in `Cargo.lock` to that version. Run the normal CI checks.
2. Choose the exact reviewed source commit. Create an annotated `vX.Y.Z` tag for
   that commit and push it as a **separate, authorized operator action**. Never
   move a release tag. The workflow does not create or move tags.
3. Record the full lowercase 40-character source SHA. It must match the tag and
   be an ancestor of the selected `main` workflow commit. Source and workflow
   commits can differ, for example when recovering with a reviewed workflow fix.

The workflow uses release helpers from its own `main` checkout. It checks out the
resolved source SHA separately and builds only that directory. Cargo metadata
and the lockfile must agree with the tag. The source container is built once,
loaded locally, and tested before the same image is pushed.

## Dispatch an established package release

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

Run these checks from the repository root with the pinned Rust toolchain:

```sh
python3 -m unittest discover -s tests/release -v
bash -n scripts/container-smoke.sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
docker build --file Containerfile --tag tines-mcp:ci .
bash scripts/container-smoke.sh tines-mcp:ci
```

Release tests mock registry/engine commands and test the actual shared shell
script. They do not publish or establish live Actions-token authority. CI keeps
the Rust suite and real container build/runtime check. A separately authorized
live publication must still establish GHCR access, both signed statements,
anonymous pull, and runtime acceptance.
