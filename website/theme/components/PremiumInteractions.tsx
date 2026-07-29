import { useEffect, useRef } from 'react';

const SURFACE_SELECTOR = [
  '.a3s-install-console',
  '.a3s-feature-card',
  '.a3s-bento-card',
  '.a3s-surface-card',
].join(',');

type PointerPosition = {
  clientX: number;
  clientY: number;
  target: EventTarget | null;
};

/**
 * Coordinates the lightweight Glass / Hex Float-inspired surface lighting.
 * The visual treatment stays in CSS; this component only updates local
 * pointer coordinates and never changes layout or intercepts interaction.
 */
export function PremiumInteractions() {
  const anchorRef = useRef<HTMLSpanElement>(null);

  useEffect(() => {
    const host = anchorRef.current?.closest<HTMLElement>('.a3s-home');
    if (!host) return undefined;

    const hero = host.querySelector<HTMLElement>('.a3s-hero');
    const motionPreference = window.matchMedia(
      '(prefers-reduced-motion: reduce)',
    );
    let activeSurface: HTMLElement | null = null;
    let animationFrame = 0;
    let latestPointer: PointerPosition | null = null;

    const clearActiveSurface = () => {
      activeSurface?.classList.remove('is-pointer-active');
      activeSurface = null;
    };

    const paintPointer = () => {
      animationFrame = 0;
      const pointer = latestPointer;
      if (!pointer || motionPreference.matches) {
        clearActiveSurface();
        return;
      }

      const target =
        pointer.target instanceof Element ? pointer.target : undefined;
      const surface = target?.closest<HTMLElement>(SURFACE_SELECTOR) ?? null;

      if (surface && host.contains(surface)) {
        if (surface !== activeSurface) {
          clearActiveSurface();
          activeSurface = surface;
          surface.classList.add('is-pointer-active');
        }

        const bounds = surface.getBoundingClientRect();
        surface.style.setProperty(
          '--a3s-spot-x',
          `${pointer.clientX - bounds.left}px`,
        );
        surface.style.setProperty(
          '--a3s-spot-y',
          `${pointer.clientY - bounds.top}px`,
        );
      } else {
        clearActiveSurface();
      }

      if (hero && target && hero.contains(target)) {
        const bounds = hero.getBoundingClientRect();
        const x = ((pointer.clientX - bounds.left) / bounds.width) * 100;
        const y = ((pointer.clientY - bounds.top) / bounds.height) * 100;
        hero.style.setProperty(
          '--a3s-hero-x',
          `${Math.max(0, Math.min(x, 100))}%`,
        );
        hero.style.setProperty(
          '--a3s-hero-y',
          `${Math.max(0, Math.min(y, 100))}%`,
        );
      } else {
        hero?.style.removeProperty('--a3s-hero-x');
        hero?.style.removeProperty('--a3s-hero-y');
      }
    };

    const handlePointerMove = (event: PointerEvent) => {
      if (event.pointerType === 'touch') return;
      latestPointer = {
        clientX: event.clientX,
        clientY: event.clientY,
        target: event.target,
      };
      if (!animationFrame) {
        animationFrame = window.requestAnimationFrame(paintPointer);
      }
    };

    const handlePointerLeave = () => {
      latestPointer = null;
      clearActiveSurface();
      hero?.style.removeProperty('--a3s-hero-x');
      hero?.style.removeProperty('--a3s-hero-y');
    };

    const handleMotionChange = () => {
      if (motionPreference.matches) handlePointerLeave();
    };

    host.dataset.premiumEffects = 'ready';
    host.addEventListener('pointermove', handlePointerMove);
    host.addEventListener('pointerleave', handlePointerLeave);
    motionPreference.addEventListener('change', handleMotionChange);

    return () => {
      window.cancelAnimationFrame(animationFrame);
      clearActiveSurface();
      delete host.dataset.premiumEffects;
      host.removeEventListener('pointermove', handlePointerMove);
      host.removeEventListener('pointerleave', handlePointerLeave);
      motionPreference.removeEventListener('change', handleMotionChange);
    };
  }, []);

  return (
    <span
      aria-hidden="true"
      className="a3s-premium-effects-anchor"
      ref={anchorRef}
    />
  );
}
