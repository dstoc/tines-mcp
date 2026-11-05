import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {test} from 'node:test';
import {createRequire} from 'node:module';
import {Manifest} from 'release-please/build/src/manifest.js';
import {CargoToml} from 'release-please/build/src/updaters/rust/cargo-toml.js';
import {Version} from 'release-please/build/src/version.js';
import TOML from '@iarna/toml';
import {setLogger} from 'release-please/build/src/util/logger.js';

const require = createRequire(import.meta.url);
const root = new URL('../../', import.meta.url);
const read = path => readFileSync(new URL(path, root), 'utf8');
const config = JSON.parse(read('release-please-config.json'));
const initial = Object.fromEntries(['Cargo.toml', 'Cargo.lock', '.release-please-manifest.json',
  'release-please-config.json'].map(path => [path, read(path)]));
const baseline = JSON.parse(initial['.release-please-manifest.json'])['.'];
const baseVersion = Version.parse(baseline);
const quiet = Object.fromEntries(['info', 'warn', 'error', 'debug', 'trace'].map(k => [k, () => {}]));

setLogger(quiet);

// Reads only: no token, network client, or mutation methods exist.
async function candidate(message, files = initial, changedPaths = ['src/main.rs']) {
  const github = {
    repository: {owner: 'dstoc', repo: 'tines-mcp'},
    async getFileContentsOnBranch(path, branch) {
      assert.equal(branch, 'main');
      assert.ok(path in files, `Unexpected file read: ${path}`);
      return {content: Buffer.from(files[path]).toString('base64'), parsedContent: files[path]};
    },
    async getFileJson(path, branch) {
      return JSON.parse((await this.getFileContentsOnBranch(path, branch)).parsedContent);
    },
    async *releaseIterator() {},
    async *tagIterator() {},
    async *mergeCommitIterator() {
      yield {sha: 'a'.repeat(40), message, files: changedPaths};
      yield {sha: config['bootstrap-sha'], message: 'feat: old history must be excluded', files: ['src/main.rs']};
      throw new Error('Read past bootstrap boundary');
    },
  };
  const manifest = await Manifest.fromManifest(github, 'main', undefined, undefined, {logger: quiet});
  return manifest.buildPullRequests();
}

function apply(pr, files = initial) {
  const result = {...files};
  for (const update of pr.updates) {
    result[update.path] = update.updater.updateContent(files[update.path] ?? '', quiet);
  }
  return result;
}

function unchangedDependencies(files, expected) {
  const before = TOML.parse(initial['Cargo.toml']);
  const after = TOML.parse(files['Cargo.toml']);
  assert.equal(after.package.version, expected);
  after.package.version = before.package.version;
  assert.deepEqual(after, before);
  const oldLock = TOML.parse(initial['Cargo.lock']);
  const lock = TOML.parse(files['Cargo.lock']);
  const local = lock.package.filter(p => p.name === 'tines-mcp' && !p.source);
  assert.equal(local.length, 1);
  assert.equal(local[0].version, expected);
  local[0].version = baseline;
  assert.deepEqual(lock, oldLock); // Dependency versions, sources, checksums and edges.
  assert.equal(JSON.parse(files['.release-please-manifest.json'])['.'], expected);
  assert.match(files['CHANGELOG.md'], new RegExp(expected.replaceAll('.', '\\.')));
}

test('CI package matches the version bundled in the pinned release action', () => {
  assert.equal(require('release-please/package.json').version, '17.3.0');
  assert.equal(require('@iarna/toml/package.json').version, '3.0.0');
  assert.match(read('.github/workflows/release.yml'), /release-please-action@5c625bfb5d1ff62eadeeb3772007f7f66fdcf071/);
  assert.equal(TOML.parse(initial['Cargo.toml']).package.version, baseline);
  assert.equal(TOML.parse(initial['Cargo.lock']).package.find(p => p.name === 'tines-mcp').version, baseline);
  assert.deepEqual(Object.keys(config.packages), ['.']);
  assert.equal(config.packages['.']['release-type'], 'rust');
  assert.equal(config['include-component-in-tag'], false);
});

test('previous multiline manifest fails; standard table preserves its meaning', () => {
  const old = read('tests/release-please/fixtures/Cargo.multiline.toml');
  const version = Version.parse('0.1.1');
  const updater = new CargoToml({version, versionsMap: new Map([['tines-mcp', version]])});
  assert.throws(() => updater.updateContent(old, quiet), /Unterminated inline array/);
  const normalized = old.replace(/rmcp = \{\n([\s\S]*?)\n\}\n/, '')
    + '\n[dependencies.rmcp]\nversion = "=3.8.0"\ndefault-features = false\n'
    + 'features = ["client", "server", "transport-streamable-http-client-reqwest", "transport-streamable-http-server"]\n';
  const parsed = TOML.parse(normalized);
  assert.deepEqual(TOML.parse(initial['Cargo.toml']).dependencies.rmcp, parsed.dependencies.rmcp);
});

for (const [kind, expected] of [
  ['fix', `${baseVersion.major}.${baseVersion.minor}.${baseVersion.patch + 1}`],
  ['feat', `${baseVersion.major}.${baseVersion.minor + 1}.0`],
]) {
  test(`${kind} PR synchronizes Cargo, lock, manifest and changelog; repeated update is stable`, async () => {
    const [pr] = await candidate(`${kind}: exercise updater`);
    assert.ok(pr);
    assert.equal(pr.version.toString(), expected);
    assert.deepEqual(pr.updates.map(u => u.path).sort(),
      ['.release-please-manifest.json', 'CHANGELOG.md', 'Cargo.lock', 'Cargo.toml']);
    const files = apply(pr);
    unchangedDependencies(files, expected);
    for (const path of ['Cargo.toml', 'Cargo.lock', '.release-please-manifest.json']) {
      assert.equal(apply(pr, files)[path], files[path]);
    }
    const [again] = await candidate(`${kind}: exercise updater`);
    assert.equal(again.version.toString(), expected);
    unchangedDependencies(apply(again), expected);
    assert.deepEqual(apply(again), files);
    assert.doesNotMatch(files['CHANGELOG.md'], /old history/);
  });
}

test('non-release commits produce no candidate', async () => {
  assert.deepEqual(await candidate('ci: adjust workflow'), []);
});

test('initial chore squash footer creates only the intended 0.1.1 release PR', async () => {
  const message = 'chore: prepare initial 0.1.1 release PR\n\n'
    + 'Document the initial release PR checks and preserve the owner-only 0.1.0 bootstrap.\n'
    + 'Refs: tines-mcp/36';
  const changedPaths = ['docs/releases.md', 'tests/release-please/updater.test.mjs'];
  const files = {...initial, 'docs/releases.md': read('docs/releases.md')};
  // A footer in documentation alone is not a commit-body release instruction.
  assert.match(files['docs/releases.md'], /Release-As: 0\.1\.1/);
  assert.deepEqual(await candidate(message, files, changedPaths), []);
  const prs = await candidate(`${message}\n\nRelease-As: 0.1.1`, files, changedPaths);
  assert.equal(prs.length, 1);
  const [pr] = prs;
  assert.equal(pr.version.toString(), '0.1.1');
  assert.equal(pr.title.toString(), 'chore(main): release 0.1.1');
  assert.deepEqual(pr.labels, ['autorelease: pending']);
  assert.deepEqual(pr.updates.map(u => u.path).sort(),
    ['.release-please-manifest.json', 'CHANGELOG.md', 'Cargo.lock', 'Cargo.toml']);
  const updated = apply(pr, files);
  unchangedDependencies(updated, '0.1.1');
  assert.doesNotMatch(updated['CHANGELOG.md'], /old history/);
  assert.match(updated['CHANGELOG.md'], /prepare initial 0\.1\.1 release PR/);
});
