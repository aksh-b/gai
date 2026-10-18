import { useEffect, useState } from 'react';
import { Button, toast } from '@flame';
import { api, type SetupStatus } from '../api';
import { IconCheck, IconChat, IconModel, IconSpark } from '../icons';
import { modelLabel } from '../labels';
import { CloudConnect, LocalModels } from '../ModelSetup';
import { navigate } from '../router';

const SKIP_KEY = 'gai.welcomeDone';

/** The welcome screen was finished or skipped in this browser. */
export function welcomeDone(): boolean {
  try {
    return localStorage.getItem(SKIP_KEY) === '1';
  } catch {
    return false;
  }
}

function markDone() {
  try {
    localStorage.setItem(SKIP_KEY, '1');
  } catch {
    /* fine: the screen may show again */
  }
}

type Path = 'cloud' | 'local' | 'demo';

/** First run: connect an AI in a minute, without a terminal. */
export function Welcome() {
  const [status, setStatus] = useState<SetupStatus | null>(null);
  const [path, setPath] = useState<Path | null>(null);
  const [ready, setReady] = useState<string | null>(null);

  useEffect(() => {
    api
      .setupStatus()
      .then((s) => {
        setStatus(s);
        if (s.ollama.reachable && s.ollama.models.length) setPath('local');
      })
      .catch((e: Error) => toast.error(e.message));
  }, []);

  async function use(model: string) {
    try {
      await api.setDefaultModel(model);
      setReady(model);
    } catch (e) {
      toast.error((e as Error).message);
    }
  }

  function finish() {
    markDone();
    navigate('/chat');
  }

  if (ready) {
    return (
      <div className="welcome-page">
        <div className="welcome-card done">
          <div className="welcome-mark">
            <IconCheck width={28} height={28} />
          </div>
          <h1>You're all set</h1>
          <p className="muted">
            Chats will use <strong>{modelLabel(ready)}</strong>. You can change this any time in Settings.
          </p>
          <Button tone="accent" size="lg" leading={<IconChat />} onClick={finish} autoFocus>
            Start chatting
          </Button>
        </div>
      </div>
    );
  }

  return (
    <div className="welcome-page">
      <header className="welcome-head">
        <span className="logo big">g</span>
        <h1>Welcome to gai</h1>
        <p className="muted">Your own AI assistants, on your own server. First, choose where the AI runs.</p>
      </header>

      <div className="path-grid">
        <PathCard
          active={path === 'cloud'}
          onClick={() => setPath('cloud')}
          icon={<IconSpark />}
          title="Use a cloud AI"
          text="The smartest models (Claude, GPT, Gemini). Paste a key from the provider; you pay them for what you use."
        />
        <PathCard
          active={path === 'local'}
          onClick={() => setPath('local')}
          icon={<IconModel />}
          title="Run free on this computer"
          text="Private and offline after a one-time download. Slower and less capable than cloud models."
        />
        <PathCard
          active={path === 'demo'}
          onClick={() => setPath('demo')}
          icon={<IconChat />}
          title="Just look around"
          text="Try the app with a demo model that repeats what you say. No setup."
        />
      </div>

      {path && status && (
        <section className="welcome-card">
          {path === 'cloud' && <CloudConnect providers={status.providers} onConnected={use} />}
          {path === 'local' && <LocalModels onUse={use} />}
          {path === 'demo' && (
            <div className="stack">
              <p>
                The demo model doesn't think; it echoes your message so you can explore chats, assistants and automations. Connect a real AI
                any time from <strong>Settings → AI models</strong>.
              </p>
              <div>
                <Button
                  tone="accent"
                  onClick={async () => {
                    await use('mock/echo');
                  }}
                >
                  Continue with the demo
                </Button>
              </div>
            </div>
          )}
        </section>
      )}

      <p className="muted small center">
        Already set up?{' '}
        <button type="button" className="link small" onClick={finish}>
          Skip to the chat
        </button>
      </p>
    </div>
  );
}

function PathCard({
  active,
  onClick,
  icon,
  title,
  text,
}: {
  active: boolean;
  onClick: () => void;
  icon: React.ReactNode;
  title: string;
  text: string;
}) {
  return (
    <button type="button" className="path-card" aria-pressed={active} onClick={onClick}>
      <span className="path-icon">{icon}</span>
      <strong>{title}</strong>
      <span className="muted small">{text}</span>
    </button>
  );
}
