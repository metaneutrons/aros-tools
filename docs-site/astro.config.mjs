import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

export default defineConfig({
  site: 'https://aros.metaneutrons.cc',
  base: '/aros-tools',
  integrations: [
    starlight({
      title: 'AROS tools',
      description: 'Reproducible host-side tools for upstream AROS and AROS-NX.',
      disable404Route: true,
      customCss: ['./src/styles/custom.css'],
      editLink: { baseUrl: 'https://github.com/metaneutrons/aros-tools/edit/main/docs-site/' },
      components: {
        SiteTitle: './src/components/SiteTitle.astro',
        PageTitle: './src/components/PageTitle.astro',
        Footer: './src/components/Footer.astro',
      },
      social: [
        {
          icon: 'github',
          label: 'GitHub',
          href: 'https://github.com/metaneutrons/aros-tools',
        },
      ],
      sidebar: [
        {
          label: 'Get started',
          items: [
            { label: 'Overview', slug: 'index' },
            { label: 'How the pieces fit', slug: 'getting-started/concepts' },
            { label: 'Prerequisites', slug: 'getting-started/prerequisites' },
            { label: 'Installation', slug: 'getting-started/installation' },
            { label: 'First checkout and build', slug: 'getting-started/quick-start' },
            { label: 'Update and uninstall', slug: 'getting-started/update-uninstall' },
          ],
        },
        {
          label: 'Build and develop',
          items: [
            { label: 'Manage source checkouts', slug: 'workflows/source' },
            { label: 'Choose and verify a toolchain', slug: 'workflows/toolchains' },
            { label: 'Pristine upstream AROS', slug: 'workflows/upstream-aros' },
            { label: 'Build with AROS-NX', slug: 'workflows/aros-nx' },
            { label: 'Cross-development', slug: 'workflows/cross-development' },
            { label: 'Physical boards', slug: 'workflows/boards' },
          ],
        },
        {
          label: 'Reference',
          items: [
            { label: 'Command reference', slug: 'reference/cli' },
            { label: 'Standalone tools', slug: 'reference/standalone-tools' },
            { label: 'Configuration', slug: 'reference/configuration' },
            { label: 'Platform support', slug: 'reference/platform-support' },
            { label: 'Diagnostics and logs', slug: 'reference/diagnostics' },
          ],
        },
        {
          label: 'Help and releases',
          items: [
            { label: 'Troubleshooting', slug: 'reference/troubleshooting' },
            { label: 'Release status', slug: 'reference/release-status' },
            { label: 'Versions and verification', slug: 'reference/releases' },
            { label: 'Package channels', slug: 'reference/publication' },
          ],
        },
        {
          label: 'Contribute',
          collapsed: true,
          items: [
            { label: 'Development workflow', slug: 'contributing/development' },
            { label: 'Architecture', slug: 'reference/architecture' },
            { label: 'Writing documentation', slug: 'contributing/documentation' },
          ],
        },
      ],
    }),
  ],
});
