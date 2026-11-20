import {useState, type ReactNode} from 'react';
import {
  PROBE_ORDER,
  SANDBOX_PROBES,
  type CodeSegment,
  type InspectorTab,
  type ProbeId,
} from '../../data/landing';

function renderCodeSegments(segments: readonly CodeSegment[]): ReactNode {
  return segments.map((seg, idx) => {
    switch (seg.kind) {
      case 'kw':
        return (
          <span key={idx} className="code-kw">
            {seg.text}
          </span>
        );
      case 'cm':
        return (
          <span key={idx} className="code-cm">
            {seg.text}
          </span>
        );
      case 'plain':
        return seg.text;
      default: {
        const _exhaustive: never = seg.kind;
        return _exhaustive;
      }
    }
  });
}

export function ProtocolSandboxSection(): ReactNode {
  const [activeProbe, setActiveProbe] = useState<ProbeId>('arch');
  const [activeTab, setActiveTab] = useState<InspectorTab>('sdk');

  const currentProbe = SANDBOX_PROBES[activeProbe];
  const activeSegments =
    activeTab === 'sdk' ? currentProbe.sdkSegments : currentProbe.payloadSegments;

  return (
    <section className="band" id="inspector">
      <div className="frame">
        <span className="crosshair tl accent" aria-hidden="true" />
        <span className="crosshair tr" aria-hidden="true" />

        <div className="section-head-grid">
          <div className="section-index-cell">
            <span className="mono-tag">
              <span className="dot" /> 05 // PROTOCOL
            </span>
          </div>
          <div className="section-title-cell">
            <h2 className="section-h2 reveal-up">
              Minimal surface area. Deterministic context assembly.
            </h2>
            <p className="section-sub reveal-up" style={{'--d': '90ms'}}>
              Select a sample scenario below to simulate how Aether retrieves and ranks
              cross-session memory nodes across vector and graph backends before your model
              generates a token.
            </p>
          </div>
        </div>

        <div className="sandbox-grid">
          <div className="sandbox-controls">
            <div>
              <span className="probe-group-label">SELECT SAMPLE MEMORY PROBE</span>
              <div className="probe-list" role="listbox" aria-label="Sample memory scenarios">
                {PROBE_ORDER.map((id) => {
                  const probe = SANDBOX_PROBES[id];
                  const active = activeProbe === id;
                  return (
                    <button
                      key={id}
                      type="button"
                      className={active ? 'probe-chip active' : 'probe-chip'}
                      role="option"
                      aria-selected={active}
                      onClick={() => setActiveProbe(id)}>
                      <span className="probe-chip-title">{probe.title}</span>
                      <span className="probe-chip-tag">{probe.tag}</span>
                    </button>
                  );
                })}
              </div>
            </div>

            <div>
              <span className="probe-group-label">NAMESPACE ISOLATION</span>
              <p className="probe-isolation-copy">
                Every memory node carries cryptographic tenant boundaries, source provenance, and
                TTL policies, allowing instant audit or selective forgetting across both vector and
                graph stores.
              </p>
            </div>
          </div>

          <div className="sandbox-terminal">
            <div className="terminal-tabs">
              <div className="tab-buttons" role="tablist" aria-label="Inspector view">
                <button
                  type="button"
                  className={activeTab === 'sdk' ? 'tab-btn active' : 'tab-btn'}
                  role="tab"
                  aria-selected={activeTab === 'sdk'}
                  onClick={() => setActiveTab('sdk')}>
                  RUST / TS SDK
                </button>
                <button
                  type="button"
                  className={activeTab === 'payload' ? 'tab-btn active' : 'tab-btn'}
                  role="tab"
                  aria-selected={activeTab === 'payload'}
                  onClick={() => setActiveTab('payload')}>
                  ASSEMBLED GRAPH SLICE
                </button>
              </div>
              <div className="sim-badge">SAMPLE SIMULATION DATA</div>
            </div>

            <div className="terminal-body">
              <pre className="code-block">{renderCodeSegments(activeSegments)}</pre>

              <div className="recall-nodes" aria-live="polite">
                {currentProbe.nodes.map((node) => (
                  <div key={node.fact} className="recall-row">
                    <span className="recall-fact">{node.fact}</span>
                    <span className="recall-score">REL {node.score}</span>
                    <div className="recall-bar-track">
                      <div className="recall-bar-fill" style={{width: `${node.pct}%`}} />
                    </div>
                  </div>
                ))}
              </div>
            </div>
          </div>
        </div>
      </div>
    </section>
  );
}
