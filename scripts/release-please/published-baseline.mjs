import {createRequire} from 'node:module';
import {execFileSync} from 'node:child_process';
import {appendFileSync} from 'node:fs';
import {pathToFileURL} from 'node:url';

const require = createRequire(import.meta.url);
export const {Manifest, GitHub, VERSION, registerVersioningStrategy} = require('release-please');
// Version is not a root export in 17.6.0. This exact internal import is covered
// by the frozen dependency lock and real-library counterprobes, not a fallback.
export const {Version} = require('release-please/build/src/version.js');
const {DefaultVersioningStrategy} = require('release-please/build/src/versioning-strategies/default.js');
if (VERSION !== '17.6.0') throw new Error('Unsupported Release Please API contract');

const OID = /^[0-9a-f]{40}$/;
const TAG = /^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$/;
const committedVersions = new WeakMap();
const versionFloors = new WeakMap();
const VERSIONING = 'published-history-with-committed-floor';

export function committedFloorStrategy(options, floor) {
  requireThat(floor instanceof Version, 'Missing committed version floor');
  const strategy = new DefaultVersioningStrategy(options);
  function bump(version, commits) {
    // The library owns Conventional Commit classification and SemVer bumps.
    // A consumed, unpublished manifest version cannot be reused by a new PR.
    const calculated = strategy.bump(version, commits);
    const result = calculated.compare(floor) > 0 ? calculated : strategy.bump(floor, commits);
    requireThat(result.compare(floor) > 0, 'Version override does not advance the committed floor');
    return result;
  }
  return {
    bump,
    determineReleaseType(_version, commits) {
      return {bump: version => bump(version, commits)};
    },
  };
}
registerVersioningStrategy(VERSIONING,
  options => committedFloorStrategy(options, versionFloors.get(options.github)));
function stableTag(value) {
  const match = TAG.exec(value);
  return match !== null && match.slice(1).every(part => Number.isSafeInteger(Number(part)));
}
function requireThat(condition, message) {
  if (!condition) throw new Error(message);
}

export function metadata(path) {
  const token = process.env.GH_TOKEN;
  requireThat(typeof token === 'string' && token.length > 0, 'Workflow token is required');
  requireThat(/^repos\/[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+(?:\/|$)/.test(path)
    && !path.split('/').includes('..'), 'Invalid repository metadata endpoint');
  const env = {...process.env, GH_HOST: 'github.com', GH_TOKEN: token};
  for (const name of ['GITHUB_TOKEN', 'GH_AUTH_TOKEN', 'GH_ENTERPRISE_TOKEN',
    'GITHUB_ENTERPRISE_TOKEN', 'GOVERNANCE_TOKEN']) delete env[name];
  try {
    return JSON.parse(execFileSync('gh', ['api', '--method', 'GET', '--hostname', 'github.com',
      '-H', 'X-GitHub-Api-Version: 2022-11-28', path],
    {env, timeout: 60000, maxBuffer: 8 * 1024 * 1024, stdio: ['ignore', 'pipe', 'pipe']}));
  } catch {
    throw new Error('Published baseline metadata unavailable; no credential fallback');
  }
}

export function publishedBaseline(repository, request = metadata) {
  requireThat(/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repository), 'Invalid repository');
  const prefix = `repos/${repository}`;
  const repo = request(prefix);
  requireThat(repo.full_name === repository && repo.default_branch === 'main'
    && repo.archived === false && repo.disabled === false, 'Repository is not active main');
  const releases = [];
  const ids = new Set();
  const tags = new Set();
  let complete = false;
  for (let page = 1; page <= 20; page++) {
    const values = request(`${prefix}/releases?per_page=100&page=${page}`);
    requireThat(Array.isArray(values) && values.length <= 100, 'Invalid release page');
    for (const value of values) {
      requireThat(value && Number.isSafeInteger(value.id) && value.id > 0
        && typeof value.tag_name === 'string' && value.tag_name.length > 0
        && typeof value.draft === 'boolean' && typeof value.prerelease === 'boolean'
        && !ids.has(value.id) && !tags.has(value.tag_name), 'Ambiguous release inventory');
      ids.add(value.id);
      tags.add(value.tag_name);
      if (!value.draft && !value.prerelease && TAG.test(value.tag_name)) {
        requireThat(stableTag(value.tag_name), 'Stable release version exceeds supported bounds');
        requireThat(typeof value.published_at === 'string'
          && Number.isFinite(Date.parse(value.published_at)), 'Stable release lacks publication evidence');
        releases.push(value);
      }
    }
    if (values.length < 100) { complete = true; break; }
  }
  requireThat(complete, 'Release inventory exceeds bounded pagination');
  if (releases.length === 0) return null; // Genuine first release, not a failed lookup.
  releases.sort((a, b) => Version.parse(b.tag_name.slice(1)).compare(Version.parse(a.tag_name.slice(1))));
  const release = releases[0];
  requireThat(release.immutable === true && Array.isArray(release.assets) && release.assets.length > 0,
    'Latest stable release is not immutable or has no assets');
  const ref = request(`${prefix}/git/ref/tags/${release.tag_name}`);
  requireThat(ref.ref === `refs/tags/${release.tag_name}` && ref.object?.type === 'tag'
    && OID.test(ref.object.sha), 'Published baseline must have an annotated tag');
  const tag = request(`${prefix}/git/tags/${ref.object.sha}`);
  requireThat(tag.sha === ref.object.sha && tag.tag === release.tag_name
    && tag.object?.type === 'commit' && OID.test(tag.object.sha), 'Published tag identity differs');
  const main = request(`${prefix}/branches/main`);
  requireThat(main.protected === true && OID.test(main.commit?.sha), 'Main is not protected');
  const compare = request(`${prefix}/compare/${tag.object.sha}...${main.commit.sha}`);
  requireThat(compare.base_commit?.sha === tag.object.sha
    && compare.merge_base_commit?.sha === tag.object.sha
    && ['ahead', 'identical'].includes(compare.status), 'Published baseline is outside main history');
  return {version: release.tag_name.slice(1), tag: release.tag_name, release_id: release.id,
    commit: tag.object.sha, tag_object: tag.sha};
}

export async function loadPreparation(github, baseline) {
  requireThat(!versionFloors.has(github), 'GitHub client already belongs to a preparation');
  const manifest = await Manifest.fromManifest(github, 'main',
    'release-please-config.json', '.release-please-manifest.json');
  requireThat(Object.keys(manifest.repositoryConfig).join() === '.'
    && Object.keys(manifest.releasedVersions).join() === '.', 'Only the configured root package is supported');
  committedVersions.set(manifest, manifest.releasedVersions['.']);
  requireThat(!manifest.lastReleaseSha && !manifest.repositoryConfig['.'].releaseAs
    && [undefined, 'default'].includes(manifest.repositoryConfig['.'].versioning),
  'Unsupported explicit release baseline or version override');
  requireThat(!versionFloors.has(github), 'GitHub client already belongs to a preparation');
  versionFloors.set(github, manifest.releasedVersions['.']);
  manifest.repositoryConfig['.'].versioning = VERSIONING;
  if (baseline) {
    const published = Version.parse(baseline.version);
    requireThat(published.compare(manifest.releasedVersions['.']) <= 0,
      'Committed manifest is older than the published release');
    // In-memory published history selection only. Never rewrite or roll back
    // repository files, choose an upcoming version, or create a tag/release.
    manifest.releasedVersions['.'] = published;
    // Release Please otherwise warns and bootstraps when a bounded iterator
    // misses the release or its commit. That is not an acceptable fallback.
    const releaseIterator = github.releaseIterator.bind(github);
    github.releaseIterator = async function* (options) {
      let found = false;
      for await (const release of releaseIterator(options)) {
        if (release.tagName === baseline.tag) {
          requireThat(release.sha === baseline.commit, 'Library release identity differs from published baseline');
          found = true;
        }
        yield release;
      }
      requireThat(found, 'Published release missing from bounded library inventory');
    };
    const commitIterator = github.mergeCommitIterator.bind(github);
    github.mergeCommitIterator = async function* (branch, options) {
      requireThat(branch === 'main', 'Unexpected preparation history branch');
      let found = false;
      for await (const commit of commitIterator(branch, options)) {
        if (commit.sha === baseline.commit) found = true;
        yield commit;
      }
      requireThat(found, 'Published commit missing from bounded library history');
    };
  }
  // createPullRequests rebuilds candidates internally. Validate every build,
  // including that mutation's actual candidate set, not an earlier snapshot.
  const buildPullRequests = manifest.buildPullRequests.bind(manifest);
  manifest.buildPullRequests = async () => {
    const candidates = await buildPullRequests();
    requireAdvancingCandidates(manifest, candidates);
    return candidates;
  };
  return manifest;
}

export function requireAdvancingCandidates(manifest, candidates) {
  const committed = committedVersions.get(manifest);
  requireThat(committed && candidates.length <= 1, 'Invalid prepared root candidates');
  for (const candidate of candidates) {
    requireThat(candidate.version instanceof Version && candidate.version.compare(committed) > 0,
      'Calculated version does not advance the committed manifest; preparation is not admitted');
  }
}

async function main() {
  requireThat(process.argv.length === 2
    || (process.argv.length === 3 && process.argv[2] === '--dry-run'),
  'Invalid preparation invocation');
  const repository = process.env.GITHUB_REPOSITORY;
  requireThat(process.env.GITHUB_REF === 'refs/heads/main', 'Preparation requires main');
  const baseline = publishedBaseline(repository);
  const [owner, repo] = repository.split('/');
  const github = await GitHub.create({owner, repo, token: process.env.GH_TOKEN,
    defaultBranch: 'main', apiUrl: 'https://api.github.com', graphqlUrl: 'https://api.github.com'});
  const manifest = await loadPreparation(github, baseline);
  if (process.argv[2] === '--dry-run') {
    const candidates = await manifest.buildPullRequests();
    console.log(JSON.stringify({baseline, candidates: candidates.map(candidate => ({
      version: candidate.version?.toString(), title: candidate.title.toString(),
      files: candidate.updates.map(update => update.path)}))}));
    return;
  }
  requireThat(process.argv.length === 2 && process.env.GITHUB_OUTPUT, 'Invalid preparation invocation');
  const pulls = (await manifest.createPullRequests()).filter(Boolean);
  requireThat(pulls.length <= 1, 'Release Please returned multiple root PRs');
  appendFileSync(process.env.GITHUB_OUTPUT,
    `prs_created=${pulls.length > 0}\n${pulls.length ? `pr=${JSON.stringify(pulls[0])}\n` : ''}`);
  console.log(`Release Please prepared ${pulls.length} version PR; no release creation attempted`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch(() => {
    console.error('Release Please preparation failed; no release creation or credential fallback');
    process.exitCode = 1;
  });
}
