import {describe, it} from 'node:test';
import assert from 'node:assert/strict';
import {
  CONVERGENCE_PHASES,
  HERO_MEMORY_MODES,
  HERO_MEMORY_MODE_ORDER,
  MEMORY_PLANES,
  QUICKSTART_SNIPPETS,
  SANDBOX_PROBES,
  STORAGE_INTEGRATIONS,
  computeConvergenceState,
  filterIntegrations,
  getDbCardLayoutFlags,
} from './landing.ts';

describe('landing domain data and pure helpers', () => {
  it('defines the expected core counts for planes, integrations, probes, hero modes, and quickstart snippets', () => {
    assert.equal(CONVERGENCE_PHASES.length, 3);
    assert.equal(MEMORY_PLANES.length, 3);
    assert.equal(STORAGE_INTEGRATIONS.length, 7);
    assert.equal(Object.keys(SANDBOX_PROBES).length, 3);
    assert.equal(Object.keys(QUICKSTART_SNIPPETS).length, 2);
    assert.deepEqual(HERO_MEMORY_MODE_ORDER, ['hot', 'warm', 'private']);
    assert.equal(HERO_MEMORY_MODES.hot.vaultSample, 'Memory/brand-voice.md');
    assert.equal(HERO_MEMORY_MODES.private.blocked, true);
  });

  it('filters storage integrations by category and preserves all 7 for "all"', () => {
    assert.equal(filterIntegrations(STORAGE_INTEGRATIONS, 'all').length, 7);
    const vector = filterIntegrations(STORAGE_INTEGRATIONS, 'vector');
    assert.equal(vector.length, 4);
    assert.ok(vector.every((item) => item.category === 'vector'));

    const edgeGraph = filterIntegrations(STORAGE_INTEGRATIONS, 'edge-graph');
    assert.equal(edgeGraph.length, 3);
    assert.ok(edgeGraph.every((item) => item.category === 'edge-graph'));
  });

  it('computes desktop and tablet border termination flags for 7, 4, and 3 visible cards', () => {
    const seven = Array.from({length: 7}, (_, idx) => getDbCardLayoutFlags(idx, 7));
    assert.equal(seven[3].noRightBorder, true);
    assert.equal(seven[6].noRightBorder, true);
    assert.equal(seven[4].noBottomBorder, true);
    assert.equal(seven[6].tabletFullLast, true);
    assert.equal(seven[1].tabletNoRight, true);

    const four = Array.from({length: 4}, (_, idx) => getDbCardLayoutFlags(idx, 4));
    assert.equal(four[3].noRightBorder, true);
    assert.ok(four.every((flags) => flags.noBottomBorder));
    assert.equal(four[2].tabletNoBottom, true);
    assert.equal(four[3].tabletNoBottom, true);

    const three = Array.from({length: 3}, (_, idx) => getDbCardLayoutFlags(idx, 3));
    assert.equal(three[2].noRightBorder, true);
    assert.equal(three[2].tabletFullLast, true);
    assert.ok(three.every((flags) => flags.noBottomBorder));
  });

  it('computes convergence pipeline phase and dash ratios from scroll progress or forced phase', () => {
    const start = computeConvergenceState(0.0);
    assert.equal(start.phaseIdx, 0);
    assert.equal(start.inRatio, 0);
    assert.equal(start.outRatio, 0);

    const mid = computeConvergenceState(0.5);
    assert.equal(mid.phaseIdx, 1);
    assert.equal(mid.inRatio, 1);
    assert.equal(mid.outRatio, 0);

    const end = computeConvergenceState(1.0);
    assert.equal(end.phaseIdx, 2);
    assert.equal(end.inRatio, 1);
    assert.equal(end.outRatio, 1);

    const forcedMid = computeConvergenceState(0.0, 1);
    assert.equal(forcedMid.phaseIdx, 1);
    assert.equal(forcedMid.inRatio, 1);
    assert.equal(forcedMid.outRatio, 0.25);

    const forcedEnd = computeConvergenceState(0.0, 2);
    assert.equal(forcedEnd.phaseIdx, 2);
    assert.equal(forcedEnd.inRatio, 1);
    assert.equal(forcedEnd.outRatio, 1);
  });
});
