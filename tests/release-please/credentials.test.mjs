import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {spawnSync} from 'node:child_process';
import {test} from 'node:test';
import {getRegistryCredentials as npmCredentials} from '@sigstore/oci';
import crypto from 'node:crypto';
import {pathToFileURL} from 'node:url';

// Optional independent probe of the action's actual shipped dependency/runtime.
// CI always tests the lockfile-pinned public library; this mode additionally
// checks the action bundle bytes and omits only its startup entry point.
let getRegistryCredentials = npmCredentials;
if (process.env.ATTEST_BUNDLE) {
  const bundle = fs.readFileSync(process.env.ATTEST_BUNDLE, 'utf8');
  assert.equal(crypto.createHash('sha256').update(bundle).digest('hex'),
    '3ca89e06ffcb09ff97e9b1633575865cad0b23310ac82176d72990d7554836b5');
  const parts = bundle.split('var __webpack_exports__ = {};');
  assert.equal(parts.length, 2);
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'attest-reader-'));
  try {
    const harness = path.join(temp, 'reader.mjs');
    fs.writeFileSync(harness, parts[0] + '\nexport default __nccwpck_require__(62691).getRegistryCredentials;\n');
    getRegistryCredentials = (await import(pathToFileURL(harness).href)).default;
  } finally { fs.rmSync(temp, {recursive: true, force: true}); }
}

const root = new URL('../../', import.meta.url);
const lock = JSON.parse(fs.readFileSync(new URL('package-lock.json', import.meta.url)));
// Exact dependency identity from actions/attest 1e69f48's package-lock.json.
const expectedIntegrity = 'sha512-dhqphH/gjg8IFB0xAeD1Ppb/aSNwA2HUG+7A5mVJwiZt6rn5YoYMVc+esZREdhQLZ1H4JFyGZlTQXaZYx/l7yQ==';
const image = 'ghcr.io/dstoc/tines-mcp';
function helper(phase, env) {
  const result = spawnSync('python3', ['-c', `
import importlib.util, sys
spec = importlib.util.spec_from_file_location('release', 'scripts/release.py')
r = importlib.util.module_from_spec(spec)
spec.loader.exec_module(r)
getattr(r, sys.argv[1])()
`, phase], {cwd: root, env, encoding: 'utf8'});
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout, '');
}

test('actual pinned OCI reader ignores DOCKER_CONFIG and uses bridge; teardown removes job login', () => {
  assert.equal(lock.packages['node_modules/@sigstore/oci'].version, '0.7.2');
  assert.equal(lock.packages['node_modules/@sigstore/oci'].integrity, expectedIntegrity);
  const originalHomedir = os.homedir;
  const previousConfig = process.env.DOCKER_CONFIG;
  const previousHome = process.env.HOME;
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'release-credentials-'));
  try {
    for (const original of [false, true]) {
      const home = path.join(temp, original ? 'owned-home' : 'absent-home');
      const runner = path.join(temp, original ? 'owned-runner' : 'absent-runner');
      fs.mkdirSync(home, {mode: 0o700});
      fs.mkdirSync(runner, {mode: 0o700});
      const login = path.join(runner, 'release-docker');
      const config = path.join(home, '.docker/config.json');
      const prior = '{"auths":{"other.example":{"auth":"fixture:original"}}}\n';
      let priorInfo;
      if (original) {
        fs.mkdirSync(path.dirname(config), {mode: 0o700});
        fs.writeFileSync(config, prior, {mode: 0o640});
        priorInfo = fs.statSync(config);
      }
      const env = {...process.env, HOME: home, RUNNER_TEMP: runner, DOCKER_CONFIG: login,
        GITHUB_RUN_ID: '123', GITHUB_RUN_ATTEMPT: '1'};
      process.env.DOCKER_CONFIG = login;
      process.env.HOME = home;
      os.homedir = () => home; // Fixture home only; never changes the real home/config.
      helper('initialize_login', env);
      fs.writeFileSync(path.join(login, 'config.json'), JSON.stringify({auths: {'ghcr.io': {
        auth: Buffer.from('fixture-user:fixture-job-password').toString('base64'),
      }}}));
      assert.throws(() => getRegistryCredentials(image), /No credential/);
      helper('prepare_credentials', env);
      assert.deepEqual(getRegistryCredentials(image), {
        headers: undefined, username: 'fixture-user', password: 'fixture-job-password',
      });
      helper('cleanup_credentials', env);
      assert.throws(() => getRegistryCredentials(image), /No credential/);
      assert.equal(fs.existsSync(login), false);
      if (original) {
        assert.equal(fs.readFileSync(config, 'utf8'), prior);
        assert.equal(fs.statSync(config).ino, priorInfo.ino);
        assert.equal(fs.statSync(config).mode, priorInfo.mode);
      } else assert.equal(fs.existsSync(path.dirname(config)), false);
    }
  } finally {
    os.homedir = originalHomedir;
    if (previousConfig === undefined) delete process.env.DOCKER_CONFIG; else process.env.DOCKER_CONFIG = previousConfig;
    if (previousHome === undefined) delete process.env.HOME; else process.env.HOME = previousHome;
    fs.rmSync(temp, {recursive: true, force: true});
  }
});
