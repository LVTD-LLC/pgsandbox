import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';

// Exercise the actual production nginx config, not Astro's preview server.
const image = `pgsandbox-site-routing:${process.pid}`;
const docker = (...args) => execFileSync('docker', args, { encoding: 'utf8' }).trim();
let container;
let imageBuilt = false;
try {
  docker('build', '-t', image, 'site');
  imageBuilt = true;
  container = docker('run', '--rm', '-d', '-p', '127.0.0.1::80', image);
  const port = docker('inspect', '--format', '{{(index (index .NetworkSettings.Ports "80/tcp") 0).HostPort}}', container);
  const base = `http://127.0.0.1:${port}`;
  let ready = false;
  for (let attempt = 0; attempt < 30; attempt++) {
    try {
      ready = (await fetch(base, { signal: AbortSignal.timeout(1000) })).ok;
      if (ready) break;
    } catch {}
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  assert.ok(ready, 'nginx did not become ready');
  for (const path of ['/not-a-real-page/', '/blog/not-a-real-post/', '/missing.js', '/404.html']) {
    const response = await fetch(base + path);
    assert.equal(response.status, 404, `${path} must return a real 404`);
    const body = await response.text();
    assert.match(body, /Page not found/);
    assert.match(body, /name="robots" content="noindex"/);
  }
  for (const path of ['/docs/install', '/docs/agent-workflows', '/blog']) {
    const response = await fetch(`${base}${path}?source=routing-check`, {
      redirect: 'manual', headers: { Host: 'pgsandbox.dev', 'X-Forwarded-Proto': 'https' }
    });
    assert.equal(response.status, 301);
    const location = response.headers.get('location');
    assert.ok(location);
    assert.equal(new URL(location, 'https://pgsandbox.dev').href,
      `https://pgsandbox.dev${path}/?source=routing-check`, 'redirect must preserve HTTPS, path, and query');
  }
  const guide = await fetch(`${base}/docs/agent-workflows/`);
  assert.equal(guide.status, 200);
  const html = await guide.text();
  assert.match(html, /Direct SQL Schema Change/);
  assert.match(html, /rel="canonical" href="https:\/\/pgsandbox.dev\/docs\/agent-workflows\/"/);
  const sitemap = await (await fetch(`${base}/sitemap.xml`)).text();
  assert.match(sitemap, /<loc>https:\/\/pgsandbox.dev\/docs\/agent-workflows\/<\/loc>/);
  assert.doesNotMatch(sitemap, /<loc>[^<]*404/);
  console.log('Production routing passed: real 404s, proxy-safe redirects, workflow guide, sitemap.');
} finally {
  if (container) docker('stop', container);
  if (imageBuilt) docker('image', 'rm', image);
}
