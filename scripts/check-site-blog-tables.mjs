import { readFileSync } from 'node:fs';

const html = readFileSync('site/dist/blog/database-branching-vs-postgres-sandboxes/index.html', 'utf8');
const requiredSnippets = [
  'class="markdown-table-scroll"',
  'role="region"',
  'aria-label="Scrollable table"',
  'tabindex="0"'
];

const missing = requiredSnippets.filter((snippet) => !html.includes(snippet));

if (missing.length > 0) {
  throw new Error(`Blog table accessibility wrapper is missing: ${missing.join(', ')}`);
}
