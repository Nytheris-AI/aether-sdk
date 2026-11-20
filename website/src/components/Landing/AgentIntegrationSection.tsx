import type {ReactNode} from 'react';
import useBaseUrl from '@docusaurus/useBaseUrl';
import {AGENT_FEATURES} from '../../data/landing';

function BrandLogo({src, label}: {readonly src: string; readonly label: string}): ReactNode {
  const url = useBaseUrl(src);
  return (
    <span className="brand-logo-chip">
      <img src={url} alt="" width={20} height={20} />
      {label}
    </span>
  );
}

export function AgentIntegrationSection(): ReactNode {
  return (
    <section className="band" id="agents">
      <div className="frame">
        <span className="crosshair tl accent" aria-hidden="true" />
        <span className="crosshair tr" aria-hidden="true" />

        <div className="section-head-grid">
          <div className="section-index-cell">
            <span className="mono-tag">
              <span className="dot" /> 01 // OBSIDIAN &amp; MCP
            </span>
          </div>
          <div className="section-title-cell">
            <h2 className="section-h2 reveal-up">
              Turn your Obsidian vault into memory every AI agent can use.
            </h2>
            <p className="section-sub reveal-up" style={{'--d': '90ms'}}>
              Aether indexes your notes locally and exposes them through an MCP server, so Claude,
              Cursor, and ChatGPT share one persistent, privacy-bounded memory.
            </p>
          </div>
        </div>

        <div className="planes-grid">
          {AGENT_FEATURES.map((feature, idx) => (
            <article
              key={feature.id}
              className="plane-cell reveal-up"
              style={{'--d': `${idx * 100}ms`}}>
              <div>
                <span className="mono-tag">{feature.tag}</span>
                <h3 className="plane-title">{feature.title}</h3>
                <p className="plane-body">{feature.body}</p>
                {feature.logos.length > 0 && (
                  <div className="brand-logo-row">
                    {feature.logos.map((logo) =>
                      logo.logo ? <BrandLogo key={logo.label} src={logo.logo} label={logo.label} /> : null,
                    )}
                  </div>
                )}
              </div>
              <ul className="plane-spec-list">
                {feature.specs.map((spec) => (
                  <li key={spec.label}>
                    <span>{spec.label}</span>
                    <span>{spec.value}</span>
                  </li>
                ))}
              </ul>
            </article>
          ))}
        </div>
      </div>
    </section>
  );
}
