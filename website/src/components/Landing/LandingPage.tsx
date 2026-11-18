import {useCallback, useEffect, useRef, useState, type ReactNode} from 'react';
import {NAV_SECTIONS, type SectionId} from '../../data/landing';
import {BackgroundMatrix} from './BackgroundMatrix';
import {ConvergenceSection} from './ConvergenceSection';
import {HeroSection} from './HeroSection';
import {LandingFooter} from './LandingFooter';
import {LandingHeader} from './LandingHeader';
import {MemoryPlanesSection} from './MemoryPlanesSection';
import {OpenSourceSection} from './OpenSourceSection';
import {ProtocolSandboxSection} from './ProtocolSandboxSection';
import {TechStackSection} from './TechStackSection';
import {TickerBand} from './TickerBand';
import './styles/tokens.css';
import './styles/layout.css';
import './styles/sections.css';

const TRACKED_IDS: readonly SectionId[] = NAV_SECTIONS.map((item) => item.id);

export function LandingPage(): ReactNode {
  const rootRef = useRef<HTMLDivElement>(null);
  const [activeSection, setActiveSection] = useState<SectionId>('architecture');
  const clickedSectionRef = useRef<SectionId | null>(null);

  const clearClickLock = useCallback(() => {
    clickedSectionRef.current = null;
  }, []);

  const handleNavigateSection = useCallback((target: SectionId | 'top') => {
    const targetEl = document.getElementById(target);
    if (!targetEl) return;

    const resolvedSection: SectionId = target === 'top' ? 'architecture' : target;
    const reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    setActiveSection(resolvedSection);

    if (!reduced && 'onscrollend' in window) {
      clickedSectionRef.current = resolvedSection;
    } else {
      clickedSectionRef.current = null;
    }

    targetEl.scrollIntoView({behavior: reduced ? 'auto' : 'smooth'});
  }, []);

  useEffect(() => {
    const root = rootRef.current;
    if (!root) return;

    const revealObserver = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (entry.isIntersecting) {
            entry.target.classList.add('in-view');
          }
        }
      },
      {threshold: 0.12},
    );

    root.querySelectorAll('.band, .reveal-up').forEach((el) => {
      revealObserver.observe(el);
    });

    function handleKeyDown(e: KeyboardEvent): void {
      if (['ArrowUp', 'ArrowDown', 'PageUp', 'PageDown', 'Home', 'End', ' '].includes(e.key)) {
        clearClickLock();
      }
    }

    function handleScrollSpy(): void {
      if (clickedSectionRef.current) {
        setActiveSection(clickedSectionRef.current);
        return;
      }

      const scrollY = window.scrollY || window.pageYOffset || 0;
      const rawMaxScroll = Math.max(0, document.documentElement.scrollHeight - window.innerHeight);
      const triggerLine = window.innerHeight * 0.38;

      const thresholds = TRACKED_IDS.map((id) => {
        const sec = document.getElementById(id);
        if (!sec) return Infinity;
        return scrollY + sec.getBoundingClientRect().top - triggerLine;
      });

      const maxReachable = Math.max(0, rawMaxScroll - 8);
      const firstUnreachable = thresholds.findIndex((t) => t > maxReachable);

      if (firstUnreachable !== -1 && rawMaxScroll > 0) {
        const unreachableCount = thresholds.length - firstUnreachable;
        const prevTrigger = firstUnreachable > 0 ? thresholds[firstUnreachable - 1] : 0;
        const span = Math.max(0, maxReachable - prevTrigger);
        for (let i = firstUnreachable; i < thresholds.length; i++) {
          const stepIndex = i - firstUnreachable + 1;
          thresholds[i] = prevTrigger + (span * stepIndex) / (unreachableCount + 0.5);
        }
      }

      let currentId: SectionId = 'architecture';
      for (let i = 0; i < TRACKED_IDS.length; i++) {
        if (document.getElementById(TRACKED_IDS[i]) && scrollY >= thresholds[i]) {
          currentId = TRACKED_IDS[i];
        }
      }
      if (rawMaxScroll > 0 && scrollY >= rawMaxScroll - 6) {
        currentId = TRACKED_IDS[TRACKED_IDS.length - 1];
      }
      setActiveSection(currentId);
    }

    function handleScrollEnd(): void {
      if (clickedSectionRef.current) {
        clearClickLock();
        handleScrollSpy();
      }
    }

    window.addEventListener('wheel', clearClickLock, {passive: true});
    window.addEventListener('touchmove', clearClickLock, {passive: true});
    window.addEventListener('keydown', handleKeyDown);
    window.addEventListener('scrollend', handleScrollEnd, {passive: true});
    window.addEventListener('scroll', handleScrollSpy, {passive: true});
    window.addEventListener('resize', handleScrollSpy, {passive: true});
    handleScrollSpy();

    return () => {
      revealObserver.disconnect();
      window.removeEventListener('wheel', clearClickLock);
      window.removeEventListener('touchmove', clearClickLock);
      window.removeEventListener('keydown', handleKeyDown);
      window.removeEventListener('scrollend', handleScrollEnd);
      window.removeEventListener('scroll', handleScrollSpy);
      window.removeEventListener('resize', handleScrollSpy);
      clearClickLock();
    };
  }, [clearClickLock]);

  return (
    <div ref={rootRef} className="aether-landing">
      <BackgroundMatrix />
      <div className="app-shell">
        <LandingHeader
          activeSection={activeSection}
          onNavigateSection={handleNavigateSection}
        />
        <main id="top">
          <HeroSection onScrollToSection={handleNavigateSection} />
          <TickerBand />
          <ConvergenceSection />
          <MemoryPlanesSection />
          <TechStackSection />
          <ProtocolSandboxSection />
          <OpenSourceSection />
        </main>
        <LandingFooter />
      </div>
    </div>
  );
}
