// Connecting AI models from the dashboard: cloud providers (paste a key) and
// local models (download into Ollama). Used by the welcome screen and by
// Settings → AI models.

import { useEffect, useRef, useState } from 'react';
import { Button } from '@/components/ui/button';
import { Spinner } from '@/components/ui/spinner';
import { toast } from 'sonner';
import { api, type CloudProvider, type LibraryModel, type OllamaStatus, type PullProgress } from './api';
import { IconCheck } from './icons';
import { useDevMode } from './prefs';

/** Pick a cloud provider, paste a key, and check it works. */
export function CloudConnect({
  providers,
  onConnected,
}: {
  providers: CloudProvider[];
  onConnected: (model: string) => void;
}) {
  const [picked, setPicked] = useState(providers.find((p) => !p.connected)?.name ?? providers[0]?.name ?? '');
  const [key, setKey] = useState('');
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const provider = providers.find((p) => p.name === picked);

  async function connect() {
    if (!provider) return;
    setBusy(true);
    setProblem(null);
    try {
      if (key.trim()) await api.setProviderKey(provider.name, key.trim());
      const test = await api.testProvider(provider.name);
      if (!test.ok) {
        setProblem(test.message);
        return;
      }
      setKey('');
      toast.success(`${provider.title} is connected`);
      onConnected(test.model);
    } catch (e) {
      setProblem((e as Error).message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="stack">
      <div className="choice-grid" role="radiogroup" aria-label="Provider">
        {providers.map((p) => (
          <button
            key={p.name}
            type="button"
            role="radio"
            aria-checked={p.name === picked}
            className="choice"
            onClick={() => {
              setPicked(p.name);
              setProblem(null);
            }}
          >
            <span className="choice-title">
              {p.title}
              {p.connected && <span className="chip accent">connected</span>}
            </span>
            <span className="muted small">{p.description}</span>
          </button>
        ))}
      </div>
      {provider && (
        <form
          className="stack"
          onSubmit={(e) => {
            e.preventDefault();
            connect();
          }}
        >
          <label className="field-label" htmlFor="provider-key">
            {provider.title} API key
          </label>
          <div className="row gap">
            <input
              id="provider-key"
              className="plain-input"
              type="password"
              autoComplete="off"
              placeholder={provider.saved_key ? `saved key ending in ${provider.saved_key}` : provider.connected ? 'a key is already set' : 'paste your key here'}
              value={key}
              onChange={(e) => setKey(e.target.value)}
            />
            <Button type="submit" disabled={busy || !key.trim() && !provider.connected}>
              {busy && <Spinner data-icon="inline-start" />}
              {key.trim() || !provider.connected ? 'Connect' : 'Test again'}
            </Button>
          </div>
          <p className="muted small">
            No key yet?{' '}
            <a href={provider.key_url} target="_blank" rel="noreferrer">
              Get one from {provider.title} ↗
            </a>{' '}
            The key is stored on your gai server and never shown again.
          </p>
          {problem && <div className="error-note">{problem}</div>}
          {provider.saved_key && (
            <div>
              <button
                type="button"
                className="link small"
                onClick={async () => {
                  await api.forgetProviderKey(provider.name).catch((e: Error) => toast.error(e.message));
                  toast.info(`Removed the saved ${provider.title} key`);
                  location.reload();
                }}
              >
                Remove saved key
              </button>
            </div>
          )}
        </form>
      )}
    </div>
  );
}

/** Ollama: whether it runs, which models to get, downloads with progress. */
export function LocalModels({ onUse }: { onUse: (model: string) => void }) {
  const [dev] = useDevMode();
  const [status, setStatus] = useState<OllamaStatus | null>(null);
  const [library, setLibrary] = useState<LibraryModel[]>([]);
  const [pulls, setPulls] = useState<Record<string, PullProgress>>({});
  const [checking, setChecking] = useState(false);
  const timer = useRef<number | undefined>(undefined);

  async function load() {
    setChecking(true);
    try {
      const l = await api.ollamaLibrary();
      setStatus(l.status);
      setLibrary(l.models);
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setChecking(false);
    }
  }

  async function poll() {
    const list = await api.ollamaPulls().catch(() => [] as PullProgress[]);
    const map = Object.fromEntries(list.map((p) => [p.model, p]));
    setPulls(map);
    const active = list.some((p) => p.state === 'pulling');
    if (list.some((p) => p.state === 'done' && !library.find((m) => m.name === p.model)?.installed)) load();
    if (active) timer.current = window.setTimeout(poll, 1000);
  }

  useEffect(() => {
    load();
    poll();
    return () => window.clearTimeout(timer.current);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function download(name: string) {
    try {
      const p = await api.ollamaPull(name);
      setPulls((m) => ({ ...m, [name]: p }));
      window.clearTimeout(timer.current);
      timer.current = window.setTimeout(poll, 600);
    } catch (e) {
      toast.error((e as Error).message);
    }
  }

  if (!status) return <p className="muted">Looking for Ollama…</p>;

  if (!status.reachable) {
    return (
      <div className="stack">
        <p>
          Local models run with <strong>Ollama</strong>, a free app. It isn't running yet
          {dev && status.base_url ? (
            <>
              {' '}
              (looked at <span className="mono">{status.base_url}</span>)
            </>
          ) : null}
          .
        </p>
        <ol className="steps-list">
          <li>
            Install Ollama from{' '}
            <a href="https://ollama.com/download" target="_blank" rel="noreferrer">
              ollama.com/download ↗
            </a>{' '}
            and start it, <em>or</em>, in this project, run <span className="mono">scripts/ollama.sh</span> and then restart{' '}
            <span className="mono">scripts/serve.sh</span>.
          </li>
          <li>Come back here and press “Check again”.</li>
        </ol>
        <div>
          <Button variant="outline" onClick={load} disabled={checking}>
            {checking && <Spinner data-icon="inline-start" />}
            Check again
          </Button>
        </div>
      </div>
    );
  }

  return (
    <div className="stack">
      <p className="muted">
        Ollama is running. Models are downloaded once and then work offline. Pick one to start; you can add more later.
      </p>
      <div className="model-list">
        {library.map((m) => {
          const p = pulls[m.name];
          const pulling = p?.state === 'pulling';
          const installed = m.installed || p?.state === 'done';
          const pct = p && p.total > 0 ? Math.round((p.completed / p.total) * 100) : 0;
          const embed = m.name.includes('embed');
          return (
            <div key={m.name} className="model-row">
              <div className="model-row-text">
                <div className="row gap wrap">
                  <strong>{m.name}</strong>
                  <span className="chip">{m.size}</span>
                  {m.tools && <span className="chip accent">can use tools</span>}
                </div>
                <span className="muted small">{m.description}</span>
                {pulling && (
                  <div className="progress" aria-label={`Downloading ${m.name}`}>
                    <span style={{ width: `${pct}%` }} />
                    <em className="small muted">
                      {p.status}
                      {p.total > 0 ? ` · ${pct}%` : ''}
                    </em>
                  </div>
                )}
                {p?.state === 'failed' && <div className="error-note small">{p.error}</div>}
              </div>
              {installed ? (
                embed ? (
                  <span className="chip">
                    <IconCheck width={12} height={12} /> installed
                  </span>
                ) : (
                  <Button size="sm" onClick={() => onUse(`ollama/${m.name}`)}>
                    Use this
                  </Button>
                )
              ) : (
                <Button variant="outline" size="sm" onClick={() => download(m.name)} disabled={pulling}>
                  {pulling && <Spinner data-icon="inline-start" />}
                  Download
                </Button>
              )}
            </div>
          );
        })}
      </div>
      {status.models.filter((n) => !library.some((m) => m.name === n || `${m.name}:latest` === n)).length > 0 && (
        <div className="stack tight">
          <span className="field-label">Also installed</span>
          <div className="row gap wrap">
            {status.models
              .filter((n) => !library.some((m) => m.name === n || `${m.name}:latest` === n))
              .map((n) => (
                <Button variant="secondary" size="sm" key={n} onClick={() => onUse(`ollama/${n}`)}>
                  Use {n}
                </Button>
              ))}
          </div>
        </div>
      )}
    </div>
  );
}
