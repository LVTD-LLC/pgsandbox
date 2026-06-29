import { defineCollection } from 'astro:content';
import { glob } from 'astro/loaders';
import { z } from 'astro/zod';

const blog = defineCollection({
  loader: glob({ pattern: '**/*.{md,mdx}', base: './src/content/blog' }),
  schema: z.object({
    title: z.string(),
    excerpt: z.string(),
    author: z.string().default('PGSandbox Team'),
    status: z.enum(['draft', 'published']).optional().default('published'),
    publishedAt: z.string(),
    updatedAt: z.string().optional().default(''),
    tags: z.array(z.string()).default([]),
    category: z.string().optional().default('Engineering'),
    metaTitle: z.string().optional().default(''),
    metaDescription: z.string().optional().default(''),
    canonicalUrl: z.string().optional().default(''),
    heroImageUrl: z.string().optional().default(''),
    featured: z.boolean().optional().default(false),
    sortOrder: z.number().optional().default(0)
  })
});

export const collections = { blog };
