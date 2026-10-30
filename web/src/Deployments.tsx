import { useEffect, useState } from 'react';
import { CpuIcon, PlayIcon, ServerIcon, SquareIcon } from 'lucide-react';
import { toast } from 'sonner';
import { SelectField, SwitchField, TextField } from '@/components/fields';
import { ConfirmAction, EmptyState } from '@/components/shared';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { FieldDescription } from '@/components/ui/field';
import { Item, ItemActions, ItemContent, ItemDescription, ItemFooter, ItemGroup, ItemTitle } from '@/components/ui/item';
import { Progress } from '@/components/ui/progress';
import { api, type DeploySpec, type DeploymentView, type GpuStatus, type ServerKind } from './api';
import { IconPlus, IconTrash } from './icons';
import { useLoad } from './util';

const SERVERS: { value: ServerKind; label: string; description: string; model: string; hint: string }[] = [
  {
    value: 'llamacpp',
    label: 'llama.cpp',
    description: 'Runs GGUF models on CPU or GPU. Light and simple.',
    model: 'ggml-org/gemma-3-1b-it-GGUF',
    hint: 'A Hugging Face GGUF repository (add :Q4_K_M for a quantization) or a link to a .gguf file.',
  },
  {
    value: 'vllm',
    label: 'vLLM',
    description: 'Fast serving for many users at once. Needs an NVIDIA GPU.',
    model: 'Qwen/Qwen3-8B',
    hint: 'A Hugging Face model id. Gated models need HF_TOKEN set where gai runs.',
  },
  {
    value: 'ollama',
    label: 'Ollama',
    description: 'Pulls models by name from the Ollama library.',
    model: 'qwen3:4b',
    hint: 'Models to pull, separated by commas. Leave empty to pull later.',
  },
];

function GpuCard({ g }: { g: GpuStatus }) {
  if (g.gpus.length === 0) {
    return (
      <FieldDescription>
        No NVIDIA GPU found on this machine{g.engine_gpu ? '' : ', and the container engine has no GPU support'}. Models run on the CPU, which
        suits small models.
      </FieldDescription>
    );
  }
  return (
    <ItemGroup className="gap-2">
      {g.gpus.map((gpu) => (
        <Item key={gpu.index} variant="muted" size="sm">
          <CpuIcon />
          <ItemContent>
            <ItemTitle>{gpu.name}</ItemTitle>
            <ItemDescription>
              {(gpu.memory_used_mb / 1024).toFixed(1)} of {(gpu.memory_total_mb / 1024).toFixed(0)} GB used · {gpu.utilization}% busy
              {gpu.temperature_c != null && ` · ${gpu.temperature_c}°C`}
            </ItemDescription>
            <Progress value={(100 * gpu.memory_used_mb) / Math.max(1, gpu.memory_total_mb)} />
          </ItemContent>
        </Item>
      ))}
      {!g.engine_gpu && (
        <FieldDescription>The container engine can't use the GPU yet: install the NVIDIA Container Toolkit.</FieldDescription>
      )}
    </ItemGroup>
  );
}

function state(d: DeploymentView): { label: string; variant: 'default' | 'secondary' | 'destructive' | 'outline' } {
  if (d.health.ok) return { label: 'Ready', variant: 'default' };
  if (!d.container) return { label: d.health.error === 'not started' ? 'Not started' : 'Unavailable', variant: 'outline' };
  if (d.container.running) return { label: 'Starting', variant: 'secondary' };
  return { label: d.container.exit_code ? `Stopped (exit ${d.container.exit_code})` : 'Stopped', variant: 'outline' };
}

function DeploymentItem({ d, onChange, onUse }: { d: DeploymentView; onChange: () => void; onUse: (model: string) => void }) {
  const [logs, setLogs] = useState<string | null>(null);
  const s = state(d);

  async function act(what: 'start' | 'stop' | 'delete') {
    try {
      if (what === 'start') await api.startDeployment(d.spec.name);
      else if (what === 'stop') await api.stopDeployment(d.spec.name);
      else await api.deleteDeployment(d.spec.name);
      onChange();
    } catch (e) {
      toast.error((e as Error).message);
    }
  }

  return (
    <Item variant="outline">
      <ServerIcon />
      <ItemContent>
        <ItemTitle className="flex items-center gap-2">
          {d.spec.name}
          <Badge variant={s.variant}>{s.label}</Badge>
          {d.spec.gpu && <Badge variant="outline">GPU</Badge>}
        </ItemTitle>
        <ItemDescription>
          {SERVERS.find((x) => x.value === d.spec.server)?.label} · {d.spec.model ?? 'no model yet'}
          {!d.health.ok && d.health.error && d.container?.running ? ` · ${d.health.error}` : ''}
        </ItemDescription>
      </ItemContent>
      <ItemActions>
        {d.container?.running ? (
          <Button size="sm" variant="outline" onClick={() => act('stop')}>
            <SquareIcon data-icon="inline-start" />
            Stop
          </Button>
        ) : (
          <Button size="sm" variant="outline" onClick={() => act('start')}>
            <PlayIcon data-icon="inline-start" />
            Start
          </Button>
        )}
        <Button size="sm" variant="ghost" onClick={async () => setLogs(logs === null ? await api.deploymentLogs(d.spec.name).catch((e: Error) => e.message) : null)}>
          {logs === null ? 'Logs' : 'Hide logs'}
        </Button>
        <ConfirmAction
          title={`Remove ${d.spec.name}?`}
          description="The server stops and its container is deleted. Downloaded models are kept for next time."
          action="Remove"
          destructive
          onConfirm={() => act('delete')}
        >
          <Button size="icon-sm" variant="ghost" aria-label="Remove">
            <IconTrash />
          </Button>
        </ConfirmAction>
      </ItemActions>
      {(d.health.ok || logs !== null) && (
        <ItemFooter className="flex-col items-stretch gap-2">
          {d.health.ok && (
            <div className="flex flex-wrap gap-2">
              {d.health.models.map((m) => (
                <Button key={m} size="sm" variant="secondary" onClick={() => onUse(`${d.provider}/${m}`)}>
                  Use {m}
                </Button>
              ))}
            </div>
          )}
          {logs !== null && <pre className="max-h-64 overflow-auto rounded-md bg-muted p-2 text-xs">{logs || 'No output yet.'}</pre>}
        </ItemFooter>
      )}
    </Item>
  );
}

function NewDeployment({ onDone }: { onDone: () => void }) {
  const [spec, setSpec] = useState<DeploySpec>({ name: '', server: 'llamacpp', model: SERVERS[0].model, gpu: false });
  const [busy, setBusy] = useState(false);
  const server = SERVERS.find((s) => s.value === spec.server)!;

  async function save() {
    setBusy(true);
    try {
      const clean = { ...spec, model: spec.model?.trim() || undefined };
      await api.deploy(clean);
      toast.success(`Starting ${spec.name}. Downloading the server and model can take a while.`);
      onDone();
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="flex flex-col gap-4 rounded-lg border p-4">
      <div className="grid gap-4 sm:grid-cols-2">
        <SelectField
          label="Server"
          options={SERVERS.map((s) => ({ value: s.value, label: s.label, description: s.description }))}
          value={spec.server}
          onValueChange={(v) => {
            const s = SERVERS.find((x) => x.value === v)!;
            setSpec((x) => ({ ...x, server: s.value, model: s.model, gpu: s.value === 'vllm' ? true : x.gpu }));
          }}
        />
        <TextField
          label="Name"
          placeholder="gemma"
          value={spec.name}
          onChange={(e) => setSpec((x) => ({ ...x, name: e.target.value.toLowerCase().replace(/[^a-z0-9-]/g, '-') }))}
        />
      </div>
      <TextField label="Model" description={server.hint} value={spec.model ?? ''} onChange={(e) => setSpec((x) => ({ ...x, model: e.target.value }))} />
      <div className="grid gap-4 sm:grid-cols-2">
        <SwitchField
          label="Use the GPU"
          description={spec.server === 'vllm' ? 'vLLM always needs one.' : 'All NVIDIA GPUs on this machine.'}
          checked={!!spec.gpu}
          disabled={spec.server === 'vllm'}
          onCheckedChange={(c) => setSpec((x) => ({ ...x, gpu: c }))}
        />
        <TextField
          label="Context window (tokens)"
          type="number"
          placeholder="Server default"
          value={spec.context ?? ''}
          onChange={(e) => setSpec((x) => ({ ...x, context: e.target.value ? Number(e.target.value) : undefined }))}
        />
      </div>
      <div>
        <Button disabled={busy || !spec.name} onClick={save}>
          Start it
        </Button>
      </div>
    </div>
  );
}

/** Model servers gai runs in containers, with the machine's GPUs. */
export function Deployments({ onUse }: { onUse: (model: string) => void }) {
  const list = useLoad(() => api.deployments(), []);
  const gpus = useLoad(() => api.gpus(), []);
  const [adding, setAdding] = useState(false);
  const starting = list.data?.some((d) => d.container?.running && !d.health.ok);

  useEffect(() => {
    if (!starting) return;
    const t = setInterval(list.reload, 5000);
    return () => clearInterval(t);
  }, [starting, list.reload]);

  if (list.error) return <FieldDescription>{list.error}</FieldDescription>;

  return (
    <div className="flex flex-col gap-4">
      {gpus.data && <GpuCard g={gpus.data} />}
      {list.data?.length === 0 && !adding && (
        <EmptyState title="No model servers" icon={<ServerIcon />}>
          Run llama.cpp, vLLM or Ollama in a container, started and stopped from here. Needs Docker or Podman.
        </EmptyState>
      )}
      <ItemGroup className="gap-2">
        {list.data?.map((d) => (
          <DeploymentItem key={d.spec.name} d={d} onChange={list.reload} onUse={onUse} />
        ))}
      </ItemGroup>
      {adding ? (
        <NewDeployment
          onDone={() => {
            setAdding(false);
            list.reload();
          }}
        />
      ) : (
        <div>
          <Button variant="outline" onClick={() => setAdding(true)}>
            <IconPlus data-icon="inline-start" />
            Run a model server
          </Button>
        </div>
      )}
    </div>
  );
}
