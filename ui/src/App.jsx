import { generateFoamSvg } from '@simplepg/foam-identicon';
import { useEffect, useMemo, useRef, useState } from 'react';

const exploreLinks = [
  'dex.wei',
  'ens.eth',
  'simplepage.eth',
  'jthor.eth',
  'walletbeat.eth',
  'vitalik.eth',
];

const tabs = [
  { id: 'explore', label: 'Explore' },
  { id: 'cache', label: 'Cache' },
  { id: 'settings', label: 'Settings' },
];

const CHECKPOINT_ICON_SIZE = 36;
const CHECKPOINT_REFRESH_MS = 60000;
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

const formatCheckpoint = (hash) => {
  if (typeof hash !== 'string') {
    return '';
  }
  if (hash.startsWith('0x') && hash.length > 10) {
    return `${hash.slice(0, 6)}...${hash.slice(-4)}`;
  }
  if (hash.length > 8) {
    return `${hash.slice(0, 4)}...${hash.slice(-4)}`;
  }
  return hash;
};

function App() {
  const [activeTab, setActiveTab] = useState('explore');
  const [checkpoints, setCheckpoints] = useState([]);
  const [checkpointError, setCheckpointError] = useState('');

  useEffect(() => {
    let mounted = true;

    const loadCheckpoints = async () => {
      try {
        const response = await fetch('/api/helios/checkpoints');
        if (!response.ok) {
          throw new Error('Failed to load checkpoints');
        }
        const data = await response.json();
        if (!mounted) return;
        setCheckpoints(Array.isArray(data.checkpoints) ? data.checkpoints : []);
        setCheckpointError('');
      } catch (err) {
        if (!mounted) return;
        setCheckpoints([]);
        setCheckpointError('Failed to load checkpoints.');
      }
    };

    loadCheckpoints();
    const interval = window.setInterval(loadCheckpoints, CHECKPOINT_REFRESH_MS);
    return () => {
      mounted = false;
      window.clearInterval(interval);
    };
  }, []);

  return (
    <div className="min-h-screen">
      <div className="mx-auto max-w-6xl px-4 pb-16 pt-10">
        <Header checkpoints={checkpoints} checkpointError={checkpointError} />

        <div className="mt-8 animate-rise stagger-1">
          <div className="tabs tabs-boxed bg-base-200/70 p-1 shadow-inner">
            {tabs.map((tab) => (
              <button
                key={tab.id}
                className={`tab ${activeTab === tab.id ? 'tab-active' : ''}`}
                onClick={() => setActiveTab(tab.id)}
                type="button"
              >
                {tab.label}
              </button>
            ))}
          </div>
        </div>

        <div className="mt-6">
          {activeTab === 'explore' && <ExploreTab />}
          {activeTab === 'cache' && <CacheTab />}
          {activeTab === 'settings' && <SettingsTab />}
        </div>
      </div>
    </div>
  );
}

function Header({ checkpoints, checkpointError }) {
  const checkpointIcons = useMemo(
    () =>
      checkpoints.map((hash) => ({
        hash,
        svg: generateFoamSvg(hash, CHECKPOINT_ICON_SIZE, {
          paletteOverrides: FOAM_PALETTE_OVERRIDES,
        }),
      })),
    [checkpoints]
  );
  const [copiedHash, setCopiedHash] = useState('');
  const copyTimeoutRef = useRef(null);

  useEffect(() => {
    return () => {
      if (copyTimeoutRef.current) {
        window.clearTimeout(copyTimeoutRef.current);
      }
    };
  }, []);

  const copyCheckpoint = async (hash) => {
    try {
      if (navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(hash);
      } else {
        const textarea = document.createElement('textarea');
        textarea.value = hash;
        textarea.setAttribute('readonly', '');
        textarea.style.position = 'absolute';
        textarea.style.left = '-9999px';
        document.body.appendChild(textarea);
        textarea.select();
        document.execCommand('copy');
        document.body.removeChild(textarea);
      }
    } catch (err) {
      // noop
    }
    setCopiedHash(hash);
    if (copyTimeoutRef.current) {
      window.clearTimeout(copyTimeoutRef.current);
    }
    copyTimeoutRef.current = window.setTimeout(() => {
      setCopiedHash('');
    }, 5000);
  };

  return (
    <section className="relative overflow-hidden rounded-3xl border border-base-300 bg-base-100/80 p-6 shadow-2xl backdrop-blur animate-rise">
      <div className="pointer-events-none absolute inset-0 opacity-50">
        <div className="absolute -left-16 top-0 h-40 w-40 rounded-full bg-primary/30 blur-3xl" />
        <div className="absolute right-0 top-10 h-32 w-32 rounded-full bg-secondary/30 blur-3xl" />
      </div>

      <div className="relative z-10 flex flex-col gap-6">
        <div className="flex items-center gap-3">
          <picture>
            <source srcSet="/icon-dark.svg" media="(prefers-color-scheme: dark)" />
            <img src="/icon.svg" alt="NeoMist icon" className="h-11 w-11" />
          </picture>
          <h1 className="text-3xl font-semibold md:text-4xl">NeoMist Dashboard</h1>
        </div>

        <div className="rounded-2xl border border-base-300 bg-base-200/60 p-4">
          <div className="flex flex-col gap-2 md:flex-row md:items-center md:justify-between">
            <p className="font-medium">Latest checkpoints</p>
            <span className="text-xs uppercase tracking-[0.18em] opacity-60">
              Compare with friends to ensure you are on the canonical chain
            </span>
          </div>
          <div className="mt-3 flex flex-wrap gap-2">
            {checkpointError ? (
              <span className="text-sm text-error">{checkpointError}</span>
            ) : checkpoints.length === 0 ? (
              <span className="text-sm opacity-60">No checkpoints yet.</span>
            ) : (
              checkpointIcons.map(({ hash, svg }) => (
                <div
                  key={hash}
                  className="tooltip tooltip-bottom"
                  data-tip={copiedHash === hash ? 'Copied!' : formatCheckpoint(hash)}
                >
                  <button
                    type="button"
                    onClick={() => copyCheckpoint(hash)}
                    className="inline-flex h-9 w-9 items-center justify-center"
                    aria-label={`Copy checkpoint ${hash}`}
                  >
                    <span
                      role="img"
                      aria-hidden="true"
                      className="mask mask-squircle h-9 w-9"
                      dangerouslySetInnerHTML={{ __html: svg }}
                    />
                  </button>
                </div>
              ))
            )}
          </div>
        </div>
      </div>
    </section>
  );
}

function ExploreTab() {
  return (
    <section className="grid gap-4 md:grid-cols-2 lg:grid-cols-3 animate-rise stagger-2">
      {exploreLinks.map((domain) => (
        <a
          key={domain}
          href={`https://${domain}`}
          target="_blank"
          rel="noreferrer"
          className="rounded-2xl border border-base-300 bg-base-100/80 p-5 shadow-lg transition-colors hover:border-primary/50"
        >
          <div className="flex items-start justify-between">
            <div>
              <p className="text-xs uppercase tracking-[0.3em] opacity-60">Explore</p>
              <h3 className="mt-2 text-xl font-semibold">{domain}</h3>
            </div>
            <span className="badge badge-outline">Open</span>
          </div>
          <p className="mt-4 text-sm opacity-70">Open the site in a new tab.</p>
        </a>
      ))}
    </section>
  );
}

function CacheTab() {
  const [domains, setDomains] = useState([]);
  const [storageUsed, setStorageUsed] = useState('-');
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');
  const [actionDomain, setActionDomain] = useState('');

  const loadCache = async () => {
    setLoading(true);
    setError('');
    try {
      const [domainsRes, storageRes] = await Promise.all([
        fetch('/api/cached-domains'),
        fetch('/api/total-storage'),
      ]);

      if (!domainsRes.ok || !storageRes.ok) {
        throw new Error('Failed to load cache');
      }

      const domainsData = await domainsRes.json();
      const storageData = await storageRes.json();
      setDomains(domainsData);
      setStorageUsed(storageData.totalUsed || '-');
    } catch (err) {
      setError('Failed to load cache.');
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    loadCache();
  }, []);

  const toggleAuto = async (domain, current) => {
    setActionDomain(domain);
    try {
      await fetch(
        `/api/toggle-auto-seed?domain=${encodeURIComponent(domain)}&enable=${!current}`,
        {
          method: 'POST',
        }
      );
      await loadCache();
    } finally {
      setActionDomain('');
    }
  };

  const clearCache = async (domain) => {
    setActionDomain(domain);
    try {
      await fetch(`/api/clear-cache?domain=${encodeURIComponent(domain)}`, {
        method: 'POST',
      });
      await loadCache();
    } finally {
      setActionDomain('');
    }
  };

  return (
    <section className="grid gap-6 animate-rise stagger-2">
      <div className="rounded-3xl border border-base-300 bg-base-100/80 p-6 shadow-xl">
        <div className="flex flex-col gap-4 md:flex-row md:items-center md:justify-between">
          <div>
            <h2 className="text-2xl font-semibold">Cache status</h2>
            <p className="mt-2 text-sm opacity-70">
              Cached .eth and .wei sites available on your node.
            </p>
          </div>
          <div className="stats stats-vertical bg-base-200/70 shadow md:stats-horizontal">
            <div className="stat">
              <div className="stat-title">Cached</div>
              <div className="stat-value text-primary">{domains.length}</div>
            </div>
            <div className="stat">
              <div className="stat-title">Storage</div>
              <div className="stat-value text-secondary">{storageUsed}</div>
            </div>
          </div>
        </div>

      </div>

      <div className="rounded-3xl border border-base-300 bg-base-100/80 p-6 shadow-xl">
        <div className="flex items-center justify-between">
          <h3 className="text-xl font-semibold">Cached websites</h3>
          <button
            className="btn btn-sm btn-outline"
            type="button"
            onClick={loadCache}
          >
            Refresh
          </button>
        </div>

        <div className="mt-4 overflow-x-auto">
          <table className="table">
            <thead>
              <tr>
                <th>Domain</th>
                <th>Latest CID</th>
                <th>Cached</th>
                <th>Auto-seed</th>
                <th>Actions</th>
              </tr>
            </thead>
            <tbody>
              {loading ? (
                <tr>
                  <td colSpan="5">
                    <div className="flex items-center gap-2 text-sm opacity-70">
                      <span className="loading loading-spinner loading-sm" />
                      Loading cache...
                    </div>
                  </td>
                </tr>
              ) : error ? (
                <tr>
                  <td colSpan="5" className="text-sm text-error">
                    {error}
                  </td>
                </tr>
              ) : domains.length === 0 ? (
                <tr>
                  <td colSpan="5" className="text-sm opacity-70">
                    No cached domains yet.
                  </td>
                </tr>
              ) : (
                domains.map((domain) => (
                  <tr key={domain.domain}>
                    <td>
                      <a
                        className="link link-hover"
                        href={`https://${domain.domain}`}
                        target="_blank"
                        rel="noreferrer"
                      >
                        {domain.domain}
                      </a>
                    </td>
                    <td className="font-mono text-xs opacity-70">
                      {domain.cid}
                    </td>
                    <td>{domain.last_cached || '-'}</td>
                    <td>
                      <span
                        className={`badge ${
                          domain.auto_seeding ? 'badge-success' : 'badge-ghost'
                        }`}
                      >
                        {domain.auto_seeding ? 'Enabled' : 'Off'}
                      </span>
                    </td>
                    <td>
                      <div className="flex flex-wrap gap-2">
                        <a
                          className="btn btn-xs btn-outline btn-primary"
                          href={`https://webui.ipfs.io/#/ipfs/${domain.cid}`}
                          target="_blank"
                          rel="noreferrer"
                        >
                          View
                        </a>
                        <button
                          className="btn btn-xs btn-outline btn-error"
                          type="button"
                          onClick={() => clearCache(domain.domain)}
                          disabled={actionDomain === domain.domain}
                        >
                          Clear
                        </button>
                        <button
                          className="btn btn-xs btn-outline"
                          type="button"
                          onClick={() => toggleAuto(domain.domain, domain.auto_seeding)}
                          disabled={actionDomain === domain.domain}
                        >
                          {domain.auto_seeding ? 'Disable seed' : 'Enable seed'}
                        </button>
                      </div>
                    </td>
                  </tr>
                ))
              )}
            </tbody>
          </table>
        </div>
      </div>
    </section>
  );
}

function SettingsTab() {
  const [consensusRpc, setConsensusRpc] = useState('');
  const [executionRpc, setExecutionRpc] = useState('');
  const [status, setStatus] = useState({ type: '', message: '' });
  const [saving, setSaving] = useState(false);

  const loadConfig = async () => {
    try {
      const response = await fetch('/api/config');
      if (!response.ok) {
        throw new Error('Failed to load config');
      }
      const data = await response.json();
      setConsensusRpc(data.consensus_rpc || '');
      setExecutionRpc(data.execution_rpc || '');
    } catch (err) {
      setStatus({ type: 'error', message: 'Failed to load configuration.' });
    }
  };

  useEffect(() => {
    loadConfig();
  }, []);

  const saveConfig = async (event) => {
    event.preventDefault();
    setSaving(true);
    setStatus({ type: '', message: '' });

    try {
      const response = await fetch('/api/config', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          consensus_rpc: consensusRpc,
          execution_rpc: executionRpc,
        }),
      });

      const result = await response.json();
      if (result.success) {
        setStatus({
          type: 'success',
          message: 'Saved. Restart the app to apply new endpoints.',
        });
      } else {
        setStatus({
          type: 'error',
          message: result.error || 'Failed to save settings.',
        });
      }
    } catch (err) {
      setStatus({ type: 'error', message: 'Failed to save settings.' });
    } finally {
      setSaving(false);
    }
  };

  return (
    <section className="grid gap-6 animate-rise stagger-2">
      <div className="rounded-3xl border border-base-300 bg-base-100/80 p-6 shadow-xl">
        <h2 className="text-2xl font-semibold">RPC settings</h2>
        <p className="mt-2 text-sm opacity-70">
          Update the consensus and execution endpoints NeoMist uses for Helios.
        </p>

        <form className="mt-6 grid gap-4" onSubmit={saveConfig}>
          <div>
            <label className="label">
              <span className="label-text">Consensus RPC</span>
            </label>
            <input
              className="input input-bordered w-full"
              value={consensusRpc}
              onChange={(event) => setConsensusRpc(event.target.value)}
              placeholder="https://"
              type="text"
            />
          </div>
          <div>
            <label className="label">
              <span className="label-text">Execution RPC</span>
            </label>
            <input
              className="input input-bordered w-full"
              value={executionRpc}
              onChange={(event) => setExecutionRpc(event.target.value)}
              placeholder="https://"
              type="text"
            />
          </div>
          <div className="flex flex-wrap items-center gap-3">
            <button className="btn btn-primary" type="submit" disabled={saving}>
              {saving ? 'Saving...' : 'Save settings'}
            </button>
            <span className="text-xs opacity-60">Changes apply after restart.</span>
          </div>
        </form>

        {status.message ? (
          <div
            className={`alert mt-4 ${
              status.type === 'success' ? 'alert-success' : 'alert-error'
            }`}
          >
            <span>{status.message}</span>
          </div>
        ) : null}
      </div>
    </section>
  );
}

export default App;
