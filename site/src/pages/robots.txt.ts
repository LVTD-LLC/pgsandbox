import type { APIRoute } from 'astro';

export const GET: APIRoute = ({ site }) => {
  const baseUrl = site ?? new URL('https://pgsandbox-mcp.lvtd.dev');

  return new Response(`User-agent: *\nAllow: /\nSitemap: ${new URL('/sitemap.xml', baseUrl)}\n`, {
    headers: {
      'Content-Type': 'text/plain; charset=utf-8'
    }
  });
};
