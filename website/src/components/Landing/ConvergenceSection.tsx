import {useEffect, useRef, useState, type ReactNode} from 'react';
import {
  CONVERGENCE_PHASES,
  computeConvergenceState,
  type ConvergenceState,
} from '../../data/landing';

export function ConvergenceSection(): ReactNode {
  const scrubRef = useRef<HTMLDivElement>(null);
  const manualLockTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const manualLockedRef = useRef(false);

  const [state, setState] = useState<ConvergenceState>(() => computeConvergenceState(0.18));

  useEffect(() => {
    function handleScroll(): void {
      if (manualLockedRef.current || !scrubRef.current) return;
      const rect = scrubRef.current.getBoundingClientRect();
      const scrollable = Math.max(1, rect.height - window.innerHeight * 0.75);
      const progressed = (-rect.top + 110) / scrollable;
      setState(computeConvergenceState(progressed));
    }

    window.addEventListener('scroll', handleScroll, {passive: true});
    window.addEventListener('resize', handleScroll, {passive: true});
    return () => {
      window.removeEventListener('scroll', handleScroll);
      window.removeEventListener('resize', handleScroll);
      if (manualLockTimerRef.current) {
        clearTimeout(manualLockTimerRef.current);
      }
    };
  }, []);

  function handleSelectPhase(phaseId: 0 | 1 | 2): void {
    manualLockedRef.current = true;
    setState(computeConvergenceState((phaseId + 0.5) / 3, phaseId));
    if (manualLockTimerRef.current) {
      clearTimeout(manualLockTimerRef.current);
    }
    manualLockTimerRef.current = setTimeout(() => {
      manualLockedRef.current = false;
    }, 1400);
  }

  const inOffset = 420 * (1 - state.inRatio);
  const outOffset = 420 * (1 - state.outRatio);
  const activePhase = CONVERGENCE_PHASES[state.phaseIdx];

  return (
    <section className="band" id="architecture">
      <div className="frame">
        <span className="crosshair tl" aria-hidden="true" />
        <span className="crosshair tr accent" aria-hidden="true" />

        <div className="section-head-grid">
          <div className="section-index-cell">
            <span className="mono-tag">
              <span className="dot" /> 01 // CONVERGENCE
            </span>
          </div>
          <div className="section-title-cell">
            <h2 className="section-h2 reveal-up">
              From fragmented silos to continuous shared cognition.
            </h2>
            <p className="section-sub reveal-up" style={{'--d': '90ms'}}>
              Scroll through the memory lifecycle, or select any stage, to trace how raw multi-agent
              events are distilled into structured, conflict-free context.
            </p>
          </div>
        </div>

        <div className="scroll-scrub-outer" ref={scrubRef}>
          <div className="scroll-scrub-sticky">
            <div className="convergence-layout">
              <div className="phase-rail" role="tablist" aria-label="Memory pipeline stages">
                {CONVERGENCE_PHASES.map((phase) => {
                  const active = phase.id === state.phaseIdx;
                  return (
                    <button
                      key={phase.id}
                      type="button"
                      className={active ? 'phase-step active' : 'phase-step'}
                      role="tab"
                      aria-selected={active}
                      onClick={() => handleSelectPhase(phase.id)}>
                      <span className="phase-num">{phase.num}</span>
                      <span className="phase-name">{phase.name}</span>
                      <span className="phase-desc">{phase.desc}</span>
                    </button>
                  );
                })}
              </div>

              <div className="convergence-stage">
                <div className="stage-top-bar">
                  <span>{activePhase.caption}</span>
                  <span>RUST ASYNC PIPELINE</span>
                </div>

                <svg
                  className="convergence-svg"
                  viewBox="0 0 760 340"
                  role="img"
                  aria-label="Pipeline diagram connecting input channels through Aether consolidation to recall">
                  <line
                    x1="210"
                    y1="10"
                    x2="210"
                    y2="330"
                    stroke="rgba(17,19,21,0.08)"
                    strokeDasharray="3 3"
                  />
                  <line
                    x1="550"
                    y1="10"
                    x2="550"
                    y2="330"
                    stroke="rgba(17,19,21,0.08)"
                    strokeDasharray="3 3"
                  />

                  <path d="M 150 65 C 235 65, 245 170, 315 170" className="flow-wire" />
                  <path d="M 150 170 L 315 170" className="flow-wire" />
                  <path d="M 150 275 C 235 275, 245 170, 315 170" className="flow-wire" />

                  <path
                    d="M 150 65 C 235 65, 245 170, 315 170"
                    className="flow-wire-active"
                    style={{strokeDashoffset: inOffset}}
                  />
                  <path
                    d="M 150 170 L 315 170"
                    className="flow-wire-active"
                    style={{strokeDashoffset: inOffset}}
                  />
                  <path
                    d="M 150 275 C 235 275, 245 170, 315 170"
                    className="flow-wire-active"
                    style={{strokeDashoffset: inOffset}}
                  />

                  <path d="M 445 170 C 515 170, 525 85, 610 85" className="flow-wire" />
                  <path d="M 445 170 C 515 170, 525 255, 610 255" className="flow-wire" />

                  <path
                    d="M 445 170 C 515 170, 525 85, 610 85"
                    className="flow-wire-active"
                    style={{strokeDashoffset: outOffset}}
                  />
                  <path
                    d="M 445 170 C 515 170, 525 255, 610 255"
                    className="flow-wire-active"
                    style={{strokeDashoffset: outOffset}}
                  />

                  <g>
                    <rect x="24" y="46" width="126" height="38" fill="#F7F7F4" stroke="#111315" />
                    <text x="87" y="69" textAnchor="middle" className="node-label">
                      SUPPORT THREADS
                    </text>

                    <rect x="24" y="151" width="126" height="38" fill="#F7F7F4" stroke="#111315" />
                    <text x="87" y="174" textAnchor="middle" className="node-label">
                      IDE COPILOT LOGS
                    </text>

                    <rect x="24" y="256" width="126" height="38" fill="#F7F7F4" stroke="#111315" />
                    <text x="87" y="279" textAnchor="middle" className="node-label">
                      AGENT TOOL RUNS
                    </text>
                  </g>

                  <g>
                    <rect
                      x="305"
                      y="102"
                      width="150"
                      height="136"
                      fill="#F7F7F4"
                      stroke={state.phaseIdx >= 1 ? '#2248FF' : '#111315'}
                      strokeWidth="1.25"
                    />
                    <rect
                      x="317"
                      y="114"
                      width="126"
                      height="112"
                      fill="none"
                      stroke="rgba(34,72,255,0.35)"
                      strokeDasharray="4 4"
                    />
                    <circle
                      cx="380"
                      cy="156"
                      r={state.phaseIdx === 1 ? 25 : 18}
                      fill="none"
                      stroke="#2248FF"
                      strokeWidth="1.5"
                    />
                    <circle cx="380" cy="156" r="4.5" fill="#2248FF" />
                    <text x="380" y="196" textAnchor="middle" className="node-label" fill="#111315">
                      AETHER RUST CORE
                    </text>
                    <text
                      x="380"
                      y="212"
                      textAnchor="middle"
                      className="node-label"
                      style={{fill: 'rgba(17,19,21,0.5)', fontSize: '8.5px'}}>
                      VECTOR + GRAPH
                    </text>
                  </g>

                  <g>
                    <rect
                      x="610"
                      y="64"
                      width="128"
                      height="42"
                      fill="#F7F7F4"
                      stroke={state.phaseIdx === 2 ? '#2248FF' : '#111315'}
                    />
                    <text x="674" y="83" textAnchor="middle" className="node-label">
                      REASONING LLM
                    </text>
                    <text
                      x="674"
                      y="96"
                      textAnchor="middle"
                      className="node-label"
                      style={{fill: '#2248FF', fontSize: '8px'}}>
                      RANKED CONTEXT
                    </text>

                    <rect
                      x="610"
                      y="234"
                      width="128"
                      height="42"
                      fill="#F7F7F4"
                      stroke={state.phaseIdx === 2 ? '#2248FF' : '#111315'}
                    />
                    <text x="674" y="253" textAnchor="middle" className="node-label">
                      AUTONOMOUS OPS
                    </text>
                    <text
                      x="674"
                      y="266"
                      textAnchor="middle"
                      className="node-label"
                      style={{fill: '#2248FF', fontSize: '8px'}}>
                      STATEFUL GUARD
                    </text>
                  </g>
                </svg>

                <div className="stage-bottom-bar">
                  <span>WRITE ONCE // RECALL ACROSS ANY MODEL PROVIDER</span>
                  <span>gRPC &amp; REST ENDPOINTS</span>
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </section>
  );
}
