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

  url: 'https://muninn-hq.github.io',
  baseUrl: '/aether-sdk/',

  organizationName: 'muninn-hq',
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

  plugins: [
    [
      '@docusaurus/plugin-client-redirects',
      {
        redirects: [
          {from: '/docs/guides/obsidian-setup', to: '/docs/get-started/quickstart'},
          {from: '/docs/guides/obsidian-walkthrough', to: '/docs/guides/amc-walkthrough'},
          {from: '/docs/guides/usage', to: '/docs/sdk/usage'},
          {from: '/docs/guides/memory', to: '/docs/sdk/memory'},
          {from: '/docs/guides/observability', to: '/docs/sdk/observability'},
          {from: '/docs/reference/crates-and-apis', to: '/docs/sdk/crates-and-apis'},
          {from: '/docs/reference/storage-backends-and-flags', to: '/docs/sdk/storage-backends-and-flags'},
          {from: '/docs/reference/aether-mcp-and-http-api', to: '/docs/reference/aether-mcp'},
        ],
      },
    ],
  ],

  presets: [
    [
      'classic',
      {
        docs: {
          path: '../docs',
          routeBasePath: '/docs',
          sidebarPath: './sidebars.ts',
          editUrl: 'https://github.com/muninn-hq/aether-sdk/tree/main/',
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
          to: '/docs/get-started/quickstart',
          label: 'Get started',
          position: 'left',
        },
        {
          to: '/docs/guides/amc-walkthrough',
          label: 'Guides',
          position: 'left',
        },
        {
          to: '/docs/reference/aether-mcp',
          label: 'Reference',
          position: 'left',
        },
        {
          to: '/docs/sdk/usage',
          label: 'Rust SDK',
          position: 'left',
        },
        {
          to: '/docs/architecture',
          label: 'Architecture',
          position: 'left',
        },
        {
          href: 'https://github.com/muninn-hq/aether-sdk',
          label: 'GitHub',
          position: 'right',
        },
      ],
    },
    footer: {
      style: 'dark',
      links: [
        {
          title: 'Use AMC and aether-mcp',
          items: [
            {
              label: 'Quickstart',
              to: '/docs/get-started/quickstart',
            },
            {
              label: 'Guides',
              to: '/docs/guides/amc-walkthrough',
            },
            {
              label: 'Reference',
              to: '/docs/reference/aether-mcp',
            },
          ],
        },
        {
          title: 'Build with Rust',
          items: [
            {
              label: 'Learning Series',
              to: '/docs/series/01-quickstart-first-memory',
            },
            {
              label: 'Concepts',
              to: '/docs/concepts',
            },
            {
              label: 'Rust SDK guides',
              to: '/docs/sdk/usage',
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
              href: 'https://github.com/muninn-hq/aether-sdk',
            },
          ],
        },
      ],
      copyright: `Copyright © ${new Date().getFullYear()} muninn-hq. Built with Docusaurus.`,
    },
    prism: {
      theme: prismThemes.github,
      darkTheme: prismThemes.dracula,
      additionalLanguages: ['python', 'bash', 'toml', 'yaml', 'rust', 'json'],
    },
  } satisfies Preset.ThemeConfig,
};

export default config;
