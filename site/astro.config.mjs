import { unified } from '@astrojs/markdown-remark';
import { defineConfig } from 'astro/config';
import { rehypeResponsiveTables } from './src/lib/markdownTables.mjs';

export default defineConfig({
  site: 'https://pgsandbox.dev',
  markdown: {
    processor: unified({
      rehypePlugins: [rehypeResponsiveTables]
    })
  },
  output: 'static'
});
