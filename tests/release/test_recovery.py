"""Called exact-draft recovery with synthetic API/engine fixtures, never incident archive bytes."""
import copy
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import test_release as baseline
from test_release import r, environment, visible, version, unavailable, WORKFLOW_SHA, DIGEST, OTHER
from test_draft_release import release_row


class RecoveryTests(unittest.TestCase):
    pushes = baseline.OrchestrationTests.pushes
    http = baseline.OrchestrationTests.http

    def setUp(self):
        self.doCleanups()
        if hasattr(self, "default_sha"):
            del self.default_sha
        baseline.OrchestrationTests.setUp(self)
        self.patch_releases.stop()
        os.environ['RELEASE_SOURCE'] = r.INCIDENT_SOURCE
        self.head = self.remote = r.INCIDENT_SOURCE
        self.row = release_row(id=r.INCIDENT_RELEASE, target_commitish=r.INCIDENT_SOURCE)
        self.assets, self.events, self.timeline = [], [], []
        self.api_fault = None
        self.default_sha = WORKFLOW_SHA
        self.tree = '100644 blob ' + 'e' * 40 + '\t.github/workflows/release.yml\0'
        self.trees = {}
        self.patch_api = patch.object(r.Releases, 'request', side_effect=self.request)
        self.patch_api.start()
        self.addCleanup(self.patch_api.stop)
        self.fresh()
        self.replies = [unavailable(), unavailable()] + visible([version(tags=['0.1.2'])])

    def command(self, *args, **kwargs):
        if hasattr(self, 'timeline'):
            self.timeline.append(('command', args))
        # Base fixture calls this during setup before incident selection.
        if hasattr(self, 'default_sha') and (args[:3] in (('git', 'ls-remote', '--symref'),
                ('git', 'rev-parse', '--verify')) or args[:2] == ('git', 'ls-tree')):
            self.commands.append((args, kwargs.get('cwd'), kwargs.get('env'), kwargs.get('input')))
            if self.fail_command and self.fail_command(args):
                raise r.ReleaseError('Injected command failure')
            if args[:3] == ('git', 'ls-remote', '--symref'):
                return f'ref: refs/heads/main\tHEAD\n{self.default_sha}\tHEAD\n{self.default_sha}\trefs/heads/main'
            if args[:2] == ('git', 'ls-tree'):
                return self.trees.get(args[4], self.tree)
            if args[:3] == ('git', 'rev-parse', '--verify'):
                return r.INCIDENT_SOURCE
        return baseline.OrchestrationTests.command(self, *args, **kwargs)

    def request(self, method, path, body=None, host='api.github.com', binary=False):
        self.events.append((method, path))
        self.timeline.append(('api', method, path))
        if self.api_fault:
            result = self.api_fault(method, path, body)
            if result is not None:
                return result
        if method == 'POST':
            self.assertEqual(host, 'uploads.github.com')
            name = path.split('?name=')[1]
            self.assertIn(name, (r.RECOVERY_ASSET, r.EVIDENCE))
            self.assertFalse(any(a['name'] == name for a, _ in self.assets))
            record = r.read_json(self.release.record.read_bytes())
            if name == r.RECOVERY_ASSET:
                self.assertTrue(record['marker_upload_attempted'])
                self.assertFalse(record['push_attempted'])
                self.assertFalse(any(c[:2] == ('docker', 'build') for c, *_ in self.commands))
            else:
                self.assertEqual(record['validation'], 'passed')
                self.assertTrue(record['evidence_upload_attempted'])
            asset = dict(id=51 + len(self.assets), name=name, state='uploaded', size=len(body),
                         digest=r.content_digest(body), content_type='application/json')
            self.assets.append((asset, body))
            return 201, json.dumps(asset).encode(), None, None
        if method == 'PATCH':
            self.assertEqual(path, f'{r.RELEASES}/{r.INCIDENT_RELEASE}')
            self.assertEqual(body, b'{"draft":false}')
            record = r.read_json(self.release.record.read_bytes())
            self.assertTrue(record['publication_attempted'])
            self.assertTrue(record['evidence_retained'])
            self.row.update(draft=False, immutable=True, published_at='2026-11-05')
            return 200, json.dumps(self.row).encode(), None, None
        self.assertEqual(method, 'GET')
        if path == r.RELEASES + '?per_page=100&page=1':
            value = [self.row]
        elif path == f'{r.RELEASES}/{self.row["id"]}':
            value = self.row
        elif path == f'{r.RELEASES}/{self.row["id"]}/assets?per_page=100&page=1':
            value = [a for a, _ in self.assets]
        else:
            for asset, content in self.assets:
                if path == f'{r.RELEASES}/assets/{asset["id"]}':
                    self.assertTrue(binary)
                    return 200, content, None, None
            self.fail(f'Unexpected API read {path}')
        return 200, json.dumps(value).encode(), None, None

    def fresh(self, **env):
        os.environ.update(env)
        self.serial = getattr(self, 'serial', 0) + 1
        self.release = r.Release(self.workflow, self.source, self.base / f'attempt{self.serial}/record.json')
        self.release.resolve()

    def build_calls(self):
        return [c for c, *_ in self.commands if c[:2] == ('docker', 'build')]

    def mutations(self):
        return [event for event in self.events if event[0] != 'GET']

    def accepted(self):
        self.release.publish()
        self.replies += visible([version(tags=['0.1.2'])])
        self.release.accept()

    def final(self):
        self.replies += visible([version(tags=['0.1.2'])])
        self.release.finalize()

    def test_complete_creation_through_both_missing_version_paths(self):
        for accessible in (False, True):
            with self.subTest(accessible=accessible):
                self.setUp()
                if accessible:
                    self.replies = visible() + visible() + visible([version(tags=['0.1.2'])])
                self.accepted()
                pin = copy.deepcopy(self.release.state['recovery'])
                self.release.retain()
                self.final()
                self.assertEqual(len(self.build_calls()), 1)
                self.assertEqual(len(self.pushes()), 1)
                self.assertEqual(self.mutations(), [
                    ('POST', f'{r.RELEASES}/{r.INCIDENT_RELEASE}/assets?name={r.RECOVERY_ASSET}'),
                    ('POST', f'{r.RELEASES}/{r.INCIDENT_RELEASE}/assets?name={r.EVIDENCE}'),
                    ('PATCH', f'{r.RELEASES}/{r.INCIDENT_RELEASE}')])
                self.assertEqual(self.release.state['publication'], 'published')
                self.assertEqual(self.release.state['recovery'], pin)
                self.assertEqual(r.read_json(self.assets[1][1])['snapshot']['recovery'], pin)
                self.assertEqual(r.read_json(self.assets[0][1])['no_push_evidence'], r.NO_PUSH_EVIDENCE)
                events = self.timeline
                marker_read = events.index(('api', 'GET', f'{r.RELEASES}/assets/51'))
                build = next(i for i, e in enumerate(events) if e[0] == 'command' and e[1][:2] == ('docker', 'build'))
                push = next(i for i, e in enumerate(events) if e[0] == 'command' and e[1][:2] == ('docker', 'push'))
                verify = [i for i, e in enumerate(events) if e[0] == 'command' and e[1][:3] == ('gh', 'attestation', 'verify') and e[1][-1] == 'json']
                anonymous = next(i for i, e in enumerate(events) if e[0] == 'command' and e[1][:2] == ('docker', '--config'))
                self.assertLess(marker_read, build)
                self.assertLess(build, push)
                self.assertEqual(len(verify), 2)
                self.assertTrue(all(push < i < anonymous for i in verify))
                # A later read-only published recovery retains both assets byte for byte.
                original = copy.deepcopy(self.assets)
                self.commands.clear()
                self.events.clear()
                self.fresh(GITHUB_RUN_ID='456', GITHUB_RUN_ATTEMPT='2')
                self.replies = visible([version(tags=['0.1.2'])])
                self.accepted()
                self.release.retain()
                self.final()
                self.assertEqual(self.assets, original)
                self.assertEqual(self.build_calls(), [])
                self.assertEqual(self.pushes(), [])
                self.assertEqual(self.mutations(), [])

    def test_fresh_attempt_one_with_marker_never_reopens_either_creation_path(self):
        self.fail_command = lambda c: c[:2] == ('docker', 'build')
        with self.assertRaises(r.ReleaseError):
            self.release.publish()
        original = copy.deepcopy(self.assets)
        self.fail_command = None
        for missing in ([unavailable()], visible()):
            self.commands.clear()
            self.fresh(GITHUB_RUN_ID='456', GITHUB_RUN_ATTEMPT='1')
            self.replies = missing
            with self.assertRaisesRegex(r.ReleaseError, 'consumed'):
                self.release.publish()
            self.assertEqual(self.assets, original)
            self.assertEqual(self.build_calls(), [])
            self.assertEqual(self.pushes(), [])
        self.assertEqual(len(self.mutations()), 1)

    def test_old_handoff_and_rerun_cannot_create_inside_accessible_package(self):
        for env in (dict(GITHUB_RUN_ATTEMPT='2'), dict(GITHUB_EVENT_NAME='push', RELEASE_CREATED='true',
                    RELEASE_VERSION='0.1.2', RELEASE_PLEASE_RESULT='success', RELEASE_ID=str(r.INCIDENT_RELEASE))):
            with self.subTest(env=env):
                self.setUp()
                self.fresh(**env)
                self.replies = visible()
                with self.assertRaisesRegex(r.ReleaseError, 'first-attempt'):
                    self.release.publish()
                self.assertEqual(self.build_calls(), [])
                self.assertEqual(self.mutations(), [])
                self.replies = [unavailable()]
                with self.assertRaises(r.ReleaseError):
                    self.release.publish()

    def test_wrong_draft_and_source_never_acquire_marker(self):
        self.row['id'] = 99
        with self.assertRaises(r.ReleaseError):
            self.fresh()
        self.assertEqual(self.mutations(), [])
        self.assertEqual(self.build_calls(), [])
        for field, value in (('RELEASE_SOURCE', 'a' * 40), ('RELEASE_TAG', 'v0.1.3'),
                             ('GITHUB_RUN_ATTEMPT', '2'), ('GITHUB_RUN_ID', '42010011054')):
            env = environment(RELEASE_SOURCE=r.INCIDENT_SOURCE, **{field: value}) if field != 'RELEASE_SOURCE' else environment(RELEASE_SOURCE=value)
            try:
                selected = r.dispatch(env)
            except r.ReleaseError:
                continue
            self.assertFalse(selected['creation_eligible'])

    def test_existing_partial_malformed_or_duplicate_marker_never_builds(self):
        body = r.marker_bytes(r.recovery_marker(self.release.state))
        valid = dict(id=51, name=r.RECOVERY_ASSET, state='uploaded', size=len(body),
                     digest=r.content_digest(body), content_type='application/json')
        cases = [[(valid, body)], [(dict(valid, state='starter'), b'')], [(dict(valid, digest=OTHER), body)],
                 [(dict(valid, size=2, digest=r.content_digest(b'{}')), b'{}')], [(valid, body)] * 2]
        for assets in cases:
            with self.subTest(assets=assets):
                self.assets = copy.deepcopy(assets)
                with self.assertRaises(r.ReleaseError):
                    self.fresh()
                    self.replies = visible()
                    self.release.publish()
                self.assertEqual(self.build_calls(), [])
                self.assertEqual(self.mutations(), [])

    def test_marker_upload_errors_never_build_retry_or_fallback(self):
        for status in (301, 401, 403, 404, 422, 429, 500, 503, None):
            with self.subTest(status=status):
                self.setUp()
                def fail(method, path, body):
                    if method == 'POST':
                        if status is None:
                            raise r.ReleaseError('Transport response lost')
                        return status, b'{"message":"SECRET"}', None, None
                self.api_fault = fail
                with self.assertRaises(r.ReleaseError):
                    self.release.publish()
                self.assertTrue(r.read_json(self.release.record.read_bytes())['marker_upload_attempted'])
                self.replies = visible()
                with self.assertRaisesRegex(r.ReleaseError, 'consumed'):
                    self.release.publish()
                self.assertEqual(len(self.mutations()), 1)
                self.assertEqual(self.build_calls(), [])
                self.assertEqual(self.pushes(), [])
                self.assertNotIn('SECRET', self.release.record.read_text())

    def test_marker_post_response_validation_and_uncertain_success(self):
        for point in ('loss', 'invalid_json', 'wrong_id', 'starter', 'size', 'digest', 'name', 'content_type', 'link'):
            with self.subTest(point=point):
                self.setUp()
                original_request = self.request
                def injected(method, path, body=None, host='api.github.com', binary=False):
                    reply = original_request(method, path, body, host, binary)
                    if method == 'POST':
                        if point == 'loss':
                            raise r.ReleaseError('Response lost after upload')
                        if point == 'invalid_json':
                            return 201, b'not json', None, None
                        row = r.read_json(reply[1])
                        if point == 'wrong_id': row['id'] = True
                        elif point == 'starter': row['state'] = 'starter'
                        elif point == 'size': row['size'] = 0
                        elif point == 'digest': row['digest'] = OTHER
                        elif point == 'name': row['name'] = 'other'
                        elif point == 'content_type': row['content_type'] = 'text/plain'
                        return 201, json.dumps(row).encode(), '' if point == 'link' else None, None
                    return reply
                with patch.object(r.Releases, 'request', side_effect=injected), self.assertRaises(r.ReleaseError):
                    self.release.publish()
                self.assertEqual(self.build_calls(), [])
                self.assertEqual(self.pushes(), [])
                self.assertEqual(len(self.assets), 1)
                self.fresh(GITHUB_RUN_ID='456')
                self.replies = visible()
                with self.assertRaisesRegex(r.ReleaseError, 'consumed'):
                    self.release.publish()
                self.assertEqual(len(self.mutations()), 1)

    def test_marker_readback_or_local_save_failure_consumes_opportunity(self):
        for point in ('download', 'pin_save', 'upload_save'):
            with self.subTest(point=point):
                self.setUp()
                write = r.write_json
                def save(path, state):
                    if (point == 'pin_save' and 'recovery' in state) or (point == 'upload_save' and state.get('marker_upload_attempted')):
                        raise OSError('Injected save failure')
                    return write(path, state)
                def fail(method, path, body):
                    if point == 'download' and '/assets/51' in path:
                        return 403, b'{}', None, None
                self.api_fault = fail
                with patch.object(r, 'write_json', side_effect=save), self.assertRaises((r.ReleaseError, OSError)):
                    self.release.publish()
                self.assertEqual(self.build_calls(), [])
                self.assertEqual(self.pushes(), [])
                self.assertEqual(len(self.assets), 0 if point == 'upload_save' else 1)
                if self.assets:
                    self.api_fault = None
                    self.fresh(GITHUB_RUN_ID='456')
                    self.replies = visible()
                    with self.assertRaisesRegex(r.ReleaseError, 'consumed'):
                        self.release.publish()

    def test_changed_or_removed_marker_blocks_push_and_later_release_writes(self):
        for phase in ('push', 'retain', 'finalize'):
            for change in ('delete', 'replace_id', 'bytes'):
                with self.subTest(phase=phase, change=change):
                    self.setUp()
                    def mutate():
                        row, body = self.assets[0]
                        if change == 'delete': self.assets = []
                        elif change == 'replace_id': self.assets[0] = dict(row, id=99), body
                        else: self.assets[0] = dict(row, size=len(body)+1, digest=r.content_digest(body+b' ')), body+b' '
                    if phase == 'push':
                        def before_push(c):
                            if c[0] == 'bash': mutate()
                            return False
                        self.fail_command = before_push
                        with self.assertRaises(r.ReleaseError): self.release.publish()
                        self.assertEqual(self.pushes(), [])
                    else:
                        self.accepted()
                        pin = copy.deepcopy(self.release.state['recovery'])
                        if phase == 'finalize': self.release.retain()
                        before = self.mutations()
                        mutate()
                        with self.assertRaises(r.ReleaseError):
                            self.release.retain() if phase == 'retain' else self.final()
                        self.assertEqual(self.mutations(), before)
                        self.assertEqual(self.release.state['recovery'], pin)
                        self.assertEqual(len(self.pushes()), 1)
                    self.assertTrue(self.row['draft'])

    def test_stale_or_denied_workflow_checks_stop_before_marker_build_push_and_writes(self):
        for phase in ('marker', 'build', 'push', 'retain', 'finalize'):
            with self.subTest(phase=phase):
                self.setUp()
                if phase in ('retain', 'finalize'):
                    self.accepted()
                    if phase == 'finalize': self.release.retain()
                    self.default_sha = 'd' * 40
                elif phase == 'marker':
                    self.default_sha = 'd' * 40
                elif phase == 'build':
                    def fail(method, path, body):
                        if '/assets/51' in path: self.default_sha = 'd' * 40
                    self.api_fault = fail
                else:
                    def drift(c):
                        if c[0] == 'bash': self.default_sha = 'd' * 40
                        return False
                    self.fail_command = drift
                before = self.mutations()
                with self.assertRaisesRegex(r.ReleaseError, 'stale'):
                    if phase == 'retain': self.release.retain()
                    elif phase == 'finalize': self.final()
                    else: self.release.publish()
                if phase == 'marker': self.assertEqual(self.mutations(), [])
                if phase in ('marker', 'build'): self.assertEqual(self.build_calls(), [])
                if phase in ('marker', 'build', 'push'): self.assertEqual(self.pushes(), [])
                else: self.assertEqual(self.mutations(), before)
        self.setUp()
        self.fail_command = lambda c: c[:2] == ('git', 'ls-tree')
        with self.assertRaises(r.ReleaseError): self.release.publish()
        self.assertEqual(self.mutations(), [])
        self.assertEqual(self.build_calls(), [])

    def test_ambiguous_push_and_postpush_failure_keep_marker_and_uncertainty(self):
        for point in ('push', 'no_digest', 'postpush'):
            with self.subTest(point=point):
                self.setUp()
                if point == 'push': self.fail_command = lambda c: c[:2] == ('docker', 'push')
                elif point == 'no_digest': self.push_output = 'unknown'
                else: self.replies = [unavailable(), unavailable(), (403, {}, None)]
                with self.assertRaises(r.ReleaseError): self.release.publish()
                record = r.read_json(self.release.record.read_bytes())
                self.assertTrue(record['push_attempted'])
                self.assertEqual(record['recovery']['asset_id'], 51)
                self.assertEqual(len(self.pushes()), 1)
                self.assertEqual(len(self.mutations()), 1)
                self.fail_command = None
                self.fresh(GITHUB_RUN_ID='456')
                self.replies = [unavailable()]
                with self.assertRaisesRegex(r.ReleaseError, 'consumed'): self.release.publish()
                self.assertEqual(len(self.pushes()), 1)

    def test_final_patch_denial_preserves_pins_assets_and_draft_with_no_retry(self):
        for status in (403, 404):
            with self.subTest(status=status):
                self.setUp()
                self.accepted()
                self.release.retain()
                pins = {key: copy.deepcopy(self.release.state[key]) for key in ('recovery', 'evidence_id',
                        'evidence_digest', 'evidence_size', 'digest', 'pinned_image', 'source_sha', 'tag', 'release_id')}
                assets, row = copy.deepcopy(self.assets), copy.deepcopy(self.row)
                self.api_fault = lambda m, p, b: (status, b'{}', None, None) if m == 'PATCH' else None
                with self.assertRaisesRegex(r.ReleaseError, f'HTTP {status}'): self.final()
                with self.assertRaisesRegex(r.ReleaseError, 'uncertain'): self.final()
                self.assertEqual(self.assets, assets)
                self.assertEqual(self.row, row)
                self.assertEqual({key: self.release.state[key] for key in pins}, pins)
                self.assertEqual(self.release.state['publication'], 'outcome_uncertain')
                self.assertEqual(len(self.pushes()), 1)
                self.assertEqual([m for m, _ in self.mutations()], ['POST', 'POST', 'PATCH'])

    def test_recovery_evidence_rejects_unknown_or_changed_coordinates(self):
        self.accepted()
        self.release.retain()
        doc = r.read_json(self.assets[1][1])
        mutations = [dict(asset_id=True), dict(asset_id=99), dict(size=True), dict(size=4097), dict(digest=OTHER),
                     dict(extra='unreviewed'), dict(marker=dict(doc['snapshot']['recovery']['marker'], run_attempt='2')),
                     dict(marker=dict(doc['snapshot']['recovery']['marker'], incident_artifact_id=1)),
                     dict(marker=dict(doc['snapshot']['recovery']['marker'], schema_version=True)),
                     dict(marker=dict(doc['snapshot']['recovery']['marker'], no_push_evidence='other'))]
        for changes in mutations:
            changed = copy.deepcopy(doc)
            changed['snapshot']['recovery'].update(changes)
            with self.subTest(changes=changes), self.assertRaises(r.ReleaseError):
                r.validate_evidence(json.dumps(changed).encode(), self.release.state)
        self.assertNotIn('package_responses', doc['snapshot'])
        self.assertNotIn('marker_upload_attempted', doc['snapshot'])

    def test_marker_on_later_asset_page_blocks_creation_without_download_or_post(self):
        route = f'{r.RELEASES}/{r.INCIDENT_RELEASE}/assets'
        pages = []
        def paginated(method, path, body):
            if path.startswith(route + '?'):
                pages.append(path)
                if path.endswith('page=1'):
                    rows = [dict(id=i + 100, name=f'unrelated-{i}') for i in range(100)]
                    link = f'<https://api.github.com{route}?per_page=100&page=2>; rel="next"'
                    return 200, json.dumps(rows).encode(), link, None
                return 200, json.dumps([dict(id=201, name=r.RECOVERY_ASSET, state='starter')]).encode(), None, None
        self.api_fault = paginated
        with self.assertRaisesRegex(r.ReleaseError, 'Existing recovery marker'):
            self.release.publish()
        self.assertEqual(pages, [route + '?per_page=100&page=1', route + '?per_page=100&page=2'])
        self.assertEqual(self.build_calls(), [])
        self.assertEqual(self.mutations(), [])

    def test_reresolving_same_record_never_repins_changed_or_missing_marker(self):
        self.fail_command = lambda c: c[:2] == ('docker', 'build')
        with self.assertRaises(r.ReleaseError): self.release.publish()
        original = copy.deepcopy(self.assets)
        pin = copy.deepcopy(self.release.state['recovery'])
        self.fail_command = None
        for assets in ([], [(dict(original[0][0], id=99), original[0][1])]):
            self.assets = assets
            with self.assertRaisesRegex(r.ReleaseError, 'Pinned recovery marker'):
                self.release.resolve()
            self.assertEqual(self.release.state['recovery'], pin)
        self.assertEqual(self.pushes(), [])
        self.assertEqual(len(self.mutations()), 1)

    def test_consumed_marker_survives_prepush_source_draft_registry_and_local_image_conflicts(self):
        for point in ('tag', 'draft', 'package_appeared', 'package_id', 'existing_version', 'local_image'):
            with self.subTest(point=point):
                self.setUp()
                if point == 'package_appeared': self.replies = [unavailable()] + visible()
                elif point == 'package_id': self.replies = visible() + visible(identifier=99)
                elif point == 'existing_version': self.replies = visible() + visible([version(tags=['0.1.2'])])
                def conflict(c):
                    if c[0] == 'bash':
                        if point == 'tag': self.remote = 'd' * 40
                        elif point == 'draft': self.row.update(draft=False, published_at='2026-11-05')
                        elif point == 'local_image': self.local_image_id = 'changed-local-image'
                    return False
                self.fail_command = conflict
                with self.assertRaises(r.ReleaseError): self.release.publish()
                record = r.read_json(self.release.record.read_bytes())
                self.assertEqual(record['recovery']['asset_id'], 51)
                self.assertFalse(record['push_attempted'])
                self.assertEqual(len(self.mutations()), 1)
                self.assertEqual(self.pushes(), [])

    def test_existing_digest_uses_no_marker_or_image_writes_and_verification_never_falls_back(self):
        for valid in (True, False):
            with self.subTest(valid=valid):
                self.setUp()
                self.replies = visible([version(tags=['0.1.2'])])
                self.release.publish()
                self.assertTrue(self.release.state['reused'])
                self.assertNotIn('recovery', self.release.state)
                if valid:
                    self.replies += visible([version(tags=['0.1.2'])])
                    self.release.accept()
                    self.release.retain()
                    self.final()
                    self.assertEqual([m for m, _ in self.mutations()], ['POST', 'PATCH'])
                else:
                    self.fail_command = lambda c: c[:3] == ('gh', 'attestation', 'verify')
                    with self.assertRaises(r.ReleaseError): self.release.accept()
                    self.assertEqual(self.mutations(), [])
                self.assertEqual(self.build_calls(), [])
                self.assertEqual(self.pushes(), [])

    def test_report_preserves_consumed_marker_and_does_not_offer_reset(self):
        import contextlib
        import io
        self.fail_command = lambda c: c[:2] == ('docker', 'build')
        with self.assertRaises(r.ReleaseError): self.release.publish()
        pin = copy.deepcopy(self.release.state['recovery'])
        os.environ['RELEASE_JOB_STATUS'] = 'failure'
        with contextlib.redirect_stdout(io.StringIO()): self.release.report()
        self.assertEqual(self.release.state['recovery'], pin)
        self.assertIn('consumed or uncertain', self.release.state['next_action'])
        self.assertEqual(self.pushes(), [])
        self.assertEqual(len(self.mutations()), 1)


class WorkflowTreeTests(unittest.TestCase):
    def git(self, *args):
        return subprocess.check_output(['git', *args], cwd=self.repo, text=True).strip()

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.repo = Path(self.temp.name)
        self.git('init', '-q', '-b', 'main')
        self.git('config', 'user.name', 'Fixture')
        self.git('config', 'user.email', 'fixture@example.test')
        workflows = self.repo / '.github/workflows'
        workflows.mkdir(parents=True)
        (workflows / 'release.yml').write_text('name: release\n')
        (workflows / 'test.yml').write_text('name: test\n')
        self.commit()
        self.source = self.git('rev-parse', 'HEAD')
        (self.repo / 'helper.py').write_text('# reviewed helper\n')
        self.commit()
        self.reviewed = self.git('rev-parse', 'HEAD')
        self.git('remote', 'add', 'origin', str(self.repo))
        self.state = dict(tag='v0.1.2', source_sha=self.source, release_id=r.INCIDENT_RELEASE, workflow_sha=self.reviewed)

    def commit(self):
        self.git('add', '.')
        self.git('commit', '-qm', 'test: synthetic workflow tree')

    def compare(self):
        # Real Git subprocesses and symbolic remote HEAD, with a synthetic source
        # commit pin only inside this offline test. No denied bytes are substituted.
        with patch.object(r, 'INCIDENT_SOURCE', self.source):
            r.recovery_workflows(self.repo, self.state)

    def test_complete_tree_equality_and_no_helper_only_restriction(self):
        self.compare()

    def test_workflow_add_delete_content_mode_default_branch_and_main_drift(self):
        for change in ('add', 'delete', 'content', 'mode', 'default_branch', 'drift'):
            with self.subTest(change=change):
                self.setUp()
                path = self.repo / '.github/workflows/test.yml'
                if change == 'add': (path.parent / 'added.yml').write_text('name: added\n')
                elif change == 'delete': path.unlink()
                elif change == 'content': path.write_text('name: changed\n')
                elif change == 'mode': path.chmod(0o755)
                elif change == 'default_branch': self.git('checkout', '-qb', 'other')
                else: (self.repo / 'helper.py').write_text('# drift\n')
                if change != 'default_branch': self.commit()
                if change not in ('default_branch', 'drift'):
                    self.state['workflow_sha'] = self.git('rev-parse', 'HEAD')
                with self.assertRaises(r.ReleaseError): self.compare()

    def test_actual_workflow_files_match_fixed_path_mode_blob_map(self):
        expected = {'release.yml': 'e0889675fbfb7bd9663d7bf302b29520210eeb7b',
                    'test.yml': 'ccb5f1b8d2d7df9be7417ced80449430ef069557'}
        workflows = baseline.ROOT / '.github/workflows'
        self.assertEqual({str(p.relative_to(workflows)) for p in workflows.rglob('*') if p.is_file()}, set(expected))
        for name, digest in expected.items():
            path = workflows / name
            actual = subprocess.check_output(['git', 'hash-object', str(path)], text=True).strip()
            self.assertEqual(actual, digest)
            self.assertEqual(path.stat().st_mode & 0o777, 0o644)
