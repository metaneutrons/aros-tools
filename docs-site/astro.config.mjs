import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

export default defineConfig({
  site: 'https://aros.metaneutrons.cc',
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
            { label: 'Prerequisites', slug: 'getting-started/prerequisites' },
            { label: 'Installation', slug: 'getting-started/installation' },
            { label: 'First checkout and build', slug: 'getting-started/quick-start' },
            { label: 'Update and uninstall', slug: 'getting-started/update-uninstall' },
          ],
        },
        {
          label: 'Workflows',
          items: [
            { label: 'Pristine upstream AROS', slug: 'workflows/upstream-aros' },
            { label: 'AROS-NX', slug: 'workflows/aros-nx' },
            { label: 'Cross-development', slug: 'workflows/cross-development' },
            { label: 'Physical boards', slug: 'workflows/boards' },
          ],
        },
        {
          label: 'Reference',
          items: [
            { label: 'Command reference', slug: 'reference/cli' },
            { label: 'Configuration', slug: 'reference/configuration' },
            { label: 'Platform support', slug: 'reference/platform-support' },
            { label: 'Troubleshooting', slug: 'reference/troubleshooting' },
            { label: 'Architecture', slug: 'reference/architecture' },
            { label: 'Diagnostics and logs', slug: 'reference/diagnostics' },
            { label: 'Release engineering', slug: 'reference/releases' },
            { label: 'Package publication', slug: 'reference/publication' },
            { label: 'Release status', slug: 'reference/release-status' },
          ],
        },
      ],
    }),
  ],
});
