import type {SidebarsConfig} from '@docusaurus/plugin-content-docs';

const sidebars: SidebarsConfig = {
  docsSidebar: [
    'index',
    {
      type: 'category',
      label: 'Architecture',
      items: [
        'architecture/index',
        'architecture/rust-migration',
        'architecture/connectors',
        'architecture/sdk-design',
        'architecture/hardening-design',
      ],
    },
    {
      type: 'category',
      label: 'Guides',
      items: [
        'guides/usage',
        'guides/memory',
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
