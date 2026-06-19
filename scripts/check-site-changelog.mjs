import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const rootChangelogPath = resolve(repoRoot, 'CHANGELOG.md');
const siteChangelogPath = resolve(repoRoot, 'site', 'CHANGELOG.md');

const [rootChangelog, siteChangelog] = await Promise.all([
  readFile(rootChangelogPath, 'utf8'),
  readFile(siteChangelogPath, 'utf8')
]);

if (rootChangelog !== siteChangelog) {
  console.error('site/CHANGELOG.md must match CHANGELOG.md.');
  console.error('The site-local copy is used as the Docker build fallback when deploying from site/.');
  process.exit(1);
}
