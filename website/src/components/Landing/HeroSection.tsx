import type {ReactNode} from 'react';
import Link from '@docusaurus/Link';
import {GITHUB_REPO_URL, HERO_META_ITEMS} from '../../data/landing';
import {MemoryVaultVisual} from './MemoryVaultVisual';

export function HeroSection(): ReactNode {
  return (
    <section className="band in-view" id="hero">
      <div className="frame">
        <span className="crosshair tl accent" aria-hidden="true" />
        <span className="crosshair tr" aria-hidden="true" />
        <span className="crosshair bl" aria-hidden="true" />
        <span className="crosshair br accent" aria-hidden="true" />

        <div className="hero-meta-strip">
          {HERO_META_ITEMS.map((item) => (
            <div key={item.label} className="hero-meta-cell">
              <span>{item.label}</span>
              <span>{item.value}</span>
            </div>
          ))}
        </div>

        <div className="hero-grid">
          <div className="hero-copy">
            <div>
              <div className="mono-tag reveal-up" style={{['--d' as string]: '40ms'}}>
                <span className="dot" />
                <span>OPEN-SOURCE RUST MEMORY LAYER FOR MULTI-MODEL AI</span>
              </div>

              <h1 className="hero-title reveal-up" style={{['--d' as string]: '120ms'}}>
                One shared <span className="accent-word">memory</span> across every agent, model,
                and session.
              </h1>

              <p className="hero-lead reveal-up" style={{['--d' as string]: '200ms'}}>
                Stateless prompts forget the moment a window closes. Open-source and built natively
                in Rust, Aether turns Obsidian vaults and vector, cache, and graph stores into
                shared, privacy-bounded memory served over MCP and OpenAPI 3.1.
              </p>
            </div>

            <div className="hero-actions reveal-up" style={{['--d' as string]: '280ms'}}>
              <a
                href={GITHUB_REPO_URL}
                target="_blank"
                rel="noopener noreferrer"
                className="btn-action btn-primary">
                View on GitHub ↗
              </a>
              <Link to="/docs" className="btn-action btn-ghost">
                Read Documentation ↗
              </Link>
            </div>
          </div>

          <MemoryVaultVisual />
        </div>
      </div>
    </section>
  );
}
