export type SectionId = 'architecture' | 'planes' | 'stack' | 'inspector' | 'access';

export type StackFilter = 'all' | 'vector' | 'edge-graph';

export type StorageCategory = Exclude<StackFilter, 'all'>;

export type ProbeId = 'arch' | 'exec' | 'support';

export type InspectorTab = 'sdk' | 'payload';

export type QuickstartTab = 'clone' | 'cargo';

export type CodeTokenKind = 'plain' | 'kw' | 'cm';

export interface CodeSegment {
  readonly kind: CodeTokenKind;
  readonly text: string;
}

export interface NavSectionItem {
  readonly id: SectionId;
  readonly label: string;
}

export interface MetaPair {
  readonly label: string;
  readonly value: string;
}

export interface ConvergencePhase {
  readonly id: 0 | 1 | 2;
  readonly num: string;
  readonly name: string;
  readonly desc: string;
  readonly caption: string;
}

export interface MemoryPlane {
  readonly id: 'episodic' | 'semantic' | 'procedural';
  readonly tag: string;
  readonly title: string;
  readonly body: string;
  readonly specs: readonly [MetaPair, MetaPair];
}

export interface StorageIntegration {
  readonly id: string;
  readonly name: string;
  readonly kind: string;
  readonly category: StorageCategory;
  readonly span4?: boolean;
  readonly logoSrc: string;
  readonly fallbackSrcs: readonly string[];
  readonly shortCode: string;
  readonly alt: string;
  readonly description: string;
  readonly metaLabel: string;
  readonly metaValue: string;
}

export interface StackFilterOption {
  readonly id: StackFilter;
  readonly label: string;
}

export interface RecallNode {
  readonly fact: string;
  readonly score: string;
  readonly pct: number;
}

export interface SandboxProbe {
  readonly id: ProbeId;
  readonly title: string;
  readonly tag: string;
  readonly sdkSegments: readonly CodeSegment[];
  readonly payloadSegments: readonly CodeSegment[];
  readonly nodes: readonly RecallNode[];
}

export interface QuickstartOption {
  readonly id: QuickstartTab;
  readonly label: string;
  readonly command: string;
}

export interface DbCardLayoutFlags {
  readonly noRightBorder: boolean;
  readonly noBottomBorder: boolean;
  readonly tabletNoRight: boolean;
  readonly tabletFullLast: boolean;
  readonly tabletNoBottom: boolean;
}

export interface ConvergenceState {
  readonly phaseIdx: 0 | 1 | 2;
  readonly inRatio: number;
  readonly outRatio: number;
}

export const GITHUB_REPO_URL = 'https://github.com/Nytheris-AI/aether-sdk';

export const GLB_MODEL_URL =
  'https://cdn.jsdelivr.net/gh/KhronosGroup/glTF-Sample-Assets@main/Models/DamagedHelmet/glTF-Binary/DamagedHelmet.glb';

export const NAV_SECTIONS: readonly NavSectionItem[] = [
  {id: 'architecture', label: '01 // Convergence'},
  {id: 'planes', label: '02 // Memory Planes'},
  {id: 'stack', label: '03 // Tech Stack'},
  {id: 'inspector', label: '04 // Protocol Sandbox'},
  {id: 'access', label: '05 // Open Source'},
];

export const HERO_META_ITEMS: readonly MetaPair[] = [
  {label: 'RUNTIME', value: 'NATIVE RUST'},
  {label: 'VECTOR STORE', value: 'OPEN-SOURCE ANN'},
  {label: 'GRAPH STORE', value: 'PROPERTY GRAPH'},
  {label: 'LICENSE', value: 'APACHE-2.0'},
];

export const HERO_DIAGRAM_STATS: readonly MetaPair[] = [
  {label: 'ENGINE', value: 'NATIVE RUST'},
  {label: 'BACKENDS', value: 'VECTOR + GRAPH'},
  {label: 'ISOLATION', value: 'SCOPED TENANT'},
];

export const TICKER_ITEMS: readonly string[] = [
  'NATIVE RUST CORE',
  'OPEN-SOURCE VECTOR DATABASES',
  'OPEN-SOURCE GRAPH DATABASES',
  'EPISODIC TIMELINES',
  'SEMANTIC ENTITY GRAPHS',
  'PROCEDURAL DIRECTIVES',
];

export const CONVERGENCE_PHASES: readonly ConvergencePhase[] = [
  {
    id: 0,
    num: 'STAGE 01 // INGEST',
    name: 'Multi-Surface Capture',
    desc: 'Conversations, tool calls, documents, and agent actions stream into a single gRPC/HTTP write endpoint.',
    caption: 'STAGE 01 // MULTI-SURFACE CAPTURE',
  },
  {
    id: 1,
    num: 'STAGE 02 // CONSOLIDATE',
    name: 'Entity & Temporal Synthesis',
    desc: 'Aether extracts durable facts, deduplicates redundant signals, and updates vector and graph indexes atomically.',
    caption: 'STAGE 02 // ENTITY & TEMPORAL SYNTHESIS',
  },
  {
    id: 2,
    num: 'STAGE 03 // RECALL',
    name: 'Sub-Graph Context Assembly',
    desc: 'Any downstream model queries the exact slice of memory relevant to the active task.',
    caption: 'STAGE 03 // SUB-GRAPH CONTEXT ASSEMBLY',
  },
];

export const MEMORY_PLANES: readonly MemoryPlane[] = [
  {
    id: 'episodic',
    tag: 'PLANE 01 // TEMPORAL',
    title: 'Episodic Memory',
    body: 'Chronological event traces capture what happened, when, and in which session, preserving causal sequences and evolving project history.',
    specs: [
      {label: 'STRUCTURE', value: 'TIME-ORDERED LOG'},
      {label: 'VERSIONING', value: 'IMMUTABLE LINEAGE'},
    ],
  },
  {
    id: 'semantic',
    tag: 'PLANE 02 // RELATIONAL',
    title: 'Semantic Knowledge Graph',
    body: 'Entities, relationships, and domain facts are distilled into a live property graph that resolves contradictions when facts change.',
    specs: [
      {label: 'STRUCTURE', value: 'TYPED ENTITY EDGES'},
      {label: 'CONFLICTS', value: 'AUTO-SUPERSEDED'},
    ],
  },
  {
    id: 'procedural',
    tag: 'PLANE 03 // BEHAVIORAL',
    title: 'Procedural Rules',
    body: 'Learned workflows, formatting standards, and user constraints stay attached to every agent invocation without repeating system instructions.',
    specs: [
      {label: 'STRUCTURE', value: 'SCOPED DIRECTIVES'},
      {label: 'SCOPE', value: 'USER / TEAM / ORG'},
    ],
  },
];

export const RUST_CORE_SPECS: readonly MetaPair[] = [
  {label: 'MEMORY MODEL', value: 'ZERO-GC OWNERSHIP'},
  {label: 'CONCURRENCY', value: 'TOKIO ASYNC + WORK-STEALING'},
  {label: 'WIRE PROTOCOL', value: 'TONIC gRPC · AXUM REST'},
];

export const STACK_FILTER_OPTIONS: readonly StackFilterOption[] = [
  {id: 'all', label: 'All Integrations (7)'},
  {id: 'vector', label: 'Vector & Hybrid Search (4)'},
  {id: 'edge-graph', label: 'Embedded, Cache & Graph (3)'},
];

export const STORAGE_INTEGRATIONS: readonly StorageIntegration[] = [
  {
    id: 'qdrant',
    name: 'Qdrant',
    kind: 'Vector DB',
    category: 'vector',
    logoSrc: 'https://cdn.jsdelivr.net/gh/homarr-labs/dashboard-icons/svg/qdrant.svg',
    fallbackSrcs: [
      'https://cdn.jsdelivr.net/npm/@thesvg/static/icons/qdrant/default.svg',
      'https://cdn.jsdelivr.net/gh/glincker/thesvg@main/public/icons/qdrant/default.svg',
    ],
    shortCode: 'QD',
    alt: 'Qdrant brand logo',
    description:
      'Rust-native open-source HNSW vector search engine with single-stage payload filtering, scalar/binary quantization, and tenant-keyed collections.',
    metaLabel: 'LICENSE',
    metaValue: 'APACHE 2.0',
  },
  {
    id: 'chroma',
    name: 'Chroma',
    kind: 'Embedding DB',
    category: 'vector',
    logoSrc: 'https://cdn.jsdelivr.net/gh/homarr-labs/dashboard-icons/svg/chroma.svg',
    fallbackSrcs: ['https://cdn.jsdelivr.net/npm/@thesvg/static/icons/chroma/default.svg'],
    shortCode: 'CH',
    alt: 'Chroma brand logo',
    description:
      'Open-source AI-native embedding store combining dense and sparse vector similarity, BM25 full-text, regex, and metadata filtering across local or server setups.',
    metaLabel: 'LICENSE',
    metaValue: 'APACHE 2.0',
  },
  {
    id: 'turbopuffer',
    name: 'turbopuffer',
    kind: 'Object Vector + BM25',
    category: 'vector',
    logoSrc: 'https://www.gstatic.com/bricks/image/eefae7dd-9fe6-4789-b499-d0a8b3111aa8.png',
    fallbackSrcs: [
      'https://cdn.jsdelivr.net/npm/@thesvg/static/icons/turbopuffer/default.svg',
      'https://cdn.jsdelivr.net/gh/cmu-db/dbdb.io@master/dbdb/media/logos/turbopuffer.svg',
      'https://cdn.jsdelivr.net/gh/homarr-labs/dashboard-icons/svg/turbopuffer.svg',
    ],
    shortCode: '<(°O°)>',
    alt: 'turbopuffer brand logo',
    description:
      'Object-storage-native vector and BM25 search engine tiered across S3, NVMe SSD, and RAM cache, integrated via open-source Rust bindings for massive multi-tenant namespaces.',
    metaLabel: 'STORAGE TIER',
    metaValue: 'S3 + NVMe / RAM CACHE',
  },
  {
    id: 'pgvector',
    name: 'pgvector',
    kind: 'Postgres Vector',
    category: 'vector',
    logoSrc: 'https://cdn.jsdelivr.net/gh/devicons/devicon@latest/icons/postgresql/postgresql-original.svg',
    fallbackSrcs: ['https://cdn.jsdelivr.net/npm/@thesvg/static/icons/postgresql-badge/default.svg'],
    shortCode: 'PG',
    alt: 'PostgreSQL pgvector brand logo',
    description:
      'Open-source PostgreSQL extension adding exact and approximate HNSW / IVFFlat kNN search directly alongside ACID relational schemas and SQL joins.',
    metaLabel: 'LICENSE',
    metaValue: 'POSTGRESQL OSS',
  },
  {
    id: 'sqlite',
    name: 'SQLite · sqlite-vec',
    kind: 'Embedded Memory',
    category: 'edge-graph',
    span4: true,
    logoSrc: 'https://cdn.jsdelivr.net/gh/devicons/devicon@latest/icons/sqlite/sqlite-original.svg',
    fallbackSrcs: ['https://cdn.jsdelivr.net/npm/@thesvg/static/icons/sqlite/default.svg'],
    shortCode: 'SQ',
    alt: 'SQLite brand logo',
    description:
      'Zero-server single-file local memory combining SQLite WAL transactions, FTS5 lexical indexing, and SIMD-accelerated embedded vector search for desktop and edge agents.',
    metaLabel: 'LICENSE',
    metaValue: 'PUBLIC DOMAIN / MIT',
  },
  {
    id: 'redis',
    name: 'Redis',
    kind: 'In-Memory Store',
    category: 'edge-graph',
    span4: true,
    logoSrc: 'https://cdn.jsdelivr.net/gh/devicons/devicon@latest/icons/redis/redis-original.svg',
    fallbackSrcs: ['https://cdn.jsdelivr.net/npm/@thesvg/static/icons/redis/default.svg'],
    shortCode: 'RD',
    alt: 'Redis brand logo',
    description:
      'Sub-millisecond working memory, short-term session buffers, TTL eviction, and real-time HNSW vector indexing for high-concurrency agent loops.',
    metaLabel: 'ROLE',
    metaValue: 'HOT CACHE + VECTOR',
  },
  {
    id: 'neo4j',
    name: 'Neo4j',
    kind: 'Graph DB',
    category: 'edge-graph',
    span4: true,
    logoSrc: 'https://cdn.jsdelivr.net/gh/devicons/devicon@latest/icons/neo4j/neo4j-original.svg',
    fallbackSrcs: ['https://cdn.jsdelivr.net/npm/@thesvg/static/icons/neo4j/default.svg'],
    shortCode: 'N4',
    alt: 'Neo4j brand logo',
    description:
      'Native property-graph engine executing Cypher multi-hop entity traversal, contradiction resolution, and GraphRAG lineage queries across semantic memory.',
    metaLabel: 'LICENSE',
    metaValue: 'GPLV3 COMMUNITY',
  },
];

export const SANDBOX_PROBES: Record<ProbeId, SandboxProbe> = {
  arch: {
    id: 'arch',
    title: 'Prepare migration brief for service auth',
    tag: 'ENGINEERING',
    sdkSegments: [
      {kind: 'cm', text: '// Illustrative Rust SDK call — hybrid vector + graph recall\n'},
      {kind: 'kw', text: 'use'},
      {kind: 'plain', text: ' nytheris_aether_sdk::{AetherClient, Plane, RecallRequest};\n\n'},
      {kind: 'kw', text: 'let'},
      {kind: 'plain', text: ' slice = client.recall(RecallRequest {\n'},
      {kind: 'plain', text: '    namespace: "org_core_platform".into(),\n'},
      {kind: 'plain', text: '    query: "Prepare migration brief for service auth".into(),\n'},
      {kind: 'plain', text: '    planes: '},
      {kind: 'kw', text: 'vec!'},
      {kind: 'plain', text: '[Plane::Episodic, Plane::Semantic, Plane::Procedural],\n'},
      {kind: 'plain', text: '}).'},
      {kind: 'kw', text: 'await'},
      {kind: 'plain', text: '?;'},
    ],
    payloadSegments: [
      {kind: 'cm', text: '// Sample assembled subgraph slice (Illustrative Simulation)\n'},
      {
        kind: 'plain',
        text: `{\n  "namespace": "org_core_platform",\n  "backends_queried": ["qdrant_hnsw", "neo4j_cypher", "pgvector"],\n  "superseded_conflicts_filtered": 2,\n  "token_budget_used": 410\n}`,
      },
    ],
    nodes: [
      {
        fact: '[SEMANTIC · NEO4J] Auth tokens migrated from HS256 to EdDSA asymmetric signing in staging.',
        score: '0.97',
        pct: 97,
      },
      {
        fact: '[EPISODIC · QDRANT] Code Agent flagged legacy refresh-cookie fallback in gateway-proxy (Session #482).',
        score: '0.91',
        pct: 91,
      },
      {
        fact: '[PROCEDURAL · PGVECTOR] Architecture briefs require rollback checklist and zero-downtime phase gates.',
        score: '0.88',
        pct: 88,
      },
    ],
  },
  exec: {
    id: 'exec',
    title: 'Draft weekly status update for leadership',
    tag: 'OPERATIONS',
    sdkSegments: [
      {kind: 'cm', text: '// Illustrative TypeScript SDK — query Rust gRPC memory gateway\n'},
      {kind: 'kw', text: 'const'},
      {kind: 'plain', text: ' context = '},
      {kind: 'kw', text: 'await'},
      {
        kind: 'plain',
        text: ` aether.recall({\n  namespace: 'team_product_ops',\n  query: 'Draft weekly status update for leadership',\n  temporalWindow: '7d'\n});`,
      },
    ],
    payloadSegments: [
      {kind: 'cm', text: '// Sample assembled subgraph slice (Illustrative Simulation)\n'},
      {
        kind: 'plain',
        text: `{\n  "namespace": "team_product_ops",\n  "backends_queried": ["turbopuffer_bm25_ann", "redis_session", "neo4j_cypher"],\n  "consolidated_events": 14,\n  "token_budget_used": 365\n}`,
      },
    ],
    nodes: [
      {
        fact: '[EPISODIC · TURBOPUFFER] Release milestone M3 cleared QA sign-off after schema index patch.',
        score: '0.95',
        pct: 95,
      },
      {
        fact: '[SEMANTIC · NEO4J] Primary Q2 blocker shifted from ingestion throughput to enterprise SSO review.',
        score: '0.90',
        pct: 90,
      },
      {
        fact: '[WORKING · REDIS] Leadership summaries use concise bullet deltas: Shipped, Risks, Next 7 Days.',
        score: '0.86',
        pct: 86,
      },
    ],
  },
  support: {
    id: 'support',
    title: 'Diagnose recurring webhook timeout ticket',
    tag: 'SUPPORT AGENT',
    sdkSegments: [
      {kind: 'cm', text: '// Illustrative Rust SDK — local + hybrid cross-agent recall\n'},
      {kind: 'kw', text: 'let'},
      {kind: 'plain', text: ' slice = client.recall(RecallRequest {\n'},
      {kind: 'plain', text: '    namespace: "tenant_acme_sandbox".into(),\n'},
      {kind: 'plain', text: '    query: "Diagnose recurring webhook timeout ticket".into(),\n'},
      {kind: 'plain', text: '    include_lineage: '},
      {kind: 'kw', text: 'true'},
      {kind: 'plain', text: ',\n    ..Default::default()\n}).'},
      {kind: 'kw', text: 'await'},
      {kind: 'plain', text: '?;'},
    ],
    payloadSegments: [
      {kind: 'cm', text: '// Sample assembled subgraph slice (Illustrative Simulation)\n'},
      {
        kind: 'plain',
        text: `{\n  "namespace": "tenant_acme_sandbox",\n  "backends_queried": ["chroma_hybrid", "sqlite_vec_fts5", "pgvector_hnsw"],\n  "cross_agent_sources": ["voice_triage", "workflow_runner"],\n  "token_budget_used": 390\n}`,
      },
    ],
    nodes: [
      {
        fact: '[SEMANTIC · CHROMA] Webhook endpoint enforces 5-second TLS handshake ceiling on batch payloads.',
        score: '0.96',
        pct: 96,
      },
      {
        fact: '[EPISODIC · SQLITE] Edge Runner resolved identical timeout locally by enabling async queue ack mode.',
        score: '0.92',
        pct: 92,
      },
      {
        fact: '[PROCEDURAL · PGVECTOR] Provide copy-ready config diff before escalating to tier-3 on-call.',
        score: '0.84',
        pct: 84,
      },
    ],
  },
};

export const PROBE_ORDER: readonly ProbeId[] = ['arch', 'exec', 'support'];

export const QUICKSTART_SNIPPETS: Record<QuickstartTab, QuickstartOption> = {
  clone: {
    id: 'clone',
    label: 'GIT CLONE',
    command: `git clone https://github.com/Nytheris-AI/aether-sdk.git\ncd aether-sdk\ncargo build --release`,
  },
  cargo: {
    id: 'cargo',
    label: 'CARGO.TOML',
    command: `[dependencies]\nnytheris-aether-sdk = { git = "https://github.com/Nytheris-AI/aether-sdk.git" }`,
  },
};

export const QUICKSTART_ORDER: readonly QuickstartTab[] = ['clone', 'cargo'];

export function filterIntegrations(
  integrations: readonly StorageIntegration[],
  filter: StackFilter,
): readonly StorageIntegration[] {
  if (filter === 'all') {
    return integrations;
  }
  return integrations.filter((item) => item.category === filter);
}

export function getDbCardLayoutFlags(idx: number, count: number): DbCardLayoutFlags {
  let noRightBorder = false;
  let noBottomBorder = false;

  if (count === 7) {
    noRightBorder = idx === 3 || idx === 6;
    noBottomBorder = idx >= 4;
  } else if (count === 4) {
    noRightBorder = idx === 3;
    noBottomBorder = true;
  } else if (count === 3) {
    noRightBorder = idx === 2;
    noBottomBorder = true;
  }

  const isOddTotal = count % 2 === 1;
  const tabletFullLast = isOddTotal && idx === count - 1;
  const tabletNoRight = !tabletFullLast && idx % 2 === 1;
  const tabletNoBottom = !tabletFullLast && !isOddTotal && idx >= count - 2;

  return {
    noRightBorder,
    noBottomBorder,
    tabletNoRight,
    tabletFullLast,
    tabletNoBottom,
  };
}

export function computeConvergenceState(
  progress: number,
  forcedPhaseIndex?: 0 | 1 | 2,
): ConvergenceState {
  const clamped = Math.max(0, Math.min(1, progress));
  const phaseIdx: 0 | 1 | 2 =
    forcedPhaseIndex !== undefined
      ? forcedPhaseIndex
      : clamped < 0.36
        ? 0
        : clamped < 0.72
          ? 1
          : 2;

  const inRatio =
    forcedPhaseIndex !== undefined ? 1 : Math.min(1, clamped / 0.42);
  const outRatio =
    forcedPhaseIndex !== undefined
      ? forcedPhaseIndex >= 2
        ? 1
        : forcedPhaseIndex === 1
          ? 0.25
          : 0
      : Math.max(0, Math.min(1, (clamped - 0.52) / 0.42));

  return {phaseIdx, inRatio, outRatio};
}
