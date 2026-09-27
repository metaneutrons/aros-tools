import { createHash } from 'node:crypto';

export const TOOL_HOSTS = [
  'x86_64-unknown-linux-gnu',
  'aarch64-unknown-linux-gnu',
  'aarch64-apple-darwin',
];
export const TOOLCHAIN_HOSTS = ['linux-x86_64', 'linux-aarch64', 'macos-aarch64'];
export const PROFILES = ['pc-x86_64', 'arm-raspi', 'rpi-aarch64'];
const digestPattern = /^sha256:([0-9a-f]{64})$/;
const commitPattern = /^[0-9a-f]{40}$/;

function assert(condition, message) {
  if (!condition) throw new Error(`Release status: ${message}`);
}

function assetMap(release, expectedNames) {
  assert(Array.isArray(release.assets), 'assets are missing');
  const found = new Map();
  for (const asset of release.assets) {
    assert(typeof asset.name === 'string' && !found.has(asset.name), 'duplicate or unnamed asset');
    assert(Number.isSafeInteger(asset.size) && asset.size > 0, `invalid size: ${asset.name}`);
    assert(digestPattern.test(asset.digest), `missing SHA-256 digest: ${asset.name}`);
    assert(asset.state === 'uploaded', `asset not uploaded: ${asset.name}`);
    found.set(asset.name, asset);
  }
  assert(found.size === expectedNames.size, `expected ${expectedNames.size} assets, found ${found.size}`);
  for (const name of expectedNames) assert(found.has(name), `missing asset: ${name}`);
  return found;
}

function validateRelease(release, repository) {
  assert(release && !release.draft && !release.prerelease && release.immutable === true,
    `${repository} is not a stable immutable release`);
  assert(/^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(release.tag_name),
    `invalid SemVer tag for ${repository}`);
  assert(Number.isSafeInteger(release.id) && release.id > 0, `invalid release ID for ${repository}`);
  assert(!Number.isNaN(Date.parse(release.published_at)), `missing publication date for ${repository}`);
  const url = `https://github.com/metaneutrons/${repository}/releases/tag/${release.tag_name}`;
  assert(release.html_url === url, `unexpected release URL for ${repository}`);
  return { tag: release.tag_name, url, publishedAt: release.published_at };
}

function toolAssetNames(tag) {
  const version = tag.slice(1);
  const names = new Set();
  for (const host of TOOL_HOSTS) {
    const prefix = `aros-tools-${tag}-${host}`;
    for (const base of [`${prefix}.tar.gz`, `${prefix}.tar.gz.manifest.json`,
      `${prefix}.tar.gz.sha256`, `${prefix}.spdx.json`]) {
      names.add(base);
      names.add(`${base}.sigstore.json`);
    }
  }
  for (const base of ['aros-tools.rb', `aros-tools_${version}_amd64.deb`,
    `aros-tools_${version}_arm64.deb`, `aros-tools_${version}_amd64.spdx.json`,
    `aros-tools_${version}_arm64.spdx.json`, 'PKGBUILD', 'RELEASE_NOTES.md', 'SHA256SUMS']) {
    names.add(base);
    names.add(`${base}.sigstore.json`);
  }
  return names;
}

function toolchainAssetNames(index) {
  const names = new Set(['llvm-11.0.0.sources.json', 'profiles-v1.json', 'SHA256SUMS',
    'toolchain-index-v1.json', 'toolchain-manifest-v1.schema.json',
    'toolchain-provenance.sigstore.json', 'toolchain-recipe-v2.json',
    'tree-digest-v1.fixture.json']);
  for (const item of index.artifacts) {
    for (const suffix of ['', '.manifest.json', '.sha256', '.spdx.json']) {
      names.add(`${item.asset}${suffix}`);
    }
  }
  return names;
}

export function buildReleaseStatus(toolsRelease, toolchainsRelease, indexBytes) {
  const tools = validateRelease(toolsRelease, 'aros-tools');
  const toolchains = validateRelease(toolchainsRelease, 'aros-toolchains');
  assetMap(toolsRelease, toolAssetNames(tools.tag));
  const indexAsset = toolchainsRelease.assets.find(asset => asset.name === 'toolchain-index-v1.json');
  assert(indexAsset, 'toolchain index asset is missing');
  assert(indexBytes.length === indexAsset.size, 'toolchain index size differs from GitHub metadata');
  assert(`sha256:${createHash('sha256').update(indexBytes).digest('hex')}` === indexAsset.digest,
    'toolchain index digest differs from GitHub metadata');
  const index = JSON.parse(indexBytes.toString('utf8'));
  assert(index.release_id === toolchains.tag, 'toolchain index release ID mismatch');
  assert(index.base_url === `https://github.com/metaneutrons/aros-toolchains/releases/download/${toolchains.tag}`,
    'toolchain index base URL mismatch');
  assert(commitPattern.test(index.source_commit) && commitPattern.test(index.producer_commit)
    && commitPattern.test(index.tools_commit), 'toolchain index has invalid commit identity');
  assert(Array.isArray(index.artifacts) && index.artifacts.length === 9,
    'toolchain index must contain exactly nine artifacts');
  const lanes = new Set();
  for (const item of index.artifacts) {
    const lane = `${item.host}/${item.target_profile}`;
    assert(TOOLCHAIN_HOSTS.includes(item.host) && PROFILES.includes(item.target_profile),
      `unexpected toolchain lane: ${lane}`);
    assert(!lanes.has(lane) && item.enabled === true, `duplicate or disabled lane: ${lane}`);
    assert(item.asset === `aros-toolchain-v1-llvm11.0.0-${item.host}-${item.target_profile}.tar.xz`,
      `unexpected archive name: ${lane}`);
    assert(/^[0-9a-f]{64}$/.test(item.sha256) && /^[0-9a-f]{64}$/.test(item.tree_sha256)
      && Number.isSafeInteger(item.size) && item.size > 0, `invalid artifact measurement: ${lane}`);
    lanes.add(lane);
  }
  for (const host of TOOLCHAIN_HOSTS) for (const profile of PROFILES) {
    assert(lanes.has(`${host}/${profile}`), `missing toolchain lane: ${host}/${profile}`);
  }
  const assets = assetMap(toolchainsRelease, toolchainAssetNames(index));
  for (const item of index.artifacts) {
    const asset = assets.get(item.asset);
    assert(asset.size === item.size && asset.digest === `sha256:${item.sha256}`,
      `archive measurement differs from index: ${item.asset}`);
  }
  return {
    tools: { ...tools, hosts: TOOL_HOSTS },
    toolchains: { ...toolchains, hosts: TOOLCHAIN_HOSTS, profiles: PROFILES,
      sourceCommit: index.source_commit, producerCommit: index.producer_commit,
      toolsCommit: index.tools_commit },
  };
}
