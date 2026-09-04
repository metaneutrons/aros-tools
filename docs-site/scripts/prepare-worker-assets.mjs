import { cp, lstat, mkdir, readdir, rm } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const documentationRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  '..',
);
const sourceRoot = path.join(documentationRoot, 'dist');
const stagingRoot = path.join(documentationRoot, 'worker-dist');
const destinationRoot = path.join(stagingRoot, 'aros-tools');

async function inspectTree(root, label) {
  const rootStatus = await lstat(root).catch((error) => {
    if (error.code === 'ENOENT') {
      throw new Error(`${label} is missing: ${root}`);
    }
    throw error;
  });
  if (!rootStatus.isDirectory() || rootStatus.isSymbolicLink()) {
    throw new Error(`${label} is not a real directory: ${root}`);
  }

  let files = 0;
  const pending = [root];
  while (pending.length > 0) {
    const directory = pending.pop();
    const entries = await readdir(directory, { withFileTypes: true });
    for (const entry of entries) {
      const entryPath = path.join(directory, entry.name);
      if (entry.isSymbolicLink()) {
        throw new Error(`${label} contains a symbolic link: ${entryPath}`);
      }
      if (entry.isDirectory()) {
        pending.push(entryPath);
      } else if (entry.isFile()) {
        files += 1;
      } else {
        throw new Error(`${label} contains a non-regular entry: ${entryPath}`);
      }
    }
  }
  return files;
}

try {
  const sourceFiles = await inspectTree(sourceRoot, 'Astro output');
  if (sourceFiles === 0) {
    throw new Error('Astro output contains no files');
  }
  for (const required of ['index.html', '404.html']) {
    const requiredStatus = await lstat(path.join(sourceRoot, required)).catch(() => null);
    if (!requiredStatus?.isFile() || requiredStatus.isSymbolicLink()) {
      throw new Error(`Astro output lacks regular ${required}`);
    }
  }
  const retiredPagesDomain = await lstat(path.join(sourceRoot, 'CNAME')).catch(
    (error) => {
      if (error.code === 'ENOENT') {
        return null;
      }
      throw error;
    },
  );
  if (retiredPagesDomain !== null) {
    throw new Error('Astro output still contains the retired GitHub Pages CNAME');
  }

  await rm(stagingRoot, { recursive: true, force: true });
  await mkdir(stagingRoot, { recursive: true });
  await cp(sourceRoot, destinationRoot, {
    recursive: true,
    force: false,
    errorOnExist: true,
    dereference: false,
  });

  const stagedFiles = await inspectTree(stagingRoot, 'Worker asset staging');
  if (stagedFiles !== sourceFiles) {
    throw new Error(
      `Worker staging changed the file count: ${sourceFiles} -> ${stagedFiles}`,
    );
  }
  console.log(`Prepared ${stagedFiles} regular documentation assets below /aros-tools/.`);
} catch (error) {
  console.error(`error: ${error instanceof Error ? error.message : String(error)}`);
  process.exitCode = 1;
}
