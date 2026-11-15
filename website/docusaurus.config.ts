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
          routeBasePath: '/',
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
      items: [
        {
          type: 'docSidebar',
          sidebarId: 'docsSidebar',
          position: 'left',
          label: 'Docs',
        },
        {
          to: '/guides/usage',
          label: 'Guides',
          position: 'left',
        },
        {
          to: '/architecture',
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
          title: 'Documentation',
          items: [
            {
              label: 'Home',
              to: '/',
            },
            {
              label: 'Architecture',
              to: '/architecture',
            },
            {
              label: 'Usage Guide',
              to: '/guides/usage',
            },
          ],
        },
        {
          title: 'Project',
          items: [
            {
              label: 'Roadmap',
              to: '/about/roadmap',
            },
            {
              label: 'Development Guidelines',
              to: '/development/guidelines',
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
      additionalLanguages: ['python', 'bash', 'toml', 'yaml'],
    },
  } satisfies Preset.ThemeConfig,
};

export default config;
