# PGSandbox MCP Site

The Astro site can render blog posts from Rowset at build time.

Set these environment variables in the deployment environment:

```bash
ROWSET_API_KEY=your-rowset-api-key
PGSANDBOX_BLOG_ROWSET_DATASET_KEY=1e629b1a-89e5-4c56-8772-5c6ae5784753
PGSANDBOX_BLOG_ROWSET_API_BASE=https://rowset.lvtd.dev/api
```

Only rows with `status=published` render. The `slug` column becomes the blog
route under `/blog/{slug}/`, and `body_markdown` is rendered as Markdown without
raw HTML.

Local builds without `ROWSET_API_KEY` still succeed and render an empty blog
index.
