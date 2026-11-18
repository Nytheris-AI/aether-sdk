import type {ReactNode} from 'react';
import Link from '@docusaurus/Link';
import {GITHUB_REPO_URL} from '../../data/landing';

export function LandingFooter(): ReactNode {
  return (
    <footer className="band">
      <div className="frame footer-grid">
        <div className="footer-cell">
          <strong style={{color: 'var(--c-ink)', letterSpacing: '0.1em'}}>
            AETHER // UNIFIED AI MEMORY LAYER
          </strong>
          <span>OPEN-SOURCE RUST EPISODIC, SEMANTIC &amp; PROCEDURAL CONTEXT INFRASTRUCTURE</span>
        </div>
        <div className="footer-cell">
          <span>STORAGE &amp; MEMORY INTEGRATIONS</span>
          <span>SQLITE · REDIS · TURBOPUFFER · QDRANT · CHROMA · PGVECTOR · NEO4J</span>
        </div>
        <div className="footer-cell">
          <span>OPEN-SOURCE REPOSITORY</span>
          <a
            href={GITHUB_REPO_URL}
            target="_blank"
            rel="noopener noreferrer"
            className="footer-repo-link">
            github.com/Nytheris-AI/aether-sdk ↗
          </a>
          <Link to="/docs" className="footer-repo-link">
            Documentation Portal ↗
          </Link>
        </div>
      </div>
    </footer>
  );
}
