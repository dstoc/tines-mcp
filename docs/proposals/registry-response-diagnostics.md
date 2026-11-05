# Proposal: Retain safe package-response diagnostics

## Motivation

The v0.1.1 publisher stopped at `registry_authority` without recording which
package-response predicate failed. Implement tines-mcp/39's diagnostic correction
before a separately authorized new patch release. Douglas chose a new release
instead of legacy image recovery. This proposal authorizes no publication.

## Problem statement

At main `8853ca2e0af4ae78487443b3c6f0d9054c51ed66`, `Registry.get` in
`scripts/release.py` uses Python `http.client`, bounded strict JSON parsing, and
the fixed GitHub API origin. `Registry.lookup` combines five package-404 checks
in one error. `Release.publish` saves the package observation only after lookup
returns, so a rejected response leaves no predicate evidence in `record.json`.
The same predicate runs initially and immediately before a creation push.

The inherited incident record identifies run `41999036676`, attempt 1,
`stage=no_push_attempted`, `push_attempted=false`, and `validation=incomplete`.
Its artifact is `adapter-release-41999036676-1`, ID `13401955987`, archive SHA-256
`d792be91cf22bd4b53d6ae1ffbf3330b6349d32d5381224ade6f5ed4bd4b6775`.
The previous investigation could read artifact metadata but encountered HTTP 403
when downloading logs and the archive. The archive hash is inherited evidence,
not an independently verified download. Preserve these distinctions.

The error establishes a parsed JSON value and HTTP 404 in the eligible branch.
It does not identify the failed predicate or establish package absence, an
authority denial, or a compatible response variant. The current official
[package endpoint reference](https://docs.github.com/en/rest/packages/packages#get-a-package-for-a-user)
lists a 200 response without an exact 404 body contract.
[GitHub authentication documentation](https://docs.github.com/en/rest/authentication/authenticating-to-the-rest-api#failed-login-limit)
also permits 404 for insufficient permission. No functional response-predicate
change is justified by this evidence.

`dispatch` separately hard-codes creation eligibility to a successful first
attempt of the `v0.1.1` handoff. A new patch cannot use that allowance. The
approved tines-mcp/38 proposal preserves this restriction while adding draft,
retention, and finalization checks. This requires an explicit review decision,
not an incidental broadening of the diagnostic fix.

## Proposal

### Fixed diagnostic fields

Keep the current acceptance predicate exactly equivalent. Compute its individual
results without short-circuiting away useful evidence. Do not relax message,
documentation-prefix, object, status, route, or pagination checks.

Add one small, explicit diagnostic snapshot for the fixed package metadata GET.
Use only fields constructed by the helper:

| Field | Allowed values |
| --- | --- |
| `schema_version` | Integer `1` |
| `route` | Literal `package_metadata` |
| `http_status` | Received integer HTTP status, or null before headers |
| `body_bytes_observed` | Integer from 0 through the existing 8 MiB read cap plus one, or null before body completion |
| `json_result` | Fixed enum: `not_read`, `too_large`, `invalid`, `duplicate_field`, `invalid_constant`, `parsed` |
| `json_type` | Fixed enum: `unknown`, `object`, `array`, `string`, `number`, `boolean`, `null` |
| `link_state` | Fixed enum: `unknown`, `absent`, `empty`, `present` |
| `json_object` | Boolean, or null before parsing |
| `message_exact` | Boolean, or null before parsing |
| `documentation_string` | Boolean, or null before parsing |
| `documentation_prefix` | Boolean, or null before parsing |
| `link_absent` | Boolean, or null before headers |

For parsed non-objects, all body-field predicate results are false. For an
unparsed response, unavailable body-field results stay null. A null result must
never be interpreted as a passing check. Preserve the difference between a
missing Link header and an empty Link header. Both empty and nonempty headers
continue to fail the creation predicate.

Do not emit raw bodies, headers, messages, documentation URLs, request URLs,
arbitrary JSON keys, exception text, tokens, response previews, or hashes of
response strings. Field names such as `message_exact` describe comparisons;
their values must never contain the compared string. Do not enable HTTP debug
logging, subprocess verbosity, extra authority probes, or extra GETs.

### Persistence and failure behavior

Pass a narrow observation sink from `Release` into `Registry`. Persist the
snapshot through the existing atomic `save`/`write_json` path before a failing
predicate raises. Capture parse failures and transport failures using fixed
classifications; keep the existing bounded read, duplicate-key rejection,
invalid-constant rejection, timeout, no-redirect behavior, and fixed errors.
Do not serialize an exception to identify its category.

Keep snapshots in a fixed-purpose map in `record.json`, with entries for initial
authority, pre-push, post-push, acceptance, and finalization metadata checks.
Each purpose holds only its latest snapshot. Add only purposes actually used by
the combined #38 implementation. The maximum is five entries; version pages do
not append entries or introduce a second response collector. Purpose names come
from fixed call sites, never response or exception data.

The sink must finish before execution proceeds. A record-write failure stops the
operation before a subsequent push; it cannot make an unsuitable response
acceptable. Diagnostics cannot alter `push_attempted`, the selected digest,
creation eligibility, acceptance, retention, or publication gates. Preserve an
already recorded push boundary on every later failure.

Existing `main` error handling, `report`, step summary, and always-run Actions
evidence retention must preserve the snapshots on rejection. Report reload in a
separate process must retain them. A later diagnostic or artifact-upload failure
must not claim that evidence was retained successfully. Do not add snapshots to
#38's durable acceptance asset by copying arbitrary process state; retain that
asset's allowlisted schema and validation.

### Decision requested: bounded next-patch eligibility

Approve moving the single exact-version creation allowance from `v0.1.1` to
`v0.1.2`, rather than making it available to arbitrary stable versions. This is
a proposed code-policy pin, not a version bump or a release request. If review
does not approve this pin, implement diagnostics only and explicitly report that
the unavailable-package path remains closed to a new patch. Do not silently
choose another version or generalize eligibility.

If approved, require the genuine successful main-push Release Please handoff,
exact tag/version `v0.1.2`/`0.1.2`, attempt exactly `1`, and #38's validated new
draft identity before this allowance can reach registry work. Manual dispatch,
reruns, legacy 0.1.1, other versions, and already-published releases remain
ineligible. Bind the eventual source through the existing tag, ancestry, Cargo
manifest/lockfile, release-output, and draft checks. Add no source placeholder
that can be mistaken for an approved release commit.

Preserve the exact package-response predicate at initial lookup and pre-push
recheck. An eligible 404 remains an unavailable observation, not proof of
absence or permission to publish. Before any future publication authorization,
the owner must establish authoritative namespace absence, exclusive writers,
and no unresolved prior push across all attempts. The incident's no-push record
does not establish those namespace facts. If a later eligible response fails,
retain the new flags and stop. Any acceptance change requires trustworthy
endpoint-specific evidence and separate review.

Retain positive-access identity checks, complete active-version enumeration,
no-overwrite behavior, one push, exact-source attestations, original signer
checks, anonymous pull, runtime acceptance, and ambiguous-outcome safeguards.
Remove the bounded allowance through a reviewed follow-up after successful
first-package publication.

### Integration with tines-mcp/38

Use #38's reviewed implementation as the landing base. As inspected on November
5, #38 is implementing its approved proposal; no remote implementation branch
or open PR was available to inspect. Its thread states that `Registry.get` and
`Registry.lookup` remain unchanged, and that retain/finalize phases are added.
This is coordination evidence, not verification of a combined diff.

Reconcile `scripts/release.py`, `.github/workflows/release.yml`,
`tests/release/test_release.py`, Release Please handoff tests, and
`docs/releases.md` against the actual combined source before landing. Preserve
#38's immutable-release handling, durable evidence, successful-upload gates,
and refusal to backfill published releases. Prefer no workflow change for #39
unless a persistence test demonstrates that existing failure retention omits
the new safe fields.

Replace stale operator guidance about open PR #34 and legacy creation recovery
with the preserved incident identity and the separately authorized next-patch
path. Retain useful operational authority requirements. Do not change historical
proposal approvals to imply that a new eligibility policy was already approved.

## Non-goals

- Recovering, backfilling, redrafting, deleting, or amending v0.1.1. Keep tag and
  source `8853ca2e0af4ae78487443b3c6f0d9054c51ed66` and published release ID
  `434971264` unchanged, including its no-push evidence.
- Claiming the original failed predicate or authority outcome is known.
- Accepting arbitrary or merely schema-compatible 404s, authentication failures,
  empty namespace listings, or version-list errors as creation permission.
- New dependencies, credentials, settings, publication modes, or debug switches.
- Release/version PRs, workflow dispatch/retry/cancellation, image builds or
  pushes, retags, release/tag mutations, deployment, or alternate credentials.

## Suggested implementation shape

1. After proposal approval, implement the fixed snapshot and observation sink in
   the existing helper. Add offline HTTP/CLI fixtures before changing callers.
2. Reconcile #38's final call sites and retention paths. Apply the exact next-patch
   pin only if the review explicitly approves it.
3. Update operator guidance and submit a conventional-title implementation PR
   referencing tines-mcp/39 and coordinating with tines-mcp/38. Require normal CI
   and independent review of the combined source. Do not wait for a live release
   to validate redaction or persistence.

## Verification

- Baseline on `8853ca2`: all 60 Python release tests pass. This reproduces the
  diagnostic gap through fixtures, not the unknown original response.
- Vary each predicate independently, then combinations: scalar/array/null JSON,
  absent/wrong/mistyped message, missing/null/non-string/unaccepted documentation
  URL, and missing/empty/nonempty Link. Assert exact flags and unchanged eligible
  acceptance. Treat hypothetical variants as negative fixtures, not contracts.
- Exercise strict and eligible callers with 200, redirects, 401, 403, 404, 429,
  and 5xx; malformed JSON, duplicate keys, invalid constants, oversized bodies,
  and transport failures. Assert unchanged request counts and zero pushes on
  rejection. Version-list errors must never enter the allowance.
- Inject secret markers and control characters into body fields, unknown fields,
  headers, messages, URLs, and exception text. Assert their absence from stdout,
  stderr, `record.json`, step summaries, and all retained diagnostic files. Assert
  the complete allowlisted schema and bounded map, not just marker absence.
- Drive `main` with a failing response, reload its record for `report`, and
  inspect the actual retained directory and summary. Cover initial and pre-push
  rejection, write failure, failed evidence upload, and post-push failure. Verify
  no success/retention claim after failure and no reset of ambiguous outcomes.
- If approved, test v0.1.2 attempt 1 with valid fresh draft/handoff, alongside
  manual dispatch, attempts 2/empty/01, 0.1.1, other versions, failed handoffs,
  published releases, source mismatches, and changed pre-push observations.
  Preserve #38's published-legacy refusal before any image/release writes.
- Run `python3 -m unittest discover -s tests/release -v`, the pinned Node Release
  Please/workflow expression suite, and actionlint against combined workflows.
  Run required Rust format/clippy/locked tests for the implementation PR. Use
  mocked container commands here; live image actions are outside this issue's
  authorization. Local fixtures do not establish live package authority.

## Success criteria

- A rejected package response leaves bounded, fixed predicate evidence without
  raw response data or secrets in any emitted diagnostic output.
- Response acceptance and all authority/no-overwrite/ambiguous-outcome safeguards
  remain unchanged; the original incident remains explicitly unresolved.
- The next-patch eligibility decision is explicit and tested, and the combined
  #38/#39 source passes CI and independent review before landing.
- Legacy v0.1.1 resources and evidence remain unchanged. No release action occurs.
