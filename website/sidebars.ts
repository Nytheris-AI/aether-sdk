import type {SidebarsConfig} from '@docusaurus/plugin-content-docs';

const sidebars: SidebarsConfig = {
  docsSidebar: [
    'index',
    {
      type: 'category',
      label: 'Architecture',
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
      label: 'Guides',
      items: [
        'guides/usage',
        'guides/memory',
        'guides/observability',
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
