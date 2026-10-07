import { unified } from '@astrojs/markdown-remark';
import { defineConfig } from 'astro/config';
import posthog from '@posthog/rollup-plugin';
import { rehypeResponsiveTables } from './src/lib/markdownTables.mjs';

export default defineConfig({
  site: 'https://pgsandbox.dev',
  markdown: {
    processor: unified({
      rehypePlugins: [rehypeResponsiveTables]
    })
  },
  output: 'static',
  vite: {
    plugins: process.env.POSTHOG_SOURCEMAP_TOKEN ? [posthog({
      personalApiKey: process.env.POSTHOG_SOURCEMAP_TOKEN,
      projectId: '471530',
      host: 'https://us.posthog.com',
      sourcemaps: {
        enabled: true,
        releaseName: 'pgsandbox-site',
        releaseVersion: process.env.GITHUB_SHA || 'local-verification',
        deleteAfterUpload: true,
      },
    })] : [],
  }
});
