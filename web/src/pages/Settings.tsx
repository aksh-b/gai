import { useEffect, useState } from 'react';
import { Button } from '@/components/ui/button';
import { RadioField, SelectField, SwitchField } from '@/components/fields';
import { toast } from 'sonner';
import { api, type SetupStatus } from '../api';
import { Disclosure, PageHeader } from '../components';
import { modelLabel } from '../labels';
import { modelOptions, useModelGroups } from '../models';
import { CloudConnect, LocalModels } from '../ModelSetup';
import { useDevMode, useTheme, type Theme } from '../prefs';
import { Models } from './Models';

const SOURCE: Record<string, string> = {
  settings: 'Chosen here.',
  config: 'Set in gai.toml (or GAI_DEFAULT_MODEL). Choosing one here overrides it.',
  detected: 'Picked automatically because its key is set.',
};

/** Settings → AI models: the default model, cloud keys and local models. */
export function ModelsSettings() {
  const [dev] = useDevMode();
  const [version, setVersion] = useState(0);
  const groups = useModelGroups(version);
  const [status, setStatus] = useState<SetupStatus | null>(null);

  useEffect(() => {
    api.setupStatus().then(setStatus).catch((e: Error) => toast.error(e.message));
  }, [version]);

  async function setDefault(model: string | null, quiet = false) {
    try {
      await api.setDefaultModel(model);
      if (!quiet) toast.success(model ? `Chats now use ${modelLabel(model)}` : 'Back to the model in gai.toml');
      setVersion((v) => v + 1);
    } catch (e) {
      toast.error((e as Error).message);
    }
  }

  const options = modelOptions(groups, dev);
  const current = status?.default_model ?? null;
  if (current && !options.some((o) => o.value === current)) {
    options.unshift({ value: current, label: dev ? current : modelLabel(current), description: 'Current default' });
  }

  return (
    <div className="page narrow">
      <PageHeader title="AI models" subtitle="Choose the AI your assistants use, and connect more." />

      <section className="card stack">
        <div>
          <h3>Used for new chats</h3>
          <p className="muted small">Assistants that name their own model keep using it.</p>
        </div>
        <div className="row gap wrap">
          <div className="grow">
            <SelectField aria-label="Default model" options={options} value={current} onValueChange={(m) => setDefault(m)} placeholder="Choose a model" />
          </div>
          {status?.default_model_source === 'settings' && (
            <Button variant="ghost" size="sm" onClick={() => setDefault(null)}>
              Use gai.toml's choice
            </Button>
          )}
        </div>
        {status?.default_model_source && <p className="muted small">{SOURCE[status.default_model_source]}</p>}
      </section>

      <section className="card stack">
        <div>
          <h3>Cloud AI</h3>
          <p className="muted small">Paste a key from a provider. You pay them for what you use.</p>
        </div>
        {status && (
          <CloudConnect
            providers={status.providers}
            onConnected={(model) => {
              const demo = !current || current.startsWith('mock/');
              if (demo) setDefault(model, true);
              setVersion((v) => v + 1);
            }}
          />
        )}
      </section>

      <section className="card stack">
        <div>
          <h3>On this computer</h3>
          <p className="muted small">Free and private, with Ollama.</p>
        </div>
        <LocalModels onUse={(m) => setDefault(m)} />
      </section>

      <Disclosure summary={<strong>Every model and provider</strong>} open={dev}>
        <Models embedded />
      </Disclosure>
    </div>
  );
}

/** Settings → Preferences: theme and developer details. */
export function Preferences() {
  const [dev, setDev] = useDevMode();
  const [theme, setTheme] = useTheme();
  return (
    <div className="page narrow">
      <PageHeader title="Preferences" subtitle="These apply to this browser only." />
      <section className="card stack">
        <h3>Appearance</h3>
        <RadioField
          label="Theme"
          value={theme}
          onValueChange={(v) => setTheme(v as Theme)}
          options={[
            { value: 'system', label: 'Match the system' },
            { value: 'light', label: 'Light' },
            { value: 'dark', label: 'Dark' },
          ]}
        />
      </section>
      <section className="card stack">
        <h3>Developer details</h3>
        <SwitchField
          label="Show developer details"
          description="Run and session IDs, raw JSON and YAML editors, event logs, token counts, model IDs and command-line hints."
          checked={dev}
          onCheckedChange={(checked) => setDev(checked)}
        />
        {dev && (
          <p className="muted small">
            The REST API is documented at{' '}
            <a href="/docs" target="_blank" rel="noreferrer">
              /docs ↗
            </a>
            .
          </p>
        )}
      </section>
      <section className="card stack">
        <h3>Start over</h3>
        <p className="muted small">Show the welcome screen again, to connect another AI step by step.</p>
        <div>
          <Button variant="outline" onClick={() => (location.hash = '#/welcome')}>
            Open the welcome screen
          </Button>
        </div>
      </section>
    </div>
  );
}
