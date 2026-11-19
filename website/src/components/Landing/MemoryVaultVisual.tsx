import {useState, type ReactNode} from 'react';
import useBaseUrl from '@docusaurus/useBaseUrl';
import {
  HERO_DIAGRAM_STATS,
  HERO_MEMORY_MODES,
  HERO_MEMORY_MODE_ORDER,
  type HeroMemoryModeId,
} from '../../data/landing';

export function MemoryVaultVisual(): ReactNode {
  const [activeMode, setActiveMode] = useState<HeroMemoryModeId>('hot');
  const [isFlowPaused, setIsFlowPaused] = useState(false);

  const obsidianLogoUrl = useBaseUrl('/img/obsidian-logo.svg');
  const aetherLogoUrl = useBaseUrl('/img/aether-logo.svg');
  const mode = HERO_MEMORY_MODES[activeMode];

  return (
    <div className="hero-diagram-cell">
      <div className="diagram-header">
        <span>MEMORY SERVING // OBSIDIAN VAULT</span>
        <button
          type="button"
          className={isFlowPaused ? 'wire-toggle-btn active' : 'wire-toggle-btn'}
          onClick={() => setIsFlowPaused((prev) => !prev)}>
          {isFlowPaused ? 'PLAY FLOW' : 'PAUSE FLOW'}
        </button>
      </div>

      <div
        className={isFlowPaused ? 'vault-hero-stage is-paused' : 'vault-hero-stage'}
        aria-label="Interactive diagram showing Obsidian vault notes served as memory to AI agents through Aether">
        <div className="vault-mode-tabs" role="tablist" aria-label="Memory serving tier preview">
          {HERO_MEMORY_MODE_ORDER.map((modeId) => {
            const item = HERO_MEMORY_MODES[modeId];
            const selected = activeMode === modeId;
            return (
              <button
                key={modeId}
                type="button"
                role="tab"
                aria-selected={selected}
                className={
                  selected
                    ? `vault-mode-tab active${item.blocked ? ' is-blocked' : ''}`
                    : 'vault-mode-tab'
                }
                onClick={() => setActiveMode(modeId)}>
                {item.label}
              </button>
            );
          })}
        </div>

        <div className="vault-topology">
          <div className="vault-node vault-node-source">
            <div className="vault-node-head">
              <div className="vault-brand-badge obsidian-badge">
                <img
                  src={obsidianLogoUrl}
                  alt="Obsidian logo"
                  width={28}
                  height={28}
                  fetchPriority="high"
                />
              </div>
              <div className="vault-node-titles">
                <span className="vault-node-kicker">SOURCE VAULT // OBSIDIAN</span>
                <strong className="vault-node-name">Local Markdown Vault</strong>
              </div>
              <span className="vault-pill">{mode.badge}</span>
            </div>

            <div className="vault-note-row">
              <code className="vault-note-path">{mode.vaultSample}</code>
              <span className="vault-note-rule">{mode.ruleHint}</span>
            </div>
          </div>

          <div className="vault-pipe" aria-hidden="true">
            <span className="vault-pipe-rail" />
            <span className={mode.blocked ? 'vault-pipe-pulse is-blocked' : 'vault-pipe-pulse'} />
            <span className="vault-pipe-label">
              {mode.blocked ? 'BLOCKED AT PARSE BOUNDARY' : 'FTS5 · VECTOR · PPR'}
            </span>
          </div>

          <div className="vault-node vault-node-engine">
            <div className="vault-node-head">
              <div className="vault-brand-badge aether-badge">
                <img
                  src={aetherLogoUrl}
                  alt="Aether SDK logo"
                  width={26}
                  height={26}
                  fetchPriority="high"
                />
              </div>
              <div className="vault-node-titles">
                <span className="vault-node-kicker">AETHER ENGINE // :14771</span>
                <strong className="vault-node-name">{mode.engineStage}</strong>
              </div>
            </div>
            <p className="vault-engine-summary">{mode.routeSummary}</p>
          </div>

          <div className="vault-pipe" aria-hidden="true">
            <span className="vault-pipe-rail" />
            {!mode.blocked && <span className="vault-pipe-pulse delay" />}
            <span className="vault-pipe-label">
              {mode.blocked ? '0 BYTES SERVED' : 'AETHER_RECALL · INBOX.MD'}
            </span>
          </div>

          <div className={mode.blocked ? 'vault-clients is-blocked' : 'vault-clients'}>
            {mode.clientTargets.map((target) => (
              <span key={target} className="vault-client-chip">
                {target}
              </span>
            ))}
          </div>
        </div>
      </div>

      <div className="diagram-footer">
        {HERO_DIAGRAM_STATS.map((stat) => (
          <div key={stat.label}>
            <span className="stat-mini-label">{stat.label}</span>
            <span className="stat-mini-val">{stat.value}</span>
          </div>
        ))}
      </div>
    </div>
  );
}
