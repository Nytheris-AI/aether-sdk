import type {SidebarsConfig} from '@docusaurus/plugin-content-docs';

const sidebars: SidebarsConfig = {
  docsSidebar: [
    'index',
    {
      type: 'category',
      label: 'Architecture',
      items: [
        'architecture/index',
        'architecture/cortex',
        'architecture/protocols',
        'architecture/rust-migration',
      ],
    },
    {
      type: 'category',
      label: 'Guides',
      items: [
        'guides/usage',
        'guides/memory',
        'guides/resources',
        'guides/adapters',
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
