import {useEffect, useRef, type ReactNode} from 'react';

const STEP = 24;
const MODULE = STEP * 4;

interface RegisterCell {
  readonly col: number;
  readonly docRow: number;
  readonly mode: 'cobalt-dither' | 'ink-hatch';
  readonly phase: number;
}

function generatePaperGrainDataUrl(): string {
  const size = 192;
  const gCanvas = document.createElement('canvas');
  gCanvas.width = size;
  gCanvas.height = size;
  const gCtx = gCanvas.getContext('2d');
  if (!gCtx) return '';

  const img = gCtx.createImageData(size, size);
  const data = img.data;

  let seed = 0x6d2b79f5;
  function rand(): number {
    seed = (seed + 0x6d2b79f5) | 0;
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  }

  for (let i = 0; i < data.length; i += 4) {
    const r = rand();
    if (r < 0.48) {
      data[i] = 17;
      data[i + 1] = 19;
      data[i + 2] = 21;
      data[i + 3] = Math.floor(3 + rand() * 10);
    } else if (r > 0.86) {
      data[i] = 255;
      data[i + 1] = 255;
      data[i + 2] = 253;
      data[i + 3] = Math.floor(8 + rand() * 14);
    }
  }
  gCtx.putImageData(img, 0, 0);

  gCtx.fillStyle = 'rgba(17, 19, 21, 0.032)';
  for (let f = 0; f < 64; f++) {
    const fx = Math.floor(rand() * size);
    const fy = Math.floor(rand() * size);
    const horiz = rand() > 0.5;
    gCtx.fillRect(fx, fy, horiz ? 3 : 1, horiz ? 1 : 3);
  }

  return gCanvas.toDataURL('image/png');
}

function createDraftingPattern(
  ctx: CanvasRenderingContext2D,
  dpr: number,
): CanvasPattern | null {
  const pCanvas = document.createElement('canvas');
  pCanvas.width = Math.floor(MODULE * dpr);
  pCanvas.height = Math.floor(MODULE * dpr);
  const pCtx = pCanvas.getContext('2d');
  if (!pCtx) return null;

  pCtx.scale(dpr, dpr);

  pCtx.fillStyle = 'rgba(17, 19, 21, 0.14)';
  for (let gx = 0; gx < MODULE; gx += STEP) {
    for (let gy = 0; gy < MODULE; gy += STEP) {
      if (gx === 0 && gy === 0) continue;
      pCtx.fillRect(gx, gy, 1, 1);
    }
  }

  pCtx.fillStyle = 'rgba(17, 19, 21, 0.23)';
  pCtx.fillRect(0, 0, 3, 1);
  pCtx.fillRect(MODULE - 2, 0, 2, 1);
  pCtx.fillRect(0, 0, 1, 3);
  pCtx.fillRect(0, MODULE - 2, 1, 2);

  return ctx.createPattern(pCanvas, 'repeat');
}

function drawHalftoneStamp(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  mode: RegisterCell['mode'],
  intensity: number,
): void {
  if (intensity <= 0.02) return;
  if (mode === 'cobalt-dither') {
    ctx.fillStyle = `rgba(34, 72, 255, ${(intensity * 0.24).toFixed(3)})`;
    for (let dx = 3; dx < STEP - 2; dx += 5) {
      for (let dy = 3; dy < STEP - 2; dy += 5) {
        ctx.fillRect(x + dx, y + dy, 1.25, 1.25);
      }
    }
    ctx.fillStyle = `rgba(34, 72, 255, ${(intensity * 0.45).toFixed(3)})`;
    ctx.fillRect(x, y, 4, 1);
    ctx.fillRect(x, y, 1, 4);
  } else {
    ctx.strokeStyle = `rgba(17, 19, 21, ${(intensity * 0.085).toFixed(3)})`;
    ctx.lineWidth = 1;
    ctx.beginPath();
    for (let k = 6; k <= STEP; k += 6) {
      ctx.moveTo(x + k, y + 1);
      ctx.lineTo(x + 1, y + k);
      if (k < STEP) {
        ctx.moveTo(x + STEP - 1, y + k);
        ctx.lineTo(x + k, y + STEP - 1);
      }
    }
    ctx.stroke();
  }
}

export function BackgroundMatrix(): ReactNode {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const grainRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const grainEl = grainRef.current;
    if (grainEl) {
      const dataUrl = generatePaperGrainDataUrl();
      if (dataUrl) {
        grainEl.style.backgroundImage = `url(${dataUrl})`;
      }
    }

    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;

    const prefersReducedMotion = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    let width = 0;
    let height = 0;
    let dpr = Math.min(window.devicePixelRatio || 1, 2);
    let draftingPattern: CanvasPattern | null = null;
    let registers: RegisterCell[] = [];
    let rafId = 0;

    function buildRegisters(viewWidth: number): RegisterCell[] {
      const cols = Math.max(8, Math.floor(viewWidth / STEP));
      const count = Math.min(26, Math.max(12, Math.floor(cols * 0.42)));
      const next: RegisterCell[] = [];
      for (let i = 0; i < count; i++) {
        next.push({
          col: Math.floor((i * 7 + 3) % cols),
          docRow: i * 5 + ((i * 3) % 7),
          mode: i % 3 === 0 ? 'cobalt-dither' : 'ink-hatch',
          phase: i * 0.65,
        });
      }
      return next;
    }

    function renderFrame(now: number): void {
      if (!ctx) return;
      const scrollY = window.scrollY || window.pageYOffset || 0;
      const t = now * 0.001;
      ctx.clearRect(0, 0, width, height);

      const yOffset = ((scrollY % MODULE) + MODULE) % MODULE;
      if (draftingPattern) {
        ctx.save();
        ctx.scale(1 / dpr, 1 / dpr);
        ctx.translate(0, -Math.round(yOffset * dpr));
        ctx.fillStyle = draftingPattern;
        ctx.fillRect(0, 0, Math.ceil(width * dpr), Math.ceil((height + MODULE) * dpr));
        ctx.restore();
      }

      const baseDocRow = Math.floor(scrollY / STEP);
      const visibleRows = Math.ceil(height / STEP) + 2;
      const qTime = Math.floor(t * 2.5) / 2.5;

      for (const reg of registers) {
        const wrappedRow =
          baseDocRow + (((reg.docRow - baseDocRow) % visibleRows + visibleRows) % visibleRows);
        const screenX = reg.col * STEP;
        const screenY = wrappedRow * STEP - scrollY;

        if (screenX >= 0 && screenX <= width - STEP && screenY >= -STEP && screenY <= height) {
          const wave = prefersReducedMotion
            ? 0.55
            : 0.5 + 0.5 * Math.sin(qTime * 1.4 + reg.phase + wrappedRow * 0.15);
          const stepped = Math.floor(wave * 4) / 3;
          drawHalftoneStamp(ctx, screenX, Math.round(screenY), reg.mode, stepped);
        }
      }

      const frameLeft = Math.max(0, Math.floor((width - 1240) / 2));
      if (frameLeft > 14) {
        const minorOffset = ((scrollY % STEP) + STEP) % STEP;
        ctx.fillStyle = 'rgba(17, 19, 21, 0.16)';
        for (let ry = -minorOffset; ry < height; ry += STEP) {
          const docTickIndex = Math.round((ry + scrollY) / STEP);
          const tickW = docTickIndex % 4 === 0 ? 7 : 3;
          const yPos = Math.round(ry);
          ctx.fillRect(frameLeft - tickW - 1, yPos, tickW, 1);
          ctx.fillRect(width - frameLeft + 1, yPos, tickW, 1);
        }
      }

      if (!prefersReducedMotion) {
        rafId = requestAnimationFrame(renderFrame);
      }
    }

    function handleResize(): void {
      if (!canvas || !ctx) return;
      width = window.innerWidth;
      height = window.innerHeight;
      dpr = Math.min(window.devicePixelRatio || 1, 2);
      canvas.width = Math.floor(width * dpr);
      canvas.height = Math.floor(height * dpr);
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      draftingPattern = createDraftingPattern(ctx, dpr);
      registers = buildRegisters(width);
      if (prefersReducedMotion) {
        renderFrame(0);
      }
    }

    function handleReducedScroll(): void {
      if (prefersReducedMotion) {
        renderFrame(0);
      }
    }

    window.addEventListener('resize', handleResize);
    window.addEventListener('scroll', handleReducedScroll, {passive: true});
    handleResize();

    if (!prefersReducedMotion) {
      rafId = requestAnimationFrame(renderFrame);
    }

    return () => {
      cancelAnimationFrame(rafId);
      window.removeEventListener('resize', handleResize);
      window.removeEventListener('scroll', handleReducedScroll);
    };
  }, []);

  return (
    <>
      <canvas ref={canvasRef} className="bg-canvas" aria-hidden="true" />
      <div ref={grainRef} className="paper-grain-overlay" aria-hidden="true" />
      <div className="viewport-rails" aria-hidden="true">
        <div className="viewport-rails-inner">
          <div className="rail-col" />
          <div className="rail-col" />
          <div className="rail-col" />
          <div className="rail-col" />
        </div>
      </div>
    </>
  );
}
