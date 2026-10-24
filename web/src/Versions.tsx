import { useEffect, useState } from 'react';
import { XIcon } from 'lucide-react';
import { toast } from 'sonner';
import { SelectField } from '@/components/fields';
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { CodeBlock, CodeBlockContent, CodeBlockHeader, CodeBlockLine, CodeBlockTitle } from '@/components/ui/code-block';
import { Input } from '@/components/ui/input';
import { Item, ItemActions, ItemContent, ItemDescription, ItemFooter, ItemGroup, ItemTitle } from '@/components/ui/item';
import { Label } from '@/components/ui/label';
import { api, type VersionDiff } from './api';
import { ErrorNote } from '@/components/shared';
import { ago } from './util';

interface VersionRow {
  version: number;
  note?: string | null;
  created_at: string;
}

/** What a version history works on: agent, workflow or persona endpoints. */
const ENDPOINTS = {
  agent: { diff: api.agentDiff, rollback: api.rollbackAgent, label: api.setAgentLabel },
  workflow: { diff: api.workflowDiff, rollback: api.rollbackWorkflow, label: api.setWorkflowLabel },
  persona: { diff: api.personaDiff, rollback: api.rollbackPersona, label: api.setPersonaLabel },
};

/**
 * Every stored version, newest first, with labels, a diff between any two
 * versions, rollback (saves the old spec again as the newest version) and
 * labelling (e.g. point `prod` at v3, then run `name@prod`).
 */
export function VersionHistory({
  kind,
  name,
  versions,
  labels,
  onView,
  onChanged,
}: {
  kind: 'agent' | 'workflow' | 'persona';
  name: string;
  versions: VersionRow[];
  labels: Record<string, number>;
  onView?: (version: number) => void;
  onChanged: () => void;
}) {
  const ep = ENDPOINTS[kind];
  const latest = versions[0]?.version ?? 0;
  const [from, setFrom] = useState<number>(Math.max(1, latest - 1));
  const [to, setTo] = useState<number>(latest);
  const [diff, setDiff] = useState<VersionDiff>();
  const [error, setError] = useState<string>();
  const [labelling, setLabelling] = useState<number | null>(null);
  const [labelName, setLabelName] = useState('');
  const [confirming, setConfirming] = useState<number | null>(null);

  useEffect(() => {
    setFrom(Math.max(1, latest - 1));
    setTo(latest);
    setDiff(undefined);
  }, [name, latest]);

  async function compare(a: number, b: number) {
    setFrom(a);
    setTo(b);
    try {
      setDiff(await ep.diff(name, a, b));
      setError(undefined);
    } catch (e) {
      setError((e as Error).message);
    }
  }

  async function rollback(v: number) {
    setConfirming(null);
    try {
      const r = await ep.rollback(name, v);
      toast.success(r.version === latest ? `v${v} is already the latest` : `Rolled back: v${r.version} = v${v}`);
      onChanged();
    } catch (e) {
      toast.error((e as Error).message);
    }
  }

  async function label(l: string, v: number | null) {
    try {
      await ep.label(name, l, v);
      toast.success(v === null ? `Removed ${l}` : `${l} → v${v}`);
      setLabelling(null);
      setLabelName('');
      onChanged();
    } catch (e) {
      toast.error((e as Error).message);
    }
  }

  const byVersion = new Map<number, string[]>();
  for (const [l, v] of Object.entries(labels)) byVersion.set(v, [...(byVersion.get(v) ?? []), l]);
  const options = versions.map((v) => ({ value: String(v.version), label: `v${v.version}` }));

  if (!versions.length) return <p className="text-muted-foreground">No stored versions.</p>;
  return (
    <div className="flex flex-col gap-4">
      <ItemGroup className="gap-2">
        {versions.map((v) => (
          <Item key={v.version} variant="outline" size="sm">
            <ItemContent>
              <ItemTitle className="flex-wrap">
                {onView ? (
                  <Button variant="link" size="sm" className="h-auto px-0" onClick={() => onView(v.version)}>
                    v{v.version}
                  </Button>
                ) : (
                  `v${v.version}`
                )}
                {v.version === latest && <Badge variant="outline">Latest</Badge>}
                {(byVersion.get(v.version) ?? []).map((l) => (
                  <Badge key={l} variant="secondary">
                    {l}
                    <Button
                      variant="ghost"
                      size="icon-xs"
                      className="-mr-1.5 size-4"
                      aria-label={`Remove label ${l}`}
                      onClick={() => label(l, null)}
                    >
                      <XIcon />
                    </Button>
                  </Badge>
                ))}
              </ItemTitle>
              <ItemDescription>
                {ago(v.created_at)}
                {v.note && ` · ${v.note}`}
              </ItemDescription>
            </ItemContent>
            <ItemActions>
              {v.version > 1 && (
                <Button variant="ghost" size="xs" onClick={() => compare(v.version - 1, v.version)}>
                  Changes
                </Button>
              )}
              <Button variant="ghost" size="xs" onClick={() => setLabelling(labelling === v.version ? null : v.version)}>
                Label
              </Button>
              {v.version !== latest && (
                <Button variant="ghost" size="xs" onClick={() => setConfirming(v.version)}>
                  Roll back
                </Button>
              )}
            </ItemActions>
            {labelling === v.version && (
              <ItemFooter>
                <form
                  className="flex w-full gap-2"
                  onSubmit={(e) => {
                    e.preventDefault();
                    if (labelName.trim()) label(labelName.trim(), v.version);
                  }}
                >
                  <Input
                    placeholder="prod, staging…"
                    aria-label="Label name"
                    value={labelName}
                    onChange={(e) => setLabelName(e.target.value)}
                    autoFocus
                  />
                  <Button size="sm" type="submit" disabled={!/^[A-Za-z0-9_.-]+$/.test(labelName.trim())}>
                    Point at v{v.version}
                  </Button>
                </form>
              </ItemFooter>
            )}
          </Item>
        ))}
      </ItemGroup>

      {versions.length > 1 && (
        <div className="flex flex-col gap-3">
          <div className="flex flex-wrap items-center gap-2">
            <Label>Compare</Label>
            <SelectField
              aria-label="Older version"
              className="w-24"
              options={options}
              value={String(from)}
              onValueChange={(v) => setFrom(Number(v))}
            />
            <span className="text-muted-foreground">→</span>
            <SelectField
              aria-label="Newer version"
              className="w-24"
              options={options}
              value={String(to)}
              onValueChange={(v) => setTo(Number(v))}
            />
            <Button variant="outline" size="sm" onClick={() => compare(from, to)}>
              Show changes
            </Button>
          </div>
          <ErrorNote error={error} />
          {diff && <DiffView diff={diff.diff} title={`v${diff.from} → v${diff.to}`} />}
        </div>
      )}

      <AlertDialog open={confirming !== null} onOpenChange={(o) => !o && setConfirming(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Roll back to v{confirming}?</AlertDialogTitle>
            <AlertDialogDescription>
              v{confirming} of {name} is saved again as the newest version. Nothing is deleted: the current version stays in the history.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction onClick={() => confirming !== null && rollback(confirming)}>Roll back</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

/** A unified diff, coloured by line. */
export function DiffView({ diff, title }: { diff: string; title?: string }) {
  if (!diff) return <p className="text-muted-foreground">{title ? `${title}: ` : ''}no differences.</p>;
  const lines = diff.split('\n').filter((l, i, all) => !(i === all.length - 1 && l === ''));
  return (
    <CodeBlock size="sm" aria-label={title ?? 'Changes'}>
      {title && (
        <CodeBlockHeader>
          <CodeBlockTitle>{title}</CodeBlockTitle>
        </CodeBlockHeader>
      )}
      <CodeBlockContent wrap className="max-h-[32rem]">
        {lines.map((l, i) => {
          const file = l.startsWith('+++') || l.startsWith('---');
          const variant = file
            ? 'default'
            : l.startsWith('@@')
              ? 'highlighted'
              : l[0] === '+'
                ? 'added'
                : l[0] === '-'
                  ? 'removed'
                  : 'default';
          return (
            <CodeBlockLine key={i} variant={variant}>
              {(file || variant === 'added' || variant === 'removed' ? l.slice(1) : l) || ' '}
            </CodeBlockLine>
          );
        })}
      </CodeBlockContent>
    </CodeBlock>
  );
}

export type LineOp = { op: 'same' | 'add' | 'del'; text: string };

/** Line diff of two texts (longest common subsequence). */
export function lineDiff(a: string, b: string): LineOp[] {
  const x = a.split('\n');
  const y = b.split('\n');
  const n = x.length;
  const m = y.length;
  // Too big to compare line by line: show old then new.
  if (n * m > 4_000_000) return [...x.map((text) => ({ op: 'del' as const, text })), ...y.map((text) => ({ op: 'add' as const, text }))];
  const lcs: number[][] = Array.from({ length: n + 1 }, () => new Array<number>(m + 1).fill(0));
  for (let i = n - 1; i >= 0; i--)
    for (let j = m - 1; j >= 0; j--) lcs[i][j] = x[i] === y[j] ? lcs[i + 1][j + 1] + 1 : Math.max(lcs[i + 1][j], lcs[i][j + 1]);
  const out: LineOp[] = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (x[i] === y[j]) {
      out.push({ op: 'same', text: x[i] });
      i++;
      j++;
    } else if (lcs[i + 1][j] >= lcs[i][j + 1]) out.push({ op: 'del', text: x[i++] });
    else out.push({ op: 'add', text: y[j++] });
  }
  while (i < n) out.push({ op: 'del', text: x[i++] });
  while (j < m) out.push({ op: 'add', text: y[j++] });
  return out;
}
