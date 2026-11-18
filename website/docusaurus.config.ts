import {themes as prismThemes} from 'prism-react-renderer';
import type {Config} from '@docusaurus/types';
import type * as Preset from '@docusaurus/preset-classic';

const config: Config = {
  title: 'Aether SDK',
  tagline: 'Production-ready AI Context Engineering SDK',
  favicon: 'img/favicon.ico',

  future: {
    v4: true,
  },

  url: 'https://nytheris-ai.github.io',
  baseUrl: '/aether-sdk/',

  organizationName: 'Nytheris-AI',
  projectName: 'aether-sdk',

  onBrokenLinks: 'throw',

  i18n: {
    defaultLocale: 'en',
    locales: ['en'],
  },

  markdown: {
    mermaid: true,
  },

  themes: ['@docusaurus/theme-mermaid'],

  presets: [
    [
      'classic',
      {
        docs: {
          path: '../docs',
          routeBasePath: '/docs',
          sidebarPath: './sidebars.ts',
          editUrl: 'https://github.com/Nytheris-AI/aether-sdk/tree/main/',
        },
        blog: false,
        theme: {
          customCss: './src/css/custom.css',
        },
      } satisfies Preset.Options,
    ],
  ],

  themeConfig: {
    colorMode: {
      respectPrefersColorScheme: true,
    },
    navbar: {
      title: 'Aether SDK',
      logo: {
        alt: 'Aether SDK Logo',
        src: 'img/aether-logo.svg',
      },
      items: [
        {
          type: 'docSidebar',
          sidebarId: 'docsSidebar',
          position: 'left',
          label: 'Docs',
        },
        {
          to: '/docs/series/01-quickstart-first-memory',
          label: 'Series',
          position: 'left',
        },
        {
          to: '/docs/concepts',
          label: 'Concepts',
          position: 'left',
        },
        {
          to: '/docs/guides/usage',
          label: 'Guides',
          position: 'left',
        },
        {
          to: '/docs/reference/crates-and-apis',
          label: 'Reference',
          position: 'left',
        },
        {
          to: '/docs/architecture',
          label: 'Architecture',
          position: 'left',
        },
        {
          href: 'https://github.com/Nytheris-AI/aether-sdk',
          label: 'GitHub',
          position: 'right',
        },
      ],
    },
    footer: {
      style: 'dark',
      links: [
        {
          title: 'Learn',
          items: [
            {
              label: 'Docs Home',
              to: '/docs',
            },
            {
              label: 'Learning Series',
              to: '/docs/series/01-quickstart-first-memory',
            },
            {
              label: 'Concepts',
              to: '/docs/concepts',
            },
          ],
        },
        {
          title: 'Build',
          items: [
            {
              label: 'How-To Guides',
              to: '/docs/guides/usage',
            },
            {
              label: 'Developer Reference',
              to: '/docs/reference/crates-and-apis',
            },
            {
              label: 'Architecture & ADRs',
              to: '/docs/architecture',
            },
          ],
        },
        {
          title: 'Project',
          items: [
            {
              label: 'Landing Page',
              to: '/',
            },
            {
              label: 'Roadmap',
              to: '/docs/about/roadmap',
            },
            {
              label: 'Development Guidelines',
              to: '/docs/development/guidelines',
            },
            {
              label: 'GitHub',
              href: 'https://github.com/Nytheris-AI/aether-sdk',
            },
          ],
        },
      ],
      copyright: `Copyright © ${new Date().getFullYear()} Nytheris-AI. Built with Docusaurus.`,
    },
    prism: {
      theme: prismThemes.github,
      darkTheme: prismThemes.dracula,
      additionalLanguages: ['python', 'bash', 'toml', 'yaml', 'rust', 'json'],
    },
  } satisfies Preset.ThemeConfig,
};

export default config;
