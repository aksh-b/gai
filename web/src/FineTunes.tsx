import { useEffect, useState } from 'react';
import { SparklesIcon } from 'lucide-react';
import { toast } from 'sonner';
import { SelectField, TextField } from '@/components/fields';
import { ConfirmAction, EmptyState } from '@/components/shared';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { FieldDescription } from '@/components/ui/field';
import { Item, ItemActions, ItemContent, ItemDescription, ItemGroup, ItemTitle } from '@/components/ui/item';
import { api, type FineTuneJob, type FineTuneSpec, type Trainer } from './api';
import { IconPlus, IconTrash } from './icons';
import { ago, useLoad } from './util';

const TRAINERS: { value: Trainer; label: string; description: string; base: string }[] = [
  { value: 'openai', label: 'OpenAI', description: 'Fine-tunes a GPT model on their servers. Needs an OpenAI key.', base: 'gpt-4.1-mini-2025-04-14' },
  {
    value: 'together',
    label: 'Together',
    description: 'LoRA fine-tune of an open model on their servers. Needs TOGETHER_API_KEY.',
    base: 'meta-llama/Meta-Llama-3.1-8B-Instruct-Reference',
  },
  {
    value: 'local',
    label: 'This machine',
    description: 'Trains a LoRA adapter here on the GPU, then serves it with vLLM.',
    base: 'Qwen/Qwen2.5-1.5B-Instruct',
  },
];

const BADGE: Record<FineTuneJob['status'], 'default' | 'secondary' | 'destructive' | 'outline'> = {
  preparing: 'secondary',
  running: 'secondary',
  succeeded: 'default',
  failed: 'destructive',
  cancelled: 'outline',
};

function JobItem({ j, onChange, onUse }: { j: FineTuneJob; onChange: () => void; onUse: (m: string) => void }) {
  const running = j.status === 'running' || j.status === 'preparing';
  async function act(what: 'check' | 'cancel' | 'delete') {
    try {
      if (what === 'check') await api.checkFineTune(j.spec.name);
      else if (what === 'cancel') await api.cancelFineTune(j.spec.name);
      else await api.deleteFineTune(j.spec.name);
      onChange();
    } catch (e) {
      toast.error((e as Error).message);
    }
  }
  return (
    <Item variant="outline">
      <ItemContent>
        <ItemTitle className="flex items-center gap-2">
          {j.spec.name}
          <Badge variant={BADGE[j.status]}>{j.status}</Badge>
        </ItemTitle>
        <ItemDescription>
          {TRAINERS.find((t) => t.value === j.spec.trainer)?.label} · {j.spec.base_model} · {j.examples} examples · started {ago(j.created_at)}
          {j.error ? ` · ${j.error}` : ''}
        </ItemDescription>
      </ItemContent>
      <ItemActions>
        {j.model && (
          <Button size="sm" variant="secondary" onClick={() => onUse(j.model!)}>
            Use it
          </Button>
        )}
        {running && (
          <>
            <Button size="sm" variant="outline" onClick={() => act('check')}>
              Check now
            </Button>
            <Button size="sm" variant="ghost" onClick={() => act('cancel')}>
              Cancel
            </Button>
          </>
        )}
        {!running && (
          <ConfirmAction title={`Forget ${j.spec.name}?`} description="The model it made stays available." action="Forget" onConfirm={() => act('delete')}>
            <Button size="icon-sm" variant="ghost" aria-label="Forget">
              <IconTrash />
            </Button>
          </ConfirmAction>
        )}
      </ItemActions>
    </Item>
  );
}

function NewFineTune({ onDone }: { onDone: () => void }) {
  const agents = useLoad(() => api.agents(), []);
  const [spec, setSpec] = useState<FineTuneSpec>({ name: '', trainer: 'openai', base_model: TRAINERS[0].base, dataset: {} });
  const [preview, setPreview] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const trainer = TRAINERS.find((t) => t.value === spec.trainer)!;

  useEffect(() => {
    setPreview(null);
    api
      .previewDataset(spec.dataset ?? {})
      .then((p) =>
        setPreview(
          p.stats.examples === 0
            ? 'No usable rated runs yet: rate good answers with thumbs up.'
            : `${p.stats.examples} rated runs to train on${p.stats.skipped.length ? ` (${p.stats.skipped.length} can't be used)` : ''}.`,
        ),
      )
      .catch((e: Error) => setPreview(e.message));
  }, [spec.dataset]);

  async function start() {
    setBusy(true);
    try {
      const job = await api.startFineTune(spec);
      if (job.status === 'failed') toast.error(job.error ?? 'It failed to start');
      else toast.success(`Training ${spec.name}. It usually takes from minutes to hours.`);
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
          label="Where to train"
          options={TRAINERS.map((t) => ({ value: t.value, label: t.label, description: t.description }))}
          value={spec.trainer}
          onValueChange={(v) => {
            const t = TRAINERS.find((x) => x.value === v)!;
            setSpec((x) => ({ ...x, trainer: t.value, base_model: t.base }));
          }}
        />
        <TextField
          label="Name"
          placeholder="support-v2"
          value={spec.name}
          onChange={(e) => setSpec((x) => ({ ...x, name: e.target.value.toLowerCase().replace(/[^a-z0-9-]/g, '-') }))}
        />
        <TextField label="Start from" description={trainer.description} value={spec.base_model} onChange={(e) => setSpec((x) => ({ ...x, base_model: e.target.value }))} />
        <SelectField
          label="Learn from"
          options={[{ value: '', label: 'Every assistant' }, ...(agents.data ?? []).map((a) => ({ value: a.summary.name, label: a.summary.name }))]}
          value={spec.dataset?.agent ?? ''}
          onValueChange={(v) => setSpec((x) => ({ ...x, dataset: { ...x.dataset, agent: v || undefined } }))}
        />
      </div>
      {preview && <FieldDescription>{preview}</FieldDescription>}
      <div className="flex gap-2">
        <Button disabled={busy || !spec.name} onClick={start}>
          Start training
        </Button>
        <Button variant="ghost" onClick={() => api.downloadDataset(spec.dataset ?? {}).catch((e: Error) => toast.error(e.message))}>
          Download the training set
        </Button>
      </div>
    </div>
  );
}

/** Fine-tuning on rated runs. */
export function FineTunes({ onUse }: { onUse: (model: string) => void }) {
  const list = useLoad(() => api.fineTunes(), []);
  const [adding, setAdding] = useState(false);
  const running = list.data?.some((j) => j.status === 'running');

  useEffect(() => {
    if (!running) return;
    const t = setInterval(list.reload, 30000);
    return () => clearInterval(t);
  }, [running, list.reload]);

  return (
    <div className="flex flex-col gap-4">
      {list.error && <FieldDescription>{list.error}</FieldDescription>}
      {list.data?.length === 0 && !adding && (
        <EmptyState title="No fine-tuned models" icon={<SparklesIcon />}>
          Teach a model your assistants' best answers: the replies people rated thumbs up become its training set.
        </EmptyState>
      )}
      <ItemGroup className="gap-2">
        {list.data?.map((j) => (
          <JobItem key={j.spec.name} j={j} onChange={list.reload} onUse={onUse} />
        ))}
      </ItemGroup>
      {adding ? (
        <NewFineTune
          onDone={() => {
            setAdding(false);
            list.reload();
          }}
        />
      ) : (
        <div>
          <Button variant="outline" onClick={() => setAdding(true)}>
            <IconPlus data-icon="inline-start" />
            Fine-tune a model
          </Button>
        </div>
      )}
    </div>
  );
}
