import { useEffect, useState } from 'react';
import { toast } from 'sonner';
import { SelectField, SwitchField, ToggleField } from '@/components/fields';
import { Disclosure, Page, PageHeader } from '@/components/shared';
import { Button } from '@/components/ui/button';
import { Card, CardAction, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from '@/components/ui/card';
import { FieldDescription } from '@/components/ui/field';
import { api, type SetupStatus } from '../api';
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
    api
      .setupStatus()
      .then(setStatus)
      .catch((e: Error) => toast.error(e.message));
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
    options.unshift({ value: current, label: dev ? current : modelLabel(current), description: 'Current default', group: 'Current' });
  }

  return (
    <Page className="max-w-4xl">
      <PageHeader title="AI models" subtitle="Choose the AI your assistants use, and connect more." />

      <Card>
        <CardHeader>
          <CardTitle>Used for new chats</CardTitle>
          <CardDescription>Assistants that name their own model keep using it.</CardDescription>
          {status?.default_model_source === 'settings' && (
            <CardAction>
              <Button variant="ghost" size="sm" onClick={() => setDefault(null)}>
                Use gai.toml's choice
              </Button>
            </CardAction>
          )}
        </CardHeader>
        <CardContent className="flex flex-col gap-2">
          <SelectField
            aria-label="Default model"
            options={options}
            value={current}
            onValueChange={(m) => setDefault(m)}
            placeholder="Choose a model"
          />
          {status?.default_model_source && <FieldDescription>{SOURCE[status.default_model_source]}</FieldDescription>}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Cloud AI</CardTitle>
          <CardDescription>Paste a key from a provider. You pay them for what you use.</CardDescription>
        </CardHeader>
        <CardContent>
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
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>On this computer</CardTitle>
          <CardDescription>Free and private, with Ollama.</CardDescription>
        </CardHeader>
        <CardContent>
          <LocalModels onUse={(m) => setDefault(m)} />
        </CardContent>
      </Card>

      <Disclosure summary="Every model and provider" open={dev}>
        <Models embedded />
      </Disclosure>
    </Page>
  );
}

/** Settings → Preferences: theme and developer details. */
export function Preferences() {
  const [dev, setDev] = useDevMode();
  const [theme, setTheme] = useTheme();
  return (
    <Page className="max-w-4xl">
      <PageHeader title="Preferences" subtitle="These apply to this browser only." />
      <Card>
        <CardHeader>
          <CardTitle>Appearance</CardTitle>
        </CardHeader>
        <CardContent>
          <ToggleField
            label="Theme"
            value={theme}
            onValueChange={(v) => setTheme(v as Theme)}
            options={[
              { value: 'system', label: 'Match the system' },
              { value: 'light', label: 'Light' },
              { value: 'dark', label: 'Dark' },
            ]}
          />
        </CardContent>
      </Card>
      <Card>
        <CardHeader>
          <CardTitle>Developer details</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          <SwitchField
            label="Show developer details"
            description="Run and session IDs, raw JSON and YAML editors, event logs, token counts, model IDs and command-line hints."
            checked={dev}
            onCheckedChange={(checked) => setDev(checked)}
          />
          {dev && (
            <FieldDescription>
              The REST API is documented at{' '}
              <a href="/docs" target="_blank" rel="noreferrer">
                /docs
              </a>
              .
            </FieldDescription>
          )}
        </CardContent>
      </Card>
      <Card>
        <CardHeader>
          <CardTitle>Start over</CardTitle>
          <CardDescription>Show the welcome screen again, to connect another AI step by step.</CardDescription>
        </CardHeader>
        <CardFooter>
          <Button variant="outline" onClick={() => (location.hash = '#/welcome')}>
            Open the welcome screen
          </Button>
        </CardFooter>
      </Card>
    </Page>
  );
}
