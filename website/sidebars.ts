import type {SidebarsConfig} from '@docusaurus/plugin-content-docs';

const sidebars: SidebarsConfig = {
  docsSidebar: [
    'index',
    {
      type: 'category',
      label: 'Learning Series',
      items: [
        'series/01-quickstart-first-memory',
        'series/02-five-composable-levels',
        'series/03-production-storage-backends',
        'series/04-obsidian-vault-and-mcp',
        'series/05-team-git-auth-and-kv-warming',
      ],
    },
    {
      type: 'category',
      label: 'Concepts',
      items: [
        'concepts/index',
        'concepts/storage-and-consistency',
        'concepts/scopes-tiers-and-forgetting',
        'concepts/hybrid-recall-and-ppr',
        'concepts/vault-amu-and-kv-cache',
      ],
    },
    {
      type: 'category',
      label: 'How-To Guides',
      items: [
        'guides/obsidian-setup',
        'guides/usage',
        'guides/memory',
        'guides/observability',
      ],
    },
    {
      type: 'category',
      label: 'Developer Reference',
      items: [
        'reference/crates-and-apis',
        'reference/storage-backends-and-flags',
        'reference/aether-mcp-and-http-api',
      ],
    },
    {
      type: 'category',
      label: 'Architecture & ADRs',
      items: [
        'architecture/index',
        'architecture/connectors',
        'architecture/sdk-design',
        'architecture/hardening-design',
        'architecture/m1-vault-mcp-rationale',
        'architecture/change-overview',
      ],
    },
    {
      type: 'category',
      label: 'Development',
      items: ['development/guidelines'],
    },
    {
      type: 'category',
      label: 'About',
      items: ['about/roadmap'],
    },
  ],
};

export default sidebars;
