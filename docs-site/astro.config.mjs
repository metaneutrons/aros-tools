import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

export default defineConfig({
  site: 'https://metaneutrons.github.io',
  base: '/aros-tools',
  integrations: [
    starlight({
      title: 'AROS tools',
      description: 'Reproducible host-side tools for upstream AROS and AROS-NX.',
      customCss: ['./src/styles/custom.css'],
      social: [
        {
          icon: 'github',
          label: 'GitHub',
          href: 'https://github.com/metaneutrons/aros-tools',
        },
      ],
      sidebar: [
        {
          label: 'Start here',
          items: [
            { label: 'Overview', slug: 'index' },
            { label: 'Installation', slug: 'getting-started/installation' },
          ],
        },
        {
          label: 'Workflows',
          items: [
            { label: 'Pristine upstream AROS', slug: 'workflows/upstream-aros' },
            { label: 'AROS-NX', slug: 'workflows/aros-nx' },
          ],
        },
        {
          label: 'Reference',
          items: [
            { label: 'Architecture', slug: 'reference/architecture' },
            { label: 'Diagnostics and logs', slug: 'reference/diagnostics' },
            { label: 'Release engineering', slug: 'reference/releases' },
            { label: 'Release status', slug: 'reference/release-status' },
          ],
        },
      ],
    }),
  ],
});
