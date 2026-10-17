import { api } from '../api';
import { Empty, ErrorNote, PageHeader, Stat } from '../components';
import { money, tokens, totalIn, useLoad } from '../util';

export function UsagePage() {
  const { data, error } = useLoad(() => api.usage(), []);
  const rows = [...(data ?? [])].sort((a, b) => b.cost - a.cost || totalIn(b.usage) - totalIn(a.usage));
  const totals = rows.reduce(
    (t, r) => ({ calls: t.calls + r.calls, input: t.input + totalIn(r.usage), output: t.output + r.usage.output_tokens, cost: t.cost + r.cost }),
    { calls: 0, input: 0, output: 0, cost: 0 },
  );
  const maxTokens = Math.max(1, ...rows.map((r) => totalIn(r.usage) + r.usage.output_tokens));

  return (
    <div className="page">
      <PageHeader title="Usage" subtitle="Tokens and cost across all runs, by model." />
      <ErrorNote error={error} />
      <div className="stats">
        <Stat label="Model calls" value={totals.calls.toLocaleString()} />
        <Stat label="Input tokens" value={tokens(totals.input)} />
        <Stat label="Output tokens" value={tokens(totals.output)} />
        <Stat label="Cost" value={money(totals.cost)} />
      </div>
      <section className="card">
        <h3>By model</h3>
        {rows.length === 0 && <Empty title="No usage yet" />}
        <div className="usage-bars">
          {rows.map((r) => {
            const inW = (totalIn(r.usage) / maxTokens) * 100;
            const outW = (r.usage.output_tokens / maxTokens) * 100;
            return (
              <div key={r.model} className="usage-row">
                <span className="mono small">{r.model}</span>
                <span className="bar-track" title={`${totalIn(r.usage)} in · ${r.usage.output_tokens} out`}>
                  <span className="bar in" style={{ width: `${inW}%` }} />
                  <span className="bar out" style={{ width: `${outW}%` }} />
                </span>
                <span className="small muted num">
                  {r.calls} calls · {tokens(totalIn(r.usage))} / {tokens(r.usage.output_tokens)}
                </span>
                <strong className="num">{money(r.cost)}</strong>
              </div>
            );
          })}
        </div>
        {rows.length > 0 && (
          <div className="row gap legend small muted">
            <span className="swatch in" /> input <span className="swatch out" /> output
          </div>
        )}
      </section>
    </div>
  );
}
