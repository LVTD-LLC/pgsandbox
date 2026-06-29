import type { APIRoute } from 'astro';
import { getPublishedBlogPosts } from '../lib/rowsetBlog';

const staticPages = [
  '/',
  '/docs/',
  '/docs/install/',
  '/docs/mcp-tools/',
  '/docs/architecture/',
  '/docs/homebrew/',
  '/blog/',
  '/changelog/'
];

async function getSitemapBlogPosts() {
  try {
    return await getPublishedBlogPosts();
  } catch (error) {
    console.warn(`Skipping Rowset blog posts in sitemap: ${(error as Error).message}`);
    return [];
  }
}

function sitemapLastmod(...values: Array<string | undefined>) {
  for (const value of values) {
    const trimmed = String(value || '').trim();
    if (!trimmed) {
      continue;
    }

    const normalized = trimmed.includes('T') ? trimmed : `${trimmed}T00:00:00.000Z`;
    const date = new Date(normalized);

    if (!Number.isNaN(date.valueOf())) {
      return date.toISOString().slice(0, 10);
    }
  }

  return undefined;
}

function escapeXml(value: string) {
  return value
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&apos;');
}

export const GET: APIRoute = async ({ site }) => {
  const baseUrl = site ?? new URL('https://pgsandbox-mcp.cap.gregagi.com');
  const posts = await getSitemapBlogPosts();
  const latestPost = posts[0];
  const latestLastmod = sitemapLastmod(latestPost?.updatedAt, latestPost?.publishedAt);
  const pages = [
    ...staticPages.map((path) => ({ path, lastmod: latestLastmod })),
    ...posts.map((post) => ({
      path: post.canonicalUrl || `/blog/${post.slug}/`,
      lastmod: sitemapLastmod(post.updatedAt, post.publishedAt)
    }))
  ];

  const urls = pages
    .map(({ path, lastmod }) => {
      const location = escapeXml(new URL(path, baseUrl).toString());
      const lastmodTag = lastmod ? `\n    <lastmod>${escapeXml(lastmod)}</lastmod>` : '';

      return `  <url>\n    <loc>${location}</loc>${lastmodTag}\n  </url>`;
    })
    .join('\n');

  return new Response(`<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n${urls}\n</urlset>\n`, {
    headers: {
      'Content-Type': 'application/xml; charset=utf-8'
    }
  });
};
