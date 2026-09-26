import {
  removeBase,
  useLocation,
  useNav,
  usePage,
  usePages,
  useSite,
  useVersion,
} from '@rspress/core/runtime';
import {
  IconSmallMenu,
  NavTitle,
  Search,
  SocialLinks,
  SvgWrapper,
  SwitchAppearance,
  type NavProps,
  useHoverGroup,
} from '@rspress/core/theme-original';
import {
  NavLangs,
  NavMenu,
  NavMenuDivider,
  NavMenuItemWithChildren,
} from '@rspress/core/dist/theme/components/Nav/NavMenu.js';
import {
  NavScreen,
  NavScreenDivider,
} from '@rspress/core/dist/theme/components/NavScreen/index.js';
import { NavScreenAppearance } from '@rspress/core/dist/theme/components/NavScreen/NavScreenAppearance.js';
import { NavScreenLangs } from '@rspress/core/dist/theme/components/NavScreen/NavScreenLangs.js';
import { useNavScreen } from '@rspress/core/dist/theme/components/NavHamburger/useNavScreen.js';
import '@rspress/core/dist/theme/components/Nav/index.css';
import '@rspress/core/dist/theme/components/NavHamburger/index.css';
import { useMemo } from 'react';
import { createPortal } from 'react-dom';

function versionHref(
  pathname: string,
  currentVersion: string,
  targetVersion: string,
  defaultVersion: string,
  cleanUrls: boolean,
) {
  const parts = removeBase(pathname).split('/').filter(Boolean);

  if (currentVersion !== defaultVersion && parts[0] === currentVersion) {
    parts.shift();
  }

  if (targetVersion !== defaultVersion) {
    parts.unshift(targetVersion);
  }

  if (parts.length === 0) {
    return '/';
  }

  if (parts.length === 1 && targetVersion !== defaultVersion) {
    parts.push(cleanUrls ? 'index' : 'index.html');
  }

  return `/${parts.join('/')}`;
}

function normalizeRoute(route: string) {
  const trimmed = removeBase(route)
    .replace(/\.html$/, '')
    .replace(/\/index$/, '')
    .replace(/\/+$/, '');
  return trimmed === '' ? '/' : trimmed;
}

// Pages are version-specific (a page can be new in one version or removed in
// another), so link to the same page only when the target version has it.
function guideIndexPath(pathname: string, currentVersion: string) {
  const parts = removeBase(pathname).split('/').filter(Boolean);
  if (parts[0] === currentVersion) {
    parts.shift();
  }
  const lang = parts[0] === 'en' ? '/en' : '';
  return `${lang}/guide/`;
}

function NavVersions() {
  const { pathname } = useLocation();
  const { page } = usePage();
  const { pages } = usePages();
  const { site } = useSite();
  const currentVersion = useVersion();
  const defaultVersion = site.multiVersion.default ?? '';
  const versions = site.multiVersion.versions ?? [];
  const cleanUrls = site.route?.cleanUrls ?? false;
  const routes = useMemo(
    () => new Set(pages.map((entry) => normalizeRoute(entry.routePath))),
    [pages],
  );
  const items = versions.map((version) => {
    const samePage = versionHref(
      page.pageType === '404' ? '/' : pathname,
      currentVersion,
      version,
      defaultVersion,
      cleanUrls,
    );
    if (routes.has(normalizeRoute(samePage))) {
      return { text: version, link: samePage };
    }
    const guideIndex = versionHref(
      guideIndexPath(pathname, currentVersion),
      defaultVersion,
      version,
      defaultVersion,
      cleanUrls,
    );
    return {
      text: version,
      link: routes.has(normalizeRoute(guideIndex))
        ? guideIndex
        : versionHref('/', defaultVersion, version, defaultVersion, cleanUrls),
    };
  });

  return items.length > 1 ? (
    <NavMenuItemWithChildren
      menuItem={{ text: currentVersion, items }}
      activeMatcher={(item) => item.text === currentVersion}
    />
  ) : null;
}

function NavHamburger() {
  const { isScreenOpen, toggleScreen } = useNavScreen();
  const { handleMouseEnter, handleMouseLeave, hoverGroup } = useHoverGroup({
    position: 'right',
    customChildren: (
      <div className="rp-nav-menu__others-mobile__container">
        <div className="rp-nav-hamburger__md__hover-group">
          <NavScreenAppearance />
          <NavVersions />
          <NavScreenLangs />
          <NavScreenDivider />
          <SocialLinks />
        </div>
      </div>
    ),
  });
  const activeClass = isScreenOpen ? ' rp-nav-hamburger--active' : '';

  return (
    <>
      {isScreenOpen &&
        createPortal(
          <NavScreen isScreenOpen={isScreenOpen} toggleScreen={toggleScreen} />,
          document.getElementById('__rspress_modal_container')!,
        )}
      <button
        onClick={toggleScreen}
        aria-label="mobile hamburger"
        className={`rp-nav-hamburger rp-nav-hamburger__sm${activeClass}`}
      >
        <SvgWrapper icon={IconSmallMenu} />
      </button>
      <button
        aria-label="mobile hamburger"
        className={`rp-nav-hamburger rp-nav-hamburger__md${activeClass}`}
        onClick={handleMouseEnter}
        onMouseEnter={handleMouseEnter}
        onMouseLeave={handleMouseLeave}
      >
        <SvgWrapper icon={IconSmallMenu} />
        {hoverGroup}
      </button>
    </>
  );
}

function isAppearanceSwitchEnabled(darkMode: unknown) {
  const normalized =
    darkMode === false
      ? 'force-light'
      : darkMode === true || darkMode === undefined
        ? 'auto'
        : String(darkMode);
  return !normalized.startsWith('force-');
}

function Nav({
  beforeNavTitle,
  afterNavTitle,
  beforeNavMenu,
  afterNavMenu,
  navTitle,
}: NavProps) {
  const navList = useNav();
  const { site } = useSite();
  const hasAppearanceSwitch = isAppearanceSwitchEnabled(
    site.themeConfig.darkMode,
  );

  return (
    <header className="rp-nav">
      <div className="rp-nav__left">
        {beforeNavTitle}
        {navTitle ?? <NavTitle />}
        <NavMenu menuItems={navList} position="left" />
        {afterNavTitle}
      </div>
      <div className="rp-nav__right">
        {beforeNavMenu}
        <Search />
        <NavMenu menuItems={navList} position="right" />
        <div className="rp-nav__others">
          <NavMenuDivider />
          <NavLangs />
          <NavVersions />
          {hasAppearanceSwitch && <SwitchAppearance />}
          <SocialLinks />
        </div>
        <NavHamburger />
        {afterNavMenu}
      </div>
    </header>
  );
}

export { Nav };
