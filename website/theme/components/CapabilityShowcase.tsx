import { useEffect, useRef, useState } from 'react';
import { CapabilityTuiDemo } from './CapabilityTuiDemo';
import {
  capabilityStories,
  localized,
  sectionCopy,
  type CapabilityKey,
  type Locale,
} from './capability-stories';

function CapabilityIcon({ story }: { story: CapabilityKey }) {
  const paths: Record<CapabilityKey, React.ReactNode> = {
    hitl: (
      <>
        <path d="M12 2.8 19 6v5.2c0 4.3-2.8 8.1-7 10-4.2-1.9-7-5.7-7-10V6l7-3.2Z" />
        <path d="M9.2 12.1 11 14l4-4.3" />
      </>
    ),
    progressive: (
      <>
        <path d="M4 6h5M4 12h10M4 18h16" />
        <path d="m7 3 3 3-3 3M12 9l3 3-3 3m5 0 3 3-3 3" />
      </>
    ),
    runtime: (
      <>
        <rect x="3" y="4" width="18" height="5" rx="1.5" />
        <rect x="3" y="15" width="5" height="5" rx="1.2" />
        <rect x="10" y="15" width="5" height="5" rx="1.2" />
        <rect x="17" y="15" width="4" height="5" rx="1.2" />
        <path d="M12 9v3M5.5 12h13M5.5 12v3m6.5-3v3m6.5-3v3" />
      </>
    ),
    intelligence: (
      <>
        <path d="m9 6-6 6 6 6m6-12 6 6-6 6M14 3l-4 18" />
      </>
    ),
    ctx: (
      <>
        <path d="M4 5.5h11a4 4 0 0 1 4 4v1.5" />
        <path d="m16 8 3 3 3-3M20 18.5H9a4 4 0 0 1-4-4V13" />
        <path d="m8 16-3-3-3 3" />
      </>
    ),
  };

  return (
    <svg aria-hidden="true" viewBox="0 0 24 24">
      {paths[story]}
    </svg>
  );
}

export function CapabilityStoriesMarkdown({ locale }: { locale: Locale }) {
  const labels = sectionCopy[locale];

  return (
    <section>
      <h2>{labels.title}</h2>
      <p>{labels.body}</p>
      {capabilityStories.map((story) => (
        <section key={story.key}>
          <h3>{localized(story.title, locale)}</h3>
          <p>{localized(story.body, locale)}</p>
          <p>{localized(story.availability, locale)}</p>
        </section>
      ))}
    </section>
  );
}

export function CapabilityShowcase({
  guideHref,
  locale,
}: {
  guideHref: string;
  locale: Locale;
}) {
  const hostRef = useRef<HTMLDivElement>(null);
  const selectorRef = useRef<HTMLElement>(null);
  const [activeIndex, setActiveIndex] = useState(0);
  const [stage, setStage] = useState(0);
  const [isPlaying, setIsPlaying] = useState(false);
  const [isVisible, setIsVisible] = useState(false);
  const [reducedMotion, setReducedMotion] = useState(false);
  const story = capabilityStories[activeIndex] ?? capabilityStories[0];
  const labels = sectionCopy[locale];

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return undefined;

    const preference = window.matchMedia('(prefers-reduced-motion: reduce)');
    const applyPreference = () => {
      setReducedMotion(preference.matches);
      if (preference.matches) {
        setStage(3);
        setIsPlaying(false);
      }
    };
    applyPreference();

    const observer = new IntersectionObserver(
      ([entry]) => {
        const visible = entry?.isIntersecting ?? false;
        setIsVisible(visible);
        if (visible && !preference.matches) setIsPlaying(true);
      },
      { threshold: 0.32 },
    );
    observer.observe(host);
    preference.addEventListener('change', applyPreference);

    return () => {
      observer.disconnect();
      preference.removeEventListener('change', applyPreference);
    };
  }, []);

  useEffect(() => {
    if (!isPlaying || !isVisible || reducedMotion) return undefined;

    const timer = window.setTimeout(
      () => {
        if (stage < 3) {
          setStage((current) => current + 1);
          return;
        }
        setStage(0);
        setActiveIndex((current) => (current + 1) % capabilityStories.length);
      },
      stage === 3 ? 3000 : 1450,
    );

    return () => window.clearTimeout(timer);
  }, [isPlaying, isVisible, reducedMotion, stage]);

  useEffect(() => {
    const selector = selectorRef.current;
    const button = selector?.children.item(activeIndex);
    if (!(button instanceof HTMLElement) || !selector) return;
    if (selector.scrollWidth <= selector.clientWidth) return;

    const centeredLeft =
      button.offsetLeft - (selector.clientWidth - button.clientWidth) / 2;
    selector.scrollTo({
      behavior: reducedMotion ? 'auto' : 'smooth',
      left: Math.max(0, centeredLeft),
    });
  }, [activeIndex, reducedMotion]);

  function selectStory(index: number) {
    setActiveIndex(index);
    setStage(reducedMotion ? 3 : 0);
    if (!reducedMotion) setIsPlaying(true);
  }

  function togglePlayback() {
    if (isPlaying) {
      setIsPlaying(false);
      return;
    }
    if (stage === 3) setStage(0);
    setIsPlaying(true);
  }

  return (
    <section
      className="a3s-section a3s-capability-stories"
      id="capability-stories"
    >
      <header className="a3s-section-header a3s-capability-stories-header">
        <div>
          <span className="a3s-section-eyebrow">{labels.eyebrow}</span>
          <h2>{labels.title}</h2>
        </div>
        <div>
          <p>{labels.body}</p>
          <a href={guideHref}>
            {labels.guide}
            <span aria-hidden="true">→</span>
          </a>
        </div>
      </header>

      <div
        className={`a3s-capability-console is-${story.key}`}
        data-stage={stage}
        ref={hostRef}
      >
        <nav
          aria-label={labels.select}
          className="a3s-capability-selector"
          ref={selectorRef}
        >
          {capabilityStories.map((item, index) => (
            <button
              aria-current={index === activeIndex ? 'true' : undefined}
              className={index === activeIndex ? 'is-active' : ''}
              key={item.key}
              onClick={() => selectStory(index)}
              type="button"
            >
              <span>{item.index}</span>
              <i>
                <CapabilityIcon story={item.key} />
              </i>
              <p>
                <small>{item.eyebrow}</small>
                <strong>{localized(item.title, locale)}</strong>
              </p>
              <em aria-hidden="true">
                <b />
              </em>
            </button>
          ))}
        </nav>

        <CapabilityTuiDemo
          isPlaying={isPlaying}
          isVisible={isVisible}
          locale={locale}
          onPlayback={togglePlayback}
          reducedMotion={reducedMotion}
          stage={stage}
          story={story}
        />
      </div>
    </section>
  );
}
