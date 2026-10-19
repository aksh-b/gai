import { useEffect, useMemo, useState, type ComponentType, type SVGProps } from 'react';
import { CommandMenu, type CommandEntry } from '@/components/command-menu';
import { TabBar } from '@/components/fields';
import { Toaster } from '@/components/ui/sonner';
import { api, getApiKey, setApiKey } from './api';
import { KeyDialog } from './KeyDialog';
import { modelLabel } from './labels';
import { applyTheme, setDevMode, useDevMode, useTheme, type Theme } from './prefs';
import { navigate, useRoute } from './router';
import {
  IconAgent,
  IconBook,
  IconChat,
  IconFlow,
  IconGear,
  IconMoon,
  IconRuns,
  IconSpark,
  IconStack,
  IconSun,
} from './icons';
import { MediaStudio } from './pages/MediaStudio';
import { Triggers } from './pages/Triggers';
import { Connect } from './pages/Connect';
import { Playground } from './pages/Playground';
import { Runs } from './pages/Runs';
import { RunDetail } from './pages/RunDetail';
import { Compare } from './pages/Compare';
import { Agents } from './pages/Agents';
import { Tools } from './pages/Tools';
import { UsagePage } from './pages/Usage';
import { Keys } from './pages/Keys';
import { Approvals } from './pages/Approvals';
import { Knowledge } from './pages/Knowledge';
import { Memory } from './pages/Memory';
import { Skills } from './pages/Skills';
import { Workflows } from './pages/Workflows';
import { Welcome, welcomeDone } from './pages/Welcome';
import { ModelsSettings, Preferences } from './pages/Settings';

type Icon = ComponentType<SVGProps<SVGSVGElement>>;

/** Top-level sections. Each owns some route ids (old ones keep working). */
const NAV: { id: string; label: string; icon: Icon; routes: string[]; tabs?: { id: string; label: string }[] }[] = [
  { id: 'chat', label: 'Chat', icon: IconChat, routes: ['chat', 'playground'] },
  {
    id: 'assistants',
    label: 'Assistants',
    icon: IconAgent,
    routes: ['assistants', 'agents', 'memory', 'skills'],
    tabs: [
      { id: 'assistants', label: 'Assistants' },
      { id: 'memory', label: 'Memory' },
      { id: 'skills', label: 'Skills' },
    ],
  },
  {
    id: 'automations',
    label: 'Automations',
    icon: IconFlow,
    routes: ['automations', 'workflows', 'schedules', 'triggers'],
    tabs: [
      { id: 'automations', label: 'Workflows' },
      { id: 'schedules', label: 'Schedules & webhooks' },
    ],
  },
  { id: 'knowledge', label: 'Knowledge', icon: IconStack, routes: ['knowledge'] },
  {
    id: 'activity',
    label: 'Activity',
    icon: IconRuns,
    routes: ['activity', 'runs', 'approvals', 'usage'],
    tabs: [
      { id: 'activity', label: 'History' },
      { id: 'approvals', label: 'Needs your OK' },
      { id: 'usage', label: 'Spending' },
    ],
  },
  {
    id: 'settings',
    label: 'Settings',
    icon: IconGear,
    routes: ['settings', 'models', 'tools', 'media', 'keys', 'connect', 'preferences'],
    tabs: [
      { id: 'settings', label: 'AI models' },
      { id: 'tools', label: 'Tools & connections' },
      { id: 'media', label: 'Voice & images' },
      { id: 'keys', label: 'Access keys' },
      { id: 'connect', label: 'Use from other apps' },
      { id: 'preferences', label: 'Preferences' },
    ],
  },
];

/** Route ids that are one tab's alias for another. */
const TAB_ALIAS: Record<string, string> = {
  agents: 'assistants',
  workflows: 'automations',
  triggers: 'schedules',
  runs: 'activity',
  models: 'settings',
  playground: 'chat',
};

type Boot = 'loading' | 'ready';

export function App() {
  const route = useRoute();
  const raw = route.parts[0] ?? 'chat';
  const [boot, setBoot] = useState<Boot>('loading');
  const [health, setHealth] = useState<{ ok: boolean; version?: string; model?: string | null; auth?: string }>({ ok: true });
  const [pending, setPending] = useState(0);
  const [palette, setPalette] = useState(false);
  const [agents, setAgents] = useState<string[]>([]);
  const [dev] = useDevMode();
  const [theme, setTheme] = useTheme();

  useEffect(() => applyTheme(), []);

  // Sign this browser in when the server allows it, then send a brand-new
  // install to the welcome screen. Pages load only after this, so they never
  // trip the key dialog on the way.
  useEffect(() => {
    (async () => {
      try {
        let status = await api.setupStatus();
        if (status.needs_key && status.can_claim) {
          const claimed = await api.claim();
          setApiKey(claimed.secret);
          status = await api.setupStatus();
        }
        if (status.needs_key) {
          window.dispatchEvent(new CustomEvent('gai:unauthorized'));
        } else if (!status.has_chat_model && !welcomeDone() && (raw === 'chat' || raw === 'playground')) {
          navigate('/welcome');
        }
      } catch {
        /* server offline: the footer says so */
      }
      setBoot('ready');
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (boot !== 'ready') return;
    const poll = () =>
      api
        .approvals()
        .then((a) => setPending(a.length))
        .catch(() => {});
    poll();
    const t = setInterval(poll, 4000);
    return () => clearInterval(t);
  }, [boot]);

  useEffect(() => {
    const check = () =>
      api
        .health()
        .then((h) => setHealth({ ok: true, version: h.version, model: h.default_model, auth: h.auth }))
        .catch(() => setHealth({ ok: false }));
    check();
    const t = setInterval(check, 15_000);
    return () => clearInterval(t);
  }, [route]);

  useEffect(() => {
    if (palette) api.agents().then((a) => setAgents(a.map((x) => x.summary.name))).catch(() => {});
  }, [palette]);

  const section = NAV.find((n) => n.routes.includes(raw))?.id ?? (raw === 'welcome' ? 'welcome' : 'chat');
  const tab = TAB_ALIAS[raw] ?? raw;
  const nav = NAV.find((n) => n.id === section);

  const commands = useMemo<CommandEntry[]>(() => {
    const go = (to: string) => () => navigate(to);
    const pages: CommandEntry[] = NAV.flatMap((n) =>
      (n.tabs ?? [{ id: n.id, label: n.label }]).map((t) => ({
        id: `go-${t.id}`,
        group: 'Go to',
        label: n.tabs && t.label !== n.label ? `${n.label} › ${t.label}` : n.label,
        onSelect: go(`/${t.id}`),
      })),
    );
    const themes: { id: Theme; label: string }[] = [
      { id: 'system', label: 'Theme: match the system' },
      { id: 'light', label: 'Theme: light' },
      { id: 'dark', label: 'Theme: dark' },
    ];
    return [
      { id: 'new-chat', group: 'Do', label: 'New chat', onSelect: go('/chat'), keywords: ['message', 'talk'] },
      { id: 'new-assistant', group: 'Do', label: 'New assistant', onSelect: go('/assistants?new=1'), keywords: ['agent', 'create'] },
      { id: 'add-knowledge', group: 'Do', label: 'Add documents to Knowledge', onSelect: go('/knowledge'), keywords: ['upload', 'files', 'rag'] },
      { id: 'add-model', group: 'Do', label: 'Connect an AI model', onSelect: go('/welcome'), keywords: ['key', 'ollama', 'provider', 'setup'] },
      ...pages,
      ...agents.map((a) => ({
        id: `agent-${a}`,
        group: 'Chat with',
        label: a,
        onSelect: go(`/chat?agent=${encodeURIComponent(a)}`),
      })),
      {
        id: 'dev',
        group: 'Preferences',
        label: dev ? 'Hide developer details' : 'Show developer details',
        keywords: ['ids', 'json', 'yaml', 'advanced', 'debug'],
        onSelect: () => setDevMode(!dev),
      },
      ...themes.map((t) => ({ id: `theme-${t.id}`, group: 'Preferences', label: t.label, onSelect: () => setTheme(t.id) })),
    ];
  }, [agents, dev, setTheme]);

  let page;
  if (boot === 'loading') {
    page = <div className="boot" aria-busy="true" />;
  } else {
    switch (tab) {
      case 'welcome':
        page = <Welcome />;
        break;
      case 'activity':
        page =
          route.parts[2] === 'compare' && route.parts[3] ? (
            <Compare a={route.parts[1]} b={route.parts[3]} />
          ) : route.parts[1] ? (
            <RunDetail id={route.parts[1]} />
          ) : (
            <Runs />
          );
        break;
      case 'assistants':
        page = <Agents selected={route.parts[1] ? decodeURIComponent(route.parts[1]) : undefined} creating={route.query.get('new') === '1'} />;
        break;
      case 'automations':
        page = <Workflows selected={route.parts[1] ? decodeURIComponent(route.parts[1]) : undefined} />;
        break;
      case 'knowledge':
        page = <Knowledge selected={route.parts[1] ? decodeURIComponent(route.parts[1]) : undefined} />;
        break;
      case 'memory':
        page = <Memory />;
        break;
      case 'skills':
        page = <Skills />;
        break;
      case 'media':
        page = <MediaStudio />;
        break;
      case 'schedules':
        page = <Triggers selected={route.parts[1]} />;
        break;
      case 'connect':
        page = <Connect />;
        break;
      case 'settings':
        page = <ModelsSettings />;
        break;
      case 'preferences':
        page = <Preferences />;
        break;
      case 'tools':
        page = <Tools />;
        break;
      case 'usage':
        page = <UsagePage />;
        break;
      case 'keys':
        page = <Keys />;
        break;
      case 'approvals':
        page = <Approvals />;
        break;
      default:
        page = <Playground route={route} />;
    }
  }

  // Sub-pages (a run, an agent) keep their section's tabs but no tab is lit.
  const tabs = nav?.tabs && boot === 'ready' && (
    <div className="hub-tabs">
      <TabBar
        variant="underline"
        aria-label={nav.label}
        items={nav.tabs.map((t) => ({
          id: t.id,
          label: t.label,
          badge: t.id === 'approvals' && pending > 0 ? <span className="badge">{pending}</span> : undefined,
        }))}
        value={tab}
        onValueChange={(id) => navigate(`/${id}`)}
      />
    </div>
  );

  const signedIn = !!getApiKey();

  return (
    <div className="shell" data-section={section}>
      <aside className="sidebar">
        <a className="brand" href="#/chat">
          <span className="logo">g</span>
          <span>gai</span>
        </a>
        <button type="button" className="search-btn" onClick={() => setPalette(true)}>
          <IconSpark />
          <span>Search or jump to…</span>
          <kbd>Ctrl K</kbd>
        </button>
        <nav aria-label="Main">
          {NAV.map(({ id, label, icon: Icon }) => (
            <a
              key={id}
              href={`#/${id}`}
              className="nav-item"
              aria-current={section === id ? 'page' : undefined}
              onClick={(e) => {
                e.preventDefault();
                navigate(`/${id}`);
              }}
            >
              <Icon />
              <span className="nav-label">{label}</span>
              {id === 'activity' && pending > 0 && (
                <span className="badge" title={`${pending} waiting for your OK`}>
                  {pending}
                </span>
              )}
            </a>
          ))}
        </nav>
        {dev && (
          <a className="nav-item" href="/docs" target="_blank" rel="noreferrer">
            <IconBook />
            <span className="nav-label">API docs</span>
          </a>
        )}
        <div className="sidebar-foot">
          <span className="dot" data-ok={health.ok} />
          <div className="foot-text">
            <div>{health.ok ? 'Connected' : 'Server offline'}</div>
            {health.model && (
              <a className="muted small foot-model" href="#/settings" title={dev ? health.model : 'Change in Settings'}>
                {dev ? health.model : modelLabel(health.model)}
              </a>
            )}
            {dev && (
              <div className="muted small">
                {health.version && `v${health.version} · `}
                {health.auth === 'open' ? 'open (no keys yet)' : signedIn ? 'signed in with a key' : 'key required'}
                {signedIn && (
                  <>
                    {' · '}
                    <button
                      type="button"
                      className="link small"
                      onClick={() => {
                        setApiKey(null);
                        location.reload();
                      }}
                    >
                      sign out
                    </button>
                  </>
                )}
              </div>
            )}
          </div>
          <button
            type="button"
            className="icon-btn"
            aria-label={`Theme: ${theme}. Switch theme`}
            title="Switch light / dark"
            onClick={() => {
              const dark = theme === 'dark' || (theme === 'system' && matchMedia('(prefers-color-scheme: dark)').matches);
              setTheme(dark ? 'light' : 'dark');
            }}
          >
            {theme === 'dark' || (theme === 'system' && matchMedia('(prefers-color-scheme: dark)').matches) ? <IconSun /> : <IconMoon />}
          </button>
        </div>
      </aside>
      <main className="main">
        {tabs}
        {page}
      </main>
      <CommandMenu open={palette} onOpenChange={setPalette} items={commands} placeholder="Search pages, assistants and actions…" />
      <Toaster position="bottom-right" theme={theme} />
      <KeyDialog />
    </div>
  );
}
