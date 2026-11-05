# Full-surface acceptance record

This record belongs to tines-mcp/29 (`mcp-15`). It consolidates the shipped
surface and the independent mcp-12, mcp-13 and mcp-14 records. It does not delay
earlier deliveries accepted under those issues' revised testing directions.

**Status: local adapter checks pass; deployment and intended-client acceptance
remain incomplete.** No inspected source, SDK result, process health check or
ignored test is an intended OpenAI client/tunnel pass.

On November 5, 2026, Douglas approved separating code landing from live
deployment/client validation in
[tines-mcp/29](https://tines.tbuckley.dev/issues/prj_yruhXD41qH2onQN2/29).
The code audit, documentation and automated checks can proceed through normal
CI and independent review. Deployment/client acceptance and residual native
lifecycle evidence remain open in
[tines-mcp/34](https://tines.tbuckley.dev/issues/prj_yruhXD41qH2onQN2/34)
and do not block this code landing. This scoped decision is not a GitHub
approval, a live pass or a waiver of unrelated acceptance requirements.

## Revisions and tested paths

Original implementation-run observations, recorded on November 5, 2026:

| Item | Observation |
| --- | --- |
| Adapter base | `37f166a` in `dstoc/tines-mcp`; final changes are in the PR attached to tines-mcp/29. |
| Reference source and native-shape fixtures | `af6cb25febf1b45d7eff32251f78fcf6355aed4a` in `tbuckley/tines`. Source inspection does not establish deployed parity. |
| Deployed Tines target | `https://tines.tbuckley.dev`; a credential-free `GET /api/version` returned HTTP 403 in this run. The environment did not supply `TINES_API_KEY`; `tines config --json` reported no local key source, although harness-managed CLI issue bookkeeping worked. No usable key was recovered from the reported config path. Credentials were not stored, logged or changed. |
| Deployed Tines revision | **Unverified.** No native version response was obtained. |
| Current test client path | Local HTTP requests through the production `/mcp` router, both `2025-11-25` and `2026-07-28`; existing pinned rmcp 3.8.0 SDK transport tests. All upstreams for this run's adapter tests are local fixtures. |
| Intended OpenAI client/version and tunnel | **Not supplied and not tested.** No existing acceptance record for that connection was supplied. |
| Deployment listener, allowlists, token authority and request budget | **Not observed.** Repository defaults/examples were checked against code and automated tests; they are not deployed settings. |

No native mutation was performed for this code acceptance work. The prior artifact
SDK record remains a separate historical result, not a rerun or a substitute for
the intended client.

## Combined automated checks

Use the installed pinned Rust 1.98.1 toolchain and run from the repository root:

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo test --locked --test issue_workflow_acceptance --test artifact_acceptance --test context_recovery_acceptance --test full_surface
```

The three existing integration suites passed together before this change:
**33 passed and 3 ignored**. The ignored tests are the historical artifact native
scenario and two context/recovery native scenarios. They remain compiled and
unrun. Final normal checks passed: formatting, clippy with warnings denied, and
**241 tests passed, 3 ignored**. The combined command including `full_surface`
passed **35 tests, 3 ignored**. No container runtime was available in this run.

After the owner's scoped decision, the documentation update reran formatting,
clippy, the full Rust suite (**241 passed, 3 ignored**), the combined command
(**35 passed, 3 ignored**) and release-helper tests (**44 passed**). Local links
in the README, inventory and this record resolved. This rerun used local
fixtures; no container runtime was available and no live acceptance was added.

The final discovery fixture pins 41 implemented tools: 25 reads and 16 mutations.
It includes descriptions, strict input schemas, all four annotations and the
shared output schema. The full-surface HTTP tests compare the entire registry
on both protocol revisions, reconcile its exact names with the normative
endpoint catalog, and check every advertised tool for:

- Zero upstream requests during discovery and rejection of an unknown field.
- Exactly one upstream request for native 302, 403, 429 and 500 responses.
- Native status, code and conservative unknown write outcomes without a retry.

The domain and integration suites retain their independent success, receipt,
null/omission, revision, permission and cancellation fixtures. The existing
issue-workflow suite also covers both issue selectors and log modes. Discovery
uses no credentials and has no authority lookup. For the 2026 protocol,
`tools/call` test requests supply both `Mcp-Method` and `Mcp-Name` headers plus
request metadata; the SDK validates these against the request body.

## Executor and diagnostics audit

The runtime path is `ReadTools::call_tool` → `tools::dispatch` → pure module
preparation → `TinesClient::execute_prepared` → `execute_once` → local projection
and result budgeting. Preparation selects one endpoint. Projection receives
response data and local context; it receives no client or request callback.
Unknown tools never select an executor. Unconfigured calls validate locally.

The facade's dormant comment fallback, including its parent-issue preflight,
was removed. The obsolete `Route`, `Query`, `run_tool`, `TinesToolCall`, old
error/envelope code and second HTTP executor were removed. Their internal
legacy tests were retired with that implementation. Existing domain/HTTP suites
remain unchanged; the protocol authorization test now uses the current shared
dispatcher. Diagnostics, token-bucket admission/refill and exact serialized
wire-size checks now exercise the current executor and serializer. Historical
baseline test counts therefore do not equal the final suite count.

The source audit found one authenticated network execution expression,
`self.inner.http.execute(request).await`, in `src/tines.rs::execute_once`.
No domain file imports reqwest or the authenticated client. The client is built
without upstream I/O, with redirects disabled and `reqwest::retry::never()`.
No automatic auth check, parent lookup, fallback, retry, polling, enrichment or
remediation request exists in the runtime dispatch path. Every endpoint comes
from a registered typed tool; no generic HTTP tool is advertised.

Startup logs contain the listener address. Executor diagnostics contain a fixed
route template, method, received status, elapsed time, request count and write
outcome. They omit credentials, dynamic IDs, query values, payloads and arbitrary
native error text. The diagnostics regression test supplies distinct private
values for all these fields and captures DEBUG output. Result serialization
redacts the configured credential recursively, including object keys and
receipts. Native content and errors returned to the caller remain data; they
are not diagnostic logs or instructions to execute.

## Limits and operator guidance

The [README](../../README.md) describes deployment and configuration. The
[tool inventory](../tool-inventory.md) maps every shipped tool to its native
endpoint and explains transitions, context scope, artifacts and recovery.
All context mutations now explicitly advertise write, destructive,
non-idempotent and open-world hints according to their operation. No tool grants
personal consent or expands the configured server token's native authority.

Separate limits remain visible to operators:

| Budget | Value and meaning |
| --- | --- |
| Serialized request | 2 MiB default; `MCP_MAX_REQUEST_BYTES` can raise it through 36 MiB. JSON escaping, metadata, envelope and base64 expansion count. Both ingress and rmcp use the configured cap. |
| Native decoded file | 25 MiB; the serialized request and client/tunnel caps can reduce usable size. Text artifact content has a separate 256 KiB UTF-8 cap. |
| Inline content/log source | 1 MiB; each continuation downloads and hashes the bounded source again. |
| Returned content chunk | At most 32 KiB; text respects UTF-8 boundaries and binary uses base64. |
| Native success JSON / error body | 32 MiB / 1 MiB. |
| Complete serialized MCP result | 512 KiB, including text and structured representations. Validated oversized successes retain a compact receipt and omission notice. |
| Admission and deadlines | Four POST bodies before buffering, four concurrent tool calls, 30-second body deadline, 45-second tool deadline and 5-second connect timeout. Local rate limit: 10 calls/second, burst 20. |

Raising upload limits does not raise read or output limits. Request budgets can
also be lower in the intended client or tunnel. There is no staging, range
upload, URL import or attachment-handle resolver.

## Earlier evidence and remaining work

The remaining deployment/client and native lifecycle work below is tracked in
[tines-mcp/34](https://tines.tbuckley.dev/issues/prj_yruhXD41qH2onQN2/34).
It remains unverified and is separate from review and landing of this change.

The following records retain their own source/CI evidence and limitations:

- [Issue/workflow](issue-workflow.md): HTTP forwarding and request counts;
  native graph, shared decisions, permissions, consent and dispatch gaps.
- [Artifacts](artifacts.md): recorded 25-request native SDK result and upstream
  gate/freshness coverage; native gate transitions and intended-client uploads
  remain unverified.
- [Context/recovery](../testing/context-recovery.md): HTTP executor evidence
  and pinned upstream unit/source coverage; native authorization, conflict
  preservation, resume-to-dispatch and eventual cancellation remain untested
  through this adapter.

Upstream unit CI at the reference revision passed, but its overall E2E run
failed. The recorded context-write socket hang-up and concurrent runner fencing
409 overlap these domains. Consolidating the adapter suites does not resolve
those native gaps or make the upstream E2E result green. The revised prerequisite
gates did not require provisioning another Tines service or retrying denied
setup. This gate does not reinstate that requirement.

To complete deployment acceptance, an operator must supply the intended
client/version, configured tunnel connection, deployment identity/settings and
an authorized disposable issue. Record the adapter commit/image digest, native
`GET /api/version` result, exact Host/Origin allowlists, effective request budget
and configured token authority without recording credentials. Connect through
that actual client and:

1. Discover the complete reviewed surface and make an authorized safe read.
2. Upload a UTF-8 proposal, then read metadata and exact bytes in separate calls.
3. Exercise caller-supplied base64 only if supported. Record tested byte sizes,
   effective limits, exact round-trip evidence and observed failures. Record any
   unsupported binary path instead of inventing another transfer mechanism.
4. Record the approved workflow scenario and its native evidence. Keep every
   mutation, witness read and later status observation separate. Do not infer
   launch or stopped execution from a successful API receipt.

No live deployment or intended-client acceptance is claimed. The owner's scoped
decision permits code landing while the evidence above remains open in
tines-mcp/34. The original domain reports and unrelated acceptance requirements
remain in effect. This document does not authorize production recovery mutations,
credential changes or a release publication.
