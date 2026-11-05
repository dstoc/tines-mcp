"""Synthetic original evidence and admission fixtures, never live incident bytes."""
import copy
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import sys
import unittest
import tempfile
from unittest.mock import patch
import warnings
import zipfile

import test_release as baseline
from test_release import r, environment, visible

# The offline module shares strict JSON and errors with the executable helper.
with patch.dict(sys.modules, release=r):
    spec = importlib.util.spec_from_file_location("release_incident", Path(r.__file__).with_name("release_incident.py"))
    incident = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(incident)


def original_record():
    return dict(tag="v0.1.2", version="0.1.2", source_sha=incident.SOURCE, workflow_sha=incident.SOURCE,
                release_id=incident.RELEASE, run_id=str(incident.RUN), run_attempt="1",
                run_url=f"https://github.com/{incident.REPO}/actions/runs/{incident.RUN}",
                push_attempted=False, stage="no_push_attempted", validation="incomplete",
                operation="registry_authority", creation_eligible=True, release_draft=True,
                package_responses=dict(initial_authority=dict(
                    schema_version=1, route="package_metadata", http_status=404, body_bytes_observed=155,
                    json_result="parsed", json_type="object", link_state="absent", json_object=True,
                    message_exact=False, documentation_string=True, documentation_prefix=True, link_absent=True)))


def archive(record, extra=()):
    stream = io.BytesIO()
    with warnings.catch_warnings(), zipfile.ZipFile(stream, "w", zipfile.ZIP_DEFLATED) as zipped:
        warnings.simplefilter("ignore", UserWarning)
        zipped.writestr("record.json", json.dumps(record))
        for name, body in extra:
            zipped.writestr(name, body)
    return stream.getvalue()


class IncidentEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.run = dict(id=incident.RUN, workflow_id=incident.WORKFLOW, run_number=18, run_attempt=1,
                        head_sha=incident.SOURCE, event="push", head_branch="main", status="completed",
                        conclusion="failure", repository=dict(full_name=incident.REPO))
        self.job = dict(id=incident.JOB, run_id=incident.RUN, run_attempt=1, head_sha=incident.SOURCE,
                        status="completed", conclusion="failure")
        self.metadata = dict(id=incident.ARTIFACT, name="adapter-release-diagnostics-42010011054-1",
                             expired=False, size_in_bytes=819, digest=incident.ARCHIVE_DIGEST,
                             workflow_run=dict(id=incident.RUN, head_sha=incident.SOURCE, head_branch="main",
                                               repository_id=1435671871, head_repository_id=1435671871))

    def check(self, body, *, synthetic=False, attempts=None):
        if synthetic:
            # Replace byte pins ONLY in this offline fixture. No production input
            # exposes them; this is not an assertion about the denied incident ZIP.
            digest = "sha256:" + hashlib.sha256(body).hexdigest()
            self.metadata.update(digest=digest, size_in_bytes=len(body))
            with patch.object(incident, "ARCHIVE_SIZE", len(body)), patch.object(incident, "ARCHIVE_DIGEST", digest):
                return incident.validate_original(self.run, [self.run] if attempts is None else attempts,
                                                  self.job, self.metadata, body)
        return incident.validate_original(self.run, [self.run], self.job, self.metadata, body)

    def test_original_pins_reject_synthetic_bytes(self):
        with self.assertRaisesRegex(r.ReleaseError, "bytes unavailable or changed"):
            self.check(archive(original_record()))

    def test_archive_and_metadata_byte_pins_cannot_be_replaced_or_omitted(self):
        body = archive(original_record())
        digest = "sha256:" + hashlib.sha256(body).hexdigest()
        self.metadata.update(size_in_bytes=len(body), digest=digest)
        with patch.object(incident, "ARCHIVE_SIZE", len(body)), patch.object(incident, "ARCHIVE_DIGEST", digest):
            for changed in (None, b'', body + b'changed', body[:-1] + b'x'):
                with self.assertRaises(r.ReleaseError):
                    self.check(changed)
            for key, value in (("size_in_bytes", len(body) + 1), ("digest", "sha256:" + "0" * 64)):
                original = self.metadata[key]
                self.metadata[key] = value
                with self.assertRaises(r.ReleaseError):
                    self.check(body)
                self.metadata[key] = original
            for key in self.metadata["workflow_run"]:
                original = self.metadata["workflow_run"][key]
                self.metadata["workflow_run"][key] = None
                with self.assertRaises(r.ReleaseError):
                    self.check(body)
                self.metadata["workflow_run"][key] = original

    def test_synthetic_no_push_and_legacy_snapshot_are_readable(self):
        record = original_record()
        observed = self.check(archive(record), synthetic=True)
        self.assertEqual(set(observed), {"incident_run_id", "artifact_id", "archive_digest", "release_id", "source_sha"})
        self.assertEqual(observed["source_sha"], incident.SOURCE)
        for flags in ((True, True), (True, False), (False, False)):
            record["package_responses"]["initial_authority"].update(schema_version=2,
                                                                  message_string=flags[0], message_nonempty=flags[1])
            self.check(archive(record), synthetic=True)

    def test_missing_or_contradictory_no_push_evidence_fails(self):
        fields = dict(push_attempted=True, stage="digest_observed", digest="sha256:" + "1" * 64,
                      pinned_image="selected", validation="passed", operation="postpush_registry_check",
                      run_attempt="2", run_id="1", release_id=1, source_sha="a" * 40,
                      workflow_sha="a" * 40, tag="v0.1.1", version="0.1.3", creation_eligible=False,
                      publication_attempted=True, evidence_upload_attempted=True, evidence_retained=True,
                      release_draft=False, original_workflow_sha="a" * 40)
        for field, value in fields.items():
            for missing in (True, False):
                record = original_record()
                if missing:
                    record.pop(field, None)
                else:
                    record[field] = value
                # Optional evidence absence is legitimate; its positive presence is not.
                if missing and field in ("digest", "pinned_image", "original_workflow_sha", "publication_attempted",
                                          "evidence_upload_attempted", "evidence_retained"):
                    continue
                with self.subTest(field=field, missing=missing), self.assertRaises(r.ReleaseError):
                    self.check(archive(record), synthetic=True)
        for snapshots in ({}, None, dict(original_record()["package_responses"], prepush={})):
            record = dict(original_record(), package_responses=snapshots)
            with self.assertRaises(r.ReleaseError):
                self.check(archive(record), synthetic=True)

    def test_unknown_or_contradictory_snapshot_fails(self):
        baseline = original_record()
        flags = baseline["package_responses"]["initial_authority"]
        for field, value in dict(schema_version=3, message_exact=True, documentation_prefix=False,
                                 link_absent=False, http_status=403, json_object=False, body_bytes_observed=0,
                                 unknown="SECRET_RESPONSE_MARKER").items():
            record = copy.deepcopy(baseline)
            record["package_responses"]["initial_authority"][field] = value
            with self.subTest(field=field), self.assertRaises(r.ReleaseError):
                self.check(archive(record), synthetic=True)
        for field in flags:
            record = copy.deepcopy(baseline)
            del record["package_responses"]["initial_authority"][field]
            with self.subTest(missing=field), self.assertRaises(r.ReleaseError):
                self.check(archive(record), synthetic=True)

    def test_zip_limits_and_unsafe_entries(self):
        for extras in ([('record.json', '{}')], [('../escape', 'x')], [('/absolute', 'x')],
                       [('a\\b', 'x')], [('a/./b', 'x')], [('record.tmp', 'x' * incident.MAX_RECORD)],
                       [(f'{i}', 'x') for i in range(9)]):
            with self.subTest(entries=[x[0] for x in extras]), self.assertRaises(r.ReleaseError):
                self.check(archive(original_record(), extras), synthetic=True)
        with self.assertRaises(r.ReleaseError):
            self.check(b'not a ZIP', synthetic=True)
        with self.assertRaises(r.ReleaseError):
            self.check(archive('not a record'), synthetic=True)

    def test_original_run_job_artifact_identity_and_later_attempts(self):
        body = archive(original_record())
        for name, changes in (
            ('run', dict(id=1, workflow_id=1, run_number=19, run_attempt=2, head_sha='a' * 40,
                         event='workflow_dispatch', head_branch='other', status='in_progress', repository={})),
            ('job', dict(id=1, run_id=1, run_attempt=True, head_sha='a' * 40, conclusion='success')),
            ('metadata', dict(id=1, name='other', expired=True, workflow_run={}))):
            original = copy.deepcopy(getattr(self, name))
            for key, value in changes.items():
                with self.subTest(name=name, key=key), self.assertRaises(r.ReleaseError):
                    setattr(self, name, dict(original, **{key: value}))
                    self.check(body, synthetic=True)
            setattr(self, name, original)
        for attempts in ([], [self.run, dict(self.run, run_attempt=2)], [dict(self.run, run_attempt=2)]):
            with self.assertRaises(r.ReleaseError):
                self.check(body, synthetic=True, attempts=attempts)


class OpportunityTests(unittest.TestCase):
    def setUp(self):
        self.env = environment(RELEASE_SOURCE=incident.SOURCE, RELEASE_INCIDENT=str(incident.RUN),
                               GITHUB_RUN_NUMBER='23')
        self.current = dict(id=123, workflow_id=incident.WORKFLOW, run_number=23, run_attempt=1,
                            event='workflow_dispatch', head_branch='main', head_sha=self.env['GITHUB_SHA'],
                            repository=dict(full_name=incident.REPO))
        self.release = dict(id=incident.RELEASE, tag_name='v0.1.2', target_commitish=incident.SOURCE,
                            draft=True, prerelease=False, immutable=False, published_at=None, assets=[])

    def check(self):
        return incident.validate_opportunity(self.env, self.current, self.release)

    def test_production_has_no_run_pin_even_with_all_coordinates(self):
        self.assertIsNone(incident.REVIEWED_RUN_NUMBER)
        with self.assertRaisesRegex(r.ReleaseError, 'No reviewed'):
            self.check()
        # Environment cannot override the reviewed constant.
        self.env['REVIEWED_RUN_NUMBER'] = '23'
        with self.assertRaises(r.ReleaseError):
            self.check()

    def test_exact_fixture_selection_and_rejections(self):
        with patch.object(incident, 'REVIEWED_RUN_NUMBER', 23):
            self.assertIsNone(self.check())  # No eligibility or authorization is returned.
            for name, changes in (
                ('env', dict(GITHUB_EVENT_NAME='push', GITHUB_REPOSITORY='other/repo',
                             GITHUB_REF='refs/heads/other', GITHUB_RUN_ATTEMPT='2', GITHUB_RUN_NUMBER='24',
                             GITHUB_RUN_ID=str(incident.RUN), RELEASE_SOURCE='a' * 40, RELEASE_TAG='v0.1.3',
                             RELEASE_INCIDENT='other', GITHUB_SHA='main')),
                ('current', dict(id=124, workflow_id=1, run_number=24, run_attempt=2, event='push',
                                 head_branch='other', head_sha='a' * 40, repository={})),
                ('release', dict(id=1, tag_name='v0.1.1', target_commitish='a' * 40, draft=False,
                                 prerelease=True, immutable=True, published_at='today', assets=[{}]))):
                original = copy.deepcopy(getattr(self, name))
                for field, value in changes.items():
                    for missing in (False, True):
                        altered = dict(original, **{field: value})
                        if missing:
                            altered.pop(field)
                        setattr(self, name, altered)
                        with self.subTest(name=name, field=field, missing=missing), self.assertRaises(r.ReleaseError):
                            self.check()
                setattr(self, name, original)

    def test_workflow_tree_authority_and_stale_or_denied_comparisons(self):
        tree = {'.github/workflows/release.yml': 'a' * 40, '.github/workflows/test.yml': 'b' * 40}
        def compare(fixed=tree, reviewed=tree, default=tree, **changes):
            args = dict(workflow_sha='b' * 40, default_sha='c' * 40, current_default_sha='c' * 40)
            observations = [dict(commit=commit, files=files) if files is not None else None
                            for commit, files in ((incident.SOURCE, fixed), ('b' * 40, reviewed), ('c' * 40, default))]
            return incident.validate_workflow_comparison(*observations, **dict(args, **changes))
        compare()
        for changed in (dict(tree, **{'.github/workflows/release.yml': 'c' * 40}),
                        {'.github/workflows/release.yml': 'a' * 40},
                        dict(tree, **{'.github/workflows/added.yml': 'c' * 40}), {}, None):
            for which in ('fixed', 'reviewed', 'default'):
                with self.subTest(which=which, changed=changed), self.assertRaises(r.ReleaseError):
                    compare(**{which: changed})
        with self.assertRaisesRegex(r.ReleaseError, 'Stale'):
            compare(current_default_sha='d' * 40)
        with self.assertRaisesRegex(r.ReleaseError, 'commit mismatch'):
            compare(workflow_sha='d' * 40)


class ProductionDenialTests(unittest.TestCase):
    def test_incident_denial_is_before_any_io_regardless_of_read_authority(self):
        # Simulated artifact/run/attempt/tree 401/403/404/410 responses are never
        # requested while recovery is disabled; no alternate download can occur.
        for resource in ('artifact', 'run', 'attempt', 'workflow_tree'):
            for status in (401, 403, 404, 410):
                with self.subTest(resource=resource, status=status), tempfile.TemporaryDirectory() as temp, patch.dict(
                        'os.environ', environment(RELEASE_INCIDENT=str(incident.RUN)), clear=True), \
                        patch.object(r.Releases, 'request', side_effect=r.ReleaseError(f'HTTP {status}')) as api, \
                        patch.object(r.Registry, 'get') as registry, patch.object(r, 'run') as commands:
                    with self.assertRaisesRegex(r.ReleaseError, 'recovery is disabled'):
                        r.Release(temp, temp, Path(temp) / 'record.json').resolve()
                    api.assert_not_called()
                    registry.assert_not_called()
                    commands.assert_not_called()


class IncidentBuildDenialTests(unittest.TestCase):
    setUp = baseline.OrchestrationTests.setUp
    command = baseline.OrchestrationTests.command
    http = baseline.OrchestrationTests.http
    def test_exact_incident_visible_namespace_missing_version_cannot_build(self):
        self.release.state.update(source_sha=incident.SOURCE, release_id=incident.RELEASE)
        self.commands.clear()
        # Bypass only unrelated source fixtures so the new build guard is reached.
        self.replies = visible()
        with patch.object(r, 'dispatch', return_value={}), patch.object(self.release, 'check_release'), \
                patch.object(r, 'source_version'), patch.object(r, 'resolve'), \
                self.assertRaisesRegex(r.ReleaseError, 'Exact incident image creation is disabled'):
            self.release.publish()
        self.assertFalse(any(c[:2] in (('docker', 'build'), ('docker', 'push')) for c, *_ in self.commands))
