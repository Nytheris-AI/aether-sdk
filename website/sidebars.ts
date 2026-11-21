import type {SidebarsConfig} from '@docusaurus/plugin-content-docs';

const sidebars: SidebarsConfig = {
  docsSidebar: [
    'index',
    {
      type: 'category',
      label: 'Get started',
      collapsed: false,
      items: [
        'get-started/quickstart',
        'get-started/install-aether-mcp',
        'get-started/install-amc',
        'get-started/connect-an-agent',
      ],
    },
    {
      type: 'category',
      label: 'Guides',
      collapsed: false,
      items: [
        'guides/amc-walkthrough',
        'guides/organize-and-protect-notes',
        'guides/review-agent-memories',
        'guides/team-and-remote-setup',
        'guides/troubleshooting',
      ],
    },
    {
      type: 'category',
      label: 'Reference',
      items: [
        'reference/aether-mcp',
        'reference/http-api',
        'reference/vault-files',
        'reference/amc-plugin',
      ],
    },
    {
      type: 'category',
      label: 'Rust SDK',
      items: [
        {
          type: 'category',
          label: 'Learning series',
          items: [
            'series/01-quickstart-first-memory',
            'series/02-five-composable-levels',
            'series/03-production-storage-backends',
            'series/04-obsidian-vault-and-mcp',
            'series/05-team-git-auth-and-kv-warming',
          ],
        },
        'sdk/usage',
        'sdk/memory',
        'sdk/observability',
        'sdk/crates-and-apis',
        'sdk/storage-backends-and-flags',
      ],
    },
    {
      type: 'category',
      label: 'Concepts',
      items: [
        'concepts/index',
        'concepts/vault-amu-and-kv-cache',
        'concepts/scopes-tiers-and-forgetting',
        'concepts/hybrid-recall-and-ppr',
        'concepts/storage-and-consistency',
      ],
    },
    {
      type: 'category',
      label: 'Architecture and ADRs',
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
      label: 'Project',
      items: [
        'about/roadmap',
        'development/guidelines',
        'development/obsidian-directory-submission',
        'development/releasing-the-npm-package',
      ],
    },
  ],
};

export default sidebars;
