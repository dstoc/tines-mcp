"""Fixed diagnostics observe the real HTTP reader without disclosing response data."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from test_release import r, package, unavailable, version

FIELDS = {
    "schema_version", "route", "http_status", "body_bytes_observed", "json_result", "json_type",
    "link_state", "json_object", "message_exact", "documentation_string", "documentation_prefix", "link_absent",
}
MARKER = "SECRET_RESPONSE_MARKER"
HOSTILE = MARKER + '\x1b[31m\r\n::error::unsafe'


def snapshot(**values):
    result = dict(schema_version=1, route="package_metadata", http_status=404,
                  body_bytes_observed=None, json_result="parsed", json_type="object", link_state="absent",
                  json_object=True, message_exact=True, documentation_string=True,
                  documentation_prefix=True, link_absent=True)
    result.update(values)
    return result


def assert_safe_snapshot(test, value):
    test.assertEqual(set(value), FIELDS)
    test.assertEqual(type(value["schema_version"]), int)
    test.assertEqual(value["schema_version"], 1)
    test.assertEqual(value["route"], "package_metadata")
    for key in ("json_object", "message_exact", "documentation_string", "documentation_prefix", "link_absent"):
        test.assertTrue(value[key] is None or type(value[key]) is bool)
    for key, upper in (("http_status", 999), ("body_bytes_observed", r.PACKAGE_BODY_LIMIT + 1)):
        test.assertTrue(value[key] is None or type(value[key]) is int and 0 <= value[key] <= upper)
    test.assertIn(value["json_result"], {"not_read", "too_large", "invalid", "duplicate_field", "invalid_constant", "parsed"})
    test.assertIn(value["json_type"], {"unknown", "object", "array", "string", "number", "boolean", "null"})
    test.assertIn(value["link_state"], {"unknown", "absent", "empty", "present"})


class PackageResponseTests(unittest.TestCase):
    def observe(self, body, status=404, link=None, eligible=True, failure=None):
        observations, paths = [], []
        with patch.object(r.http.client, "HTTPSConnection") as connection:
            response = connection.return_value.getresponse.return_value
            response.status, response.read.return_value = status, body
            response.getheader.return_value = link
            if failure == "request":
                connection.return_value.request.side_effect = OSError(HOSTILE)
            elif failure == "headers":
                connection.return_value.getresponse.side_effect = r.http.client.HTTPException(HOSTILE)
            elif failure == "close":
                connection.return_value.close.side_effect = r.http.client.HTTPException(HOSTILE)
            elif failure == "body":
                response.read.side_effect = r.http.client.IncompleteRead(b"secret partial " + MARKER.encode())
            registry = r.Registry("TOKEN_" + MARKER, lambda purpose, value: observations.append((purpose, value)))
            result, error = None, None
            try:
                result = registry.lookup("0.1.2", eligible)
            except r.ReleaseError as caught:
                error = str(caught)
            paths = [call.args[1] for call in connection.return_value.request.call_args_list]
            connection.return_value.close.assert_called_once()
            if failure != "request" and failure != "headers":
                response.read.assert_called_once_with(r.PACKAGE_BODY_LIMIT + 1)
            self.assertEqual(paths, [r.PACKAGE])
        self.assertEqual(len(observations), 1)
        purpose, value = observations[0]
        self.assertEqual(purpose, "initial_authority")
        assert_safe_snapshot(self, value)
        self.assertNotIn(MARKER, json.dumps(value) + (error or ""))
        return result, error, value

    def test_each_predicate_is_recorded_without_relaxing_acceptance(self):
        valid = unavailable()[1]
        cases = [(valid, None, {}),
                 ({}, None, dict(message_exact=False, documentation_string=False, documentation_prefix=False)),
                 (dict(valid, message=HOSTILE), None, dict(message_exact=False)),
                 (dict(valid, message=None), None, dict(message_exact=False)),
                 (dict(documentation_url=valid["documentation_url"]), None, dict(message_exact=False)),
                 (dict(message="Not Found"), None, dict(documentation_string=False, documentation_prefix=False)),
                 (dict(valid, documentation_url=[HOSTILE]), None,
                  dict(documentation_string=False, documentation_prefix=False)),
                 (dict(valid, documentation_url=None), None,
                  dict(documentation_string=False, documentation_prefix=False)),
                 (dict(valid, documentation_url=HOSTILE), None, dict(documentation_prefix=False)),
                 (dict(valid, documentation_url="https://docs.github.com/en/rest/packages/packages"),
                  None, dict(documentation_prefix=False)),
                 (dict(valid, documentation_url="https://docs.github.com/rest/" + HOSTILE), None, {}),
                 (dict(valid, status="404", unknown=HOSTILE), None, {}),
                 (valid, "", dict(link_state="empty", link_absent=False)),
                 (valid, HOSTILE, dict(link_state="present", link_absent=False))]
        for data, link, flags in cases:
            with self.subTest(data=data, link=link):
                body = json.dumps(data).encode()
                result, error, observed = self.observe(body, link=link)
                self.assertEqual(observed, snapshot(body_bytes_observed=len(body), **flags))
                if flags:
                    self.assertIsNotNone(error)
                else:
                    self.assertIsNone(error)
                    self.assertEqual(result, (None, None))

    def test_nonobjects_and_all_json_types(self):
        for data, kind in [(None, "null"), ([], "array"), (HOSTILE, "string"),
                           (42, "number"), (1.2, "number"), (True, "boolean")]:
            with self.subTest(kind=kind):
                body = json.dumps(data).encode()
                _, error, observed = self.observe(body)
                self.assertIsNotNone(error)
                self.assertEqual(observed, snapshot(body_bytes_observed=len(body), json_type=kind,
                                                   json_object=False, message_exact=False,
                                                   documentation_string=False, documentation_prefix=False))

    def test_status_is_observed_but_never_converted_to_absence(self):
        for eligible in (False, True):
            for status in (200, 301, 302, 401, 403, 404, 429, 500, 503):
                with self.subTest(eligible=eligible, status=status):
                    body = json.dumps(unavailable()[1]).encode()
                    _, error, observed = self.observe(body, status, eligible=eligible)
                    self.assertEqual(error is None, eligible and status == 404)
                    self.assertEqual(observed["http_status"], status)
                    self.assertTrue(observed["message_exact"])

    def test_parser_failures_keep_header_observations_and_null_predicates(self):
        cases = [(b'not json ' + MARKER.encode(), "invalid"), (b'[' * 10000 + b'0' + b']' * 10000, "invalid"), (b'"\xff"', "invalid"),
                 (b'{"' + MARKER.encode() + b'":1,"' + MARKER.encode() + b'":2}', "duplicate_field"),
                 (b'{"' + MARKER.encode() + b'":NaN}', "invalid_constant"),
                 (b'x' * (r.PACKAGE_BODY_LIMIT + 1), "too_large")]
        for body, classification in cases:
            with self.subTest(classification=classification):
                _, error, observed = self.observe(body, link=HOSTILE)
                self.assertIsNotNone(error)
                self.assertEqual(observed, snapshot(body_bytes_observed=len(body), json_result=classification,
                                                   json_type="unknown", link_state="present", link_absent=False,
                                                   json_object=None, message_exact=None,
                                                   documentation_string=None, documentation_prefix=None))

    def test_transport_failures_keep_only_available_observations(self):
        for failure in ("request", "headers", "body"):
            with self.subTest(failure=failure):
                _, error, observed = self.observe(b'{}', link="", failure=failure)
                self.assertEqual(error, "Package API transport failure; no absence established")
                headers = failure == "body"
                self.assertEqual(observed, snapshot(
                    http_status=404 if headers else None, json_result="not_read", json_type="unknown",
                    link_state="empty" if headers else "unknown", link_absent=False if headers else None,
                    json_object=None, message_exact=None, documentation_string=None, documentation_prefix=None))

    def test_connection_creation_and_close_failures_do_not_disclose_exceptions(self):
        with patch.object(r.http.client, "HTTPSConnection", side_effect=OSError(HOSTILE)):
            seen = []
            with self.assertRaisesRegex(r.ReleaseError, "transport failure"):
                r.Registry("test", lambda purpose, value: seen.append(value)).lookup("0.1.2", True)
            self.assertIsNone(seen[0]["http_status"])
            self.assertEqual(seen[0]["link_state"], "unknown")
            self.assertEqual(seen[0]["json_result"], "not_read")
            self.assertNotIn(MARKER, json.dumps(seen))
        _, error, observed = self.observe(json.dumps(unavailable()[1]).encode(), failure="close")
        self.assertEqual(error, "Package API transport failure; no absence established")
        self.assertEqual(observed["json_result"], "parsed")

    def test_purposes_are_bounded_and_version_pages_do_not_replace_metadata(self):
        with tempfile.TemporaryDirectory() as temp, patch.object(r.http.client, "HTTPSConnection") as conn:
            release = r.Release(temp, temp, Path(temp) / "record.json")
            response = conn.return_value.getresponse.return_value
            response.status, response.getheader.return_value = 200, None
            for purpose in r.METADATA_PURPOSES:
                response.read.side_effect = [json.dumps(package()).encode(), json.dumps([version(tags=["0.1.2"])]).encode()]
                self.assertEqual(r.Registry("test", release.observe_package).lookup("0.1.2", purpose=purpose), (17, version()["name"]))
            self.assertEqual(set(release.state["package_responses"]), r.METADATA_PURPOSES)
            for observed in release.state["package_responses"].values():
                assert_safe_snapshot(self, observed)
                self.assertEqual(observed["json_type"], "object")
                self.assertEqual(observed["body_bytes_observed"], len(json.dumps(package()).encode()))
            response.status, response.read.side_effect, response.read.return_value = 404, None, b'{}'
            with self.assertRaises(r.ReleaseError):
                r.Registry("test", release.observe_package).lookup("0.1.2", purpose="prepush")
            self.assertFalse(release.state["package_responses"]["prepush"]["message_exact"])
            count = conn.return_value.request.call_count
            for invalid in ("arbitrary_" + HOSTILE, "versions", ""):
                with self.assertRaises(r.ReleaseError):
                    r.Registry("test", release.observe_package).lookup("0.1.2", purpose=invalid)
            self.assertEqual(conn.return_value.request.call_count, count)
            self.assertLess(len(json.dumps(release.state["package_responses"])), 4096)


class DiagnosticCliTests(unittest.TestCase):
    def test_main_and_separate_report_retain_only_safe_evidence(self):
        fixture = Path(__file__).parent / "fixtures/diagnostic_cli.py"
        cases = [(phase, "predicate") for phase in (
            "initial_authority", "prepush", "postpush", "sink_failure", "postpush_sink_failure",
            "persistent_sink_failure", "postpush_persistent_sink_failure", "push_failure")]
        cases += [("initial_authority", variant) for variant in (
            "invalid", "duplicate_field", "invalid_constant", "too_large", "body_failure", "request_failure")]
        cases.append(("postpush", "invalid"))
        for phase, variant in cases:
            with self.subTest(phase=phase, variant=variant), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                result = subprocess.run([sys.executable, str(fixture), str(root), phase, variant],
                                        text=True, capture_output=True, check=False)
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                record_path = root / "release/record.json"
                before = json.loads(record_path.read_text())
                pushed = phase.startswith("postpush") or phase == "push_failure"
                selected = phase.startswith("postpush")
                report_env = dict(os.environ, RELEASE_JOB_STATUS="failure", GITHUB_STEP_SUMMARY=str(root / "summary"))
                report = subprocess.run([sys.executable, str(r.__file__), "report", "--workflow", temp,
                                         "--source", temp, "--record", str(record_path)],
                                        env=report_env, text=True, capture_output=True, check=False)
                if pushed:
                    # A failing report sink must retain selected digest/uncertainty,
                    # suppress raw filesystem errors, and claim no retention success.
                    bad_summary = root / MARKER
                    bad_summary.mkdir()
                    failed_report = subprocess.run(
                        [sys.executable, str(r.__file__), "report", "--workflow", temp,
                         "--source", temp, "--record", str(record_path)],
                        env=dict(report_env, GITHUB_STEP_SUMMARY=str(bad_summary)),
                        text=True, capture_output=True, check=False)
                    self.assertEqual(failed_report.returncode, 1)
                    self.assertNotIn(MARKER, failed_report.stdout + failed_report.stderr)
                    self.assertEqual(json.loads(record_path.read_text()).get("digest"), version()["name"] if selected else None)
                self.assertEqual(report.returncode, 0, report.stderr)
                record = json.loads(record_path.read_text())
                self.assertEqual(record["validation"], "incomplete")
                self.assertEqual(record.get("package_responses", {}), before.get("package_responses", {}))
                self.assertLessEqual(set(record.get("package_responses", {})), r.METADATA_PURPOSES)
                for observed in record.get("package_responses", {}).values():
                    assert_safe_snapshot(self, observed)
                self.assertEqual(record["push_attempted"], pushed)
                self.assertEqual(record.get("digest"), version()["name"] if selected else None)
                self.assertEqual(record["stage"], "digest_observed" if selected else "push_attempted_outcome_uncertain" if pushed else "no_push_attempted")
                commands = json.loads((root / "commands.json").read_text())
                self.assertEqual(sum(c[:2] == ["docker", "push"] for c in commands), int(pushed))
                paths = json.loads((root / "paths.json").read_text())
                self.assertEqual(paths, [r.PACKAGE] * (3 if selected else 2 if phase in ("prepush", "push_failure") else 1))
                if "sink_failure" not in phase and phase != "push_failure":
                    observed = record["package_responses"][phase]
                    if variant == "predicate":
                        self.assertFalse(observed["message_exact"])
                        self.assertFalse(observed["documentation_prefix"])
                    else:
                        self.assertIsNone(observed["message_exact"])
                        self.assertIsNone(observed["documentation_prefix"])
                        self.assertEqual(observed["json_result"], "not_read" if variant.endswith("failure") else variant)
                    self.assertEqual(observed["link_state"], "unknown" if variant == "request_failure" else "present")
                # The actual workflow uploads release/, including any temporary file
                # left by atomic-write failure. Inspect every byte, not just record.json.
                emitted = result.stdout + result.stderr + report.stdout + report.stderr
                for path in (root / "release").rglob("*"):
                    if path.is_file():
                        emitted += path.read_text()
                        if path.name in ("record.json", "record.tmp"):
                            retained = json.loads(path.read_text()).get("package_responses", {})
                            self.assertLessEqual(set(retained), r.METADATA_PURPOSES)
                            for observed in retained.values():
                                assert_safe_snapshot(self, observed)
                emitted += (root / "summary").read_text()
                self.assertNotIn(MARKER, emitted)
                self.assertNotIn("::error::unsafe", emitted)
                self.assertNotIn("retained successfully", emitted)
                self.assertIn("Positive-access investigation only" if pushed else "Inspect failure", record["next_action"])
