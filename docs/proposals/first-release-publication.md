# Proposal: Let the first Release Please release create its package

## Motivation

The first genuine release should publish `ghcr.io/dstoc/tines-mcp:0.1.1`
directly. Douglas approved this direction in tines-mcp/37. A separate owner-only
`0.1.0` publication is no longer a prerequisite. Release PR
[34](https://github.com/dstoc/tines-mcp/pull/34) must remain open until the
publishing change has passed proposal and implementation review. Those reviews
do not authorize merging the release PR or performing a release.

## Problem statement

At main `b6570c17c4270be7187c1d0afb6c077ce9f2efc9`,
`.github/workflows/release.yml` sends every successful Release Please handoff to
`normal` mode. `Registry.lookup` in `scripts/release.py` requires positively
accessible package metadata and complete active-version enumeration. A package
404 stops the job. The only creation exception is a manual, owner-confirmed
`v0.1.0` bootstrap. Thus the prepared 0.1.1 release cannot create the missing
package naturally.

The strict lookup protects existing versions and uncertain publication outcomes.
It must remain the default. A
[GitHub API 404 can mean insufficient permissions](https://docs.github.com/en/rest/authentication/authenticating-to-the-rest-api#failed-login-limit).
Neither successful Docker login nor an empty access-filtered package listing
establishes namespace-wide absence.

[Chime at 7f5460d](https://github.com/dstoc/chime/blob/7f5460dfa40497ef3b2aa00f529b7795542a4d54/.github/workflows/release.yml)
uses a version-list 404 as its publication fallback. Do not copy that behavior:
it does not distinguish an inaccessible package from an absent one.

## Proposal

Keep one publisher and one manual recovery entry point. Replace the manual
0.1.0 bootstrap with a bounded creation allowance derived inside the helper
from the first genuine Release Please handoff. Do not expose a new dispatch
mode or confirmation input.

### Creation authority and the 404 limitation

Creation is eligible only when all existing source/handoff checks pass and:

- The event is the successful `main` push handoff with `release_created=true`.
- The tag and version are exactly `v0.1.1` and `0.1.1`.
- `GITHUB_RUN_ATTEMPT` is exactly `1`.

Bind the source SHA through the existing tag, ancestry, Cargo manifest, lockfile,
and release-output checks. The source SHA is the eventual reviewed release
commit, not PR #34's mutable branch head. Record creation eligibility and the
actual registry observation separately in `record.json`.

Within this allowance only, accept the exact authenticated package metadata
route's structured `404 Not Found` as an **unavailable package observation**.
It is not evidence of absence. Retain the current structured-error validation;
reject unexpected pagination headers on this response as well. Require the same
observation initially and immediately before the sole push. An accessible
package instead follows ordinary identity validation and complete enumeration:
an existing version selects its digest; an absent version can be published.
If a package appears between an initial unavailable observation and pre-push
recheck, stop without pushing. Restart through positive-access recovery.

This changes the source of creation authority. Before the separately authorized
release PR merge, the owner must verify the exact package is absent in an
authoritative owner view, exclude other package creators/writers, and establish
that no earlier publication attempt remains unresolved. This check belongs to
release authorization; it creates no image and requires no bootstrap dispatch,
tag, dummy package, PAT, or credential change. Approval of this proposal alone
does not establish those operational facts.

The review must explicitly accept this assumption for the one 0.1.1 handoff.
If an existing package is hidden from the token and produces the identical 404,
the helper cannot distinguish it from absence. Tests cannot prove otherwise.
The no-overwrite guarantee therefore depends on the owner namespace check and
exclusive writers in this path. It is not an atomic GHCR guarantee. If these
conditions cannot be established, do not authorize the release. Arbitrary 401,
403, 429, 5xx, redirects, malformed JSON, transport errors, and version-list 404
responses remain hard failures with zero push commands.

[GitHub documents](https://docs.github.com/en/packages/working-with-a-github-packages-registry/working-with-the-container-registry)
that publishing with a repository's `GITHUB_TOKEN` links the package to that
repository; a preexisting unconnected package can reject its push. That is
useful behavior, but it is not a replacement for the namespace check. Positive
post-push metadata and enumeration remain mandatory and establish the package
ID and exact digest before the attestation steps can run.

### Preserve the established publication path

Do not change the source checkout separation, single local build, OCI labels,
platform/user checks, smoke test, source-tag rechecks, one push, persisted
pre-dispatch uncertainty marker, or post-push digest check. An accessible
package must retain its ID across rechecks. A new package must become positively
accessible and associated with `dstoc/tines-mcp` after publication.

Keep both signed statements, their verified certificate/source bindings,
anonymous digest pull, final runtime checks, ephemeral Docker configurations,
and unconditional credential cleanup. Existing digest reuse must perform zero
builds, pushes, retags, and new attestations. Keep the adapter and tunnel-client
containers separate. Do not change workflow permissions or action revisions
for this feature.

### Reruns and partial publication

Manual dispatch accepts only the existing tag and full expected source SHA.
Remove `mode` and `bootstrap_confirmation` from the workflow inputs and delete
the old owner/confirmation/bootstrap branches. Reject stale bootstrap-related
environment values in direct helper invocation rather than silently honoring
them. Internally derive creation eligibility; callers cannot select it.

All manual dispatches, later versions, and run attempts after attempt 1 use
strict positive-access lookup. A package 404 never enables creation there.
Do not introduce a persistent latch, publication database, or history scanner.

| Observed state | Allowed next step |
| --- | --- |
| No push attempted; package positively accessible | Fix the reviewed workflow/access problem and use explicit recovery with the same source/tag. |
| No push attempted; package still absent | Creation cannot be retried by an ordinary rerun or dispatch. Preserve proof of no push and request a narrowly reviewed workflow adjustment before a new creation attempt. Do not run the superseded 0.1.0 bootstrap. |
| Push attempted, even if Docker failed or no digest was captured | Investigate the exact package and retained evidence. A subsequent 404 stops recovery; it does not reopen creation. |
| Existing digest has both valid signed statements and matching source/labels | Recover using that digest with no replacement writes or new attestations. |
| Image exists but statements, identity, or runtime checks are incomplete/conflicting | Stop for investigation. Do not sign, rebuild, overwrite, or retag it to repair acceptance; consider a reviewed new version. |
| Identity verified; anonymous pull fails because the new package is private | Owner changes package visibility separately, then positive-access recovery verifies the same digest. |

This intentionally favors safety after failure over automatic creation retries.
Release Please may not re-emit release outputs on rerun. Missing evidence never
establishes that no push happened. Later deletion or access loss does not make
another attempt eligible. After first publication, remove the bounded allowance
through a reviewed follow-up; package creation is not a general recovery mode.

### Documentation and issue context

Rewrite `docs/releases.md` around the first real 0.1.1 publication and ordinary
recovery. Remove the separate 0.1.0 bootstrap procedure, confirmation string,
dispatch modes, and its prerequisites throughout the guide. Replace the now
completed initial-PR preparation procedure with the pending PR #34 status and
the required review/release-authorization boundary. Retain the 0.1.0 manifest
seed as history; it does not claim publication. Do not change seed configuration
or the pending release PR in this implementation.

Explain the namespace check, exclusive-writer assumption, inaccessible-package
ambiguity, first-attempt restriction, and pre-push failure boundary explicitly.
GitHub creates new packages as private by default. The owner still needs to
[make the package public](https://docs.github.com/en/packages/learn-github-packages/configuring-a-packages-access-control-and-visibility)
before anonymous acceptance can pass. The workflow must not change visibility
or credentials. If access fails after creation, investigate repository
association/Actions access; do not treat it as another missing package.

Add issue comments to tines-mcp/31, /35, and /36 linking tines-mcp/37's superseding
decision. Do not rewrite shared context directly. Preserve historical evidence,
but make current instructions unambiguous. Keep release PR #34 unmerged during
proposal and implementation work.

## Non-goals

- No release PR approval/merge, source tag or GitHub Release creation,
  publication dispatch, registry writes, deployment, or security-setting changes
  during this issue's implementation and review.
- No separate 0.1.0 release, CLI archive flow, floating image tags, tunnel image,
  new credential, broad registry framework, or automatic repair of existing bytes.
- No claim that mocked tests establish live Actions-token authority, public
  visibility, signed acceptance, or atomic no-overwrite behavior.

## Suggested implementation shape

1. Obtain approval of this proposal, including the explicit namespace authority
   assumption and conservative retry restriction.
2. Update `dispatch` and `Registry.lookup` in `scripts/release.py`; use one
   internally derived creation-eligibility field and keep strict lookup as the
   default. Separate the 404 observation from package/tag absence established by
   positive enumeration. Adapt `Release.publish` and `report` without duplicating
   build, push, or acceptance paths.
3. Simplify `.github/workflows/release.yml` inputs/environment. Preserve the
   existing job admission, handoff outputs, permissions, attestations, cleanup,
   and main/source checkout boundaries.
4. Replace bootstrap tests in `tests/release/test_release.py` and
   `tests/release-please/workflow.test.mjs`. Update the guide and issue comments.
   Open an implementation PR referencing tines-mcp/37 for independent review.

## Verification

Use the existing mocked registry/engine orchestration tests to check observable
command counts, request paths, output handoff, and saved outcome records:

- A genuine 0.1.1 first-attempt handoff with two structured package 404s builds
  once, passes local runtime, pushes once, records uncertainty before dispatch,
  then requires positive package metadata and the returned digest. Exercise both
  signed-verification and anonymous/runtime acceptance in the mocked flow.
- Existing package/absent tag uses complete enumeration and one push. Existing
  version selects its digest with zero builds/pushes/new attestations. Preserve
  multi-page, package-ID, source, label, digest and signature-conflict tests.
- 401/403/429/5xx, redirects, malformed/oversized responses, invalid package
  identity, failed pagination, and version-list 404s stop before push. A package
  404 outside the bounded handoff stops too. Document that an indistinguishable
  hidden-package 404 inside the allowance depends on the owner assumption;
  do not label that fixture proof of absence.
- A package or version appearing before push, moved source tag, local image
  change, or runtime failure yields zero pushes. Unexpected source or handoff
  values reach no registry/engine commands.
- Push errors, missing push digest, post-push metadata 404/digest conflict,
  missing attestations, and private-package anonymous failure retain incomplete
  outcomes. A fresh recovery or run attempt 2 never treats a 404 as creation
  authority. A visible matching digest is reused; an incomplete image is never
  rebuilt or signed. Later release versions cannot use the creation allowance.
- Evaluate actual YAML job conditions and environment expressions through the
  pinned Actions expression library, including failed/skipped/cancelled Release
  Please jobs, malformed outputs, fork/ref guards, manual dispatch, and attempts
  1/2. Check removed inputs cannot select the old bootstrap path.
- Run `actionlint` against both actual workflow files to validate syntax and
  Actions context availability. Record the tool version and output. YAML parsing
  and helper tests alone do not establish valid Actions contexts.

Run `npm ci --prefix tests/release-please --ignore-scripts`,
`npm test --prefix tests/release-please`,
`python3 -m unittest discover -s tests/release -v`, and
`bash -n scripts/container-smoke.sh`. Run the repository's Rust fmt, locked
clippy, and locked test checks, plus the documented real container build/smoke
checks where available. Report unavailable local checks accurately; keep CI's
Rust and real container gate. Do not wait for CI completion before Request Review.
Do not test by publishing any image or invoking a live release.

## Success criteria

- The separately authorized first Release Please 0.1.1 handoff can create the
  package without a 0.1.0 bootstrap.
- The 404 limitation and required owner namespace checks are independently
  reviewed and documented; authentication errors do not silently imply absence.
- Existing digest recovery, no replacement writes, source identity, both signed
  statements, runtime checks and credential cleanup remain effective.
- Reruns and uncertain outcomes cannot silently reopen package creation.
- Release documentation and linked issue guidance agree with the superseding
  direction, and all specified offline behavior/workflow checks pass.
