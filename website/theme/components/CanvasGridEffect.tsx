import { useEffect, useRef } from 'react';

type CanvasGridEffectProps = {
  cellSize?: number;
  className?: string;
  intensity?: number;
};

type GridWave = {
  bornAt: number;
  x: number;
  y: number;
};

const GRID_TINT = [125, 182, 255] as const;
const WAVE_LIFETIME = 1_250;

function clamp(value: number, minimum: number, maximum: number) {
  return Math.min(Math.max(value, minimum), maximum);
}

/**
 * A lightweight interpretation of Canvas UI's Grid interaction.
 *
 * It keeps the cursor-driven tile wave while avoiding the experimental
 * HTML-in-canvas API, so the effect behaves consistently on the Pages site.
 * https://canvasui.dev/docs/components/grid
 */
export function CanvasGridEffect({
  cellSize = 34,
  className,
  intensity = 1,
}: CanvasGridEffectProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    const host = canvas?.parentElement;
    const context = canvas?.getContext('2d');
    if (!canvas || !host || !context) return undefined;
    const drawingContext = context;

    const motionPreference = window.matchMedia(
      '(prefers-reduced-motion: reduce)',
    );
    let reducedMotion = motionPreference.matches;
    let frame = 0;
    let isVisible = true;
    let isPointerInside = false;
    let lastWaveAt = 0;
    let previousPointer = { x: -cellSize, y: -cellSize };
    let pointer = { x: -cellSize * 4, y: -cellSize * 4 };
    let waves: GridWave[] = [];
    let width = 1;
    let height = 1;
    let pixelRatio = 1;

    const scheduleDraw = () => {
      if (frame || !isVisible) return;
      frame = window.requestAnimationFrame(draw);
    };

    const resize = () => {
      const bounds = host.getBoundingClientRect();
      width = Math.max(bounds.width, 1);
      height = Math.max(bounds.height, 1);
      pixelRatio = Math.min(window.devicePixelRatio || 1, 2);
      canvas.width = Math.round(width * pixelRatio);
      canvas.height = Math.round(height * pixelRatio);
      scheduleDraw();
    };

    function draw(now: number) {
      frame = 0;
      drawingContext.setTransform(pixelRatio, 0, 0, pixelRatio, 0, 0);
      drawingContext.clearRect(0, 0, width, height);

      const activeWaves = reducedMotion
        ? []
        : waves.filter((wave) => now - wave.bornAt < WAVE_LIFETIME);
      waves = activeWaves;
      const columnCount = Math.ceil(width / cellSize) + 1;
      const rowCount = Math.ceil(height / cellSize) + 1;
      const maxDimension = Math.max(width, height);

      for (let row = 0; row < rowCount; row += 1) {
        for (let column = 0; column < columnCount; column += 1) {
          const centerX = column * cellSize + cellSize / 2;
          const centerY = row * cellSize + cellSize / 2;
          const pointerDistance = Math.hypot(
            centerX - pointer.x,
            centerY - pointer.y,
          );
          const pointerLift = isPointerInside
            ? Math.exp(-pointerDistance / (cellSize * 2.25))
            : 0;
          let waveLift = 0;

          for (const wave of activeWaves) {
            const progress = clamp((now - wave.bornAt) / WAVE_LIFETIME, 0, 1);
            const radius = progress * maxDimension * 0.72;
            const distance = Math.hypot(centerX - wave.x, centerY - wave.y);
            const ring = Math.exp(
              -Math.pow((distance - radius) / (cellSize * 1.35), 2),
            );
            waveLift = Math.max(waveLift, ring * (1 - progress));
          }

          const lift = clamp(pointerLift * 0.58 + waveLift, 0, 1);
          const inset = 4 - lift * 2.2;
          const offsetY = -lift * 4;
          const alpha = intensity * (0.018 + lift * 0.16);
          const fillAlpha = intensity * lift * 0.025;

          drawingContext.strokeStyle = `rgba(${GRID_TINT.join(', ')}, ${alpha})`;
          drawingContext.lineWidth = 1;
          drawingContext.strokeRect(
            column * cellSize + inset,
            row * cellSize + inset + offsetY,
            cellSize - inset * 2,
            cellSize - inset * 2,
          );

          if (fillAlpha > 0.002) {
            drawingContext.fillStyle = `rgba(${GRID_TINT.join(', ')}, ${fillAlpha})`;
            drawingContext.fillRect(
              column * cellSize + inset,
              row * cellSize + inset + offsetY,
              cellSize - inset * 2,
              cellSize - inset * 2,
            );
          }
        }
      }

      if (activeWaves.length > 0) scheduleDraw();
    }

    const addWave = (x: number, y: number, now: number) => {
      waves = [...waves.slice(-3), { bornAt: now, x, y }];
      lastWaveAt = now;
    };

    const handlePointerMove = (event: PointerEvent) => {
      if (reducedMotion) return;
      const bounds = host.getBoundingClientRect();
      const x = event.clientX - bounds.left;
      const y = event.clientY - bounds.top;
      const now = performance.now();
      const distance = Math.hypot(x - previousPointer.x, y - previousPointer.y);

      pointer = { x, y };
      isPointerInside = true;
      if (distance > cellSize * 1.25 && now - lastWaveAt > 90) {
        addWave(x, y, now);
        previousPointer = { x, y };
      }
      scheduleDraw();
    };

    const handlePointerLeave = () => {
      isPointerInside = false;
      pointer = { x: -cellSize * 4, y: -cellSize * 4 };
      scheduleDraw();
    };

    const handleMotionChange = () => {
      reducedMotion = motionPreference.matches;
      if (reducedMotion) waves = [];
      scheduleDraw();
    };

    const resizeObserver = new ResizeObserver(resize);
    resizeObserver.observe(host);
    const intersectionObserver = new IntersectionObserver(([entry]) => {
      isVisible = entry?.isIntersecting ?? true;
      if (isVisible) scheduleDraw();
    });
    intersectionObserver.observe(host);
    motionPreference.addEventListener('change', handleMotionChange);
    host.addEventListener('pointermove', handlePointerMove);
    host.addEventListener('pointerleave', handlePointerLeave);
    resize();

    return () => {
      window.cancelAnimationFrame(frame);
      resizeObserver.disconnect();
      intersectionObserver.disconnect();
      motionPreference.removeEventListener('change', handleMotionChange);
      host.removeEventListener('pointermove', handlePointerMove);
      host.removeEventListener('pointerleave', handlePointerLeave);
    };
  }, [cellSize, intensity]);

  return <canvas aria-hidden="true" className={className} ref={canvasRef} />;
}
