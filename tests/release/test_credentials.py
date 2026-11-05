"""Real filesystem failures and called entry points, using fixture credentials only."""
import base64
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import test_release as baseline
from test_release import r, ROOT, visible, version

SECRET = 'FIXTURE_JOB_PASSWORD_DO_NOT_LOG'
ORIGINAL = b'{"auths":{"other.example":{"auth":"ORIGINAL_FIXTURE"}}}\n'


class CredentialTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.home, self.runner = self.root / 'home', self.root / 'runner'
        self.home.mkdir(mode=0o700)
        self.runner.mkdir(mode=0o700)
        self.login, self.bridge = self.runner / 'release-docker', self.runner / 'release-attestation-bridge'
        self.docker, self.config = self.home / '.docker', self.home / '.docker/config.json'
        self.env = dict(HOME=str(self.home), RUNNER_TEMP=str(self.runner), DOCKER_CONFIG=str(self.login),
                        GITHUB_RUN_ID='123', GITHUB_RUN_ATTEMPT='1')
        env = patch.dict(os.environ, self.env)
        env.start()
        self.addCleanup(env.stop)

    def original(self):
        self.docker.mkdir(mode=0o700)
        self.config.write_bytes(ORIGINAL)
        self.config.chmod(0o640)
        return self.config.stat()

    def login_fixture(self):
        r.initialize_login()
        (self.login / 'config.json').write_text(json.dumps({'auths': {'ghcr.io': {
            'auth': base64.b64encode(('fixture:' + SECRET).encode()).decode()}}}))

    def assert_restored(self, prior):
        self.assertFalse(self.login.exists())
        if prior is None:
            self.assertFalse(self.config.exists() or self.config.is_symlink())
        else:
            self.assertEqual(self.config.read_bytes(), ORIGINAL)
            self.assertEqual(r.file_identity(self.config.stat()), r.file_identity(prior))

    def report(self, record=None, **env):
        return subprocess.run([sys.executable, str(ROOT / 'scripts/release.py'), 'report',
            '--workflow', str(self.root), '--source', str(self.root),
            '--record', str(record or self.root / 'release/record.json')],
            env=dict(os.environ, RELEASE_JOB_STATUS='failure') | env, capture_output=True, text=True)

    def test_absent_and_owned_original_restore_exactly_and_cleanup_is_repeatable(self):
        for existing in (False, True):
            with self.subTest(existing=existing):
                prior = self.original() if existing else None
                self.login_fixture()
                r.prepare_credentials()
                self.assertTrue(self.config.is_symlink())
                self.assertEqual(self.config.resolve(), self.login / 'config.json')
                self.assertEqual(stat.S_IMODE(self.login.stat().st_mode), 0o700)
                self.assertEqual(stat.S_IMODE((self.login / 'config.json').stat().st_mode), 0o600)
                self.assertNotIn(SECRET, (self.bridge / 'state.json').read_text())
                self.assertEqual(stat.S_IMODE((self.bridge / 'state.json').stat().st_mode), 0o600)
                r.cleanup_credentials()
                r.cleanup_credentials()
                self.assert_restored(prior)
                self.assertFalse(self.bridge.exists())
                if existing:
                    self.config.unlink()
                    self.docker.rmdir()

    def test_cleanup_preserves_unrelated_files_in_a_created_default_directory(self):
        self.login_fixture()
        r.prepare_credentials()
        unrelated = self.docker / 'unrelated'
        unrelated.write_text('KEEP')
        r.cleanup_credentials()
        self.assertEqual(unrelated.read_text(), 'KEEP')
        self.assertFalse(self.config.exists() or self.config.is_symlink())
        self.assertFalse(self.login.exists())
        self.assertFalse(self.bridge.exists())

    def test_exclusive_login_and_bridge_do_not_adopt_prior_state(self):
        self.login.mkdir()
        (self.login / 'config.json').write_text('KEEP')
        with self.assertRaises(FileExistsError): r.initialize_login()
        self.assertEqual((self.login / 'config.json').read_text(), 'KEEP')
        r.cleanup_credentials()
        self.login_fixture()
        self.bridge.mkdir(mode=0o700)
        with self.assertRaises(FileExistsError): r.prepare_credentials()
        self.assertFalse(self.config.exists())

    def test_unsafe_default_paths_modes_owners_and_special_files_are_rejected(self):
        for kind in ('directory_link', 'file_link', 'writable_directory', 'writable_file', 'fifo', 'foreign_file', 'hardlink'):
            with self.subTest(kind=kind):
                if self.docker.is_symlink(): self.docker.unlink()
                elif self.docker.exists():
                    for p in self.docker.iterdir(): p.unlink()
                    self.docker.rmdir()
                self.login_fixture()
                if kind == 'directory_link': self.docker.symlink_to(self.runner)
                else:
                    self.docker.mkdir(mode=0o700)
                    if kind == 'file_link': self.config.symlink_to(self.login / 'config.json')
                    elif kind == 'fifo': os.mkfifo(self.config)
                    else: self.config.write_bytes(ORIGINAL)
                    if kind == 'writable_directory': self.docker.chmod(0o777)
                    if kind == 'writable_file': self.config.chmod(0o666)
                    if kind == 'hardlink': os.link(self.config, self.root / 'other-link')
                if kind == 'foreign_file':
                    real = r.owned_file
                    def foreign(path):
                        if path == self.config: raise r.ReleaseError('Unsafe credential file')
                        return real(path)
                    with patch.object(r, 'owned_file', side_effect=foreign), self.assertRaises(r.ReleaseError):
                        r.prepare_credentials()
                else:
                    with self.assertRaises(r.ReleaseError): r.prepare_credentials()
                self.assertFalse(self.bridge.exists())
                r.cleanup_credentials()
                self.assertFalse(self.login.exists())

    def test_cleanup_does_not_adopt_another_run_or_attempts_bridge(self):
        self.original()
        self.login_fixture()
        r.prepare_credentials()
        with patch.dict(os.environ, GITHUB_RUN_ID='456'), self.assertRaises(r.ReleaseError):
            r.cleanup_credentials()
        self.assertFalse(self.login.exists())
        self.assertEqual((self.bridge / 'original').read_bytes(), ORIGINAL)
        # The original admitting execution can still restore its own backup.
        r.cleanup_credentials()
        self.assertEqual(self.config.read_bytes(), ORIGINAL)

    def test_login_symlinks_and_foreign_default_file_are_rejected_by_actual_checks(self):
        self.login.symlink_to(self.runner)
        with self.assertRaises(FileExistsError): r.initialize_login()
        self.login.unlink()
        self.login_fixture()
        self.original()
        real = Path.lstat
        def foreign(path):
            info = real(path)
            if path == self.config:
                values = list(info)
                values[4] = os.getuid() + 1
                return os.stat_result(values)
            return info
        with patch.object(Path, 'lstat', autospec=True, side_effect=foreign), self.assertRaises(r.ReleaseError):
            r.prepare_credentials()
        self.assertFalse(self.bridge.exists())
        r.cleanup_credentials()
        self.assertEqual(self.config.read_bytes(), ORIGINAL)

    def test_login_configuration_cannot_forward_headers_or_other_credentials(self):
        for content in ({'auths': {'ghcr.io': {'auth': 'invalid'}}},
                        {'auths': {'ghcr.io': {'auth': 'YTo='}}},
                        {'auths': {'ghcr.io': {'auth': 'YTpi', 'identitytoken': SECRET}}},
                        {'auths': {'ghcr.io': {'auth': 'YTpi'}, 'other': {}}},
                        {'auths': {'ghcr.io': {'auth': 'YTpi'}}, 'HttpHeaders': {'secret': SECRET}},
                        {'auths': {'ghcr.io': {'auth': 'YTpi'}}, 'credsStore': 'helper'}):
            with self.subTest(content=content):
                self.login_fixture()
                (self.login / 'config.json').write_text(json.dumps(content))
                with self.assertRaises(r.ReleaseError): r.prepare_credentials()
                self.assertFalse(self.config.exists())
                r.cleanup_credentials()

    def test_unexpected_replacement_preserves_original_backup_but_removes_login(self):
        prior = self.original()
        self.login_fixture()
        r.prepare_credentials()
        self.config.unlink()
        self.config.write_text('UNRELATED_REPLACEMENT')
        with self.assertRaises(r.ReleaseError): r.cleanup_credentials()
        self.assertFalse(self.login.exists())
        self.assertEqual(self.config.read_text(), 'UNRELATED_REPLACEMENT')
        self.assertEqual((self.bridge / 'original').read_bytes(), ORIGINAL)
        self.assertEqual(r.file_identity((self.bridge / 'original').stat()), r.file_identity(prior))

    def test_partial_preparation_failures_restore_before_return_and_remove_login(self):
        # Fail every journal save, both exclusive links, original unlink, and
        # symlink creation. No injected failure repeats during restoration.
        for operation, count in [('journal', 1), ('journal', 2), ('journal', 3),
                                 ('link', 1), ('link', 2), ('unlink', 1), ('symlink', 1)]:
            with self.subTest(operation=operation, count=count), tempfile.TemporaryDirectory() as temp:
                # Each case uses an independent state; incomplete private state
                # is preserved when its journal cannot prove safe removal.
                with patch.dict(os.environ, RUNNER_TEMP=temp, DOCKER_CONFIG=str(Path(temp)/'release-docker')):
                    prior = self.original()
                    r.initialize_login()
                    (Path(temp)/'release-docker/config.json').write_text(json.dumps({'auths': {'ghcr.io': {'auth': 'YTpi'}}}))
                    target, name = {'journal': (r, 'credential_journal'), 'link': (os, 'link'),
                                    'unlink': (Path, 'unlink'), 'symlink': (Path, 'symlink_to')}[operation]
                    real, calls = getattr(target, name), 0
                    def fail(*args, **kwargs):
                        nonlocal calls
                        calls += 1
                        if calls == count: raise OSError(SECRET)
                        return real(*args, **kwargs)
                    with patch.object(target, name, side_effect=fail, autospec=True), self.assertRaises((OSError, r.ReleaseError)):
                        r.prepare_credentials()
                    self.assertFalse((Path(temp)/'release-docker').exists())
                    self.assertEqual(self.config.read_bytes(), ORIGINAL)
                    self.assertEqual(r.file_identity(self.config.stat()), r.file_identity(prior))
                    self.config.unlink()
                    self.docker.rmdir()

    def test_report_restores_after_each_attestation_failure_or_cooperative_cancellation(self):
        # The actual workflow-expression suite proves that report is scheduled
        # for either attestation failure and cooperative cancellation. Exercise
        # its real CLI cleanup for each resulting status here.
        for status in ('success', 'failure', 'cancelled'):
            for failed_attestation in ('build', 'source'):
                with self.subTest(status=status, step=failed_attestation):
                    prior = self.original()
                    self.login_fixture()
                    r.prepare_credentials()
                    record = self.root / 'release/record.json'
                    record.parent.mkdir(exist_ok=True)
                    record.write_text('{}')
                    result = self.report(RELEASE_JOB_STATUS=status)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assert_restored(prior)
                    self.config.unlink()
                    self.docker.rmdir()

    def test_interruption_after_each_exclusive_link_is_restored_by_new_report_process(self):
        for count in (1, 2):
            with self.subTest(link=count):
                prior = self.original()
                self.login_fixture()
                real, calls = os.link, 0
                def interrupt(*args, **kwargs):
                    nonlocal calls
                    result = real(*args, **kwargs)
                    calls += 1
                    if calls == count: raise KeyboardInterrupt()
                    return result
                with patch.object(os, 'link', side_effect=interrupt), self.assertRaises(KeyboardInterrupt):
                    r.prepare_credentials()
                result = self.report()
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assert_restored(prior)
                self.config.unlink()
                self.docker.rmdir()

    def test_restore_io_failure_still_removes_target_and_preserves_backup(self):
        self.original()
        self.login_fixture()
        r.prepare_credentials()
        with patch.object(os, 'link', side_effect=OSError(SECRET)), self.assertRaises(OSError):
            r.cleanup_credentials()
        self.assertFalse(self.login.exists())
        self.assertEqual((self.bridge / 'original').read_bytes(), ORIGINAL)
        self.assertFalse(self.config.exists())
        # A later cleanup can safely restore the retained original; it cannot
        # make the removed job login readable again.
        r.cleanup_credentials()
        self.assertEqual(self.config.read_bytes(), ORIGINAL)

    def test_report_restores_before_corrupt_record_or_summary_failure(self):
        for failure in ('record', 'summary'):
            with self.subTest(failure=failure):
                prior = self.original()
                self.login_fixture()
                r.prepare_credentials()
                record = self.root / 'release/record.json'
                record.parent.mkdir(exist_ok=True)
                record.write_text('INVALID' if failure == 'record' else '{}')
                result = self.report(record, GITHUB_STEP_SUMMARY=str(self.root))
                self.assertEqual(result.returncode, 1)
                self.assert_restored(prior)
                self.assertNotIn(SECRET, result.stdout + result.stderr + record.read_text())
                self.config.unlink()
                self.docker.rmdir()

    def test_report_without_resolve_and_failed_restore_removes_credentials_without_logging_them(self):
        self.original()
        self.login_fixture()
        r.prepare_credentials()
        self.config.unlink()
        self.config.write_text('UNRELATED')
        result = self.report()
        self.assertEqual(result.returncode, 1)
        self.assertIn('Credential restoration failed', result.stderr)
        self.assertFalse(self.login.exists())
        self.assertTrue((self.bridge / 'original').exists())
        self.assertNotIn(SECRET, result.stdout + result.stderr)
        artifacts = self.root / 'release'
        self.assertNotIn('ORIGINAL_FIXTURE', ''.join(p.read_text() for p in artifacts.iterdir()))

    def test_checkout_or_init_failure_never_exposes_credentials_and_final_shell_cleanup_is_independent(self):
        workflow = (ROOT / '.github/workflows/release.yml').read_text()
        command = workflow.split('name: Remove ephemeral registry login')[1].split('run: ')[1].splitlines()[0]
        self.login_fixture()
        # No helper checkout is available, so only the existing always() shell
        # step can run. It does not depend on DOCKER_CONFIG initialization.
        result = subprocess.run(['bash', '-e', '-c', command], env={**os.environ, 'DOCKER_CONFIG': ''})
        self.assertEqual(result.returncode, 0)
        self.assertFalse(self.login.exists())
        self.assertFalse(self.config.exists())


class CalledCredentialTests(unittest.TestCase):
    def fixture(self):
        fixture = baseline.OrchestrationTests()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        fixture.patch_login.stop()
        fixture.patch_prepare.stop()
        home = fixture.base / 'home'
        home.mkdir(mode=0o700)
        os.environ.update(HOME=str(home), RUNNER_TEMP=str(fixture.base),
                          DOCKER_CONFIG=str(fixture.base / 'release-docker'))
        command = fixture.command
        def simulate_login(*args, **kwargs):
            result = command(*args, **kwargs)
            if args[:2] == ('docker', 'login'):
                (fixture.base / 'release-docker/config.json').write_text(json.dumps({'auths': {'ghcr.io': {'auth': 'YTpi'}}}))
            return result
        # patch.start() returns the mock, not the patcher.
        r.run.side_effect = simulate_login
        return fixture, home

    def test_called_publish_prepares_only_for_new_image_and_emits_no_authority_on_failure(self):
        for reused in (False, True):
            with self.subTest(reused=reused):
                f, home = self.fixture()
                if reused: f.replies = visible([version(tags=['0.1.2'])])
                f.release.publish()
                self.assertEqual((home / '.docker/config.json').is_symlink(), not reused)
                self.assertIn('new_image=' + str(not reused).lower(), (f.base / 'outputs').read_text())
                r.cleanup_credentials()
                f.doCleanups()
        f, home = self.fixture()
        (home / '.docker').symlink_to(f.base)
        with self.assertRaises(r.ReleaseError): f.release.publish()
        self.assertNotIn('new_image=true', (f.base / 'outputs').read_text())
        self.assertEqual(f.release.state['stage'], 'digest_observed')
        # No retry or rebuild follows the bridge failure.
        self.assertEqual(len(f.pushes()), 1)
