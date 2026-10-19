import { useEffect, useState } from 'react';
import { toast } from 'sonner';
import { Avatar, AvatarFallback } from '@/components/ui/avatar';
import { Button } from '@/components/ui/button';
import { Card, CardContent } from '@/components/ui/card';
import { Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from '@/components/ui/empty';
import { Field, FieldContent, FieldDescription, FieldLabel, FieldTitle } from '@/components/ui/field';
import { RadioGroup, RadioGroupItem } from '@/components/ui/radio-group';
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
      <Empty className="mx-auto mt-[12vh] max-w-lg">
        <EmptyHeader>
          <EmptyMedia variant="icon">
            <IconCheck />
          </EmptyMedia>
          <EmptyTitle>You're all set</EmptyTitle>
          <EmptyDescription>Chats will use {modelLabel(ready)}. You can change this any time in Settings.</EmptyDescription>
        </EmptyHeader>
        <EmptyContent>
          <Button size="lg" onClick={finish} autoFocus>
            <IconChat data-icon="inline-start" />
            Start chatting
          </Button>
        </EmptyContent>
      </Empty>
    );
  }

  return (
    <div className="mx-auto flex w-full max-w-4xl flex-col gap-8 px-6 py-12">
      <header className="flex flex-col items-center gap-3 text-center">
        <Avatar className="size-12 rounded-xl">
          <AvatarFallback className="rounded-xl bg-primary text-xl font-semibold text-primary-foreground">g</AvatarFallback>
        </Avatar>
        <h1 className="text-3xl font-semibold tracking-tight">Welcome to gai</h1>
        <p className="text-muted-foreground">Your own AI assistants, on your own server. First, choose where the AI runs.</p>
      </header>

      <RadioGroup
        value={path ?? ''}
        onValueChange={(v) => setPath(v as Path)}
        aria-label="Where the AI runs"
        className="grid gap-3 md:grid-cols-3"
      >
        {PATHS.map((p) => (
          <FieldLabel key={p.id} htmlFor={`path-${p.id}`}>
            <Field orientation="horizontal" className="items-start">
              <FieldContent>
                <FieldTitle>
                  <p.icon />
                  {p.title}
                </FieldTitle>
                <FieldDescription>{p.text}</FieldDescription>
              </FieldContent>
              <RadioGroupItem value={p.id} id={`path-${p.id}`} />
            </Field>
          </FieldLabel>
        ))}
      </RadioGroup>

      {path && status && (
        <Card>
          <CardContent>
            {path === 'cloud' && <CloudConnect providers={status.providers} onConnected={use} />}
            {path === 'local' && <LocalModels onUse={use} />}
            {path === 'demo' && (
              <div className="flex flex-col items-start gap-3">
                <p>
                  The demo model doesn't think; it echoes your message so you can explore chats, assistants and automations. Connect a real
                  AI any time from Settings → AI models.
                </p>
                <Button onClick={() => use('mock/echo')}>Continue with the demo</Button>
              </div>
            )}
          </CardContent>
        </Card>
      )}

      <p className="text-center text-sm text-muted-foreground">
        Already set up?{' '}
        <Button variant="link" size="sm" className="h-auto px-0" onClick={finish}>
          Skip to the chat
        </Button>
      </p>
    </div>
  );
}

const PATHS: { id: Path; icon: typeof IconSpark; title: string; text: string }[] = [
  {
    id: 'cloud',
    icon: IconSpark,
    title: 'Use a cloud AI',
    text: 'The smartest models (Claude, GPT, Gemini). Paste a key from the provider; you pay them for what you use.',
  },
  {
    id: 'local',
    icon: IconModel,
    title: 'Run free on this computer',
    text: 'Private and offline after a one-time download. Slower and less capable than cloud models.',
  },
  { id: 'demo', icon: IconChat, title: 'Just look around', text: 'Try the app with a demo model that repeats what you say. No setup.' },
];
