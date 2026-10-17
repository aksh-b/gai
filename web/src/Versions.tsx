import { useEffect, useState } from 'react';
import { Button, Select, toast } from '@flame';
import { api, type VersionDiff } from './api';
import { ErrorNote } from './components';
import { ago } from './util';

interface VersionRow {
  version: number;
  note?: string | null;
  created_at: string;
}

/** What a version history works on: agent or workflow endpoints. */
const ENDPOINTS = {
  agent: { diff: api.agentDiff, rollback: api.rollbackAgent, label: api.setAgentLabel },
  workflow: { diff: api.workflowDiff, rollback: api.rollbackWorkflow, label: api.setWorkflowLabel },
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
  kind: 'agent' | 'workflow';
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
    if (!confirm(`Save v${v} of ${name} again as the newest version?`)) return;
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

  if (!versions.length) return <p className="muted">No stored versions.</p>;
  return (
    <div className="stack">
      <ol className="versions">
        {versions.map((v) => (
          <li key={v.version} className="version-row">
            <div className="row gap wrap">
              {onView ? (
                <button type="button" className="link" onClick={() => onView(v.version)}>
                  v{v.version}
                </button>
              ) : (
                <strong>v{v.version}</strong>
              )}
              {v.version === latest && <span className="chip small">latest</span>}
              {(byVersion.get(v.version) ?? []).map((l) => (
                <span key={l} className="chip accent small label-chip">
                  {l}
                  <button type="button" aria-label={`Remove label ${l}`} onClick={() => label(l, null)}>
                    ×
                  </button>
                </span>
              ))}
              <span className="muted small">{ago(v.created_at)}</span>
              <span className="small">{v.note}</span>
            </div>
            <div className="row gap version-actions">
              {v.version > 1 && (
                <button type="button" className="link small" onClick={() => compare(v.version - 1, v.version)}>
                  Changes
                </button>
              )}
              <button type="button" className="link small" onClick={() => setLabelling(labelling === v.version ? null : v.version)}>
                Label
              </button>
              {v.version !== latest && (
                <button type="button" className="link small" onClick={() => rollback(v.version)}>
                  Roll back
                </button>
              )}
            </div>
            {labelling === v.version && (
              <form
                className="row gap"
                onSubmit={(e) => {
                  e.preventDefault();
                  if (labelName.trim()) label(labelName.trim(), v.version);
                }}
              >
                <input
                  className="plain-input"
                  placeholder="prod, staging…"
                  aria-label="Label name"
                  value={labelName}
                  onChange={(e) => setLabelName(e.target.value)}
                  autoFocus
                />
                <Button size="sm" type="submit" tone="accent" disabled={!/^[A-Za-z0-9_.-]+$/.test(labelName.trim())}>
                  Point at v{v.version}
                </Button>
              </form>
            )}
          </li>
        ))}
      </ol>

      {versions.length > 1 && (
        <div className="stack">
          <div className="row gap wrap compare-row">
            <span className="label">Compare</span>
            <Select aria-label="Older version" options={options} value={String(from)} onValueChange={(v) => setFrom(Number(v))} />
            <span className="muted">→</span>
            <Select aria-label="Newer version" options={options} value={String(to)} onValueChange={(v) => setTo(Number(v))} />
            <Button size="sm" variant="outline" onClick={() => compare(from, to)}>
              Diff
            </Button>
          </div>
          <ErrorNote error={error} />
          {diff && <DiffView diff={diff.diff} title={`v${diff.from} → v${diff.to}`} />}
        </div>
      )}
    </div>
  );
}

/** A unified diff, coloured by line. */
export function DiffView({ diff, title }: { diff: string; title?: string }) {
  if (!diff) return <p className="muted">{title ? `${title}: ` : ''}no differences.</p>;
  const lines = diff.split('\n').filter((l, i, all) => !(i === all.length - 1 && l === ''));
  return (
    <div className="diff" role="region" aria-label={title ?? 'Diff'}>
      {title && <div className="diff-title">{title}</div>}
      {lines.map((l, i) => {
        const kind = l.startsWith('+++') || l.startsWith('---') ? 'file' : l.startsWith('@@') ? 'hunk' : l[0] === '+' ? 'add' : l[0] === '-' ? 'del' : 'same';
        return (
          <div key={i} className="diff-line" data-kind={kind}>
            {l || ' '}
          </div>
        );
      })}
    </div>
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
