import { mkdir, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { buildReleaseStatus } from './release-data.mjs';

const output = fileURLToPath(new URL('../src/data/release-status.generated.json', import.meta.url));
const headers = { Accept: 'application/vnd.github+json', 'User-Agent': 'aros-tools-docs-build' };
if (process.env.GITHUB_TOKEN) headers.Authorization = `Bearer ${process.env.GITHUB_TOKEN}`;

async function request(url, limit) {
  const response = await fetch(url, { headers, signal: AbortSignal.timeout(30000) });
  if (!response.ok) throw new Error(`Release status fetch failed: HTTP ${response.status} (${url})`);
  const bytes = Buffer.from(await response.arrayBuffer());
  if (bytes.length > limit) throw new Error(`Release status response exceeds ${limit} bytes: ${url}`);
  return bytes;
}

const tools = JSON.parse((await request('https://api.github.com/repos/metaneutrons/aros-tools/releases/latest', 1048576)).toString());
const toolchains = JSON.parse((await request('https://api.github.com/repos/metaneutrons/aros-toolchains/releases/latest', 1048576)).toString());
const indexAsset = toolchains.assets?.find(asset => asset.name === 'toolchain-index-v1.json');
if (!indexAsset) throw new Error('Release status: toolchain index missing');
const expectedUrl = `https://github.com/metaneutrons/aros-toolchains/releases/download/${toolchains.tag_name}/toolchain-index-v1.json`;
if (indexAsset.browser_download_url !== expectedUrl) throw new Error('Release status: unexpected index URL');
const index = await request(expectedUrl, 1048576);
const data = buildReleaseStatus(tools, toolchains, index);
await mkdir(fileURLToPath(new URL('../src/data/', import.meta.url)), { recursive: true });
await writeFile(output, `${JSON.stringify(data, null, 2)}\n`, { flag: 'w' });
console.log(`Release status: ${data.tools.tag} tools, ${data.toolchains.tag} toolchains`);
