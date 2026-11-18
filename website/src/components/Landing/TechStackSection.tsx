import {useState, type ReactNode} from 'react';
import {
  RUST_CORE_SPECS,
  STACK_FILTER_OPTIONS,
  STORAGE_INTEGRATIONS,
  filterIntegrations,
  getDbCardLayoutFlags,
  type StackFilter,
} from '../../data/landing';

interface LogoWithFallbackProps {
  readonly src: string;
  readonly fallbacks: readonly string[];
  readonly shortCode: string;
  readonly alt: string;
  readonly className?: string;
}

function LogoWithFallback({
  src,
  fallbacks,
  shortCode,
  alt,
  className = 'logo-frame',
}: LogoWithFallbackProps): ReactNode {
  const sources = [src, ...fallbacks];
  const [sourceIndex, setSourceIndex] = useState(0);
  const exhausted = sourceIndex >= sources.length;

  return (
    <div className={className}>
      {exhausted ? (
        <span className="logo-fallback-text" aria-label={alt}>
          {shortCode}
        </span>
      ) : (
        <img
          src={sources[sourceIndex]}
          alt={alt}
          loading="lazy"
          onError={() => setSourceIndex((prev) => prev + 1)}
        />
      )}
    </div>
  );
}

export function TechStackSection(): ReactNode {
  const [activeFilter, setActiveFilter] = useState<StackFilter>('all');
  const visibleIntegrations = filterIntegrations(STORAGE_INTEGRATIONS, activeFilter);

  return (
    <section className="band" id="stack">
      <div className="frame">
        <span className="crosshair tl" aria-hidden="true" />
        <span className="crosshair tr accent" aria-hidden="true" />

        <div className="section-head-grid">
          <div className="section-index-cell">
            <span className="mono-tag">
              <span className="dot" /> 03 // TECH STACK
            </span>
          </div>
          <div className="section-title-cell">
            <h2 className="section-h2 reveal-up">
              Built native on Rust. Pluggable across vector, embedded, cache &amp; graph backends.
            </h2>
            <p className="section-sub reveal-up" style={{'--d': '90ms'}}>
              Aether compiles to a single memory-safe Rust binary with zero garbage-collection
              pauses and integrates directly with SQLite, Redis, turbopuffer, Qdrant, Chroma,
              pgvector, and Neo4j.
            </p>
          </div>
        </div>

        <div className="rust-core-banner">
          <div className="rust-core-main reveal-up">
            <LogoWithFallback
              src="https://cdn.jsdelivr.net/gh/devicons/devicon@latest/icons/rust/rust-original.svg"
              fallbacks={['https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/rust.svg']}
              shortCode="RS"
              alt="Rust official logo"
              className="logo-frame rust-frame"
            />
            <div>
              <span className="mono-tag">CORE ENGINE // WRITTEN IN RUST</span>
              <h3 className="rust-core-title">Memory-safe async execution without runtime pauses</h3>
              <p className="rust-core-desc">
                Ingestion pipelines, entity consolidation workers, and hybrid RRF rankers execute
                natively on Rust&apos;s async runtime, routing episodic, semantic, and procedural
                queries across embedded files, hot memory caches, vector stores, and property
                graphs.
              </p>
            </div>
          </div>

          <div className="rust-specs-col">
            {RUST_CORE_SPECS.map((spec) => (
              <div key={spec.label} className="rust-spec-row">
                <span>{spec.label}</span>
                <span>{spec.value}</span>
              </div>
            ))}
          </div>
        </div>

        <div className="stack-filter-bar">
          <div className="stack-filter-tabs" role="tablist" aria-label="Filter storage integrations">
            {STACK_FILTER_OPTIONS.map((opt) => {
              const active = activeFilter === opt.id;
              return (
                <button
                  key={opt.id}
                  type="button"
                  className={active ? 'stack-filter-btn active' : 'stack-filter-btn'}
                  role="tab"
                  aria-selected={active}
                  onClick={() => setActiveFilter(opt.id)}>
                  {opt.label}
                </button>
              );
            })}
          </div>
          <div className="stack-filter-note">PLUGGABLE RUST STORAGE TRAITS</div>
        </div>

        <div className="oss-db-grid">
          {visibleIntegrations.map((item, idx) => {
            const flags = getDbCardLayoutFlags(idx, visibleIntegrations.length);
            const classes = [
              'oss-db-card',
              item.span4 ? 'span-4' : '',
              flags.noRightBorder ? 'no-right-border' : '',
              flags.noBottomBorder ? 'no-bottom-border' : '',
              flags.tabletNoRight ? 'tablet-no-right' : '',
              flags.tabletFullLast ? 'tablet-full-last' : '',
              flags.tabletNoBottom ? 'tablet-no-bottom' : '',
            ]
              .filter(Boolean)
              .join(' ');

            return (
              <article key={item.id} className={classes}>
                <div>
                  <div className="db-card-top">
                    <LogoWithFallback
                      src={item.logoSrc}
                      fallbacks={item.fallbackSrcs}
                      shortCode={item.shortCode}
                      alt={item.alt}
                    />
                    <span className="db-kind-pill">{item.kind}</span>
                  </div>
                  <h3 className="db-name">{item.name}</h3>
                  <p className="db-desc">{item.description}</p>
                </div>
                <div className="db-meta-row">
                  <span>{item.metaLabel}</span>
                  <span>{item.metaValue}</span>
                </div>
              </article>
            );
          })}
        </div>
      </div>
    </section>
  );
}
