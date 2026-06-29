import { getCollection, type CollectionEntry } from 'astro:content';

export const blogAuthor = {
  name: 'Rasul Kireev',
  url: 'https://rasulkireev.com',
  credit: 'Rasul Kireev with OpenAI Codex'
};

export type BlogPost = CollectionEntry<'blog'>;

export function getBlogSlug(post: BlogPost): string {
  return post.id.replace(/\.(md|mdx)$/i, '');
}

export function getBlogTitle(post: BlogPost): string {
  return post.data.metaTitle || post.data.title;
}

export function getBlogDescription(post: BlogPost): string {
  return post.data.metaDescription || post.data.excerpt;
}

export function getBlogCanonicalUrl(post: BlogPost, site: URL | undefined): string {
  return post.data.canonicalUrl || new URL(`/blog/${getBlogSlug(post)}/`, site).toString();
}

function comparePosts(a: BlogPost, b: BlogPost): number {
  const dateCompare = b.data.publishedAt.localeCompare(a.data.publishedAt);
  if (dateCompare !== 0) {
    return dateCompare;
  }

  return a.data.sortOrder - b.data.sortOrder;
}

export async function getPublishedBlogPosts(): Promise<BlogPost[]> {
  return (await getCollection('blog')).filter((post) => post.data.status === 'published').sort(comparePosts);
}
