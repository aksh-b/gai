import { useEffect } from 'react';
import { Button } from '@flame';
import { api, type Run } from '../api';
import { Empty, ErrorNote, Json, StatusPill } from '../components';
import { IconBack } from '../icons';
import { navigate } from '../router';
import { blocksText, compactJson, duration, money, tokens, totalIn, useLoad } from '../util';
import { lineDiff } from '../Versions';
import { forkOf, turns, type Turn } from './Fork';

const terminal = ['completed', 'failed', 'cancelled', 'budget_exceeded'];

/** Two runs side by side, step by step: an original and its fork. */
export function Compare({ a, b }: { a: string; b: string }) {
  const ra = useLoad(() => api.run(a), [a]);
  const rb = useLoad(() => api.run(b), [b]);
  const sa = useLoad(() => api.steps(a), [a]);
  const sb = useLoad(() => api.steps(b), [b]);

  // Follow runs that are still going.
  const going = [ra.data, rb.data].some((r) => r && !terminal.includes(r.status));
  useEffect(() => {
    if (!going) return;
    const t = setInterval(() => {
      ra.reload();
      rb.reload();
      sa.reload();
      sb.reload();
    }, 1500);
    return () => clearInterval(t);
  }, [going]);

  const left = ra.data;
  const right = rb.data;
  const ta = turns(sa.data ?? []);
  const tb = turns(sb.data ?? []);
  const fork = right ? forkOf(right) : undefined;
  const n = Math.max(ta.at(-1)?.step ?? 0, tb.at(-1)?.step ?? 0);
  const outA = outputText(left);
  const outB = outputText(right);

  return (
    <div className="page">
      <div className="row gap">
        <Button variant="ghost" size="sm" leading={<IconBack />} onClick={() => navigate(`/runs/${a}`)}>
          Original run
        </Button>
        <Button variant="ghost" size="sm" onClick={() => navigate(`/runs/${b}`)}>
          Fork
        </Button>
      </div>
      <ErrorNote error={ra.error ?? rb.error} />
      <header className="page-head">
        <div>
          <h1>Compare runs</h1>
          {fork && fork.run_id === a && (
            <p className="muted">
              Forked at step {fork.from_step}: steps before it were replayed from the original, later steps ran live.
            </p>
          )}
        </div>
      </header>
      {fork && Object.keys(fork.overrides ?? {}).length > 0 && (
        <section className="card">
          <h3>Changes in the fork</h3>
          <ul className="overrides">
            {fork.overrides.model && (
              <li>
                Model <span className="mono">{fork.overrides.model}</span>
              </li>
            )}
            {fork.overrides.agent_version && <li>Agent version v{fork.overrides.agent_version}</li>}
            {fork.overrides.tools && <li>Tools: {fork.overrides.tools.length ? fork.overrides.tools.join(', ') : 'none'}</li>}
            {fork.overrides.system_prompt !== undefined && <li>New system prompt</li>}
            {(fork.overrides.step_edits ?? []).map((e) => (
              <li key={e.step}>Edited the reply at step {e.step}</li>
            ))}
          </ul>
        </section>
      )}

      <div className="compare">
        <RunSummary run={left} title="Original" />
        <RunSummary run={right} title="Fork" />
      </div>

      <section className="card flush">
        <h3 className="pad" style={{ margin: 0 }}>
          Steps
        </h3>
        {n === 0 && <Empty title="No model steps yet" />}
        {Array.from({ length: n }, (_, i) => i + 1).map((step) => {
          const x = ta.find((t) => t.step === step);
          const y = tb.find((t) => t.step === step);
          const same = x && y && turnKey(x) === turnKey(y);
          return (
            <div key={step} className="compare-step">
              <div className="compare-step-head">
                <span className="mono small">step {step}</span>
                {y?.replayed ? (
                  <span className="chip small">replayed</span>
                ) : same ? (
                  <span className="chip small">same</span>
                ) : x && y ? (
                  <span className="chip accent small">different</span>
                ) : null}
              </div>
              <div className="compare">
                <TurnView turn={x} />
                <TurnView turn={y} />
              </div>
            </div>
          );
        })}
      </section>

      <section className="card">
        <h3>Final output</h3>
        {outA === outB ? (
          <p className="muted">{outA ? 'The same in both runs.' : 'Neither run has an output yet.'}</p>
        ) : (
          <div className="diff">
            {lineDiff(outA, outB).map((l, i) => (
              <div key={i} className="diff-line" data-kind={l.op}>
                {l.op === 'add' ? '+ ' : l.op === 'del' ? '- ' : '  '}
                {l.text}
              </div>
            ))}
          </div>
        )}
      </section>
    </div>
  );
}

function outputText(r: Run | undefined): string {
  if (!r) return '';
  if (r.error && r.output == null) return `error: ${r.error}`;
  if (r.output == null) return '';
  return typeof r.output === 'string' ? r.output : JSON.stringify(r.output, null, 2);
}

/** What a turn said and did, to tell whether two turns match. */
function turnKey(t: Turn): string {
  return JSON.stringify([blocksText(t.content), t.tools.map((c) => [c.name, c.input, c.output])]);
}

function RunSummary({ run, title }: { run: Run | undefined; title: string }) {
  if (!run) return <section className="card" />;
  return (
    <section className="card stack tight">
      <div className="row between">
        <h3 style={{ margin: 0 }}>{title}</h3>
        <StatusPill status={run.status} />
      </div>
      <a
        className="mono small"
        href={`#/runs/${run.id}`}
        onClick={(e) => {
          e.preventDefault();
          navigate(`/runs/${run.id}`);
        }}
      >
        {run.id}
      </a>
      <div className="kv">
        <span className="muted">Agent</span>
        <span>
          {run.agent}
          {run.agent_version ? ` v${run.agent_version}` : ''}
        </span>
        <span className="muted">Model</span>
        <span className="mono">{run.metadata.model}</span>
        <span className="muted">Steps</span>
        <span>{run.steps}</span>
        <span className="muted">Tokens</span>
        <span>
          {tokens(totalIn(run.usage))} in / {tokens(run.usage.output_tokens)} out
        </span>
        <span className="muted">Cost</span>
        <span>{money(run.cost)}</span>
        <span className="muted">Duration</span>
        <span>{duration(run.created_at, run.finished_at)}</span>
      </div>
    </section>
  );
}

function TurnView({ turn }: { turn: Turn | undefined }) {
  if (!turn) return <div className="turn muted small">—</div>;
  const text = blocksText(turn.content);
  return (
    <div className="turn" data-failed={turn.failed || undefined}>
      {turn.model && <div className="muted small mono">{turn.model}</div>}
      {text && <div className="turn-text">{text}</div>}
      {turn.tools.map((c) => (
        <div key={c.id} className="turn-tool">
          <span className="mono small">
            ⚙ {c.name} {compactJson(c.input, 80)}
          </span>
          {c.output !== undefined && (
            <span className="small" data-error={c.error || undefined}>
              → {compactJson(c.output, 160)}
            </span>
          )}
        </div>
      ))}
      {!text && !turn.tools.length && (turn.failed ? <span className="small">failed</span> : <Json value={turn.content} />)}
    </div>
  );
}
