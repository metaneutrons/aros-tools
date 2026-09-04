import { rm } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const documentationRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  '..',
);

try {
  await Promise.all(
    ['worker-dist', '.wrangler-output'].map((entry) =>
      rm(path.join(documentationRoot, entry), { recursive: true, force: true }),
    ),
  );
} catch (error) {
  console.error(`error: ${error instanceof Error ? error.message : String(error)}`);
  process.exitCode = 1;
}
