import type {ReactNode} from 'react';
import Link from '@docusaurus/Link';
import {GITHUB_REPO_URL, NAV_SECTIONS, type SectionId} from '../../data/landing';
import {BrandMark} from './BrandMark';

interface LandingHeaderProps {
  readonly activeSection: SectionId;
  readonly onNavigateSection: (target: SectionId | 'top') => void;
}

export function LandingHeader({
  activeSection,
  onNavigateSection,
}: LandingHeaderProps): ReactNode {
  return (
    <header className="site-header">
      <div className="frame nav-grid">
        <button
          type="button"
          className="brand-cell"
          onClick={() => onNavigateSection('top')}
          aria-label="Aether Home">
          <BrandMark />
          <span className="brand-name">Aether</span>
        </button>

        <nav className="nav-links" aria-label="Primary sections">
          {NAV_SECTIONS.map((item) => (
            <button
              key={item.id}
              type="button"
              className={activeSection === item.id ? 'nav-btn active' : 'nav-btn'}
              onClick={() => onNavigateSection(item.id)}>
              {item.label}
            </button>
          ))}
        </nav>

        <div className="nav-right">
          <Link to="/docs" className="nav-docs-link">
            Docs ↗
          </Link>
          <a
            href={GITHUB_REPO_URL}
            target="_blank"
            rel="noopener noreferrer"
            className="btn-action btn-primary nav-cta">
            GitHub Repo ↗
          </a>
        </div>
      </div>
    </header>
  );
}
