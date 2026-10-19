// Connecting AI models from the dashboard: cloud providers (paste a key) and
// local models (download into Ollama). Used by the welcome screen and by
// Settings → AI models.

import { useEffect, useRef, useState } from 'react';
import { CircleCheckIcon, CircleHelpIcon } from 'lucide-react';
import { toast } from 'sonner';
import { ErrorNote } from '@/components/shared';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Field, FieldContent, FieldDescription, FieldLabel, FieldTitle } from '@/components/ui/field';
import { Input } from '@/components/ui/input';
import { Item, ItemActions, ItemContent, ItemDescription, ItemGroup, ItemTitle } from '@/components/ui/item';
import { Progress } from '@/components/ui/progress';
import { RadioGroup, RadioGroupItem } from '@/components/ui/radio-group';
import { Spinner } from '@/components/ui/spinner';
import { api, type CloudProvider, type LibraryModel, type OllamaStatus, type PullProgress } from './api';
import { useDevMode } from './prefs';

/** Pick a cloud provider, paste a key, and check it works. */
export function CloudConnect({ providers, onConnected }: { providers: CloudProvider[]; onConnected: (model: string) => void }) {
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
    <div className="flex flex-col gap-4">
      <RadioGroup
        aria-label="Provider"
        value={picked}
        onValueChange={(v) => {
          setPicked(v);
          setProblem(null);
        }}
        className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4"
      >
        {providers.map((p) => (
          <FieldLabel key={p.name} htmlFor={`provider-${p.name}`}>
            <Field orientation="horizontal">
              <FieldContent>
                <FieldTitle>
                  {p.title}
                  {p.connected && <Badge variant="secondary">Connected</Badge>}
                </FieldTitle>
                <FieldDescription>{p.description}</FieldDescription>
              </FieldContent>
              <RadioGroupItem value={p.name} id={`provider-${p.name}`} />
            </Field>
          </FieldLabel>
        ))}
      </RadioGroup>
      {provider && (
        <form
          className="flex flex-col gap-3"
          onSubmit={(e) => {
            e.preventDefault();
            connect();
          }}
        >
          <Field>
            <FieldLabel htmlFor="provider-key">{provider.title} API key</FieldLabel>
            <div className="flex gap-2">
              <Input
                id="provider-key"
                type="password"
                autoComplete="off"
                placeholder={
                  provider.saved_key
                    ? `saved key ending in ${provider.saved_key}`
                    : provider.connected
                      ? 'a key is already set'
                      : 'paste your key here'
                }
                value={key}
                onChange={(e) => setKey(e.target.value)}
              />
              <Button type="submit" disabled={busy || (!key.trim() && !provider.connected)}>
                {busy && <Spinner data-icon="inline-start" />}
                {key.trim() || !provider.connected ? 'Connect' : 'Test again'}
              </Button>
            </div>
            <FieldDescription>
              No key yet?{' '}
              <a href={provider.key_url} target="_blank" rel="noreferrer">
                Get one from {provider.title}
              </a>
              . The key is stored on your gai server and never shown again.
            </FieldDescription>
          </Field>
          <ErrorNote error={problem ?? undefined} />
          {provider.saved_key && (
            <Button
              type="button"
              variant="link"
              size="sm"
              className="w-fit px-0"
              onClick={async () => {
                await api.forgetProviderKey(provider.name).catch((e: Error) => toast.error(e.message));
                toast.info(`Removed the saved ${provider.title} key`);
                location.reload();
              }}
            >
              Remove saved key
            </Button>
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

  if (!status)
    return (
      <p className="flex items-center gap-2 text-muted-foreground">
        <Spinner /> Looking for Ollama…
      </p>
    );

  if (!status.reachable) {
    return (
      <Alert>
        <CircleHelpIcon />
        <AlertTitle>Ollama isn't running yet</AlertTitle>
        <AlertDescription>
          <p>
            Local models run with Ollama, a free app
            {dev && status.base_url ? (
              <>
                {' '}
                (looked at <code>{status.base_url}</code>)
              </>
            ) : null}
            .
          </p>
          <ol className="list-decimal pl-4">
            <li>
              Install Ollama from{' '}
              <a href="https://ollama.com/download" target="_blank" rel="noreferrer">
                ollama.com/download
              </a>{' '}
              and start it, <em>or</em>, in this project, run <code>scripts/ollama.sh</code> and then restart <code>scripts/serve.sh</code>.
            </li>
            <li>Come back here and press “Check again”.</li>
          </ol>
          <Button variant="outline" size="sm" className="mt-2" onClick={load} disabled={checking}>
            {checking && <Spinner data-icon="inline-start" />}
            Check again
          </Button>
        </AlertDescription>
      </Alert>
    );
  }

  const others = status.models.filter((n) => !library.some((m) => m.name === n || `${m.name}:latest` === n));
  return (
    <div className="flex flex-col gap-4">
      <p className="text-muted-foreground">
        Ollama is running. Models are downloaded once and then work offline. Pick one to start; you can add more later.
      </p>
      <ItemGroup className="gap-2">
        {library.map((m) => {
          const p = pulls[m.name];
          const pulling = p?.state === 'pulling';
          const installed = m.installed || p?.state === 'done';
          const pct = p && p.total > 0 ? Math.round((p.completed / p.total) * 100) : 0;
          const embed = m.name.includes('embed');
          return (
            <Item key={m.name} variant="outline">
              <ItemContent>
                <ItemTitle>
                  {m.name}
                  <Badge variant="outline">{m.size}</Badge>
                  {m.tools && <Badge variant="secondary">Can use tools</Badge>}
                </ItemTitle>
                <ItemDescription>{m.description}</ItemDescription>
                {pulling && (
                  <div className="flex flex-col gap-1 pt-1">
                    <Progress value={pct} aria-label={`Downloading ${m.name}`} />
                    <span className="text-xs text-muted-foreground">
                      {p.status}
                      {p.total > 0 ? ` · ${pct}%` : ''}
                    </span>
                  </div>
                )}
                {p?.state === 'failed' && <ErrorNote error={p.error ?? 'The download failed.'} />}
              </ItemContent>
              <ItemActions>
                {installed ? (
                  embed ? (
                    <Badge variant="secondary">
                      <CircleCheckIcon data-icon="inline-start" />
                      Installed
                    </Badge>
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
              </ItemActions>
            </Item>
          );
        })}
      </ItemGroup>
      {others.length > 0 && (
        <Field>
          <FieldLabel>Also installed</FieldLabel>
          <div className="flex flex-wrap gap-2">
            {others.map((n) => (
              <Button variant="secondary" size="sm" key={n} onClick={() => onUse(`ollama/${n}`)}>
                Use {n}
              </Button>
            ))}
          </div>
        </Field>
      )}
    </div>
  );
}
