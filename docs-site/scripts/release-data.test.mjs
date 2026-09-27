import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import test from 'node:test';
import { buildReleaseStatus, PROFILES, TOOL_HOSTS, TOOLCHAIN_HOSTS } from './release-data.mjs';

const sha = value => createHash('sha256').update(value).digest('hex');
const asset = (name, content = name) => ({ name, size: Buffer.byteLength(content),
  digest: `sha256:${sha(content)}`, state: 'uploaded' });

function fixture() {
  const toolsTag = 'v1.2.3';
  const toolchainTag = 'v1.2.4';
  const toolAssets = [];
  for (const host of TOOL_HOSTS) {
    const prefix = `aros-tools-${toolsTag}-${host}`;
    for (const name of [`${prefix}.tar.gz`, `${prefix}.tar.gz.manifest.json`,
      `${prefix}.tar.gz.sha256`, `${prefix}.spdx.json`]) {
      toolAssets.push(asset(name), asset(`${name}.sigstore.json`));
    }
  }
  for (const name of ['aros-tools.rb', 'aros-tools_1.2.3_amd64.deb',
    'aros-tools_1.2.3_arm64.deb', 'aros-tools_1.2.3_amd64.spdx.json',
    'aros-tools_1.2.3_arm64.spdx.json', 'PKGBUILD', 'RELEASE_NOTES.md', 'SHA256SUMS']) {
    toolAssets.push(asset(name), asset(`${name}.sigstore.json`));
  }
  const items = [];
  const chainAssets = [];
  for (const host of TOOLCHAIN_HOSTS) for (const target_profile of PROFILES) {
    const name = `aros-toolchain-v1-llvm11.0.0-${host}-${target_profile}.tar.xz`;
    items.push({ asset: name, host, target_profile, enabled: true,
      size: Buffer.byteLength(name), sha256: sha(name), tree_sha256: sha(`tree:${name}`) });
    for (const suffix of ['', '.manifest.json', '.sha256', '.spdx.json']) {
      chainAssets.push(asset(`${name}${suffix}`));
    }
  }
  const index = { release_id: toolchainTag,
    base_url: `https://github.com/metaneutrons/aros-toolchains/releases/download/${toolchainTag}`,
    source_commit: 'a'.repeat(40), producer_commit: 'b'.repeat(40), tools_commit: 'c'.repeat(40),
    artifacts: items };
  const indexBytes = Buffer.from(JSON.stringify(index));
  for (const name of ['llvm-11.0.0.sources.json', 'profiles-v1.json', 'SHA256SUMS',
    'toolchain-manifest-v1.schema.json', 'toolchain-provenance.sigstore.json',
    'toolchain-recipe-v2.json', 'tree-digest-v1.fixture.json']) chainAssets.push(asset(name));
  chainAssets.push(asset('toolchain-index-v1.json', indexBytes));
  const release = (repository, tag_name, assets) => ({ id: 100, tag_name,
    html_url: `https://github.com/metaneutrons/${repository}/releases/tag/${tag_name}`,
    published_at: '2026-01-01T00:00:00Z', draft: false, prerelease: false,
    immutable: true, assets });
  return {
    tools: release('aros-tools', toolsTag, toolAssets),
    toolchains: release('aros-toolchains', toolchainTag, chainAssets), indexBytes,
  };
}

test('complete stable releases produce the closed host/profile matrix', () => {
  const { tools, toolchains, indexBytes } = fixture();
  const status = buildReleaseStatus(tools, toolchains, indexBytes);
  assert.equal(status.tools.tag, 'v1.2.3');
  assert.equal(status.toolchains.tag, 'v1.2.4');
  assert.deepEqual(status.toolchains.profiles, PROFILES);
  assert(!status.toolchains.profiles.includes('opensbi-riscv64'));
});

for (const field of ['draft', 'prerelease', 'immutable']) {
  test(`${field} rejects an unqualified release`, () => {
    const { tools, toolchains, indexBytes } = fixture();
    tools[field] = field !== 'immutable';
    assert.throws(() => buildReleaseStatus(tools, toolchains, indexBytes), /not a stable immutable release/);
  });
}

test('missing native archive rejects release', () => {
  const { tools, toolchains, indexBytes } = fixture();
  tools.assets = tools.assets.filter(item => !item.name.endsWith('aarch64-apple-darwin.tar.gz'));
  assert.throws(() => buildReleaseStatus(tools, toolchains, indexBytes), /expected 40 assets/);
});

test('unmeasured or disabled toolchain lane rejects release', () => {
  const { tools, toolchains, indexBytes } = fixture();
  const index = JSON.parse(indexBytes.toString());
  index.artifacts[0].enabled = false;
  const modified = Buffer.from(JSON.stringify(index));
  const indexAsset = toolchains.assets.find(item => item.name === 'toolchain-index-v1.json');
  indexAsset.size = modified.length;
  indexAsset.digest = `sha256:${sha(modified)}`;
  assert.throws(() => buildReleaseStatus(tools, toolchains, modified), /disabled lane/);
});

test('changed index bytes reject release before parsing', () => {
  const { tools, toolchains, indexBytes } = fixture();
  const modified = Buffer.from(indexBytes);
  modified[0] ^= 1;
  assert.throws(() => buildReleaseStatus(tools, toolchains, modified), /index digest/);
});

test('archive hash must match the signed release index', () => {
  const { tools, toolchains, indexBytes } = fixture();
  const item = toolchains.assets.find(entry => entry.name.endsWith('pc-x86_64.tar.xz'));
  item.digest = `sha256:${'e'.repeat(64)}`;
  assert.throws(() => buildReleaseStatus(tools, toolchains, indexBytes), /measurement differs/);
});
