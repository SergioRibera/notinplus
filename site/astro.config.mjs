// @ts-check
import { defineConfig } from 'astro/config';

export default defineConfig({
  site: 'https://sergioribera.rs',
  base: '/notinplus',
  outDir: './dist/notinplus',
  trailingSlash: 'always',
  i18n: {
    locales: ['en', 'es'],
    defaultLocale: 'en',
    routing: { prefixDefaultLocale: false },
  },
});
