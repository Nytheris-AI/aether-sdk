import {useEffect, useRef, useState, type ReactNode} from 'react';
import Link from '@docusaurus/Link';
import {
  GITHUB_REPO_URL,
  QUICKSTART_ORDER,
  QUICKSTART_SNIPPETS,
  type QuickstartTab,
} from '../../data/landing';

type CopyStatus = 'idle' | 'copied' | 'fallback';

function selectElementText(el: HTMLElement | null): void {
  if (!el) return;
  const selection = window.getSelection();
  if (!selection) return;
  const range = document.createRange();
  range.selectNodeContents(el);
  selection.removeAllRanges();
  selection.addRange(range);
}

export function OpenSourceSection(): ReactNode {
  const [activeQs, setActiveQs] = useState<QuickstartTab>('clone');
  const [copyStatus, setCopyStatus] = useState<CopyStatus>('idle');
  const resetTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const codeRef = useRef<HTMLPreElement>(null);

  useEffect(() => {
    return () => {
      if (resetTimerRef.current) {
        clearTimeout(resetTimerRef.current);
      }
    };
  }, []);

  function handleSelectTab(tab: QuickstartTab): void {
    setActiveQs(tab);
    setCopyStatus('idle');
    if (resetTimerRef.current) {
      clearTimeout(resetTimerRef.current);
      resetTimerRef.current = null;
    }
  }

  async function handleCopy(): Promise<void> {
    const command = QUICKSTART_SNIPPETS[activeQs].command;
    let copied = false;
    if (navigator.clipboard && navigator.clipboard.writeText) {
      try {
        await navigator.clipboard.writeText(command);
        copied = true;
      } catch {
        copied = false;
      }
    }

    if (!copied) {
      selectElementText(codeRef.current);
    }

    setCopyStatus(copied ? 'copied' : 'fallback');
    if (resetTimerRef.current) {
      clearTimeout(resetTimerRef.current);
    }
    resetTimerRef.current = setTimeout(() => {
      setCopyStatus('idle');
    }, 1800);
  }

  const copyLabel =
    copyStatus === 'copied'
      ? 'COPIED'
      : copyStatus === 'fallback'
        ? 'SELECT TEXT'
        : 'COPY';

  return (
    <section className="band" id="access">
      <div className="frame">
        <span className="crosshair tl accent" aria-hidden="true" />
        <span className="crosshair tr" aria-hidden="true" />
        <span className="crosshair bl" aria-hidden="true" />
        <span className="crosshair br accent" aria-hidden="true" />

        <div className="cta-grid">
          <div className="cta-left">
            <div>
              <span className="mono-tag">
                <span className="dot" /> 06 // OPEN SOURCE
              </span>
              <h2 className="section-h2 cta-title reveal-up">
                Open source from the core up. Clone and build today.
              </h2>
              <p className="section-sub reveal-up" style={{'--d': '90ms'}}>
                Explore the Rust SDK and memory engine on GitHub, wire Aether into SQLite, Redis,
                turbopuffer, Qdrant, Chroma, pgvector, or Neo4j, and self-host without vendor
                lock-in.
              </p>
            </div>

            <div className="repo-cta-row reveal-up" style={{'--d': '160ms'}}>
              <a
                href={GITHUB_REPO_URL}
                target="_blank"
                rel="noopener noreferrer"
                className="btn-action btn-primary">
                Open GitHub Repository ↗
              </a>
              <Link to="/docs/get-started/quickstart" className="btn-action btn-ghost">
                Quickstart ↗
              </Link>
              <span className="mono-tag">MUNINN-HQ / AETHER-SDK</span>
            </div>
          </div>

          <div className="cta-right">
            <div className="quickstart-box">
              <div className="qs-bar">
                <div className="qs-tabs" role="tablist" aria-label="Quickstart commands">
                  {QUICKSTART_ORDER.map((tabId) => {
                    const option = QUICKSTART_SNIPPETS[tabId];
                    const active = activeQs === tabId;
                    return (
                      <button
                        key={tabId}
                        type="button"
                        className={active ? 'qs-tab-btn active' : 'qs-tab-btn'}
                        role="tab"
                        aria-selected={active}
                        onClick={() => handleSelectTab(tabId)}>
                        {option.label}
                      </button>
                    );
                  })}
                </div>
                <button
                  type="button"
                  className={copyStatus === 'copied' ? 'qs-copy-btn copied' : 'qs-copy-btn'}
                  onClick={handleCopy}>
                  {copyLabel}
                </button>
              </div>
              <div className="qs-body">
                <pre ref={codeRef} className="qs-code">
                  {QUICKSTART_SNIPPETS[activeQs].command}
                </pre>
              </div>
            </div>
          </div>
        </div>
      </div>
    </section>
  );
}
