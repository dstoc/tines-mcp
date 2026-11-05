import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {test} from 'node:test';
import {GitHub} from 'release-please/build/src/github.js';
import {Manifest} from 'release-please/build/src/manifest.js';
import {TagName} from 'release-please/build/src/util/tag-name.js';
import {Version} from 'release-please/build/src/version.js';
import {RequestError} from '@octokit/request-error';

const root = new URL('../../', import.meta.url);
const read = p => readFileSync(new URL(p, root), 'utf8');
const quiet = Object.fromEntries(['info', 'warn', 'error', 'debug', 'trace'].map(k => [k, () => {}]));
const sha = 'a'.repeat(40);
async function fixture({tagStatus, releaseStatus, labelFailure = false} = {}) {
  const calls = [];
  const error = status => new RequestError('fixture rejection', status, {
    request: {method: 'POST', url: 'https://api.github.com/fixture', headers: {}},
    response: {status, url: 'https://api.github.com/fixture', headers: {},
      data: {errors: [{code: 'already_exists'}]}},
  });
  const github = new GitHub({repository: {owner: 'dstoc', repo: 'tines-mcp'}, logger: quiet,
    octokitAPIs: {octokit: {
      git: {async createRef(value) {
        calls.push(['tag', value]);
        if (tagStatus) throw error(tagStatus);
        return {data: {}};
      }},
      repos: {async createRelease(value) {
        calls.push(['release', value]);
        if (releaseStatus) throw error(releaseStatus);
        return {data: {id: 41, tag_name: value.tag_name, target_commitish: value.target_commitish,
          draft: value.draft, html_url: 'https://github.com/dstoc/tines-mcp/releases/tag/v0.1.2'}};
      }},
      issues: {async createComment() { calls.push(['comment']); return {data: {id: 99}}; },
        async removeLabel() { calls.push(['remove-label']); },
        async addLabels() { calls.push(['add-label']); if (labelFailure) throw error(403); }},
    }}});
  github.getFileContentsOnBranch = async path => ({parsedContent: read(path)});
  github.getFileJson = async path => JSON.parse(read(path));
  const manifest = await Manifest.fromManifest(github, 'main', undefined, undefined, {logger: quiet});
  // Isolate discovery; use the real manifest option propagation, sequencing and API adapter.
  const release = {tag: new TagName(Version.parse('0.1.2')), sha, notes: 'Reviewed notes'};
  manifest.findMergedReleasePullRequests = async function* () { yield {number: 34}; };
  manifest.getStrategiesByPath = async () => ({'.': {async buildReleases() {return [release];}}});
  return {manifest, calls};
}

test('pinned manifest options create exact source tag before draft, with draft output', async () => {
  const {manifest, calls} = await fixture();
  const [result] = await manifest.createReleases();
  assert.equal(manifest.repositoryConfig['.'].draft, true);
  assert.equal(manifest.repositoryConfig['.'].forceTag, true);
  assert.deepEqual(calls.map(c => c[0]), ['tag', 'release', 'comment', 'remove-label', 'add-label']);
  assert.deepEqual(calls[0][1], {owner: 'dstoc', repo: 'tines-mcp', ref: 'refs/tags/v0.1.2', sha});
  assert.equal(calls[1][1].target_commitish, sha);
  assert.equal(calls[1][1].draft, true);
  assert.equal(result.id, 41);
  assert.equal(result.draft, true);
  assert.equal(result.sha, sha);
});

test('ignored tag 422 still creates draft and cannot establish actual remote identity', async () => {
  const {manifest, calls} = await fixture({tagStatus: 422});
  assert.equal((await manifest.createReleases())[0].draft, true);
  assert.deepEqual(calls.slice(0, 2).map(c => c[0]), ['tag', 'release']);
  // The independent Python remote-tag fixtures reject a wrong or missing tag;
  // a successful draft result from this adapter supplies no such proof.
  const denied = await fixture({tagStatus: 403});
  await assert.rejects(() => denied.manifest.createReleases());
  assert.deepEqual(denied.calls.map(c => c[0]), ['tag']);
});

test('duplicate release and post-creation labeling failure do not replay outputs', async () => {
  const duplicate = await fixture({releaseStatus: 422});
  await assert.rejects(() => duplicate.manifest.createReleases());
  assert.deepEqual(duplicate.calls.map(c => c[0]), ['tag', 'release', 'remove-label', 'add-label']);
  const labels = await fixture({labelFailure: true});
  await assert.rejects(() => labels.manifest.createReleases());
  assert.deepEqual(labels.calls.map(c => c[0]), ['tag', 'release', 'comment', 'remove-label', 'add-label']);
  assert.equal(labels.calls[1][1].draft, true); // Draft survives the failed handoff.
});

test('no release candidate makes no tag, release, label or publication calls', async () => {
  const {manifest, calls} = await fixture();
  manifest.findMergedReleasePullRequests = async function* () {};
  assert.deepEqual(await manifest.createReleases(), []);
  assert.deepEqual(calls, []);
});
