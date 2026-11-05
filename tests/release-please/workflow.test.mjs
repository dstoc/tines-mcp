import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {registerHooks} from 'node:module';
import {spawnSync} from 'node:child_process';
import {test} from 'node:test';
import YAML from 'yaml';

// @actions/expressions 0.3.0 ships extensionless ESM imports. Resolve those
// imports only within that test dependency; do not alter installed source.
registerHooks({resolve(specifier, context, nextResolve) {
  if (specifier.startsWith('.') && context.parentURL?.includes('/@actions/expressions/')) {
    try { return nextResolve(specifier, context); }
    catch (error) {
      if (error.code !== 'ERR_MODULE_NOT_FOUND' && error.code !== 'ERR_UNSUPPORTED_DIR_IMPORT') throw error;
      try { return nextResolve(specifier + '.js', context); }
      catch { return nextResolve(specifier + '/index.js', context); }
    }
  }
  return nextResolve(specifier, context);
}});
const {Parser, Lexer, Evaluator, data} = await import('@actions/expressions');
const root = new URL('../../', import.meta.url);
const workflow = YAML.parse(readFileSync(new URL('.github/workflows/release.yml', root), 'utf8'));
const job = workflow.jobs.release;
const sha = 'a'.repeat(40);
const context = (event = 'push', result = 'success', outputs = {}) => ({
  github: {event_name: event, repository: 'dstoc/tines-mcp', ref: 'refs/heads/main'},
  needs: {'release-please': {result, outputs: {release_created: 'true', tag_name: 'v0.1.0',
    version: '0.1.0', sha, ...outputs}}},
  inputs: event === 'workflow_dispatch' ? {tag: 'v0.1.0', source_sha: sha, mode: 'normal',
    bootstrap_confirmation: ''} : {},
});
function evaluate(expression, values, cancelled = false) {
  const functions = [{name: 'cancelled', minArgs: 0, maxArgs: 0,
    call: () => new data.BooleanData(cancelled)}];
  const tokens = new Lexer(expression.replace(/^\$\{\{\s*|\s*\}\}$/g, '')).lex().tokens;
  const ast = new Parser(tokens, Object.keys(values), functions).parse();
  return new Evaluator(ast, JSON.parse(JSON.stringify(values), data.reviver),
    new Map(functions.map(fn => [fn.name, fn]))).evaluate();
}
function admission(values, cancelled = false) {
  return evaluate(job.if, values, cancelled).coerceString() === 'true';
}
function helper(values) {
  const env = Object.fromEntries(Object.entries(job.env).filter(([key]) => key !== 'GH_TOKEN')
    .map(([key, expression]) => [key, evaluate(expression, values).coerceString()]));
  Object.assign(env, {GITHUB_EVENT_NAME: values.github.event_name,
    GITHUB_REPOSITORY: values.github.repository, GITHUB_REF: values.github.ref,
    GITHUB_SHA: 'b'.repeat(40), GITHUB_RUN_ID: '123', GITHUB_RUN_ATTEMPT: '1',
    GITHUB_ACTOR: 'dstoc', GITHUB_ACTOR_ID: '539597', GITHUB_TRIGGERING_ACTOR: 'dstoc'});
  const result = spawnSync('python3', ['-c', `
import importlib.util, json, sys
spec = importlib.util.spec_from_file_location('release', 'scripts/release.py')
r = importlib.util.module_from_spec(spec)
spec.loader.exec_module(r)
try:
    print(json.dumps(r.dispatch(json.load(sys.stdin))))
except r.ReleaseError:
    sys.exit(1)
`], {cwd: root, input: JSON.stringify(env), encoding: 'utf8'});
  return {result, state: result.status === 0 ? JSON.parse(result.stdout) : undefined};
}

test('real workflow conditions handle skipped, failed and cancelled dependencies', () => {
  assert.equal(job.needs, 'release-please');
  // Presence of cancelled() disables Actions' implicit success() check, allowing manual dispatch.
  assert.match(job.if, /!cancelled\(\)/);
  for (const result of ['success', 'failure', 'cancelled', 'skipped', '']) {
    for (const created of ['true', 'false', '']) {
      const automatic = context('push', result, {release_created: created});
      assert.equal(admission(automatic), result === 'success' && created === 'true');
      assert.equal(admission(automatic, true), false);
    }
    assert.equal(admission(context('workflow_dispatch', result)), true);
  }
  for (const event of ['pull_request', 'release', 'workflow_run']) {
    assert.equal(admission(context(event)), false);
  }
  for (const overrides of [{repository: 'fork/tines-mcp'}, {ref: 'refs/tags/v0.1.0'},
    {ref: 'refs/heads/other'}]) {
    const values = context();
    Object.assign(values.github, overrides);
    assert.equal(admission(values), false);
    assert.equal(evaluate(workflow.jobs['release-please'].if, values).coerceString(), 'false');
  }
  assert.equal(evaluate(workflow.jobs['release-please'].if, context('workflow_dispatch')).coerceString(), 'false');
});

test('actual environment selection reaches the shared helper with exact source and normal mode', () => {
  for (const values of [context(), context('workflow_dispatch', 'skipped')]) {
    const {result, state} = helper(values);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(state.source_sha, sha);
    assert.equal(state.version, '0.1.0');
    assert.equal(state.mode, 'normal');
    assert.equal(state.bootstrap_confirmation, null);
  }
  // Push cannot select manual bootstrap inputs, even if they are supplied synthetically.
  const push = context();
  push.inputs.mode = 'bootstrap';
  assert.equal(helper(push).state.mode, 'normal');
});

test('missing and malformed output reaches no publishing authority', () => {
  for (const outputs of [{sha: ''}, {sha: 'main'}, {sha: 'A'.repeat(40)},
    {tag_name: ''}, {tag_name: 'v0.1.0-rc.1'}, {tag_name: 'v00.1.0'},
    {version: ''}, {version: '0.2.0'}, {version: '0.1.0\n'},
    {release_created: 'TRUE'}, {tag_name: '$(touch unexpected)'}]) {
    assert.notEqual(helper(context('push', 'success', outputs)).result.status, 0);
  }
  assert.notEqual(helper(context('push', 'failure')).result.status, 0);
});

test('manual owner bootstrap stays explicit and separate from automation', () => {
  const values = context('workflow_dispatch', 'skipped');
  values.inputs.mode = 'bootstrap';
  values.inputs.bootstrap_confirmation = `CREATE ghcr.io/dstoc/tines-mcp:0.1.0 FROM ${sha}; OWNER CHECKED PACKAGE ABSENT; NO UNRESOLVED PRIOR PUSH`;
  assert.equal(helper(values).state.mode, 'bootstrap');
  values.github.event_name = 'push';
  assert.notEqual(helper(values).result.status, 0);
});

test('PR write capabilities remain isolated from publisher and default permissions', () => {
  assert.deepEqual(workflow.permissions, {contents: 'read'});
  assert.deepEqual(workflow.jobs['release-please'].permissions,
    {contents: 'write', issues: 'write', 'pull-requests': 'write'});
  assert.deepEqual(job.permissions,
    {contents: 'read', packages: 'write', attestations: 'write', 'id-token': 'write'});
  assert.deepEqual(workflow.on.push, {branches: ['main']});
  assert.deepEqual(Object.keys(workflow.on).sort(), ['push', 'workflow_dispatch']);
  for (const key of ['release_created', 'tag_name', 'version', 'sha']) {
    assert.equal(workflow.jobs['release-please'].outputs[key], `\${{ steps.release.outputs.${key} }}`);
  }
});
