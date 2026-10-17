import { useEffect, useState, type ComponentType, type SVGProps } from 'react';
import { Toaster } from '@flame';
import { api, getApiKey, setApiKey } from './api';
import { KeyDialog } from './KeyDialog';
import { navigate, useRoute } from './router';
import { IconAgent, IconBolt, IconFlow, IconBook, IconBrain, IconChat, IconHand, IconImage, IconKey, IconClock, IconPlug, IconModel, IconRuns, IconStack, IconTool, IconUsage } from './icons';
import { MediaStudio } from './pages/MediaStudio';
import { Triggers } from './pages/Triggers';
import { Connect } from './pages/Connect';
import { Playground } from './pages/Playground';
import { Runs } from './pages/Runs';
import { RunDetail } from './pages/RunDetail';
import { Compare } from './pages/Compare';
import { Agents } from './pages/Agents';
import { Models } from './pages/Models';
import { Tools } from './pages/Tools';
import { UsagePage } from './pages/Usage';
import { Keys } from './pages/Keys';
import { Approvals } from './pages/Approvals';
import { Knowledge } from './pages/Knowledge';
import { Memory } from './pages/Memory';
import { Skills } from './pages/Skills';
import { Workflows } from './pages/Workflows';

const NAV: { id: string; label: string; icon: ComponentType<SVGProps<SVGSVGElement>> }[] = [
  { id: 'playground', label: 'Playground', icon: IconChat },
  { id: 'runs', label: 'Runs', icon: IconRuns },
  { id: 'approvals', label: 'Approvals', icon: IconHand },
  { id: 'agents', label: 'Agents', icon: IconAgent },
  { id: 'workflows', label: 'Workflows', icon: IconFlow },
  { id: 'triggers', label: 'Triggers', icon: IconClock },
  { id: 'knowledge', label: 'Knowledge', icon: IconStack },
  { id: 'memory', label: 'Memory', icon: IconBrain },
  { id: 'skills', label: 'Skills', icon: IconBolt },
  { id: 'media', label: 'Media', icon: IconImage },
  { id: 'models', label: 'Models', icon: IconModel },
  { id: 'tools', label: 'Tools', icon: IconTool },
  { id: 'usage', label: 'Usage', icon: IconUsage },
  { id: 'keys', label: 'API keys', icon: IconKey },
  { id: 'connect', label: 'Connect', icon: IconPlug },
];

export function App() {
  const route = useRoute();
  const section = route.parts[0] ?? 'playground';
  const [health, setHealth] = useState<{ ok: boolean; version?: string; model?: string | null; auth?: string }>({ ok: true });

  const [pending, setPending] = useState(0);
  useEffect(() => {
    const poll = () =>
      api
        .approvals()
        .then((a) => setPending(a.length))
        .catch(() => {});
    poll();
    const t = setInterval(poll, 4000);
    return () => clearInterval(t);
  }, []);

  useEffect(() => {
    const check = () =>
      api
        .health()
        .then((h) => setHealth({ ok: true, version: h.version, model: h.default_model, auth: h.auth }))
        .catch(() => setHealth({ ok: false }));
    check();
    const t = setInterval(check, 15_000);
    return () => clearInterval(t);
  }, []);

  let page;
  switch (section) {
    case 'runs':
      page =
        route.parts[2] === 'compare' && route.parts[3] ? (
          <Compare a={route.parts[1]} b={route.parts[3]} />
        ) : route.parts[1] ? (
          <RunDetail id={route.parts[1]} />
        ) : (
          <Runs />
        );
      break;
    case 'agents':
      page = <Agents selected={route.parts[1]} />;
      break;
    case 'workflows':
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
    case 'triggers':
      page = <Triggers selected={route.parts[1]} />;
      break;
    case 'connect':
      page = <Connect />;
      break;
    case 'models':
      page = <Models />;
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

  return (
    <div className="shell">
      <aside className="sidebar">
        <a className="brand" href="#/playground">
          <span className="logo">g</span>
          <span>gai</span>
        </a>
        <nav>
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
              {label}
              {id === 'approvals' && pending > 0 && <span className="badge">{pending}</span>}
            </a>
          ))}
        </nav>
        <a className="nav-item" href="/docs" target="_blank" rel="noreferrer">
          <IconBook />
          API docs
        </a>
        <div className="sidebar-foot">
          <span className="dot" data-ok={health.ok} />
          <div>
            <div>{health.ok ? `server ${health.version ?? ''}` : 'server offline'}</div>
            {health.model && <div className="muted mono small">{health.model}</div>}
            <div className="muted small">
              {health.auth === 'open' ? 'auth open (no keys yet)' : getApiKey() ? 'signed in with a key' : 'API key required'}
              {getApiKey() && (
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
          </div>
        </div>
      </aside>
      <main className="main">{page}</main>
      <Toaster position="bottom-right" />
      <KeyDialog />
    </div>
  );
}
