import { readdirSync, readFileSync, statSync } from 'node:fs';
import { extname, join, relative } from 'node:path';

const distDir = 'site/dist';
const productionOrigin = 'https://pgsandbox.lvtd.dev';
const legacyOrigin = 'https://pgsandbox-mcp.lvtd.dev';
const textExtensions = new Set(['.html', '.xml', '.txt']);

function walk(directory) {
  return readdirSync(directory)
    .flatMap((name) => {
      const path = join(directory, name);
      return statSync(path).isDirectory() ? walk(path) : [path];
    })
    .filter((path) => textExtensions.has(extname(path)));
}

const outputFiles = walk(distDir);
const output = outputFiles.map((path) => [path, readFileSync(path, 'utf8')]);
const legacyReferences = output
  .filter(([, content]) => content.includes(legacyOrigin))
  .map(([path]) => relative(distDir, path));

if (legacyReferences.length > 0) {
  throw new Error(`Built site still references ${legacyOrigin}: ${legacyReferences.join(', ')}`);
}

const htmlFiles = output.filter(([path]) => extname(path) === '.html');
const invalidCanonicals = [];

for (const [path, content] of htmlFiles) {
  const canonical = content.match(/<link rel="canonical" href="([^"]+)"/i)?.[1];
  if (!canonical || !canonical.startsWith(`${productionOrigin}/`)) {
    invalidCanonicals.push(`${relative(distDir, path)} (${canonical || 'missing'})`);
  }
}

if (invalidCanonicals.length > 0) {
  throw new Error(`Invalid production canonicals: ${invalidCanonicals.join(', ')}`);
}

const sitemap = readFileSync(join(distDir, 'sitemap.xml'), 'utf8');
const sitemapUrls = [...sitemap.matchAll(/<loc>([^<]+)<\/loc>/g)].map((match) => match[1]);

if (sitemapUrls.length === 0 || sitemapUrls.some((url) => !url.startsWith(`${productionOrigin}/`))) {
  throw new Error('Every sitemap URL must use the production origin');
}

const robots = readFileSync(join(distDir, 'robots.txt'), 'utf8');
if (!robots.includes(`Sitemap: ${productionOrigin}/sitemap.xml`)) {
  throw new Error('robots.txt must advertise the production sitemap URL');
}

console.log(`Verified ${htmlFiles.length} canonicals and ${sitemapUrls.length} sitemap URLs.`);
