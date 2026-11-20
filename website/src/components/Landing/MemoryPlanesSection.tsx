import type {ReactNode} from 'react';
import {MEMORY_PLANES, type MemoryPlane} from '../../data/landing';

function PlaneDiagramSvg({planeId}: {readonly planeId: MemoryPlane['id']}): ReactNode {
  switch (planeId) {
    case 'episodic':
      return (
        <svg width="220" height="64" viewBox="0 0 220 64">
          <line x1="16" y1="32" x2="204" y2="32" stroke="rgba(17,19,21,0.25)" />
          <circle cx="42" cy="32" r="4" fill="#111315" />
          <circle cx="94" cy="32" r="4" fill="#111315" />
          <circle cx="146" cy="32" r="6" fill="none" stroke="#2248FF" strokeWidth="1.5" />
          <circle cx="146" cy="32" r="2.5" fill="#2248FF" />
          <circle cx="188" cy="32" r="4" fill="#2248FF" />
          <text x="42" y="18" textAnchor="middle" className="node-label">
            T-3
          </text>
          <text x="94" y="18" textAnchor="middle" className="node-label">
            T-2
          </text>
          <text x="146" y="18" textAnchor="middle" className="node-label" fill="#2248FF">
            T-1
          </text>
          <text x="188" y="18" textAnchor="middle" className="node-label" fill="#2248FF">
            NOW
          </text>
        </svg>
      );
    case 'semantic':
      return (
        <svg width="220" height="64" viewBox="0 0 220 64">
          <line x1="64" y1="18" x2="112" y2="46" stroke="rgba(17,19,21,0.3)" />
          <line x1="112" y1="46" x2="162" y2="20" stroke="#2248FF" strokeWidth="1.4" />
          <line
            x1="64"
            y1="18"
            x2="162"
            y2="20"
            stroke="rgba(17,19,21,0.2)"
            strokeDasharray="3 3"
          />
          <rect x="58" y="12" width="12" height="12" fill="#F7F7F4" stroke="#111315" />
          <rect x="106" y="40" width="12" height="12" fill="#2248FF" />
          <rect
            x="156"
            y="14"
            width="12"
            height="12"
            fill="#F7F7F4"
            stroke="#2248FF"
            strokeWidth="1.5"
          />
        </svg>
      );
    case 'procedural':
      return (
        <svg width="220" height="64" viewBox="0 0 220 64">
          <rect x="38" y="18" width="144" height="28" fill="none" stroke="rgba(17,19,21,0.28)" />
          <line x1="86" y1="18" x2="86" y2="46" stroke="rgba(17,19,21,0.28)" />
          <line x1="134" y1="18" x2="134" y2="46" stroke="rgba(17,19,21,0.28)" />
          <rect
            x="86"
            y="18"
            width="48"
            height="28"
            fill="rgba(34,72,255,0.12)"
            stroke="#2248FF"
          />
        </svg>
      );
    default: {
      const _exhaustive: never = planeId;
      return _exhaustive;
    }
  }
}

export function MemoryPlanesSection(): ReactNode {
  return (
    <section className="band" id="planes">
      <div className="frame">
        <span className="crosshair tl accent" aria-hidden="true" />
        <span className="crosshair tr" aria-hidden="true" />

        <div className="section-head-grid">
          <div className="section-index-cell">
            <span className="mono-tag">
              <span className="dot" /> 03 // MEMORY PLANES
            </span>
          </div>
          <div className="section-title-cell">
            <h2 className="section-h2 reveal-up">
              Three cognitive planes. Zero brittle prompt stuffing.
            </h2>
            <p className="section-sub reveal-up" style={{'--d': '90ms'}}>
              Vector similarity alone loses time and causality. Aether coordinates three
              synchronized representations across your storage backends.
            </p>
          </div>
        </div>

        <div className="planes-grid">
          {MEMORY_PLANES.map((plane, idx) => (
            <article
              key={plane.id}
              className="plane-cell reveal-up"
              style={{'--d': `${idx * 100}ms`}}>
              <div>
                <span className="mono-tag">{plane.tag}</span>
                <div className="plane-diagram" aria-hidden="true">
                  <PlaneDiagramSvg planeId={plane.id} />
                </div>
                <h3 className="plane-title">{plane.title}</h3>
                <p className="plane-body">{plane.body}</p>
              </div>
              <ul className="plane-spec-list">
                {plane.specs.map((spec) => (
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
