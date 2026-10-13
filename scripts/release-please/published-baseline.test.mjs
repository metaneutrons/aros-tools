import assert from 'node:assert/strict';
import childProcess from 'node:child_process';
import {readFileSync} from 'node:fs';
import {syncBuiltinESMExports} from 'node:module';
import test from 'node:test';
import {
  Manifest,
  VERSION,
  Version,
  committedFloorStrategy,
  loadPreparation,
  metadata,
  publishedBaseline,
  requireAdvancingCandidates,
} from './published-baseline.mjs';

const repository = 'octo/aros-tools';
const annotatedTagSha = '1'.repeat(40);
const releaseCommitSha = '2'.repeat(40);
const mainCommitSha = '3'.repeat(40);
const otherCommitSha = '4'.repeat(40);
const fixtureBaseline = {
  version: '0.3.19',
  tag: 'v0.3.19',
  commit: 'a'.repeat(40),
};
const repositoryPath = 'repos/' + repository;
const configDocument = JSON.parse(readFileSync(new URL('../../release-please-config.json', import.meta.url)));
// Freeze the consumed-but-unpublished regression scenario, not the live
// repository version. A subsequent version PR must not invalidate this test.
const committedManifest = {'.': '0.3.20'};

function release(tagName = 'v0.3.20', overrides = {}) {
  return {
    id: 42,
    tag_name: tagName,
    draft: false,
    prerelease: false,
    published_at: '2026-10-01T12:00:00Z',
    immutable: true,
    assets: [{id: 7, name: 'source archive'}],
    ...overrides,
  };
}

function metadataRequest({
  releases = [release()],
  repositoryInfo = {
    full_name: repository,
    default_branch: 'main',
    archived: false,
    disabled: false,
  },
  ref = {
    ref: 'refs/tags/v0.3.20',
    object: {type: 'tag', sha: annotatedTagSha},
  },
  tag = {
    sha: annotatedTagSha,
    tag: 'v0.3.20',
    object: {type: 'commit', sha: releaseCommitSha},
  },
  branch = {
    protected: true,
    commit: {sha: mainCommitSha},
  },
  compare = {
    base_commit: {sha: releaseCommitSha},
    merge_base_commit: {sha: releaseCommitSha},
    status: 'ahead',
  },
} = {}) {
  const calls = [];
  const request = path => {
    calls.push(path);
    if (path === repositoryPath) return repositoryInfo;
    if (path === repositoryPath + '/releases?per_page=100&page=1') return releases;
    if (path === repositoryPath + '/releases?per_page=100&page=2') return [];
    if (path === repositoryPath + '/git/ref/tags/v0.3.20') return ref;
    if (path === repositoryPath + '/git/tags/' + annotatedTagSha) return tag;
    if (path === repositoryPath + '/branches/main') return branch;
    if (path === repositoryPath + '/compare/' + releaseCommitSha + '...' + mainCommitSha) return compare;
    throw new Error('Unexpected metadata endpoint: ' + path);
  };
  return {request, calls};
}

function withEnvironment(overrides, callback) {
  const previous = new Map();
  for (const [name, value] of Object.entries(overrides)) {
    previous.set(name, process.env[name]);
    if (value === undefined) delete process.env[name];
    else process.env[name] = value;
  }
  try {
    return callback();
  } finally {
    for (const [name, value] of previous) {
      if (value === undefined) delete process.env[name];
      else process.env[name] = value;
    }
  }
}

function withMockedExecFileSync(mock, callback) {
  const original = childProcess.execFileSync;
  childProcess.execFileSync = mock;
  syncBuiltinESMExports();
  try {
    return callback();
  } finally {
    childProcess.execFileSync = original;
    syncBuiltinESMExports();
  }
}

function preparationGithub(config = configDocument, versions = committedManifest) {
  return {
    repository: {owner: 'octo', repo: 'aros-tools', defaultBranch: 'main'},
    async getFileJson(path, branch) {
      assert.equal(branch, 'main');
      if (path === 'release-please-config.json') return config;
      if (path === '.release-please-manifest.json') return versions;
      throw new Error('Unexpected manifest file: ' + path);
    },
    async *releaseIterator() {},
    async *mergeCommitIterator() {},
  };
}

async function exhaust(iterator) {
  for await (const _entry of iterator) {}
}

const credentialEnvironment = {
  GH_TOKEN: 'workflow-token',
  GITHUB_TOKEN: 'ambient-github-token',
  GH_AUTH_TOKEN: 'ambient-auth-token',
  GH_ENTERPRISE_TOKEN: 'enterprise-token',
  GITHUB_ENTERPRISE_TOKEN: 'enterprise-github-token',
  GOVERNANCE_TOKEN: 'governance-token',
};

test('metadata reads JSON through gh using only the GitHub.com workflow token', () => {
  let invocation;
  withEnvironment(credentialEnvironment, () => withMockedExecFileSync((file, args, options) => {
    invocation = {file, args, options};
    return Buffer.from('{"full_name":"octo/aros-tools"}');
  }, () => {
    assert.deepEqual(metadata(repositoryPath), {full_name: repository});
  }));

  assert.equal(invocation.file, 'gh');
  assert.deepEqual(invocation.args, [
    'api',
    '--method',
    'GET',
    '--hostname',
    'github.com',
    '-H',
    'X-GitHub-Api-Version: 2022-11-28',
    repositoryPath,
  ]);
  assert.equal(invocation.options.env.GH_HOST, 'github.com');
  assert.equal(invocation.options.env.GH_TOKEN, 'workflow-token');
  for (const name of Object.keys(credentialEnvironment).filter(name => name !== 'GH_TOKEN')) {
    assert.equal(invocation.options.env[name], undefined);
  }
});

test('metadata rejects missing workflow credentials and unsafe endpoints before invoking gh', () => {
  let calls = 0;
  withMockedExecFileSync(() => {
    calls += 1;
    return Buffer.from('{}');
  }, () => {
    withEnvironment({GH_TOKEN: undefined}, () => {
      assert.throws(() => metadata(repositoryPath), /Workflow token is required/);
    });
    withEnvironment({GH_TOKEN: 'workflow-token'}, () => {
      assert.throws(() => metadata('repos/octo/aros-tools/..'), /Invalid repository metadata endpoint/);
    });
  });
  assert.equal(calls, 0);
});

test('metadata treats a 403 as unavailable and does not try another credential', () => {
  let calls = 0;
  let invocation;
  withEnvironment(credentialEnvironment, () => withMockedExecFileSync((file, args, options) => {
    calls += 1;
    invocation = {file, args, options};
    const error = new Error('HTTP 403 Forbidden');
    error.status = 403;
    throw error;
  }, () => {
    assert.throws(() => metadata(repositoryPath), {
      message: 'Published baseline metadata unavailable; no credential fallback',
    });
  }));
  assert.equal(calls, 1);
  assert.equal(invocation.file, 'gh');
  assert.equal(invocation.options.env.GH_TOKEN, 'workflow-token');
  assert.equal(invocation.options.env.GITHUB_TOKEN, undefined);
  assert.equal(invocation.options.env.GH_ENTERPRISE_TOKEN, undefined);
});

test('publishedBaseline returns verified metadata for the latest published stable release', () => {
  const {request, calls} = metadataRequest();
  assert.deepEqual(publishedBaseline(repository, request), {
    version: '0.3.20',
    tag: 'v0.3.20',
    release_id: 42,
    commit: releaseCommitSha,
    tag_object: annotatedTagSha,
  });
  assert.deepEqual(calls, [
    repositoryPath,
    repositoryPath + '/releases?per_page=100&page=1',
    repositoryPath + '/git/ref/tags/v0.3.20',
    repositoryPath + '/git/tags/' + annotatedTagSha,
    repositoryPath + '/branches/main',
    repositoryPath + '/compare/' + releaseCommitSha + '...' + mainCommitSha,
  ]);
});

test('publishedBaseline rejects malformed and duplicate release inventory', () => {
  const cases = [
    {releases: {}},
    {releases: [{...release(), published_at: 'not a date'}]},
    {releases: [{...release(), id: 0}]},
    {releases: [release('v0.3.20', {id: 10}), release('v0.3.19', {id: 10})]},
    {releases: [release('v0.3.20', {id: 10}), release('v0.3.20', {id: 11})]},
  ];
  for (const options of cases) {
    const {request} = metadataRequest(options);
    assert.throws(() => publishedBaseline(repository, request));
  }
});

test('publishedBaseline does not interpret a release-list 403 as a first release', () => {
  const {request, calls} = metadataRequest();
  const deniedRequest = path => {
    if (path === repositoryPath + '/releases?per_page=100&page=1') {
      calls.push(path);
      const error = new Error('HTTP 403 Forbidden');
      error.status = 403;
      throw error;
    }
    return request(path);
  };
  assert.throws(() => publishedBaseline(repository, deniedRequest), /403/);
  assert.equal(calls.at(-1), repositoryPath + '/releases?per_page=100&page=1');
});

test('publishedBaseline ignores a newer draft and selects the latest stable publication', () => {
  const {request, calls} = metadataRequest({
    releases: [
      release('v0.3.19', {id: 19}),
      release('v0.3.20', {id: 20}),
      release('v0.3.21', {
        id: 21,
        draft: true,
        published_at: null,
        immutable: false,
        assets: [],
      }),
    ],
  });
  const baseline = publishedBaseline(repository, request);
  assert.equal(baseline.version, '0.3.20');
  assert.ok(calls.includes(repositoryPath + '/git/ref/tags/v0.3.20'));
  assert.ok(!calls.some(path => path.includes('v0.3.21')));
});

test('publishedBaseline returns no baseline when the inventory contains only drafts or prereleases', () => {
  const {request, calls} = metadataRequest({
    releases: [
      release('v0.3.21', {id: 21, draft: true, published_at: null}),
      release('v0.3.22', {id: 22, prerelease: true, published_at: null}),
    ],
  });
  assert.equal(publishedBaseline(repository, request), null);
  assert.equal(calls.length, 2);
});

test('publishedBaseline rejects a mutable newest stable release instead of falling back', () => {
  const {request} = metadataRequest({
    releases: [
      release('v0.3.19', {id: 19}),
      release('v0.3.20', {id: 20, immutable: false}),
    ],
  });
  assert.throws(() => publishedBaseline(repository, request), /not immutable or has no assets/);
});

test('publishedBaseline rejects a lightweight tag', () => {
  const {request} = metadataRequest({
    ref: {
      ref: 'refs/tags/v0.3.20',
      object: {type: 'commit', sha: releaseCommitSha},
    },
  });
  assert.throws(() => publishedBaseline(repository, request), /annotated tag/);
});

test('publishedBaseline rejects a release outside main ancestry', () => {
  const {request} = metadataRequest({
    compare: {
      base_commit: {sha: releaseCommitSha},
      merge_base_commit: {sha: otherCommitSha},
      status: 'ahead',
    },
  });
  assert.throws(() => publishedBaseline(repository, request), /outside main history/);
});

test('loadPreparation uses the real installed Manifest API and preserves the committed manifest and package extras', async () => {
  assert.equal(VERSION, '17.6.0');
  assert.equal(committedManifest['.'], '0.3.20');
  const originalManifest = structuredClone(committedManifest);
  const originalConfig = structuredClone(configDocument);
  let olderHistoryConsumed = false;
  const github = {
    repository: {owner: 'octo', repo: 'aros-tools', defaultBranch: 'main'},
    async getFileJson(path, branch) {
      assert.equal(branch, 'main');
      if (path === 'release-please-config.json') return configDocument;
      if (path === '.release-please-manifest.json') return committedManifest;
      throw new Error('Unexpected manifest file: ' + path);
    },
    async *releaseIterator() {
      yield {id: 19, tagName: 'v0.3.19', sha: 'a'.repeat(40), notes: 'Previous release'};
    },
    async *mergeCommitIterator(branch, options) {
      assert.equal(branch, 'main');
      assert.equal(options.backfillFiles, true);
      yield {
        sha: 'b'.repeat(40),
        message: 'feat: exercise the published baseline',
        files: ['README.md'],
      };
      yield {
        sha: 'a'.repeat(40),
        message: 'chore: release 0.3.19',
        files: ['CHANGELOG.md'],
      };
      olderHistoryConsumed = true;
      yield {
        sha: 'c'.repeat(40),
        message: 'feat: older history must be excluded',
        files: ['OLD.md'],
      };
    },
    async findFilesByGlobAndRef(glob, branch) {
      assert.equal(branch, 'main');
      if (glob === 'Cargo.toml' || glob === 'Cargo.lock') return [glob];
      return [];
    },
  };

  const manifest = await loadPreparation(github, fixtureBaseline);
  assert.ok(manifest instanceof Manifest);
  assert.ok(manifest.releasedVersions['.'] instanceof Version);
  assert.equal(manifest.releasedVersions['.'].toString(), '0.3.19');
  assert.deepEqual(configDocument, originalConfig);
  assert.deepEqual(committedManifest, originalManifest);
  assert.deepEqual(
    manifest.repositoryConfig['.'].extraFiles,
    configDocument.packages['.']['extra-files'],
  );

  const candidates = await manifest.buildPullRequests();
  assert.equal(candidates.length, 1);
  assert.equal(candidates[0].version.toString(), '0.3.21');
  const notes = candidates[0].body.toString();
  assert.ok(notes.includes('compare/v0.3.19...v0.3.21'));
  assert.ok(notes.includes('exercise the published baseline'));
  assert.ok(!notes.includes('older history must be excluded'));
  assert.equal(olderHistoryConsumed, false);
  assert.deepEqual(candidates[0].updates.map(update => update.path).sort(), [
    '.release-please-manifest.json',
    'CHANGELOG.md',
    'Cargo.lock',
    'Cargo.toml',
  ]);
  assert.deepEqual(committedManifest, originalManifest);
});

test('loadPreparation rejects a published version newer than the committed manifest', async () => {
  await assert.rejects(
    loadPreparation(preparationGithub(), {
      ...fixtureBaseline,
      version: '0.3.21',
      tag: 'v0.3.21',
    }),
    /Committed manifest is older than the published release/,
  );
  assert.equal(committedManifest['.'], '0.3.20');
});

test('loadPreparation rejects an explicit last-release-sha', async () => {
  const config = structuredClone(configDocument);
  config['last-release-sha'] = 'a'.repeat(40);
  await assert.rejects(
    loadPreparation(preparationGithub(config), fixtureBaseline),
    /Unsupported explicit release baseline or version override/,
  );
});

test('loadPreparation rejects package release-as overrides', async () => {
  const config = structuredClone(configDocument);
  config.packages['.']['release-as'] = '0.3.21';
  await assert.rejects(
    loadPreparation(preparationGithub(config), fixtureBaseline),
    /Unsupported explicit release baseline or version override/,
  );
});

test('loadPreparation rejects explicit custom versioning strategies', async () => {
  const config = structuredClone(configDocument);
  config['versioning'] = 'always-bump-patch';
  await assert.rejects(
    loadPreparation(preparationGithub(config), fixtureBaseline),
    /Unsupported explicit release baseline or version override/,
  );
});

function conventionalCommit(type, breaking = false) {
  return {type, breaking, notes: []};
}

test('committedFloorStrategy rebases patch and feature bumps below the committed floor', () => {
  const strategy = committedFloorStrategy({
    bumpMinorPreMajor: true,
    bumpPatchForMinorPreMajor: true,
  }, Version.parse('0.3.20'));
  const fromPublished = Version.parse('0.3.19');

  assert.equal(strategy.bump(fromPublished, [conventionalCommit('fix')]).toString(), '0.3.21');
  assert.equal(strategy.bump(fromPublished, [conventionalCommit('feat')]).toString(), '0.3.21');
});

test('committedFloorStrategy preserves breaking-change policy and leaves higher bumps intact', () => {
  const floor = Version.parse('0.3.20');
  const configured = committedFloorStrategy({
    bumpMinorPreMajor: true,
    bumpPatchForMinorPreMajor: true,
  }, floor);
  const ordinary = committedFloorStrategy({}, floor);

  assert.equal(configured.bump(Version.parse('0.3.19'), [conventionalCommit('fix')]).toString(), '0.3.21');
  assert.equal(configured.bump(Version.parse('0.3.19'), [conventionalCommit('feat')]).toString(), '0.3.21');
  assert.equal(configured.bump(Version.parse('0.3.19'), [conventionalCommit('feat', true)]).toString(), '0.4.0');
  assert.equal(ordinary.bump(Version.parse('0.3.19'), [conventionalCommit('feat')]).toString(), '0.4.0');
  assert.equal(ordinary.bump(Version.parse('0.3.19'), [conventionalCommit('feat', true)]).toString(), '1.0.0');

  const higherPublishedHistory = committedFloorStrategy({}, Version.parse('0.3.19'));
  assert.equal(higherPublishedHistory.bump(Version.parse('0.3.20'), [conventionalCommit('fix')]).toString(), '0.3.21');
});

test('committedFloorStrategy requires a Version floor', () => {
  assert.throws(() => committedFloorStrategy({}), /Missing committed version floor/);
  assert.throws(() => committedFloorStrategy({}, '0.3.20'), /Missing committed version floor/);
});

test('loadPreparation rejects exhaustion before the published tag appears in library releases', async () => {
  const github = preparationGithub();
  await loadPreparation(github, fixtureBaseline);
  await assert.rejects(
    exhaust(github.releaseIterator({maxResults: 400})),
    /Published release missing from bounded library inventory/,
  );
});

test('loadPreparation rejects a library release whose tag SHA differs from published metadata', async () => {
  const github = preparationGithub();
  github.releaseIterator = async function* () {
    yield {
      id: 19,
      tagName: fixtureBaseline.tag,
      sha: 'b'.repeat(40),
      notes: 'Mismatched tag identity',
    };
  };
  await loadPreparation(github, fixtureBaseline);
  await assert.rejects(
    exhaust(github.releaseIterator({maxResults: 400})),
    /Library release identity differs from published baseline/,
  );
});

test('loadPreparation rejects exhaustion before the published commit appears in main history', async () => {
  const github = preparationGithub();
  github.mergeCommitIterator = async function* (branch) {
    assert.equal(branch, 'main');
    yield {sha: 'b'.repeat(40), message: 'feat: unrelated commit'};
  };
  await loadPreparation(github, fixtureBaseline);
  await assert.rejects(
    exhaust(github.mergeCommitIterator('main', {maxResults: 500})),
    /Published commit missing from bounded library history/,
  );
});

test('loadPreparation rejects an unexpected history branch before iterating commits', async () => {
  const github = preparationGithub();
  await loadPreparation(github, fixtureBaseline);
  await assert.rejects(
    exhaust(github.mergeCommitIterator('release/0.3', {maxResults: 500})),
    /Unexpected preparation history branch/,
  );
});

test('requireAdvancingCandidates accepts only a prepared manifest and a strictly advancing Version', async () => {
  const github = preparationGithub();
  const manifest = await loadPreparation(github, fixtureBaseline);

  assert.doesNotThrow(() => requireAdvancingCandidates(manifest, [
    {version: Version.parse('0.3.21')},
  ]));
  assert.throws(() => requireAdvancingCandidates(manifest, [
    {version: Version.parse('0.3.20')},
  ]), /does not advance the committed manifest/);
  assert.throws(() => requireAdvancingCandidates(manifest, [
    {version: Version.parse('0.3.19')},
  ]), /does not advance the committed manifest/);
  assert.throws(() => requireAdvancingCandidates(manifest, [
    {version: '0.3.21'},
  ]), /does not advance the committed manifest/);
  assert.throws(() => requireAdvancingCandidates(manifest, [
    {version: Version.parse('0.3.21')},
    {version: Version.parse('0.3.22')},
  ]), /Invalid prepared root candidates/);

  const unprepared = await Manifest.fromManifest(
    github,
    'main',
    'release-please-config.json',
    '.release-please-manifest.json',
  );
  assert.throws(() => requireAdvancingCandidates(unprepared, []), /Invalid prepared root candidates/);
});
