import { createMarkdownProcessor } from '@astrojs/markdown-remark';

const DEFAULT_ROWSET_API_BASE = 'https://rowset.lvtd.dev/api';
const DEFAULT_BLOG_DATASET_KEY = '1e629b1a-89e5-4c56-8772-5c6ae5784753';
const BLOG_SLUG_PATTERN = /^(?=.{1,120}$)[a-z0-9]+(?:-[a-z0-9]+)*$/;

type RowsetRowsResponse = {
  rows?: RowsetRow[];
};

type RowsetRow = {
  id: number;
  row_number: number;
  index_value: string;
  data: Record<string, string>;
};

type HastNode = {
  type?: string;
  tagName?: string;
  properties?: Record<string, unknown>;
  children?: HastNode[];
};

export type BlogPost = {
  slug: string;
  title: string;
  excerpt: string;
  bodyMarkdown: string;
  bodyHtml: string;
  author: string;
  publishedAt: string;
  updatedAt: string;
  tags: string[];
  category: string;
  metaTitle: string;
  metaDescription: string;
  canonicalUrl: string;
  heroImageUrl: string;
  featured: boolean;
  sortOrder: number;
};

function isElement(node: HastNode, tagName: string): boolean {
  return node.type === 'element' && node.tagName === tagName;
}

function findFirstElement(node: HastNode, tagName: string): HastNode | undefined {
  if (isElement(node, tagName)) {
    return node;
  }

  for (const child of node.children || []) {
    const match = findFirstElement(child, tagName);

    if (match) {
      return match;
    }
  }

  return undefined;
}

function tableColumnCount(table: HastNode): number {
  const firstRow = findFirstElement(table, 'tr');

  return (firstRow?.children || []).filter((child) => isElement(child, 'th') || isElement(child, 'td')).length;
}

function wrapMarkdownTables(node: HastNode): void {
  if (!Array.isArray(node.children)) {
    return;
  }

  node.children = node.children.map((child) => {
    wrapMarkdownTables(child);

    if (!isElement(child, 'table')) {
      return child;
    }

    const frameClasses = ['markdown-table-frame'];

    if (tableColumnCount(child) >= 3) {
      frameClasses.push('markdown-table-frame-wide');
    }

    return {
      type: 'element',
      tagName: 'figure',
      properties: { className: frameClasses },
      children: [
        {
          type: 'element',
          tagName: 'div',
          properties: {
            className: ['markdown-table-scroll'],
            role: 'region',
            ariaLabel: 'Scrollable table',
            tabIndex: 0
          },
          children: [child]
        }
      ]
    };
  });
}

function rehypeResponsiveTables() {
  return function transform(tree: HastNode): void {
    wrapMarkdownTables(tree);
  };
}

const markdownProcessor = createMarkdownProcessor({
  syntaxHighlight: false,
  rehypePlugins: [rehypeResponsiveTables],
  remarkRehype: {
    allowDangerousHtml: false
  }
});

function envValue(name: string): string {
  return String(import.meta.env[name] || '').trim();
}

function rowsetConfig() {
  const apiKey = envValue('ROWSET_API_KEY');
  const datasetKey = envValue('PGSANDBOX_BLOG_ROWSET_DATASET_KEY') || DEFAULT_BLOG_DATASET_KEY;
  const apiBase = envValue('PGSANDBOX_BLOG_ROWSET_API_BASE') || DEFAULT_ROWSET_API_BASE;

  return { apiBase: apiBase.replace(/\/+$/, ''), apiKey, datasetKey };
}

function value(row: RowsetRow, key: string): string {
  return String(row.data[key] || '').trim();
}

function booleanValue(input: string): boolean {
  return ['true', '1', 'yes', 'y'].includes(input.toLowerCase());
}

function numberValue(input: string): number {
  const value = Number(input);
  return Number.isFinite(value) ? value : 0;
}

function tagsValue(input: string): string[] {
  return input
    .split(',')
    .map((tag) => tag.trim())
    .filter(Boolean);
}

function isBlogSlug(input: string): boolean {
  return BLOG_SLUG_PATTERN.test(input);
}

function comparePosts(a: BlogPost, b: BlogPost): number {
  const dateCompare = b.publishedAt.localeCompare(a.publishedAt);
  if (dateCompare !== 0) {
    return dateCompare;
  }

  return a.sortOrder - b.sortOrder;
}

function uniquePostsBySlug(posts: BlogPost[]): BlogPost[] {
  const seen = new Set<string>();

  return posts.filter((post) => {
    if (seen.has(post.slug)) {
      return false;
    }

    seen.add(post.slug);
    return true;
  });
}

async function rowToPost(row: RowsetRow): Promise<BlogPost | null> {
  const slug = value(row, 'slug') || row.index_value;
  const title = value(row, 'title');
  const bodyMarkdown = value(row, 'body_markdown');

  if (!isBlogSlug(slug) || !title || !bodyMarkdown || value(row, 'status') !== 'published') {
    return null;
  }

  const rendered = await (await markdownProcessor).render(bodyMarkdown);
  const excerpt = value(row, 'excerpt');

  return {
    slug,
    title,
    excerpt,
    bodyMarkdown,
    bodyHtml: rendered.code,
    author: value(row, 'author') || 'PGSandbox Team',
    publishedAt: value(row, 'published_at'),
    updatedAt: value(row, 'updated_at'),
    tags: tagsValue(value(row, 'tags')),
    category: value(row, 'category'),
    metaTitle: value(row, 'meta_title') || title,
    metaDescription: value(row, 'meta_description') || excerpt,
    canonicalUrl: value(row, 'canonical_url'),
    heroImageUrl: value(row, 'hero_image_url'),
    featured: booleanValue(value(row, 'featured')),
    sortOrder: numberValue(value(row, 'sort_order'))
  };
}

export async function getPublishedBlogPosts(): Promise<BlogPost[]> {
  const { apiBase, apiKey, datasetKey } = rowsetConfig();

  if (!apiKey || !datasetKey) {
    return [];
  }

  const url = new URL(`${apiBase}/datasets/${encodeURIComponent(datasetKey)}/rows`);
  url.searchParams.set('limit', '1000');

  const response = await fetch(url, {
    headers: {
      Accept: 'application/json',
      Authorization: `Bearer ${apiKey}`
    }
  });

  if (!response.ok) {
    throw new Error(`Rowset blog fetch failed with HTTP ${response.status}`);
  }

  const payload = (await response.json()) as RowsetRowsResponse;
  const posts = await Promise.all((payload.rows || []).map(rowToPost));

  return uniquePostsBySlug(posts.filter((post): post is BlogPost => post !== null).sort(comparePosts));
}

export async function getPublishedBlogPost(slug: string): Promise<BlogPost | undefined> {
  const posts = await getPublishedBlogPosts();
  return posts.find((post) => post.slug === slug);
}
