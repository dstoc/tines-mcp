"""Draft-first acceptance against stateful release API fixtures; no live writes."""
import copy
import json
import os
import unittest
from unittest.mock import patch

import test_release as baseline
from test_release import (r, environment, visible, version, verified,
                          SOURCE_SHA, WORKFLOW_SHA, OLD_WORKFLOW, DIGEST, OTHER)


def release_row(**changes):
    row = dict(id=41, tag_name="v0.1.2", target_commitish=SOURCE_SHA, prerelease=False,
               draft=True, immutable=False, published_at=None)
    return dict(row, **changes)


class DraftTests(unittest.TestCase):
    # Share only the existing command/registry fixture, not its test cases.
    command = baseline.OrchestrationTests.command
    http = baseline.OrchestrationTests.http
    pushes = baseline.OrchestrationTests.pushes

    def setUp(self):
        baseline.OrchestrationTests.setUp(self)
        self.patch_releases.stop()
        self.row, self.assets, self.events = release_row(), [], []
        self.fail_api = None
        self.lose_upload = self.lose_publication = False
        self.patch_api = patch.object(r.Releases, "request", side_effect=self.request)
        self.patch_api.start()
        self.addCleanup(self.patch_api.stop)
        self.release.resolve()

    def request(self, method, path, body=None, host="api.github.com", binary=False):
        self.events.append((method, path))
        if self.fail_api and self.fail_api(method, path):
            raise r.ReleaseError("Injected release API failure")
        if method == "POST":
            self.assertEqual(host, "uploads.github.com")
            record = r.read_json(self.release.record.read_bytes())
            self.assertTrue(record["evidence_upload_attempted"])
            self.assertEqual(record["validation"], "passed")
            self.assertFalse(self.assets, "never overwrite evidence")
            asset = dict(id=51, name=r.EVIDENCE, state="uploaded", size=len(body),
                         digest=r.content_digest(body), content_type="application/json")
            self.assets.append((asset, body))
            if self.lose_upload:
                raise r.ReleaseError("Upload response lost")
            return 201, json.dumps(asset).encode(), None, None
        if method == "PATCH":
            self.assertEqual(body, b'{"draft":false}')
            record = r.read_json(self.release.record.read_bytes())
            self.assertTrue(record["publication_attempted"])
            self.assertTrue(record["evidence_retained"])
            self.assertTrue(self.assets)
            self.row.update(draft=False, immutable=True, published_at="2026-11-05T12:00:00Z")
            if self.lose_publication:
                raise r.ReleaseError("Publication response lost")
            return 200, json.dumps(self.row).encode(), None, None
        self.assertEqual(method, "GET")
        if path == r.RELEASES + "?per_page=100&page=1":
            value = [self.row]
        elif path == r.RELEASES + "/41":
            value = self.row
        elif path == r.RELEASES + "/41/assets?per_page=100&page=1":
            value = [a for a, _ in self.assets]
        elif self.assets and path == r.RELEASES + f"/assets/{self.assets[0][0]['id']}":
            self.assertTrue(binary)
            return 200, self.assets[0][1], None, None
        else:
            self.fail(f"Unexpected API request {method} {path}")
        return 200, json.dumps(value).encode(), None, None

    def accepted(self, reuse=True):
        self.replies = visible([version(tags=["0.1.2"])]) if reuse else (
            visible() + visible() + visible([version(tags=["0.1.2"])]))
        self.release.publish()
        self.replies += visible([version(tags=["0.1.2"])])
        self.release.accept()

    def final(self):
        self.replies += visible([version(tags=["0.1.2"])])
        self.release.finalize()

    def fresh(self):
        os.environ.update(GITHUB_EVENT_NAME="workflow_dispatch", GITHUB_RUN_ID="456", GITHUB_RUN_ATTEMPT="2")
        self.recovery_count = getattr(self, "recovery_count", 0) + 1
        self.release = r.Release(self.workflow, self.source, self.base / f"recovery{self.recovery_count}/record.json")
        self.release.resolve()

    def mutations(self):
        return [(method, path) for method, path in self.events if method != "GET"]

    def test_complete_new_image_then_evidence_then_only_publication_patch(self):
        self.accepted(reuse=False)
        self.assertEqual(self.mutations(), [])
        self.release.retain()
        self.assertTrue(self.row["draft"])
        self.assertEqual([m for m, _ in self.mutations()], ["POST"])
        self.final()
        self.assertEqual([m for m, _ in self.mutations()], ["POST", "PATCH"])
        self.assertEqual(self.release.state["publication"], "published")
        self.assertTrue(self.release.state["immutable"])
        self.assertEqual(len(self.pushes()), 1)
        # Stored bytes describe acceptance, never a claim of publication.
        self.assertNotIn("publication", r.read_json(self.assets[0][1]))
        self.assertEqual(r.read_json(self.assets[0][1])["snapshot"]["run_id"], "123")

    def test_real_metadata_diagnostics_stay_outside_durable_acceptance_asset(self):
        from test_package_responses import assert_safe_snapshot, MARKER
        self.patch_http.stop()
        body = json.dumps(dict(baseline.package(), message=MARKER, documentation_url=MARKER,
                               extra=MARKER)).encode()
        rows = json.dumps([version(tags=["0.1.2"])]).encode()
        with patch.object(r.http.client, "HTTPSConnection") as conn:
            response = conn.return_value.getresponse.return_value
            response.status, response.getheader.return_value = 200, None
            response.read.side_effect = [body, rows] * 3
            self.release.publish()
            self.release.accept()
            self.assertEqual(set(self.release.state["package_responses"]), {"initial_authority", "acceptance"})
            self.release.retain()
            durable = self.assets[0][1]
            self.assertNotIn("package_responses", r.read_json(durable)["snapshot"])
            self.assertNotIn(b"json_result", durable)
            self.release.finalize()
            self.assertEqual(set(self.release.state["package_responses"]),
                             {"initial_authority", "acceptance", "finalization"})
            for value in self.release.state["package_responses"].values():
                assert_safe_snapshot(self, value)
            self.assertEqual(conn.return_value.request.call_count, 6)
            for path in self.release.record.parent.iterdir():
                if path.is_file():
                    self.assertNotIn(MARKER.encode(), path.read_bytes())
            self.assertEqual(self.assets[0][1], durable)
            self.assertEqual(self.release.state["publication"], "published")
            self.assertEqual(self.pushes(), [])

    def test_failed_final_metadata_save_cannot_dispatch_publication(self):
        self.accepted()
        self.release.retain()
        before = r.read_json(self.release.record.read_bytes())
        self.patch_http.stop()
        original_write = r.write_json

        def fail_snapshot(path, value):
            if path == self.release.record and "finalization" in value.get("package_responses", {}):
                raise OSError("SECRET_RESPONSE_MARKER")
            original_write(path, value)

        with patch.object(r.http.client, "HTTPSConnection") as conn, patch.object(r, "write_json", side_effect=fail_snapshot):
            response = conn.return_value.getresponse.return_value
            response.status, response.getheader.return_value = 200, None
            response.read.return_value = json.dumps(baseline.package()).encode()
            with self.assertRaises(OSError):
                self.release.finalize()
        self.assertEqual(r.read_json(self.release.record.read_bytes()), before)
        self.assertEqual(self.release.state["digest"], DIGEST)
        self.assertNotIn("publication_attempted", self.release.state)
        self.assertEqual([method for method, _ in self.mutations()], ["POST"])
        self.assertTrue(self.row["draft"])
        self.assertEqual(self.pushes(), [])

    def test_valid_asset_from_older_acceptance_is_reused_without_rewriting(self):
        self.signer = OLD_WORKFLOW
        self.accepted()
        self.release.retain()
        original = self.assets[0][1]
        self.fresh()
        self.accepted()
        self.release.retain()
        self.assertEqual(self.assets[0][1], original)
        self.final()
        self.assertEqual([m for m, _ in self.mutations()], ["POST", "PATCH"])
        self.assertEqual(self.pushes(), [])
        self.assertFalse(any(c[:2] == ("docker", "build") for c, *_ in self.commands))
        self.assertFalse((self.release.record.parent / "release-identity.json").exists())

    def test_lost_upload_response_fresh_recovery_discovers_exact_bytes(self):
        self.accepted()
        self.lose_upload = True
        with self.assertRaisesRegex(r.ReleaseError, "response lost"):
            self.release.retain()
        self.assertTrue(self.row["draft"])
        self.assertTrue(self.assets)
        self.fresh()
        self.accepted()
        self.release.retain()
        self.final()
        self.assertEqual([m for m, _ in self.mutations()], ["POST", "PATCH"])

    def test_lost_publication_response_recovery_is_read_only_for_mutable_and_immutable(self):
        for immutable in (True, False):
            with self.subTest(immutable=immutable):
                self.setUp()
                self.accepted()
                self.release.retain()
                self.lose_publication = True
                with self.assertRaisesRegex(r.ReleaseError, "response lost"):
                    self.final()
                self.assertEqual(self.release.state["publication"], "outcome_uncertain")
                with self.assertRaises(r.ReleaseError):
                    self.final()
                self.row["immutable"] = immutable
                self.events.clear()
                self.commands.clear()
                self.fresh()
                self.accepted()
                self.release.retain()
                self.final()
                self.assertEqual(self.release.state["publication"], "already_published_read_only")
                self.assertEqual(self.mutations(), [])
                self.assertEqual(self.pushes(), [])

    def test_published_legacy_has_zero_image_or_release_writes(self):
        for tag in ("v0.1.1", "v0.1.2"):
            for immutable in (False, True):
                with self.subTest(tag=tag, immutable=immutable):
                    os.environ["RELEASE_TAG"] = tag
                    self.remote_tag_name = tag
                    self.row.update(tag_name=tag, draft=False, immutable=immutable, published_at="2026-11-05")
                    self.commands.clear()
                    with self.assertRaisesRegex(r.ReleaseError, "legacy"):
                        self.fresh()
                    self.assertEqual(self.mutations(), [])
                    self.assertEqual(self.paths, [])
                    self.assertFalse(any(c[0] == "docker" for c, *_ in self.commands))


    def test_missing_image_cannot_build_for_published_or_retained_evidence(self):
        self.accepted()
        self.release.retain()
        for published in (False, True):
            if published:
                self.row.update(draft=False, published_at="2026-11-05")
            self.commands.clear()
            self.fresh()
            self.replies = visible()
            with self.assertRaisesRegex(r.ReleaseError, "no build authorized"):
                self.release.publish()
            self.assertFalse(any(c[:2] == ("docker", "build") for c, *_ in self.commands))
            self.assertEqual(self.pushes(), [])

    def test_acceptance_and_evidence_are_required(self):
        for phase in ("retain", "finalize"):
            with self.assertRaises(r.ReleaseError):
                getattr(self.release, phase)()
        self.accepted()
        with self.assertRaises(r.ReleaseError):
            self.final()
        self.assertEqual(self.mutations(), [])
        self.assertTrue(self.row["draft"])

    def test_external_publication_stops_before_push_or_asset_upload(self):
        self.row.update(draft=False, published_at="2026-11-05")
        with self.assertRaises(r.ReleaseError):
            self.release.publish()
        self.assertEqual(self.pushes(), [])
        self.assertEqual(self.mutations(), [])

    def test_api_failure_at_each_read_and_write_leaves_draft(self):
        for point in ("POST", "download", "PATCH"):
            with self.subTest(point=point):
                self.setUp()
                self.accepted()
                self.fail_api = lambda m, p: (m == point or (point == "download" and "/assets/51" in p))
                with self.assertRaises(r.ReleaseError):
                    self.release.retain()
                    self.final()
                self.assertTrue(self.row["draft"])
                self.assertNotEqual(self.release.state.get("publication"), "published")

    def test_documented_workflow_update_authority_denial_preserves_everything(self):
        self.accepted()
        self.release.retain()
        retained = self.assets[0][1]
        for status in (403, 404):
            def deny(method, path, body=None, host="api.github.com", binary=False):
                if method == "PATCH":
                    return status, b'{"message":"denied"}', None, None
                return self.request(method, path, body, host, binary)
            with patch.object(r.Releases, "request", side_effect=deny), self.assertRaisesRegex(
                    r.ReleaseError, f"HTTP {status}"):
                self.final()
            self.assertEqual(self.release.state["publication"], "outcome_uncertain")
            self.assertTrue(self.row["draft"])
            self.assertEqual(self.assets[0][1], retained)
            self.assertEqual(self.row["target_commitish"], SOURCE_SHA)
            self.fresh()
            self.accepted()
            self.release.retain()
        self.assertEqual([m for m, _ in self.mutations()], ["POST"])

    def test_asset_conflicts_are_never_replaced(self):
        self.accepted()
        self.release.retain()
        original = copy.deepcopy(self.assets)
        for field, value in (("state", "starter"), ("digest", OTHER), ("size", 0),
                             ("content_type", "text/plain"), ("size", r.MAX_EVIDENCE + 1)):
            self.assets = copy.deepcopy(original)
            self.assets[0][0][field] = value
            with self.subTest(field=field), self.assertRaises(r.ReleaseError):
                self.release.retain()
        self.assets = original * 2
        with self.assertRaises(r.ReleaseError):
            self.release.retain()
        self.assertEqual([m for m, _ in self.mutations()], ["POST"])

    def test_changed_asset_bytes_or_id_stop_finalization(self):
        self.accepted()
        self.release.retain()
        row, body = self.assets[0]
        for changed in ((dict(row, id=52), body), (dict(row, size=len(body)+1,
                        digest=r.content_digest(body+b' ')), body+b' ')):
            self.assets = [changed]
            with self.assertRaises(r.ReleaseError):
                self.final()
            self.assertTrue(self.row["draft"])

    def test_evidence_schema_identity_and_signer_conflicts(self):
        self.accepted()
        self.release.retain()
        doc = r.read_json(self.assets[0][1])
        for field, value in (("package_id", 99), ("digest", OTHER), ("source_sha", "d"*40),
                             ("original_workflow_sha", OLD_WORKFLOW), ("workflow_sha", "main"),
                             ("run_id", "0"), ("run_attempt", ""), ("creation_eligible", "true"),
                             ("platform", "linux/arm64")):
            changed = copy.deepcopy(doc)
            changed["snapshot"][field] = value
            with self.subTest(field=field), self.assertRaises(r.ReleaseError):
                r.validate_evidence(json.dumps(changed).encode(), self.release.state)
        for field, value in (("release_id", 42), ("schema_version", True), ("acceptance", "incomplete"),
                             ("runtime", "failed"), ("repository", "other"), ("identity", [])):
            changed = dict(doc, **{field: value})
            with self.subTest(field=field), self.assertRaises(r.ReleaseError):
                r.validate_evidence(json.dumps(changed).encode(), self.release.state)

    def test_release_missing_duplicate_wrong_id_source_and_flags_stop_resolution(self):
        for rows in ([], [self.row, self.row], [dict(self.row, id=True)], [dict(self.row, target_commitish="main")],
                     [dict(self.row, prerelease=True)], [dict(self.row, draft="true")],
                     [dict(self.row, immutable=True)], [dict(self.row, published_at="already")]):
            with self.subTest(rows=rows), patch.object(r.Releases, "listing", return_value=rows), self.assertRaises(r.ReleaseError):
                self.release.resolve()
        os.environ.update(GITHUB_EVENT_NAME="push", RELEASE_CREATED="true", RELEASE_DRAFT="true",
                          RELEASE_ID="42", RELEASE_VERSION="0.1.2", RELEASE_PLEASE_RESULT="success")
        with self.assertRaises(r.ReleaseError):
            self.release.resolve()
        self.assertEqual(self.pushes(), [])
        self.assertEqual(self.mutations(), [])

    def test_pinned_draft_asset_cannot_disappear_or_be_repinned(self):
        self.accepted()
        self.release.retain()
        original = copy.deepcopy(self.assets)
        self.fresh()
        self.accepted()
        for changed in ([], [(dict(original[0][0], id=52), original[0][1])],
                        [(dict(original[0][0], size=len(original[0][1])+1,
                               digest=r.content_digest(original[0][1]+b' ')), original[0][1]+b' ')]):
            self.assets = changed
            with self.assertRaises(r.ReleaseError):
                self.release.retain()
        self.assertEqual([m for m, _ in self.mutations()], ["POST"])
        self.assertTrue(self.row["draft"])

    def test_upload_response_loss_without_asset_never_blindly_retries(self):
        self.accepted()
        self.fail_api = lambda m, p: m == "POST"
        with self.assertRaises(r.ReleaseError):
            self.release.retain()
        self.fail_api = None
        with self.assertRaisesRegex(r.ReleaseError, "outcome uncertain"):
            self.release.retain()
        self.assertEqual([m for m, _ in self.mutations()], ["POST"])
        self.fresh()
        self.accepted()
        self.release.retain()
        self.final()


class ReleaseApiTests(unittest.TestCase):
    def test_authenticated_fixed_origin_bounded_body_and_no_redirect_mutations(self):
        api = r.Releases("test-only")
        with patch.object(r.http.client, "HTTPSConnection") as connection:
            response = connection.return_value.getresponse.return_value
            response.status, response.read.return_value = 302, b'{}'
            response.getheader.return_value = None
            with self.assertRaises(r.ReleaseError):
                api.json("PATCH", r.RELEASES + "/41", b'{"draft":false}')
            self.assertEqual(connection.call_args.args, ("api.github.com",))
            self.assertEqual(connection.return_value.request.call_args.kwargs["headers"]["Authorization"], "Bearer test-only")
            response.status, response.read.return_value = 200, b'x'*(r.MAX_EVIDENCE+1)
            with self.assertRaises(r.ReleaseError):
                api.release(41)
            with self.assertRaises(r.ReleaseError):
                api.request("GET", r.RELEASES, host="evil.example")

    def test_binary_200_and_302_redirect_without_credentials(self):
        api = r.Releases("test-only")
        with patch.object(api, "request", return_value=(200, b'asset', None, None)):
            self.assertEqual(api.asset(51), b'asset')
        url = "https://release-assets.githubusercontent.com/path?signature=test"
        with patch.object(api, "request", return_value=(302, b'', None, url)), patch.object(
                r.http.client, "HTTPSConnection") as connection:
            response = connection.return_value.getresponse.return_value
            response.status, response.read.return_value = 200, b'asset'
            self.assertEqual(api.asset(51), b'asset')
            headers = connection.return_value.request.call_args.kwargs["headers"]
            self.assertNotIn("Authorization", headers)
            connection.assert_called_once_with("release-assets.githubusercontent.com", timeout=60)
        for url in ("http://release-assets.githubusercontent.com/a", "https://evil.example/a",
                    "https://api.github.com/a", "https://user@release-assets.githubusercontent.com/a",
                    "https://release-assets.githubusercontent.com:443/a", ""):
            with self.subTest(url=url), patch.object(api, "request", return_value=(302, b'', None, url)), self.assertRaises(r.ReleaseError):
                api.asset(51)
        for status in (301, 403, 404, 500):
            with patch.object(api, "request", return_value=(status, b'', None, None)), self.assertRaises(r.ReleaseError):
                api.asset(51)

    def test_release_and_asset_pagination_is_complete_and_fixed_origin(self):
        api = r.Releases("test-only")
        for route in (r.RELEASES, r.RELEASES + "/41/assets"):
            rows = [dict(id=i+1) for i in range(100)]
            link = f'<https://api.github.com{route}?per_page=100&page=2>; rel="next"'
            with patch.object(api, "json", side_effect=[(rows, link), ([dict(id=101)], None)]) as request:
                self.assertEqual(len(api.listing(route)), 101)
                self.assertEqual(request.call_args.args, ("GET", route + "?per_page=100&page=2"))
            for bad in (link.replace('api.github.com', 'evil.example'), link.replace('page=2', 'page=3'),
                        link + ',' + link, '', link.replace('next', 'last'),
                        link.replace('per_page=100', 'per_page=99')):
                with self.subTest(bad=bad), patch.object(api, "json", return_value=(rows, bad)), self.assertRaises(r.ReleaseError):
                    api.listing(route)
            for badrows in ({}, [dict(id=True)], [dict(id=1), dict(id=1)], [dict(id=0)]):
                with patch.object(api, "json", return_value=(badrows, None)), self.assertRaises(r.ReleaseError):
                    api.listing(route)

    def test_release_http_errors_and_malformed_json_never_mean_absence(self):
        api = r.Releases("test-only")
        for status in (301, 401, 403, 404, 429, 500, 503):
            with patch.object(api, "request", return_value=(status, b'{}', None, None)), self.assertRaises(r.ReleaseError):
                api.listing(r.RELEASES)
        for body in (b'not json', b'{"id":1,"id":2}', b'{"id":NaN}'):
            with patch.object(api, "request", return_value=(200, body, None, None)), self.assertRaises(r.ReleaseError):
                api.listing(r.RELEASES)
