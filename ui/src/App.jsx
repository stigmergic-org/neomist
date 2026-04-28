import { generateFoamSvg } from '@simplepg/foam-identicon';
import { useEffect, useMemo, useRef, useState } from 'react';

const STARTER_DOMAINS = ['zfi.wei', 'ens.eth', 'simplepage.eth', 'jthor.eth', 'beta.walletbeat.eth', 'vitalik.eth', 'evmnow.eth'];
const CHECKPOINT_REFRESH_MS = 60000;
const SEEDING_REFRESH_MS = 30000;
const PROVIDER_REFRESH_MS = 60000;
const DELEGATED_ROUTING_PROVIDERS_BASE = 'https://delegated-ipfs.dev/routing/v1/providers';
const NEOMIST_NODE_MARKER_CID = 'bafkqaddomvxw22ltoqww433emu';
const RECENT_STORAGE_KEY = 'neomist.recent-domains';
const MAX_RECENT_DOMAINS = 8;
const CHECKPOINT_ICON_SIZE = 32;
const CHECKPOINT_EMOJI_COUNT = 5;
const CHECKPOINT_EMOJI_ALPHABET = [
  '😀', '😁', '😂', '😃', '😄', '😅', '😆', '😉', '😊', '😋', '😌', '😍', '😎', '😏', '😐', '😒',
  '😓', '😔', '😕', '😖', '😘', '😚', '😜', '😝', '😞', '😠', '😢', '😣', '😤', '😨', '😭', '😴',
  '🐶', '🐱', '🐭', '🐹', '🐰', '🐻', '🐼', '🐨', '🐯', '🐮', '🐷', '🐽', '🐸', '🐵', '🙈', '🙉',
  '🙊', '🐒', '🐔', '🐧', '🐦', '🐤', '🐣', '🐥', '🐺', '🐗', '🐴', '🐝', '🐛', '🐌', '🐞', '🐢',
  '🍎', '🍐', '🍊', '🍋', '🍌', '🍉', '🍇', '🍓', '🍒', '🍑', '🍍', '🍅', '🍆', '🌽', '🍄', '🌰',
  '🌷', '🌸', '🌹', '🌺', '🌻', '🌼', '🌿', '🍀', '🍁', '🍂', '🍃', '🌵', '🌴', '🌲', '🌳', '🌱',
  '🎈', '🎉', '🎁', '🎀', '🎂', '🎃', '🎄', '🎆', '🎇', '🎐', '🎨', '🎭', '🎪', '🎯', '🎲', '🎳',
  '🎵', '🎶', '🎷', '🎸', '🎹', '🎺', '🎻', '🎬', '🎮', '🎰', '🚗', '🚕', '🚙', '🚌', '🚲', '🚀',
];
const CHECKPOINT_TEXT_ENCODER = new TextEncoder();
const FOAM_PALETTE_OVERRIDES = {
  '--color-base-content': 'oklch(var(--bc))',
  '--color-primary': 'oklch(var(--p))',
  '--color-secondary': 'oklch(var(--s))',
  '--color-accent': 'oklch(var(--a))',
  '--color-info': 'oklch(var(--in))',
  '--color-success': 'oklch(var(--su))',
  '--color-warning': 'oklch(var(--wa))',
  '--color-error': 'oklch(var(--er))',
};

const PANEL_CLASS =
  'vapor-panel rounded-2xl border border-base-300/80 bg-base-100/80 shadow-sm backdrop-blur';
const SUBTLE_PANEL_CLASS =
  'vapor-subtle-panel rounded-xl border border-base-300/70 bg-base-100/70 shadow-sm backdrop-blur';
const PRIMARY_BUTTON_CLASS =
  'vapor-button-primary inline-flex h-12 items-center justify-center rounded-xl px-5 text-sm font-semibold text-primary-content transition disabled:cursor-not-allowed disabled:opacity-50';
const SECONDARY_BUTTON_CLASS =
  'vapor-button-secondary inline-flex h-12 items-center justify-center rounded-xl border border-base-300 bg-base-100/80 px-5 text-sm font-semibold text-base-content transition disabled:cursor-not-allowed disabled:opacity-50';
const SMALL_SECONDARY_BUTTON_CLASS =
  'vapor-button-secondary inline-flex h-10 items-center justify-center rounded-lg border border-base-300 bg-base-100/75 px-4 text-sm font-semibold text-base-content transition disabled:cursor-not-allowed disabled:opacity-50';
const ICON_BUTTON_CLASS =
  'vapor-icon-button inline-flex h-11 w-11 items-center justify-center rounded-xl border border-base-300 bg-base-100/75 text-base-content transition disabled:cursor-not-allowed disabled:opacity-50';
const INPUT_CLASS =
  'vapor-input h-14 w-full rounded-xl border border-base-300 bg-base-100/85 px-4 text-base outline-none transition placeholder:text-base-content/35';
const CHIP_BUTTON_CLASS =
  'vapor-chip-button rounded-full border border-base-300 px-4 py-2 text-sm font-medium text-base-content';

function classNames(...values) {
  return values.filter(Boolean).join(' ');
}

function checkpointBytes(hash) {
  if (typeof hash !== 'string' || hash.length === 0) {
    return null;
  }

  const normalized = hash.startsWith('0x') ? hash.slice(2) : hash;
  if (normalized.length > 0 && normalized.length % 2 === 0 && /^[0-9a-fA-F]+$/.test(normalized)) {
    const bytes = new Uint8Array(normalized.length / 2);
    for (let index = 0; index < normalized.length; index += 2) {
      bytes[index / 2] = Number.parseInt(normalized.slice(index, index + 2), 16);
    }
    return bytes;
  }

  return CHECKPOINT_TEXT_ENCODER.encode(hash);
}

function formatCheckpoint(hash) {
  const bytes = checkpointBytes(hash);
  if (!bytes || bytes.length === 0) {
    return '';
  }

  const symbols = [];
  let byteIndex = 0;
  let bitBuffer = 0;
  let bitCount = 0;

  while (symbols.length < CHECKPOINT_EMOJI_COUNT) {
    while (bitCount < 7) {
      bitBuffer = bitBuffer * 256 + bytes[byteIndex % bytes.length];
      bitCount += 8;
      byteIndex += 1;
    }

    const shift = bitCount - 7;
    const divisor = 2 ** shift;
    const alphabetIndex = Math.floor(bitBuffer / divisor);

    symbols.push(CHECKPOINT_EMOJI_ALPHABET[alphabetIndex]);
    bitBuffer -= alphabetIndex * divisor;
    bitCount = shift;
  }

  return symbols.join(' ');
}

function formatBytes(value) {
  if (typeof value !== 'number' || Number.isNaN(value) || value < 0) {
    return '-';
  }
  if (value === 0) {
    return '0 B';
  }

  const units = ['B', 'KB', 'MB', 'GB', 'TB', 'PB'];
  let size = value;
  let unitIndex = 0;

  while (size >= 1024 && unitIndex < units.length - 1) {
    size /= 1024;
    unitIndex += 1;
  }

  if (unitIndex === 0) {
    return `${Math.round(size)} ${units[unitIndex]}`;
  }

  return `${size.toFixed(2)} ${units[unitIndex]}`;
}

function formatTimestamp(value) {
  if (!value) {
    return 'Unknown';
  }

  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return value;
  }

  return new Intl.DateTimeFormat(undefined, {
    dateStyle: 'medium',
    timeStyle: 'short',
  }).format(date);
}

function shortCid(value) {
  if (typeof value !== 'string') {
    return '';
  }
  if (value.length <= 18) {
    return value;
  }
  return `${value.slice(0, 10)}...${value.slice(-6)}`;
}

function isIpnsContenthash(value) {
  return value?.protocol === 'ipns' && typeof value.target === 'string' && value.target.length > 0;
}

function decodeRouteSegment(value) {
  try {
    return decodeURIComponent(value);
  } catch {
    return value;
  }
}

function parseRoute(pathname) {
  const normalized = pathname === '/' ? '/' : pathname.replace(/\/+$/, '');

  if (normalized === '/' || normalized === '') {
    return { page: 'home', domain: '' };
  }

  if (normalized === '/settings') {
    return { page: 'settings', domain: '' };
  }

  if (normalized === '/seeding') {
    return { page: 'seeding', domain: '' };
  }

  if (normalized.startsWith('/seeding/')) {
    return {
      page: 'seeding',
      domain: decodeRouteSegment(normalized.slice('/seeding/'.length)),
    };
  }

  return { page: 'not-found', domain: '' };
}

function toSeedingPath(domain) {
  return `/seeding/${encodeURIComponent(domain)}`;
}

function isSupportedDappHost(hostname) {
  const host = hostname.toLowerCase();
  return host.endsWith('.eth') || host.endsWith('.wei');
}

function toRecentDisplay(url) {
  const host = url.hostname.toLowerCase();
  const suffix = `${url.pathname}${url.search}${url.hash}`;
  return suffix === '/' ? host : `${host}${suffix}`;
}

function isContractAddress(value) {
  return /^0x[0-9a-fA-F]{40}$/.test(value);
}

function buildWeb3GatewayUrl(target) {
  return `https://neomist.localhost/web3/${encodeURIComponent(target)}`;
}

function normalizeDappInput(value) {
  const trimmed = value.trim();
  if (!trimmed) {
    return { ok: false, error: 'Enter .eth, .wei, 0x address, or web3:// / web+web3:// URL.' };
  }

  if (isContractAddress(trimmed)) {
    return {
      ok: true,
      url: `https://neomist.localhost/web3/${trimmed}/`,
      recentValue: `web3://${trimmed}/`,
    };
  }

  if (/^(?:web3|web\+web3):\/\//i.test(trimmed)) {
    return {
      ok: true,
      url: buildWeb3GatewayUrl(trimmed),
      recentValue: trimmed,
    };
  }

  const candidate = /^[a-z][a-z0-9+.-]*:\/\//i.test(trimmed)
    ? trimmed
    : `https://${trimmed}`;

  let url;
  try {
    url = new URL(candidate);
  } catch {
    return { ok: false, error: 'Enter valid .eth, .wei, 0x address, or web3:// / web+web3:// URL.' };
  }

  if (!isSupportedDappHost(url.hostname)) {
    return { ok: false, error: 'Only .eth, .wei, 0x address, web3://, and web+web3:// targets are supported.' };
  }

  url.protocol = 'https:';

  return {
    ok: true,
    url: url.toString(),
    recentValue: toRecentDisplay(url),
    domain: url.hostname.toLowerCase(),
  };
}

function readRecentDomains() {
  try {
    const raw = window.localStorage.getItem(RECENT_STORAGE_KEY);
    const parsed = raw ? JSON.parse(raw) : [];
    if (!Array.isArray(parsed)) {
      return [];
    }
    return parsed.filter((value) => typeof value === 'string').slice(0, MAX_RECENT_DOMAINS);
  } catch {
    return [];
  }
}

function writeRecentDomains(values) {
  try {
    window.localStorage.setItem(RECENT_STORAGE_KEY, JSON.stringify(values));
  } catch {
    // noop
  }
}

function upsertRecentDomain(currentValues, nextValue) {
  return [nextValue, ...currentValues.filter((value) => value !== nextValue)].slice(
    0,
    MAX_RECENT_DOMAINS
  );
}

function getCoverage(localSize, fullSize) {
  const local = typeof localSize === 'number' && localSize > 0 ? localSize : 0;
  const full = typeof fullSize === 'number' && fullSize > 0 ? fullSize : 0;

  if (full === 0) {
    return {
      ratio: local > 0 ? 1 : 0,
      label: local > 0 ? 'Stored' : 'Unknown',
      detail: local > 0 ? `${formatBytes(local)} stored` : 'Size unavailable',
      tone: local > 0 ? 'success' : 'neutral',
      isPartial: false,
      isComplete: local > 0,
    };
  }

  const ratio = Math.max(0, Math.min(local / full, 1));
  const percent = Math.round(ratio * 100);

  if (ratio >= 0.999) {
    return {
      ratio: 1,
      label: 'Complete',
      detail: '100% cached',
      tone: 'success',
      isPartial: false,
      isComplete: true,
    };
  }

  if (ratio === 0) {
    return {
      ratio: 0,
      label: 'Pending',
      detail: '0% cached',
      tone: 'warning',
      isPartial: true,
      isComplete: false,
    };
  }

  return {
    ratio,
    label: 'Partial',
    detail: `${percent}% cached`,
    tone: 'warning',
    isPartial: true,
    isComplete: false,
  };
}

function buildSeedingSummary(domains) {
  let following = 0;
  let partial = 0;
  let complete = 0;

  for (const domain of domains) {
    if (domain.auto_seeding) {
      following += 1;
    }

    const coverage = getCoverage(domain.local_size, domain.full_size);
    if (coverage.isPartial) {
      partial += 1;
    }
    if (coverage.isComplete) {
      complete += 1;
    }
  }

  return {
    total: domains.length,
    following,
    partial,
    complete,
  };
}

function latestTrackingStatus(enabled) {
  return enabled ? 'Following' : 'Cached';
}

function latestTrackingAction(enabled) {
  return enabled ? 'Stop following' : 'Follow';
}

function checkpointExplorerUrl(hash) {
  return `https://beaconcha.in/slot/${encodeURIComponent(hash)}`;
}

function openInNewTab(url) {
  const link = document.createElement('a');
  link.href = url;
  link.target = '_blank';
  link.rel = 'noopener noreferrer';
  link.style.display = 'none';
  document.body.appendChild(link);
  link.click();
  document.body.removeChild(link);
}

function useBrowserPath() {
  const [pathname, setPathname] = useState(() => window.location.pathname);

  useEffect(() => {
    const handlePopState = () => {
      setPathname(window.location.pathname);
    };

    window.addEventListener('popstate', handlePopState);
    return () => {
      window.removeEventListener('popstate', handlePopState);
    };
  }, []);

  const navigate = (path, options = {}) => {
    if (!path || path === window.location.pathname) {
      return;
    }

    const method = options.replace ? 'replaceState' : 'pushState';
    window.history[method]({}, '', path);
    setPathname(path);
    window.scrollTo({ top: 0, left: 0 });
  };

  return { pathname, navigate };
}

function useCheckpoints() {
  const [checkpoints, setCheckpoints] = useState([]);
  const [error, setError] = useState('');

  useEffect(() => {
    let mounted = true;

    const load = async () => {
      try {
        const response = await fetch('/api/helios/checkpoints');
        if (!response.ok) {
          throw new Error('Failed to load checkpoints');
        }

        const data = await response.json();
        if (!mounted) {
          return;
        }

        setCheckpoints(Array.isArray(data.checkpoints) ? data.checkpoints : []);
        setError('');
      } catch {
        if (!mounted) {
          return;
        }

        setCheckpoints([]);
        setError('Failed to load checkpoints.');
      }
    };

    void load();
    const interval = window.setInterval(() => {
      void load();
    }, CHECKPOINT_REFRESH_MS);

    return () => {
      mounted = false;
      window.clearInterval(interval);
    };
  }, []);

  return { checkpoints, error };
}

function useSeedingData() {
  const [domains, setDomains] = useState([]);
  const [storageUsed, setStorageUsed] = useState('-');
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState('');
  const mountedRef = useRef(true);
  const hasLoadedRef = useRef(false);

  useEffect(() => {
    return () => {
      mountedRef.current = false;
    };
  }, []);

  async function load(background = false) {
    if (!hasLoadedRef.current) {
      setLoading(true);
    } else if (!background) {
      setRefreshing(true);
    }

    try {
      const [domainsRes, storageRes] = await Promise.all([
        fetch('/api/cached-domains'),
        fetch('/api/total-storage'),
      ]);

      if (!domainsRes.ok || !storageRes.ok) {
        throw new Error('Failed to load seeding data');
      }

      const domainsData = await domainsRes.json();
      const storageData = await storageRes.json();

      if (!mountedRef.current) {
        return;
      }

      setDomains(Array.isArray(domainsData) ? domainsData : []);
      setStorageUsed(storageData.totalUsed || '-');
      setError('');
    } catch {
      if (!mountedRef.current) {
        return;
      }

      setError('Failed to load seeding data.');
    } finally {
      if (!mountedRef.current) {
        return;
      }

      hasLoadedRef.current = true;
      setLoading(false);
      setRefreshing(false);
    }
  }

  useEffect(() => {
    void load(false);
    const interval = window.setInterval(() => {
      void load(true);
    }, SEEDING_REFRESH_MS);

    return () => {
      window.clearInterval(interval);
    };
  }, []);

  return {
    domains,
    storageUsed,
    loading,
    refreshing,
    error,
    reload: () => load(false),
  };
}

function useNodeProviderStats() {
  const [count, setCount] = useState(null);

  useEffect(() => {
    let mounted = true;

    const load = async () => {
      try {
        const nextCount = await fetchDelegatedProviderCount(NEOMIST_NODE_MARKER_CID);
        if (!mounted) {
          return;
        }

        setCount(nextCount);
      } catch {
        if (!mounted) {
          return;
        }

        setCount(null);
      }
    };

    void load();
    const interval = window.setInterval(() => {
      void load();
    }, PROVIDER_REFRESH_MS);

    return () => {
      mounted = false;
      window.clearInterval(interval);
    };
  }, []);

  return count;
}

function useProviderCounts(cids) {
  const [counts, setCounts] = useState({});
  const normalizedCids = useMemo(
    () =>
      [...new Set(cids.filter((cid) => typeof cid === 'string' && cid.length > 0))],
    [cids]
  );

  useEffect(() => {
    let mounted = true;

    const load = async () => {
      if (normalizedCids.length === 0) {
        if (mounted) {
          setCounts({});
        }
        return;
      }

      try {
        const entries = await Promise.all(
          normalizedCids.map(async (cid) => [cid, await fetchDelegatedProviderCount(cid)])
        );
        if (!mounted) {
          return;
        }

        setCounts(Object.fromEntries(entries));
      } catch {
        if (!mounted) {
          return;
        }

        setCounts({});
      }
    };

    void load();
    const interval = window.setInterval(() => {
      void load();
    }, PROVIDER_REFRESH_MS);

    return () => {
      mounted = false;
      window.clearInterval(interval);
    };
  }, [normalizedCids]);

  return counts;
}

async function fetchDelegatedProviderCount(cid) {
  const response = await fetch(`${DELEGATED_ROUTING_PROVIDERS_BASE}/${encodeURIComponent(cid)}`);
  if (!response.ok) {
    throw new Error('Failed to load delegated routing providers');
  }

  const data = await response.json();
  const providers = Array.isArray(data?.Providers) ? data.Providers : [];
  return new Set(
    providers
      .map((provider) => (typeof provider?.ID === 'string' ? provider.ID : ''))
      .filter(Boolean)
  ).size;
}

function formatProviderCount(value) {
  return typeof value === 'number' ? String(value) : '-';
}

function App() {
  const { pathname, navigate } = useBrowserPath();
  const route = useMemo(() => parseRoute(pathname), [pathname]);
  const { checkpoints, error: checkpointError } = useCheckpoints();
  const {
    domains,
    storageUsed,
    loading: seedingLoading,
    error: seedingError,
    reload: reloadSeeding,
  } = useSeedingData();
  const [recentDomains, setRecentDomains] = useState(() => readRecentDomains());

  const seedingSummary = useMemo(() => buildSeedingSummary(domains), [domains]);

  useEffect(() => {
    if (window.location.protocol !== 'https:') {
      return;
    }
    if (typeof navigator.registerProtocolHandler !== 'function') {
      return;
    }

    try {
      navigator.registerProtocolHandler(
        'web+web3',
        'https://neomist.localhost/web3/%s',
        'NeoMist'
      );
    } catch {
      // Browser can reject duplicate or unsupported registrations.
    }
  }, []);

  useEffect(() => {
    if (route.page === 'home') {
      document.title = 'NeoMist';
      return;
    }

    if (route.page === 'seeding' && route.domain) {
      document.title = `${route.domain} - NeoMist`;
      return;
    }

    if (route.page === 'seeding') {
      document.title = 'Seeding - NeoMist';
      return;
    }

    if (route.page === 'settings') {
      document.title = 'Settings - NeoMist';
      return;
    }

    document.title = 'NeoMist';
  }, [route]);

  const rememberRecentTarget = (value) => {
    const result = normalizeDappInput(value);
    if (!result.ok) {
      return result;
    }

    const nextValues = upsertRecentDomain(recentDomains, result.recentValue);
    writeRecentDomains(nextValues);
    setRecentDomains(nextValues);
    return result;
  };

  const openDapp = (value) => {
    const result = rememberRecentTarget(value);
    if (!result.ok) {
      return result;
    }

    openInNewTab(result.url);
    return result;
  };

  const clearRecent = () => {
    writeRecentDomains([]);
    setRecentDomains([]);
  };

  return (
    <div className="vapor-shell min-h-screen">
      <div className="mx-auto flex min-h-screen max-w-[1480px] flex-col px-4 pb-6 pt-4 sm:px-6 sm:pb-8 sm:pt-6">
        <Header route={route} navigate={navigate} seedingCount={seedingSummary.total} />

        <main className="mt-6 flex-1 animate-rise">
          {route.page === 'home' ? (
            <HomePage
              openDapp={openDapp}
              recentDomains={recentDomains}
              clearRecent={clearRecent}
              navigate={navigate}
              seedingSummary={seedingSummary}
              seedingDomains={domains}
              seedingLoading={seedingLoading}
              seedingError={seedingError}
              storageUsed={storageUsed}
              checkpoints={checkpoints}
              checkpointError={checkpointError}
            />
          ) : null}

          {route.page === 'seeding' ? (
            <SeedingPage
              routeDomain={route.domain}
              domains={domains}
              storageUsed={storageUsed}
              loading={seedingLoading}
              error={seedingError}
              summary={seedingSummary}
              navigate={navigate}
              reload={reloadSeeding}
              rememberRecentTarget={rememberRecentTarget}
            />
          ) : null}

          {route.page === 'settings' ? <SettingsPage /> : null}

          {route.page === 'not-found' ? <NotFoundPage navigate={navigate} /> : null}
        </main>
      </div>
    </div>
  );
}

function Header({ route, navigate, seedingCount }) {
  const openActive = route.page === 'home';
  const seedingActive = route.page === 'seeding';
  const settingsActive = route.page === 'settings';

  return (
    <header className={classNames(PANEL_CLASS, 'flex flex-col gap-4 px-4 py-4 sm:px-5 lg:flex-row lg:items-center lg:justify-between')}>
      <button
        type="button"
        onClick={() => navigate('/')}
        className="flex items-center gap-3 rounded-xl px-2 py-1 text-left transition hover:bg-base-100/55"
      >
        <picture>
          <source srcSet="/icon-dark.svg" media="(prefers-color-scheme: dark)" />
          <img src="/icon.svg" alt="NeoMist icon" className="h-10 w-10" />
        </picture>

        <div>
          <p className="text-lg font-semibold tracking-tight">NeoMist</p>
          <p className="text-sm text-base-content/60">a world computer in your pocket</p>
        </div>
      </button>

      <div className="flex w-full items-center justify-between gap-3 lg:w-auto lg:justify-end">
        <nav className="vapor-nav flex items-center gap-1 rounded-full border border-base-300/70 bg-base-100/70 p-1">
          <button
            type="button"
            aria-current={openActive ? 'page' : undefined}
            className={classNames(
              'rounded-full border px-4 py-2 text-sm font-medium transition',
              openActive
                ? 'border-primary/18 bg-primary/12 text-primary shadow-sm shadow-primary/10'
                : 'border-transparent text-base-content/65 hover:border-base-300 hover:bg-base-100/75 hover:text-base-content'
            )}
            onClick={() => navigate('/')}
          >
            Explore
          </button>

          <button
            type="button"
            aria-current={seedingActive ? 'page' : undefined}
            className={classNames(
              'flex items-center gap-2 rounded-full border px-4 py-2 text-sm font-medium transition',
              seedingActive
                ? 'border-primary/18 bg-primary/12 text-primary shadow-sm shadow-primary/10'
                : 'border-transparent text-base-content/65 hover:border-base-300 hover:bg-base-100/75 hover:text-base-content'
            )}
            onClick={() => navigate('/seeding')}
          >
            <span>Seeding</span>
            <span
              className={classNames(
                'rounded-full px-2 py-0.5 text-xs',
                seedingActive
                  ? 'bg-primary/10 text-primary'
                  : 'vapor-badge text-base-content/65'
              )}
            >
              {seedingCount}
            </span>
          </button>
        </nav>

        <button
          type="button"
          aria-label="Settings"
          className={classNames(
            ICON_BUTTON_CLASS,
            settingsActive ? 'border-accent/20 bg-base-100/85' : ''
          )}
          onClick={() => navigate('/settings')}
        >
          <SettingsIcon />
        </button>
      </div>
    </header>
  );
}

function useEnsSearch(query) {
  const [suggestions, setSuggestions] = useState([]);
  const [isSearching, setIsSearching] = useState(false);

  useEffect(() => {
    const trimmed = query.trim().toLowerCase();
    if (trimmed.length < 3) {
      setSuggestions([]);
      setIsSearching(false);
      return;
    }

    let mounted = true;
    setIsSearching(true);

    const timer = setTimeout(async () => {
      try {
        let skip = 0;
        const limit = 100;
        const maxSkip = 500;
        let found = [];

        while (found.length < 5 && skip < maxSkip && mounted) {
          const response = await fetch('https://api.mainnet.ensnode.io/subgraph', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
              query: `query Search($search: String!, $skip: Int!) {
                domains(first: ${limit}, skip: $skip, where: { name_starts_with: $search, resolver_not: null }, orderBy: subdomainCount, orderDirection: desc) {
                  name
                  resolver {
                    contentHash
                  }
                }
              }`,
              variables: { search: trimmed, skip },
            }),
          });

          if (!response.ok) throw new Error('API error');
          const { data } = await response.json();

          if (!mounted) return;

          if (data && data.domains) {
            const valid = data.domains
              .filter((d) => d.resolver?.contentHash?.startsWith('0xe3'))
              .map((d) => d.name)
              .filter((name) => name.toLowerCase().includes(trimmed));

            for (const v of valid) {
              if (!found.includes(v)) {
                found.push(v);
              }
            }

            if (mounted && found.length > 0) {
              setSuggestions([...found].slice(0, 5));
            }

            if (found.length >= 5) {
              break;
            }

            if (data.domains.length < limit) {
              break;
            }
          } else {
            break;
          }

          skip += limit;
        }
      } catch (err) {
        console.warn('Failed to fetch ENS suggestions:', err);
        if (mounted) setSuggestions([]);
      } finally {
        if (mounted) setIsSearching(false);
      }
    }, 300);

    return () => {
      mounted = false;
      clearTimeout(timer);
    };
  }, [query]);

  return { suggestions, isSearching };
}

function HomePage({
  openDapp,
  recentDomains,
  clearRecent,
  navigate,
  seedingSummary,
  seedingDomains,
  seedingLoading,
  seedingError,
  storageUsed,
  checkpoints,
  checkpointError,
}) {
  const [value, setValue] = useState('');
  const [inputError, setInputError] = useState('');
  const [isFocused, setIsFocused] = useState(false);
  const [selectedIndex, setSelectedIndex] = useState(-1);
  const inputRef = useRef(null);
  const containerRef = useRef(null);

  const { suggestions, isSearching } = useEnsSearch(value);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  useEffect(() => {
    setSelectedIndex(-1);
  }, [suggestions]);

  useEffect(() => {
    const handleClickOutside = (event) => {
      if (containerRef.current && !containerRef.current.contains(event.target)) {
        setIsFocused(false);
      }
    };
    document.addEventListener('mousedown', handleClickOutside);
    return () => document.removeEventListener('mousedown', handleClickOutside);
  }, []);

  useEffect(() => {
    const handleKeyDown = (event) => {
      if (event.key !== '/' || event.metaKey || event.ctrlKey || event.altKey) {
        return;
      }

      const target = event.target;
      const isTypingTarget =
        target instanceof HTMLElement &&
        (target.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/i.test(target.tagName));

      if (isTypingTarget) {
        return;
      }

      event.preventDefault();
      inputRef.current?.focus();
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => {
      window.removeEventListener('keydown', handleKeyDown);
    };
  }, []);

  const handleSubmit = (event) => {
    event.preventDefault();
    const result = openDapp(value);
    if (!result.ok) {
      setInputError(result.error);
      return;
    }

    setInputError('');
  };

  return (
    <div className="grid min-h-[calc(100vh-156px)] gap-6 xl:grid-cols-[minmax(0,1.15fr)_420px]">
      <section className={classNames(PANEL_CLASS, 'relative overflow-hidden px-6 py-8 sm:px-8 sm:py-10')}>
        <div className="pointer-events-none absolute inset-0 opacity-80">
          <div className="absolute -left-20 top-0 h-72 w-72 rounded-full bg-primary/10 blur-3xl" />
          <div className="absolute bottom-0 right-0 h-64 w-64 rounded-full bg-accent/10 blur-3xl" />
        </div>

        <div className="relative z-10 max-w-3xl">
          <h1 className="text-4xl font-semibold leading-[1.02] tracking-tight sm:text-5xl lg:text-6xl">
            Explore the Ethereum ecosystem.
          </h1>

          <p className="mt-5 max-w-2xl text-base leading-7 text-base-content/70">
            Type a .eth, .wei, or `web3://` URL and press
            Enter. NeoMist resolves content locally, including onchain web3 sites served straight
            from mainnet contracts.
          </p>

          <form onSubmit={handleSubmit} className="relative mt-10 grid gap-3 md:grid-cols-[minmax(0,1fr)_auto]" ref={containerRef}>
            <div className="relative">
              <input
                ref={inputRef}
                value={value}
                onFocus={() => setIsFocused(true)}
                onKeyDown={(event) => {
                  if (!isFocused || suggestions.length === 0) return;

                  if (event.key === 'ArrowDown') {
                    event.preventDefault();
                    setSelectedIndex((prev) => (prev < suggestions.length - 1 ? prev + 1 : prev));
                  } else if (event.key === 'ArrowUp') {
                    event.preventDefault();
                    setSelectedIndex((prev) => (prev > 0 ? prev - 1 : -1));
                  } else if (event.key === 'Enter' && selectedIndex >= 0) {
                    event.preventDefault();
                    const selected = suggestions[selectedIndex];
                    setValue(selected);
                    setIsFocused(false);
                    openDapp(selected);
                  }
                }}
                onChange={(event) => {
                  setValue(event.target.value);
                  setIsFocused(true);
                  if (inputError) {
                    setInputError('');
                  }
                }}
                className={INPUT_CLASS}
                placeholder="app.eth, app.wei, or web3://..."
                aria-label="Dapp target"
                autoComplete="off"
                spellCheck="false"
              />

              {isFocused && (value.trim().length >= 3) && (suggestions.length > 0 || isSearching) && (
                <div className={classNames(SUBTLE_PANEL_CLASS, 'absolute left-0 top-[calc(100%+8px)] z-50 w-full shadow-xl')}>
                  {isSearching && suggestions.length === 0 ? (
                    <div className="flex items-center gap-3 px-4 py-3 text-sm text-base-content/55">
                      <span className="loading loading-spinner loading-sm" />
                      Searching...
                    </div>
                  ) : (
                    <ul className="py-1">
                      {suggestions.map((suggestion, index) => (
                        <li key={suggestion}>
                          <button
                            type="button"
                            className={classNames(
                              'w-full px-4 py-2.5 text-left text-sm transition',
                              selectedIndex === index ? 'bg-base-200/80 text-primary' : 'hover:bg-base-200/50'
                            )}
                            onMouseEnter={() => setSelectedIndex(index)}
                            onClick={() => {
                              setValue(suggestion);
                              setIsFocused(false);
                              openDapp(suggestion);
                            }}
                          >
                            {suggestion}
                          </button>
                        </li>
                      ))}
                    </ul>
                  )}
                  
                  {isSearching && suggestions.length > 0 ? (
                    <div className="absolute right-3 top-3">
                      <span className="loading loading-spinner loading-xs text-base-content/40" />
                    </div>
                  ) : null}
                </div>
              )}
            </div>

            <button type="submit" className={PRIMARY_BUTTON_CLASS}>
              Open
            </button>
          </form>

          <p className={classNames('mt-3 text-sm', inputError ? 'text-error' : 'text-base-content/55')}>
            {inputError || 'Press Enter to open in a new tab. Press / at any time to focus the field.'}
          </p>

          <div className={classNames(SUBTLE_PANEL_CLASS, 'mt-12 p-6')}>
            {recentDomains.length > 0 ? (
              <div>
                <div className="flex items-center justify-between gap-4">
                  <div>
                    <p className="text-sm font-medium">Recently opened</p>
                    <p className="mt-1 text-sm text-base-content/60">
                      Jump back in.
                    </p>
                  </div>

                  <button
                    type="button"
                    className="text-sm text-base-content/55 transition hover:text-base-content"
                    onClick={clearRecent}
                  >
                    Clear recent
                  </button>
                </div>

                <div className="mt-5 flex flex-wrap gap-3">
                  {recentDomains.map((domain) => (
                    <button
                      key={domain}
                      type="button"
                      className={CHIP_BUTTON_CLASS}
                      onClick={() => openDapp(domain)}
                    >
                      {domain}
                    </button>
                  ))}
                </div>
              </div>
            ) : null}

            <div className={recentDomains.length > 0 ? 'mt-8 border-t border-base-300/60 pt-6' : ''}>
              <p className="text-sm font-medium">Examples</p>
              <p className="mt-1 text-sm text-base-content/60">
                Common dapps you can open in one click.
              </p>

              <div className="mt-5 flex flex-wrap gap-3">
                {STARTER_DOMAINS.map((domain) => (
                  <button
                    key={domain}
                    type="button"
                    className={CHIP_BUTTON_CLASS}
                    onClick={() => openDapp(domain)}
                  >
                    {domain}
                  </button>
                ))}
              </div>
            </div>
          </div>
        </div>
      </section>

      <aside className="grid content-start gap-6">
        <SeedingOverviewPanel
          summary={seedingSummary}
          domains={seedingDomains}
          loading={seedingLoading}
          error={seedingError}
          storageUsed={storageUsed}
          navigate={navigate}
        />

        <CheckpointPanel checkpoints={checkpoints} error={checkpointError} />
      </aside>
    </div>
  );
}

function SeedingOverviewPanel({ summary, domains, loading, error, storageUsed, navigate }) {
  const spotlightDomains = useMemo(() => {
    return [...domains]
      .sort((left, right) => {
        const timeCompare = (right.cached_at || '').localeCompare(left.cached_at || '');
        if (timeCompare !== 0) {
          return timeCompare;
        }

        return left.domain.localeCompare(right.domain);
      })
      .slice(0, 3);
  }, [domains]);

  return (
    <section className={classNames(PANEL_CLASS, 'p-6')}>
      <div className="flex items-start justify-between gap-4">
        <div>
          <h2 className="text-2xl font-semibold tracking-tight">Seeding</h2>
          <p className="mt-2 text-sm leading-6 text-base-content/65">
            See what is fully stored, what is partial, and which domains follow the latest dapp
            versions.
          </p>
        </div>

        <button type="button" className={SMALL_SECONDARY_BUTTON_CLASS} onClick={() => navigate('/seeding')}>
          Details
        </button>
      </div>

        <div className="mt-6 grid gap-3 sm:grid-cols-2">
        <MetricTile label="Sites" value={summary.total} />
        <MetricTile label="Following" value={summary.following} />
        <MetricTile label="Partial" value={summary.partial} />
        <MetricTile label="Storage" value={storageUsed} />
      </div>

      <div className="mt-6">
        <div className="flex items-center justify-between gap-4">
          <p className="text-sm font-medium">Recently cached</p>
          {loading ? <span className="text-xs text-base-content/50">Updating...</span> : null}
        </div>

        <div className="mt-4 grid gap-2">
          {error ? (
            <p className="text-sm text-error">{error}</p>
          ) : spotlightDomains.length === 0 ? (
            <p className="text-sm leading-6 text-base-content/60">
              No sites cached yet. Open a dapp to start building local coverage.
            </p>
          ) : (
            spotlightDomains.map((domain) => {
              const coverage = getCoverage(domain.local_size, domain.full_size);

              return (
                <button
                  key={domain.domain}
                  type="button"
                  className={classNames(
                    SUBTLE_PANEL_CLASS,
                    'w-full px-4 py-3 text-left transition hover:border-base-content/15 hover:bg-base-100/80'
                  )}
                  onClick={() => navigate(toSeedingPath(domain.domain))}
                >
                  <div className="flex items-center justify-between gap-4">
                    <div>
                      <p className="text-sm font-medium">{domain.domain}</p>
                      <p className="mt-1 text-xs text-base-content/55">{formatTimestamp(domain.cached_at)}</p>
                    </div>

                    <StatusPill tone={domain.auto_seeding ? 'info' : 'neutral'}>
                      {latestTrackingStatus(domain.auto_seeding)}
                    </StatusPill>
                  </div>

                  <div className="mt-3 flex items-center gap-3">
                    <div className="min-w-0 flex-1">
                      <ProgressBar ratio={coverage.ratio} tone={coverage.tone} />
                    </div>
                    <StatusPill tone={coverage.tone}>{coverage.label}</StatusPill>
                  </div>

                  <p className="mt-2 text-xs text-base-content/55">
                    {formatBytes(domain.local_size)} stored of {formatBytes(domain.full_size)}
                  </p>
                </button>
              );
            })
          )}
        </div>
      </div>
    </section>
  );
}

function CheckpointPanel({ checkpoints, error }) {
  const checkpointIcons = useMemo(
    () =>
      checkpoints.slice(0, 5).map((hash) => ({
        hash,
        fingerprint: formatCheckpoint(hash),
        svg: generateFoamSvg(hash, CHECKPOINT_ICON_SIZE, {
          paletteOverrides: FOAM_PALETTE_OVERRIDES,
        }),
      })),
    [checkpoints]
  );

  return (
    <section className={classNames(PANEL_CLASS, 'p-6')}>
      <h2 className="text-2xl font-semibold tracking-tight">Consensus checkpoints</h2>
      <p className="mt-2 text-sm leading-6 text-base-content/65">
        Compare these checkpoints with a friend to ensure you see the same version of reality.
      </p>

      <div className="mt-6 grid gap-3">
        {error ? (
          <p className="text-sm text-error">{error}</p>
        ) : checkpoints.length === 0 ? (
          <p className="text-sm leading-6 text-base-content/60">Waiting for checkpoints.</p>
        ) : (
          checkpointIcons.map(({ hash, fingerprint, svg }) => (
            <a
              key={hash}
              href={checkpointExplorerUrl(hash)}
              target="_blank"
              rel="noreferrer"
              aria-label={`Open checkpoint ${hash} in explorer`}
              title={hash}
              className={classNames(
                SUBTLE_PANEL_CLASS,
                'flex items-center justify-between gap-4 px-4 py-3 text-left transition hover:border-base-content/15 hover:bg-base-100/80'
              )}
            >
              <div className="flex items-center gap-3">
                <span className="inline-flex h-8 w-8 shrink-0 items-center justify-center overflow-hidden rounded-lg">
                  <span
                    role="img"
                    aria-hidden="true"
                    className="inline-flex h-8 w-8 [&>svg]:h-8 [&>svg]:w-8"
                    dangerouslySetInnerHTML={{ __html: svg }}
                  />
                </span>
                <span className="text-xl leading-none sm:text-2xl">{fingerprint}</span>
              </div>
              <span className="text-xs text-base-content/55">Open explorer</span>
            </a>
          ))
        )}
      </div>
    </section>
  );
}

function SeedingPage({
  routeDomain,
  domains,
  storageUsed,
  loading,
  error,
  summary,
  navigate,
  reload,
  rememberRecentTarget,
}) {
  const [search, setSearch] = useState('');
  const [filter, setFilter] = useState('all');
  const [actionKey, setActionKey] = useState('');
  const [actionError, setActionError] = useState('');
  const nodeProviderCount = useNodeProviderStats();

  const selectedDomain = useMemo(
    () => domains.find((domain) => domain.domain === routeDomain) || null,
    [domains, routeDomain]
  );

  const filteredDomains = useMemo(() => {
    const query = search.trim().toLowerCase();

    return [...domains]
      .filter((domain) => {
        const coverage = getCoverage(domain.local_size, domain.full_size);

        if (query && !domain.domain.toLowerCase().includes(query)) {
          return false;
        }

        if (filter === 'partial' && !coverage.isPartial) {
          return false;
        }

        if (filter === 'following' && !domain.auto_seeding) {
          return false;
        }

        return true;
      })
      .sort((left, right) => {
        const timeCompare = (right.cached_at || '').localeCompare(left.cached_at || '');
        if (timeCompare !== 0) {
          return timeCompare;
        }

        return left.domain.localeCompare(right.domain);
      });
  }, [domains, filter, search]);

  const detailDomain = routeDomain ? selectedDomain : filteredDomains[0] || null;
  const selectedMissing = Boolean(routeDomain) && !selectedDomain && !loading;
  const providerCids = useMemo(
    () => [
      ...new Set([
        ...domains.map((domain) => domain?.versions?.[0]?.cid),
        ...(detailDomain?.versions?.map((version) => version?.cid) || []),
      ].filter((cid) => typeof cid === 'string' && cid.length > 0)),
    ],
    [detailDomain, domains]
  );
  const providerCounts = useProviderCounts(providerCids);

  const filters = [
    { id: 'all', label: 'All' },
    { id: 'following', label: 'Following' },
    { id: 'partial', label: 'Partial' },
  ];

  const toggleLatestTracking = async (domain, current) => {
    setActionKey(`follow:${domain}`);
    setActionError('');

    try {
      const response = await fetch(
        `/api/toggle-auto-seed?domain=${encodeURIComponent(domain)}&enable=${!current}`,
        { method: 'POST' }
      );

      if (!response.ok) {
        throw new Error('Failed to update latest tracking');
      }

      await reload();
    } catch {
      setActionError('Failed to update latest tracking.');
    } finally {
      setActionKey('');
    }
  };

  const clearDomainCache = async (domain) => {
    setActionKey(`clear-domain:${domain}`);
    setActionError('');

    try {
      const response = await fetch(`/api/clear-cache?domain=${encodeURIComponent(domain)}`, {
        method: 'POST',
      });

      if (!response.ok) {
        throw new Error('Failed to remove domain cache');
      }

      await reload();
      if (routeDomain === domain) {
        navigate('/seeding');
      }
    } catch {
      setActionError('Failed to remove seeded site.');
    } finally {
      setActionKey('');
    }
  };

  const clearVersionCache = async (domain, timestamp) => {
    setActionKey(`clear-version:${domain}:${timestamp}`);
    setActionError('');

    try {
      const response = await fetch(
        `/api/clear-cache?domain=${encodeURIComponent(domain)}&version=${encodeURIComponent(
          String(timestamp)
        )}`,
        {
          method: 'POST',
        }
      );

      if (!response.ok) {
        throw new Error('Failed to remove snapshot');
      }

      await reload();
    } catch {
      setActionError('Failed to remove cached snapshot.');
    } finally {
      setActionKey('');
    }
  };

  return (
    <section className="grid gap-6">
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div>
          <h1 className="text-4xl font-semibold tracking-tight">Seeding</h1>
          <p className="mt-3 max-w-3xl text-sm leading-6 text-base-content/65">
            Track what is fully cached, what is still partial, and which domains automatically
            follow the latest dapp versions.
          </p>
        </div>

        <div className={classNames(SUBTLE_PANEL_CLASS, 'shrink-0 px-4 py-3')}>
          <p className="flex items-center gap-3 whitespace-nowrap leading-none">
            <span className="text-2xl font-semibold tracking-tight leading-none">
              {formatProviderCount(nodeProviderCount)}
            </span>
            <span className="text-[11px] font-medium uppercase tracking-[0.22em] text-base-content/45 leading-none">
              neomist nodes
            </span>
          </p>
        </div>
      </div>

      <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-4">
        <MetricTile label="Sites" value={summary.total} />
        <MetricTile label="Following" value={summary.following} />
        <MetricTile label="Partial" value={summary.partial} />
        <MetricTile label="Storage" value={storageUsed} />
      </div>

      {error ? <p className="text-sm text-error">{error}</p> : null}
      {actionError ? <p className="text-sm text-error">{actionError}</p> : null}

      <div className="grid gap-6 xl:grid-cols-[360px_minmax(0,1fr)]">
        <section className={classNames(PANEL_CLASS, 'flex flex-col p-5 xl:min-h-[720px]')}>
          <div>
            <label className="mb-2 block text-sm font-medium">Search domains</label>
            <input
              value={search}
              onChange={(event) => setSearch(event.target.value)}
              className={classNames(INPUT_CLASS, 'h-12 text-sm')}
              placeholder="Filter by domain"
              aria-label="Search seeded domains"
              autoComplete="off"
              spellCheck="false"
            />
          </div>

          <div className="mt-4 flex flex-wrap gap-2">
            {filters.map((item) => (
              <button
                key={item.id}
                type="button"
                onClick={() => setFilter(item.id)}
                className={classNames('btn btn-sm', filter === item.id ? 'btn-primary' : 'btn-ghost')}
              >
                {item.label}
              </button>
            ))}
          </div>

          <div className="mt-5 flex-1 overflow-y-auto pr-1">
            {loading ? (
              <div className="flex h-full items-center justify-center text-sm text-base-content/55">
                <span className="loading loading-spinner loading-sm mr-2" />
                Loading seeding data...
              </div>
            ) : filteredDomains.length === 0 ? (
              <div className="flex h-full items-center justify-center text-center text-sm leading-6 text-base-content/60">
                No domains match the current view.
              </div>
            ) : (
              <div className="grid gap-3">
                {filteredDomains.map((domain) => {
                  const coverage = getCoverage(domain.local_size, domain.full_size);
                  const active = detailDomain?.domain === domain.domain;
                  const latestCid = domain.versions?.[0]?.cid || '';
                  const latestProviderCount = latestCid ? providerCounts[latestCid] : null;
                  const hasIpnsContenthash = isIpnsContenthash(domain.contenthash);

                  return (
                    <button
                      key={domain.domain}
                      type="button"
                      onClick={() => navigate(toSeedingPath(domain.domain))}
                      className={classNames(
                        SUBTLE_PANEL_CLASS,
                        'w-full px-4 py-4 text-left transition',
                        active
                          ? 'border-primary/30 bg-primary/8'
                          : 'hover:border-base-content/15 hover:bg-base-100/80'
                      )}
                    >
                      <div className="flex items-start justify-between gap-4">
                        <div>
                          <p className="text-sm font-medium">{domain.domain}</p>
                          <p className="mt-1 text-xs text-base-content/55">
                            {formatTimestamp(domain.cached_at)}
                          </p>
                        </div>

                        <div className="flex flex-wrap items-center justify-end gap-2">
                          {hasIpnsContenthash ? (
                            <StatusPill tone="warning">
                              <WarningIcon className="mr-1 h-3.5 w-3.5" />
                              IPNS
                            </StatusPill>
                          ) : null}

                          <StatusPill tone={domain.auto_seeding ? 'info' : 'neutral'}>
                            {latestTrackingStatus(domain.auto_seeding)}
                          </StatusPill>
                        </div>
                      </div>

                      <div className="mt-4">
                        <div className="flex items-center justify-between gap-4 text-xs text-base-content/55">
                          <span>{coverage.detail}</span>
                          <StatusPill tone={coverage.tone}>{coverage.label}</StatusPill>
                        </div>

                        <div className="mt-2">
                          <ProgressBar ratio={coverage.ratio} tone={coverage.tone} />
                        </div>
                      </div>

                      <div className="mt-4 flex items-center justify-between gap-4 text-xs text-base-content/55">
                        <span>{formatProviderCount(latestProviderCount)} seeders</span>
                        <span>{domain.versions?.length || 0} snapshots</span>
                      </div>
                    </button>
                  );
                })}
              </div>
            )}
          </div>
        </section>

        <DomainDetailPanel
          detailDomain={detailDomain}
          routeDomain={routeDomain}
          selectedMissing={selectedMissing}
          actionKey={actionKey}
          providerCounts={providerCounts}
          rememberRecentTarget={rememberRecentTarget}
          onToggleLatestTracking={toggleLatestTracking}
          onClearDomain={clearDomainCache}
          onClearVersion={clearVersionCache}
        />
      </div>
    </section>
  );
}

function DomainDetailPanel({
  detailDomain,
  routeDomain,
  selectedMissing,
  actionKey,
  providerCounts,
  rememberRecentTarget,
  onToggleLatestTracking,
  onClearDomain,
  onClearVersion,
}) {
  if (selectedMissing) {
    return (
      <section className={classNames(PANEL_CLASS, 'flex items-center justify-center p-8 xl:min-h-[720px]')}>
        <div className="max-w-md text-center">
          <h2 className="text-2xl font-semibold tracking-tight">Site not found</h2>
          <p className="mt-3 text-sm leading-6 text-base-content/60">
            The selected domain is no longer present in local storage.
          </p>
        </div>
      </section>
    );
  }

  if (!detailDomain) {
    return (
      <section className={classNames(PANEL_CLASS, 'flex items-center justify-center p-8 xl:min-h-[720px]')}>
        <div className="max-w-md text-center">
          <h2 className="text-2xl font-semibold tracking-tight">Nothing seeded yet</h2>
          <p className="mt-3 text-sm leading-6 text-base-content/60">
            Open a .eth or .wei domain from the launcher to begin filling the local cache.
          </p>
        </div>
      </section>
    );
  }

  const coverage = getCoverage(detailDomain.local_size, detailDomain.full_size);
  const hasIpnsContenthash = isIpnsContenthash(detailDomain.contenthash);
  const isPreview = !routeDomain;

  return (
    <section className={classNames(PANEL_CLASS, 'p-6 xl:min-h-[720px]')}>
      <div className="flex flex-col gap-6 sm:flex-row sm:items-start sm:justify-between">
        <div>
          <div className="flex items-center gap-2">
            {isPreview ? <StatusPill tone="neutral">Preview</StatusPill> : null}
            <StatusPill tone={coverage.tone}>{coverage.label}</StatusPill>
            {hasIpnsContenthash ? (
              <StatusPill tone="warning">
                <WarningIcon className="mr-1 h-3.5 w-3.5" />
                IPNS
              </StatusPill>
            ) : null}
            <StatusPill tone={detailDomain.auto_seeding ? 'info' : 'neutral'}>
              {latestTrackingStatus(detailDomain.auto_seeding)}
            </StatusPill>
          </div>

          <h2 className="mt-4 text-3xl font-semibold tracking-tight">{detailDomain.domain}</h2>
          <p className="mt-3 text-sm leading-6 text-base-content/65">
            {formatBytes(detailDomain.local_size)} stored locally of {formatBytes(detailDomain.full_size)} total content.
          </p>

        </div>

        <div className="flex items-center gap-3">
          <a
            href={`https://${detailDomain.domain}`}
            target="_blank"
            rel="noreferrer"
            className={PRIMARY_BUTTON_CLASS}
            onClick={() => rememberRecentTarget(detailDomain.domain)}
          >
            Open dapp
          </a>
        </div>
      </div>

      {hasIpnsContenthash ? (
        <div className="mt-4 flex items-start gap-3 rounded-xl border border-warning/30 bg-warning/10 p-3 text-sm text-warning">
          <WarningIcon className="mt-0.5 h-4 w-4 shrink-0" />
          <div>
            <p className="font-medium">IPNS contenthash</p>
            <p className="mt-1 break-all text-warning/85">
              This ENS record resolves through IPNS name: <span className="font-mono">{detailDomain.contenthash.target}</span>.
              <br/> 
              App developer can change site content at will without another onchain transaction. Snapshot list shows pinned results from each visit.
            </p>
          </div>
        </div>
      ) : null}

      <div className={classNames(SUBTLE_PANEL_CLASS, 'mt-6 p-5')}>
        <div className="flex items-center justify-between gap-4 text-sm">
          <span className="font-medium">Coverage</span>
          <span className="text-base-content/60">{coverage.detail}</span>
        </div>

        <div className="mt-3">
          <ProgressBar ratio={coverage.ratio} tone={coverage.tone} />
        </div>
      </div>

      <div className="mt-6 grid gap-4 sm:grid-cols-2 xl:grid-cols-4">
        <MetricTile label="Snapshots" value={detailDomain.versions?.length || 0} />
        <MetricTile label="Last cached" value={formatTimestamp(detailDomain.cached_at)} />
        <MetricTile label="Stored" value={formatBytes(detailDomain.local_size)} />
        <MetricTile label="Content size" value={formatBytes(detailDomain.full_size)} />
      </div>

      <div className="mt-6 flex flex-wrap items-center gap-3">
        <button
          type="button"
          className={`btn btn-outline ${detailDomain.auto_seeding ? 'btn-warning' : 'btn-success'}`}
          onClick={() => onToggleLatestTracking(detailDomain.domain, detailDomain.auto_seeding)}
          disabled={actionKey === `follow:${detailDomain.domain}`}
        >
          {actionKey === `follow:${detailDomain.domain}`
            ? 'Updating...'
            : latestTrackingAction(detailDomain.auto_seeding)}
        </button>

        <button
          type="button"
          className="btn btn-outline btn-error"
          onClick={() => onClearDomain(detailDomain.domain)}
          disabled={actionKey === `clear-domain:${detailDomain.domain}`}
        >
          {actionKey === `clear-domain:${detailDomain.domain}` ? 'Removing...' : 'Remove site'}
        </button>
      </div>

      <div className="mt-8">
        <div className="flex items-center justify-between gap-4">
          <div>
            <h3 className="text-xl font-semibold tracking-tight">Snapshots</h3>
            <p className="mt-1 text-sm text-base-content/60">
              Each cached version is available as its own stored snapshot.
            </p>
          </div>
        </div>

        <div className="mt-5 grid gap-4">
          {Array.isArray(detailDomain.versions) && detailDomain.versions.length > 0 ? (
            detailDomain.versions.map((version, index) => {
              const versionCoverage = getCoverage(version.local_size, version.full_size);
              const versionProviderCount = providerCounts[version.cid];

              return (
                <article key={`${detailDomain.domain}:${version.timestamp}:${version.cid}`} className={classNames(SUBTLE_PANEL_CLASS, 'p-5')}>
                  <div className="flex flex-col gap-6 lg:flex-row lg:items-start lg:justify-between">
                    <div>
                      <div className="flex items-center gap-2">
                        {index === 0 ? <StatusPill tone="info">Latest</StatusPill> : null}
                        <StatusPill tone={versionCoverage.tone}>{versionCoverage.label}</StatusPill>
                      </div>

                      <h4 className="mt-4 text-lg font-semibold tracking-tight">
                        {formatTimestamp(version.cached_at)}
                      </h4>
                      <p className="mt-2 font-mono text-xs text-base-content/60 break-all">{version.cid}</p>
                    </div>

                    <div className="flex flex-wrap items-center gap-3">
                      <a
                        href={version.visit_url || '#'}
                        target="_blank"
                        rel="noreferrer"
                        className="btn btn-sm btn-outline btn-accent"
                      >
                        Open snapshot
                      </a>
                      <a
                        href={`https://ipfs.localhost/webui/#/ipfs/${version.cid}`}
                        target="_blank"
                        rel="noreferrer"
                        className="btn btn-sm btn-outline btn-info"
                      >
                        Inspect
                      </a>
                      <button
                        type="button"
                        className="btn btn-sm btn-outline btn-error"
                        onClick={() => onClearVersion(detailDomain.domain, version.timestamp)}
                        disabled={actionKey === `clear-version:${detailDomain.domain}:${version.timestamp}`}
                      >
                        {actionKey === `clear-version:${detailDomain.domain}:${version.timestamp}`
                          ? 'Removing...'
                          : 'Remove'}
                      </button>
                    </div>
                  </div>

                  <div className="mt-5 grid gap-5 lg:grid-cols-[minmax(0,1fr)_320px]">
                    <div>
                      <div className="flex items-center justify-between gap-4 text-sm">
                        <span className="font-medium">Coverage</span>
                        <span className="text-base-content/60">{versionCoverage.detail}</span>
                      </div>
                      <div className="mt-2">
                        <ProgressBar ratio={versionCoverage.ratio} tone={versionCoverage.tone} size="sm" />
                      </div>
                    </div>

                    <div className="grid gap-3 text-sm sm:grid-cols-3">
                      <div className={classNames(SUBTLE_PANEL_CLASS, 'min-w-0 p-3')}>
                        <p className="text-xs uppercase tracking-[0.2em] text-base-content/45">Stored</p>
                        <p className="mt-2 font-medium">{formatBytes(version.local_size)}</p>
                      </div>
                      <div className={classNames(SUBTLE_PANEL_CLASS, 'min-w-0 p-3')}>
                        <p className="text-xs uppercase tracking-[0.2em] text-base-content/45">Total</p>
                        <p className="mt-2 font-medium">{formatBytes(version.full_size)}</p>
                      </div>
                      <div className={classNames(SUBTLE_PANEL_CLASS, 'min-w-0 p-3')}>
                        <p className="text-xs uppercase tracking-[0.2em] text-base-content/45">Seeders</p>
                        <p className="mt-2 font-medium">{formatProviderCount(versionProviderCount)}</p>
                      </div>
                    </div>
                  </div>
                </article>
              );
            })
          ) : (
            <p className="text-sm leading-6 text-base-content/60">No stored snapshots yet.</p>
          )}
        </div>
      </div>
    </section>
  );
}

function SettingsPage() {
  const [consensusRpcs, setConsensusRpcs] = useState(['']);
  const [executionRpcs, setExecutionRpcs] = useState(['']);
  const [followingInterval, setFollowingInterval] = useState(30);
  const [showTrayGasPrice, setShowTrayGasPrice] = useState(true);
  const [heliosEnabled, setHeliosEnabled] = useState(true);
  const [startOnLogin, setStartOnLogin] = useState(false);
  const [loading, setLoading] = useState(true);
  const [isInitialLoad, setIsInitialLoad] = useState(true);
  const [initialConfig, setInitialConfig] = useState(null);
  const [status, setStatus] = useState({ type: '', message: '' });
  const [about, setAbout] = useState(null);
  const [aboutError, setAboutError] = useState('');
  const [rpcCopyStatus, setRpcCopyStatus] = useState('');
  const rpcCopyResetRef = useRef(null);

  useEffect(() => {
    let mounted = true;

    const loadConfig = async () => {
      try {
        const response = await fetch('/api/config');
        if (!response.ok) {
          throw new Error('Failed to load config');
        }

        const data = await response.json();
        if (!mounted) {
          return;
        }

        const cons = Array.isArray(data.consensus_rpcs) && data.consensus_rpcs.length > 0 ? data.consensus_rpcs : ['https://ethereum.operationsolarstorm.org'];
        const execs = Array.isArray(data.execution_rpcs) && data.execution_rpcs.length > 0 ? data.execution_rpcs : ['https://eth.drpc.org'];
        const interval = typeof data.following_check_interval_mins === 'number' ? data.following_check_interval_mins : 30;
        const trayGasPrice = typeof data.show_tray_gas_price === 'boolean' ? data.show_tray_gas_price : true;
        const helios = typeof data.helios_enabled === 'boolean' ? data.helios_enabled : true;
        const startOnLoginEnabled = typeof data.start_on_login === 'boolean' ? data.start_on_login : false;

        setConsensusRpcs(cons);
        setExecutionRpcs(execs);
        setFollowingInterval(interval);
        setShowTrayGasPrice(trayGasPrice);
        setHeliosEnabled(helios);
        setStartOnLogin(startOnLoginEnabled);
        setInitialConfig(JSON.stringify({ cons, execs, interval, trayGasPrice, helios, startOnLogin: startOnLoginEnabled }));
        setStatus({ type: '', message: '' });
      } catch {
        if (!mounted) {
          return;
        }

        setStatus({ type: 'error', message: 'Failed to load configuration.' });
      } finally {
        if (mounted) {
          setLoading(false);
          setIsInitialLoad(false);
        }
      }
    };

    void loadConfig();

    return () => {
      mounted = false;
    };
  }, []);

  useEffect(() => () => {
    if (rpcCopyResetRef.current) {
      window.clearTimeout(rpcCopyResetRef.current);
    }
  }, []);

  useEffect(() => {
    let mounted = true;

    const loadAbout = async () => {
      try {
        const response = await fetch('/api/about');
        if (!response.ok) {
          throw new Error('Failed to load version info');
        }

        const data = await response.json();
        if (!mounted) {
          return;
        }

        setAbout(data);
        setAboutError('');
      } catch {
        if (!mounted) {
          return;
        }

        setAboutError('Version info unavailable.');
      }
    };

    void loadAbout();

    return () => {
      mounted = false;
    };
  }, []);

  useEffect(() => {
    if (isInitialLoad || !initialConfig) return;

    const currentConfigStr = JSON.stringify({
      cons: consensusRpcs,
      execs: executionRpcs,
      interval: followingInterval,
      trayGasPrice: showTrayGasPrice,
      helios: heliosEnabled,
      startOnLogin,
    });

    if (currentConfigStr === initialConfig) {
      return;
    }

    const timer = setTimeout(async () => {
      const cleanExecRpcs = executionRpcs.map(r => r.trim()).filter(Boolean);
      const cleanConsensusRpcs = consensusRpcs.map(r => r.trim()).filter(Boolean);
      
      if (cleanExecRpcs.length === 0 || (heliosEnabled && cleanConsensusRpcs.length === 0)) {
        setStatus({
          type: 'error',
          message: heliosEnabled
            ? 'At least one Consensus and Execution RPC is required when Helios is enabled.'
            : 'At least one Execution RPC is required.',
        });
        return;
      }

      setStatus({ type: '', message: '' });

      try {
        const response = await fetch('/api/config', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            consensus_rpcs: cleanConsensusRpcs,
            execution_rpcs: cleanExecRpcs,
            following_check_interval_mins: Number(followingInterval),
            show_tray_gas_price: showTrayGasPrice,
            helios_enabled: heliosEnabled,
            start_on_login: startOnLogin,
          }),
        });

        if (!response.ok) {
          throw new Error('Failed to save settings');
        }

        const result = await response.json();
        if (result.success) {
          setInitialConfig(currentConfigStr);
          setStatus({
            type: 'success',
            message: 'Settings saved.',
          });
          setTimeout(() => setStatus({ type: '', message: '' }), 3000);
        } else {
          setStatus({
            type: 'error',
            message: result.error || 'Failed to save settings.',
          });
        }
      } catch {
        setStatus({ type: 'error', message: 'Failed to save settings.' });
      }
    }, 600);

    return () => clearTimeout(timer);
  }, [consensusRpcs, executionRpcs, followingInterval, heliosEnabled, isInitialLoad, showTrayGasPrice, startOnLogin]);

  const moveRpcUp = (index, list, setList) => {
    if (index === 0) return;
    const next = [...list];
    [next[index - 1], next[index]] = [next[index], next[index - 1]];
    setList(next);
  };

  const moveRpcDown = (index, list, setList) => {
    if (index === list.length - 1) return;
    const next = [...list];
    [next[index + 1], next[index]] = [next[index], next[index + 1]];
    setList(next);
  };

  const removeRpc = (index, list, setList) => {
    const next = list.filter((_, i) => i !== index);
    setList(next.length > 0 ? next : ['']);
  };

  const addRpc = (list, setList) => {
    setList([...list, '']);
  };

  const neomistVersion = about?.neomist?.version || (aboutError ? 'Unavailable' : 'Loading...');
  const heliosVersion = about?.helios?.version || (aboutError ? 'Unavailable' : 'Loading...');
  const kuboVersion = about?.kubo?.version || (aboutError ? 'Unavailable' : 'Loading...');
  const localRpcUrl = `${window.location.origin}/rpc`;
  const kuboDetail = about?.kubo?.mode === 'external'
    ? 'Using external IPFS instance'
    : about?.kubo?.mode === 'managed'
      ? 'IPFS managed by NeoMist'
      : aboutError
        ? 'Could not load IPFS runtime info'
        : 'Inspecting local IPFS runtime';
  const copyLocalRpcUrl = async () => {
    try {
      if (navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(localRpcUrl);
      } else {
        const input = document.createElement('textarea');
        input.value = localRpcUrl;
        input.setAttribute('readonly', '');
        input.style.position = 'absolute';
        input.style.left = '-9999px';
        document.body.appendChild(input);
        input.select();
        const copied = document.execCommand('copy');
        document.body.removeChild(input);
        if (!copied) {
          throw new Error('Copy failed');
        }
      }

      setRpcCopyStatus('copied');
    } catch {
      setRpcCopyStatus('failed');
    }

    if (rpcCopyResetRef.current) {
      window.clearTimeout(rpcCopyResetRef.current);
    }

    rpcCopyResetRef.current = window.setTimeout(() => {
      setRpcCopyStatus('');
      rpcCopyResetRef.current = null;
    }, 2500);
  };

  return (
    <section className="mx-auto max-w-[920px]">
      <div className={classNames(PANEL_CLASS, 'p-8 relative')}>
        <h1 className="text-4xl font-semibold tracking-tight">Settings</h1>
        <p className="mt-3 max-w-2xl text-sm leading-6 text-base-content/65">
          Network endpoints live here so the main UI can stay focused on opening dapps and managing seeding.
        </p>

        {status.message && status.type === 'success' ? (
          <div className="animate-rise mt-5 rounded-full border border-success/25 bg-success/10 px-4 py-1.5 text-xs font-medium text-success sm:absolute sm:right-8 sm:top-8 sm:mt-0">
            {status.message}
          </div>
        ) : null}

        {status.message && status.type === 'error' ? (
          <div className="animate-rise mt-5 rounded-full border border-error/25 bg-error/10 px-4 py-1.5 text-xs font-medium text-error sm:absolute sm:right-8 sm:top-8 sm:mt-0">
            {status.message}
          </div>
        ) : null}

        <div className="mt-8 grid gap-5">
          <div className={classNames(SUBTLE_PANEL_CLASS, 'p-5')}>
            <div>
              <label className="block text-sm font-medium">RPC Behaviour</label>
              <p className="mt-2 max-w-2xl text-sm text-base-content/55">
                Configure how NeoMist exposes local JSON-RPC and which upstream endpoints it uses.
              </p>
            </div>

            <div className="mt-4 grid gap-4">
              <div className={classNames(SUBTLE_PANEL_CLASS, 'p-5')}>
                <div className="flex flex-wrap items-start justify-between gap-3">
                  <div>
                    <label className="block text-sm font-medium">Local RPC</label>
                    <p className="mt-2 max-w-2xl text-sm text-base-content/55">
                      {heliosEnabled
                        ? 'NeoMist exposes a local JSON-RPC endpoint backed by Helios. Requests stay on this machine and use your configured consensus and execution fallbacks.'
                        : 'NeoMist exposes a local JSON-RPC endpoint that forwards directly to your first configured execution RPC while Helios is disabled.'}
                    </p>
                  </div>

                  <StatusPill tone={rpcCopyStatus === 'copied' ? 'success' : rpcCopyStatus === 'failed' ? 'warning' : 'info'}>
                    {rpcCopyStatus === 'copied'
                      ? 'Copied'
                      : rpcCopyStatus === 'failed'
                        ? 'Copy failed'
                        : 'JSON-RPC'}
                  </StatusPill>
                </div>

                <div className="mt-4 flex flex-col gap-3 sm:flex-row">
                  <input
                    className={classNames(INPUT_CLASS, 'font-mono text-sm')}
                    value={localRpcUrl}
                    readOnly
                    aria-label="Local RPC endpoint"
                  />
                  <button
                    type="button"
                    className={classNames(SECONDARY_BUTTON_CLASS, 'shrink-0 px-4')}
                    onClick={copyLocalRpcUrl}
                  >
                    Copy endpoint
                  </button>
                </div>
              </div>

              <label className={classNames(SUBTLE_PANEL_CLASS, 'flex cursor-pointer flex-col gap-3 p-5 sm:flex-row sm:items-center sm:justify-between')}>
                <div>
                  <p className="text-sm font-medium">Enable Helios</p>
                  <p className="mt-1 text-xs text-base-content/50">
                    Disable if you want NeoMist to forward directly to your first execution RPC. Requires app restart to take effect.
                  </p>
                </div>
                <input
                  className="toggle toggle-primary"
                  type="checkbox"
                  checked={heliosEnabled}
                  onChange={(event) => setHeliosEnabled(event.target.checked)}
                />
              </label>

              <div className={classNames(SUBTLE_PANEL_CLASS, 'p-5')}>
                <label className="mb-2 block text-sm font-medium">Consensus RPCs</label>
                <p className="mb-4 text-sm text-base-content/55">
                  Used by Helios for beacon consensus. Only required when Helios is enabled. NeoMist will automatically fall back to next one if top fails.
                </p>

                <div className="grid gap-3">
                  {consensusRpcs.map((rpc, index) => (
                    <div key={`consensus-${index}`} className="flex flex-wrap items-start gap-2 sm:flex-nowrap sm:items-center">
                      <div className="flex flex-col gap-0.5">
                        <button
                          type="button"
                          className="vapor-badge flex h-5 w-6 items-center justify-center rounded border border-base-300 bg-base-100/50 text-xs text-base-content/60 transition hover:bg-base-200 disabled:opacity-30"
                          onClick={() => moveRpcUp(index, consensusRpcs, setConsensusRpcs)}
                          disabled={index === 0}
                        >
                          ▲
                        </button>
                        <button
                          type="button"
                          className="vapor-badge flex h-5 w-6 items-center justify-center rounded border border-base-300 bg-base-100/50 text-xs text-base-content/60 transition hover:bg-base-200 disabled:opacity-30"
                          onClick={() => moveRpcDown(index, consensusRpcs, setConsensusRpcs)}
                          disabled={index === consensusRpcs.length - 1}
                        >
                          ▼
                        </button>
                      </div>
                      <input
                        className={INPUT_CLASS}
                        value={rpc}
                        onChange={(event) => {
                          const next = [...consensusRpcs];
                          next[index] = event.target.value;
                          setConsensusRpcs(next);
                        }}
                        placeholder="https://"
                        type="url"
                        autoComplete="off"
                        spellCheck="false"
                        required={index === 0}
                      />
                      <button
                        type="button"
                        className="flex h-11 w-11 shrink-0 items-center justify-center rounded-xl border border-error/30 bg-error/10 text-error transition hover:bg-error/20"
                        onClick={() => removeRpc(index, consensusRpcs, setConsensusRpcs)}
                        aria-label="Remove endpoint"
                      >
                        ✕
                      </button>
                    </div>
                  ))}
                </div>

                <button
                  type="button"
                  className="mt-4 text-sm font-medium text-primary transition hover:opacity-80"
                  onClick={() => addRpc(consensusRpcs, setConsensusRpcs)}
                >
                  + Add fallback endpoint
                </button>
              </div>

              <div className={classNames(SUBTLE_PANEL_CLASS, 'p-5')}>
                <label className="mb-2 block text-sm font-medium">Execution RPCs</label>
                <p className="mb-4 text-sm text-base-content/55">
                  Used for EVM execution calls. NeoMist will automatically fall back to next one if top fails.
                </p>

                <div className="grid gap-3">
                  {executionRpcs.map((rpc, index) => (
                    <div key={`exec-${index}`} className="flex flex-wrap items-start gap-2 sm:flex-nowrap sm:items-center">
                      <div className="flex flex-col gap-0.5">
                        <button
                          type="button"
                          className="vapor-badge flex h-5 w-6 items-center justify-center rounded border border-base-300 bg-base-100/50 text-xs text-base-content/60 transition hover:bg-base-200 disabled:opacity-30"
                          onClick={() => moveRpcUp(index, executionRpcs, setExecutionRpcs)}
                          disabled={index === 0}
                        >
                          ▲
                        </button>
                        <button
                          type="button"
                          className="vapor-badge flex h-5 w-6 items-center justify-center rounded border border-base-300 bg-base-100/50 text-xs text-base-content/60 transition hover:bg-base-200 disabled:opacity-30"
                          onClick={() => moveRpcDown(index, executionRpcs, setExecutionRpcs)}
                          disabled={index === executionRpcs.length - 1}
                        >
                          ▼
                        </button>
                      </div>
                      <input
                        className={INPUT_CLASS}
                        value={rpc}
                        onChange={(event) => {
                          const next = [...executionRpcs];
                          next[index] = event.target.value;
                          setExecutionRpcs(next);
                        }}
                        placeholder="https://"
                        type="url"
                        autoComplete="off"
                        spellCheck="false"
                        required={index === 0}
                      />
                      <button
                        type="button"
                        className="flex h-11 w-11 shrink-0 items-center justify-center rounded-xl border border-error/30 bg-error/10 text-error transition hover:bg-error/20"
                        onClick={() => removeRpc(index, executionRpcs, setExecutionRpcs)}
                        aria-label="Remove endpoint"
                      >
                        ✕
                      </button>
                    </div>
                  ))}
                </div>

                <button
                  type="button"
                  className="mt-4 text-sm font-medium text-primary transition hover:opacity-80"
                  onClick={() => addRpc(executionRpcs, setExecutionRpcs)}
                >
                  + Add fallback endpoint
                </button>
              </div>
            </div>
          </div>

          <div className={classNames(SUBTLE_PANEL_CLASS, 'p-5')}>
            <div>
              <div>
                <label className="block text-sm font-medium">App Behavior</label>
              </div>
            </div>

            <div className={classNames(SUBTLE_PANEL_CLASS, 'mt-4 overflow-hidden p-0')}>
              <div className="flex flex-col gap-4 px-4 py-4 sm:flex-row sm:items-center sm:justify-between">
                <div>
                  <p className="text-sm font-medium">Follow checks</p>
                  <p className="mt-1 text-xs text-base-content/50">How often to check followed apps for updates, 0 disables</p>
                </div>

                <div className="flex items-center gap-3">
                  <span className="text-sm text-base-content/60">minutes</span>
                  <input
                    className={classNames(INPUT_CLASS, 'h-12 w-28 px-3 text-sm')}
                    value={followingInterval}
                    onChange={(event) => setFollowingInterval(event.target.value)}
                    type="number"
                    min="0"
                    max="10080"
                  />
                </div>
              </div>

              <label className="flex cursor-pointer flex-col gap-3 border-t border-base-300/60 px-4 py-4 sm:flex-row sm:items-center sm:justify-between">
                <div>
                  <p className="text-sm font-medium">Tray gas price</p>
                </div>
                <input
                  className="toggle toggle-primary"
                  type="checkbox"
                  checked={showTrayGasPrice}
                  onChange={(event) => setShowTrayGasPrice(event.target.checked)}
                />
              </label>

              <label className="flex cursor-pointer flex-col gap-3 border-t border-base-300/60 px-4 py-4 sm:flex-row sm:items-center sm:justify-between">
                <div>
                  <p className="text-sm font-medium">Start on login</p>
                </div>
                <input
                  className="toggle toggle-primary"
                  type="checkbox"
                  checked={startOnLogin}
                  onChange={(event) => setStartOnLogin(event.target.checked)}
                />
              </label>
            </div>
          </div>

        </div>

        <div className={classNames(SUBTLE_PANEL_CLASS, 'mt-6 p-5')}>
          <div className="flex flex-wrap items-start justify-between gap-3">
            <div>
              <label className="block text-sm font-medium">About NeoMist</label>
              <p className="mt-2 text-sm text-base-content/55">
                Build and runtime versions for NeoMist and local stack.
              </p>
            </div>

            {aboutError ? (
              <span className="rounded-full border border-warning/30 bg-warning/12 px-3 py-1 text-xs font-medium text-warning">
                {aboutError}
              </span>
            ) : null}
          </div>

          <div className="mt-5 grid gap-3 md:grid-cols-3">
            <div className={classNames(SUBTLE_PANEL_CLASS, 'p-4')}>
              <p className="text-xs uppercase tracking-[0.2em] text-base-content/45">NeoMist</p>
              <p className="mt-2 text-lg font-semibold tracking-tight">{neomistVersion}</p>
              <p className="mt-2 text-sm text-base-content/55">Desktop app</p>
            </div>

            <div className={classNames(SUBTLE_PANEL_CLASS, 'p-4')}>
              <p className="text-xs uppercase tracking-[0.2em] text-base-content/45">Helios</p>
              <p className="mt-2 text-lg font-semibold tracking-tight">{heliosVersion}</p>
              <p className="mt-2 text-sm text-base-content/55">Ethereum light client</p>
            </div>

            <div className={classNames(SUBTLE_PANEL_CLASS, 'p-4')}>
              <p className="text-xs uppercase tracking-[0.2em] text-base-content/45">Kubo</p>
              <p className="mt-2 text-lg font-semibold tracking-tight">{kuboVersion}</p>
              <p className="mt-2 text-sm text-base-content/55">{kuboDetail}</p>
            </div>
          </div>
        </div>
      </div>
    </section>
  );
}

function NotFoundPage({ navigate }) {
  return (
    <section className="mx-auto max-w-[720px]">
      <div className={classNames(PANEL_CLASS, 'p-10 text-center')}>
        <SectionEyebrow>Not found</SectionEyebrow>
        <h1 className="mt-4 text-4xl font-semibold tracking-tight">This page does not exist.</h1>
        <p className="mt-4 text-sm leading-6 text-base-content/60">
          Use the launcher to open a dapp or head back to the seeding console.
        </p>
        <div className="mt-8 flex items-center justify-center gap-3">
          <button type="button" className={PRIMARY_BUTTON_CLASS} onClick={() => navigate('/')}>
            Open launcher
          </button>
          <button
            type="button"
            className={SECONDARY_BUTTON_CLASS}
            onClick={() => navigate('/seeding')}
          >
            Go to seeding
          </button>
        </div>
      </div>
    </section>
  );
}

function MetricTile({ label, value }) {
  return (
    <div className={classNames(SUBTLE_PANEL_CLASS, 'p-4')}>
      <p className="text-xs uppercase tracking-[0.22em] text-base-content/45">{label}</p>
      <p className="mt-3 text-lg font-semibold tracking-tight">{value}</p>
    </div>
  );
}

function StatusPill({ tone = 'neutral', children }) {
  const styles = {
    neutral: 'border-base-300 bg-base-100/70 text-base-content/65',
    success: 'border-success/25 bg-success/12 text-success',
    warning: 'border-warning/30 bg-warning/12 text-warning',
    info: 'border-info/30 bg-info/12 text-info',
  };

  return (
    <span
      className={classNames(
        'vapor-badge inline-flex items-center rounded-full border px-3 py-1 text-xs font-medium',
        styles[tone] || styles.neutral
      )}
    >
      {children}
    </span>
  );
}

function ProgressBar({ ratio, tone = 'neutral', size = 'md' }) {
  const styles = {
    neutral: 'bg-base-content/20',
    success: 'bg-success',
    warning: 'bg-warning',
    info: 'bg-info',
  };

  const sizeClass = size === 'sm' ? 'h-1.5' : 'h-2';

  return (
    <div className={classNames(sizeClass, 'overflow-hidden rounded-full bg-base-200/80 shadow-inner')}>
      <div
        className={classNames('h-full rounded-full transition-all', styles[tone] || styles.neutral)}
        style={{ width: `${Math.max(0, Math.min(ratio, 1)) * 100}%` }}
      />
    </div>
  );
}

function SectionEyebrow({ children }) {
  return (
    <span className="vapor-badge inline-flex rounded-full border border-base-300/70 bg-base-100/75 px-3 py-1 text-[11px] font-medium uppercase tracking-[0.28em] text-base-content/50">
      {children}
    </span>
  );
}

function SettingsIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" className="h-5 w-5">
      <path
        strokeLinecap="round"
        strokeLinejoin="round"
        d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.527-.94 3.31.843 2.37 2.37a1.724 1.724 0 001.065 2.572c1.757.426 1.757 2.924 0 3.35a1.724 1.724 0 00-1.065 2.573c.94 1.527-.843 3.31-2.37 2.37a1.724 1.724 0 00-2.572 1.065c-.426 1.757-2.924 1.757-3.35 0a1.724 1.724 0 00-2.573-1.065c-1.527.94-3.31-.843-2.37-2.37a1.724 1.724 0 00-1.066-2.572c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.527.843-3.31 2.37-2.37.996.613 2.296.07 2.573-1.065z"
      />
      <path strokeLinecap="round" strokeLinejoin="round" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
    </svg>
  );
}

function WarningIcon({ className = 'h-4 w-4' }) {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" className={className}>
      <path
        strokeLinecap="round"
        strokeLinejoin="round"
        d="M10.29 3.86L1.82 18a2 2 0 001.71 3h16.94a2 2 0 001.71-3L13.71 3.86a2 2 0 00-3.42 0z"
      />
      <path strokeLinecap="round" strokeLinejoin="round" d="M12 9v4" />
      <path strokeLinecap="round" strokeLinejoin="round" d="M12 17h.01" />
    </svg>
  );
}

export default App;
