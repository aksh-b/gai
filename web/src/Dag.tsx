import { cn } from '@/lib/utils';
import type { GraphNode } from './api';

/** Node box colours by status, from the theme tokens. */
const NODE_TONE: Record<string, string> = {
  pending: 'fill-muted stroke-border',
  running: 'fill-info/10 stroke-info',
  waiting: 'fill-warning/15 stroke-warning',
  completed: 'fill-success/10 stroke-success',
  failed: 'fill-destructive/10 stroke-destructive',
  skipped: 'fill-muted stroke-border [stroke-dasharray:4_3]',
};

/** Node status shown on the graph. */
export type NodeStatus = 'pending' | 'running' | 'completed' | 'failed' | 'skipped' | 'waiting';

const W = 168;
const H = 48;
const GAP_X = 56;
const GAP_Y = 18;
const PAD = 12;

/**
 * A workflow graph, left to right: each node sits one column after the
 * furthest node it depends on. Nodes are coloured by status when given.
 */
export function Dag({
  nodes,
  status = {},
  selected,
  onSelect,
}: {
  nodes: GraphNode[];
  status?: Record<string, NodeStatus>;
  selected?: string;
  onSelect?: (id: string) => void;
}) {
  if (!nodes.length) return <p className="text-sm text-muted-foreground">No steps yet.</p>;
  const layer: Record<string, number> = {};
  const place = (id: string, seen = new Set<string>()): number => {
    if (layer[id] !== undefined) return layer[id];
    if (seen.has(id)) return 0;
    seen.add(id);
    const n = nodes.find((x) => x.id === id);
    const l = n && n.deps.length ? Math.max(...n.deps.map((d) => place(d, seen))) + 1 : 0;
    layer[id] = l;
    return l;
  };
  nodes.forEach((n) => place(n.id));
  const columns: string[][] = [];
  for (const n of nodes) (columns[layer[n.id]] ??= []).push(n.id);
  const pos: Record<string, { x: number; y: number }> = {};
  const tallest = Math.max(1, ...columns.map((c) => c.length));
  const height = PAD * 2 + tallest * H + (tallest - 1) * GAP_Y;
  columns.forEach((col, ci) => {
    const colHeight = col.length * H + (col.length - 1) * GAP_Y;
    const top = (height - colHeight) / 2;
    col.forEach((id, ri) => {
      pos[id] = { x: PAD + ci * (W + GAP_X), y: top + ri * (H + GAP_Y) };
    });
  });
  const width = PAD * 2 + columns.length * W + (columns.length - 1) * GAP_X;

  return (
    <div className="overflow-auto py-1">
      <svg className="block" width={width} height={height} viewBox={`0 0 ${width} ${height}`} role="img" aria-label="Workflow graph">
        <defs>
          <marker id="dag-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
            <path d="M0 0 10 5 0 10z" className="fill-muted-foreground" />
          </marker>
        </defs>
        {nodes.flatMap((n) =>
          n.deps
            .filter((d) => pos[d])
            .map((d) => {
              const a = pos[d];
              const b = pos[n.id];
              const x1 = a.x + W;
              const y1 = a.y + H / 2;
              const x2 = b.x - 2;
              const y2 = b.y + H / 2;
              const mid = (x1 + x2) / 2;
              return (
                <path
                  key={`${d}->${n.id}`}
                  className={cn('fill-none stroke-[1.6]', status[d] === 'completed' ? 'stroke-success' : 'stroke-border')}
                  d={`M${x1} ${y1} C${mid} ${y1} ${mid} ${y2} ${x2} ${y2}`}
                  markerEnd="url(#dag-arrow)"
                />
              );
            }),
        )}
        {nodes.map((n) => {
          const p = pos[n.id];
          const st = status[n.id] ?? 'pending';
          return (
            <g
              key={n.id}
              className={cn('group', st === 'skipped' && 'opacity-50')}
              transform={`translate(${p.x} ${p.y})`}
              onClick={() => onSelect?.(n.id)}
              style={{ cursor: onSelect ? 'pointer' : undefined }}
            >
              <title>{`${n.id} · ${n.map ? 'map over ' : ''}${n.kind} · ${st}`}</title>
              <rect
                width={W}
                height={H}
                rx={10}
                className={cn(
                  'transition-colors group-hover:stroke-primary',
                  NODE_TONE[st] ?? NODE_TONE.pending,
                  selected === n.id ? 'stroke-primary stroke-[2.2]' : 'stroke-[1.4]',
                )}
              />
              <text x={12} y={20} className="fill-foreground font-mono text-[13px] font-semibold">
                {n.id.length > 18 ? `${n.id.slice(0, 17)}…` : n.id}
              </text>
              <text x={12} y={37} className="fill-muted-foreground text-[11px]">
                {n.map ? `map · ${n.kind}` : n.kind}
                {st !== 'pending' ? ` · ${st}` : ''}
              </text>
              {st === 'running' && <circle cx={W - 14} cy={14} r={4} className="animate-pulse fill-info" />}
            </g>
          );
        })}
      </svg>
    </div>
  );
}

/** Node statuses from a run's events. */
export function statusFromEvent(prev: Record<string, NodeStatus>, e: { type: string; [k: string]: unknown }): Record<string, NodeStatus> {
  switch (e.type) {
    case 'node_started':
      return { ...prev, [e.node as string]: 'running' };
    case 'node_finished':
      return { ...prev, [e.node as string]: e.status as NodeStatus };
    case 'approval_required': {
      const node = String(e.id).split(/[[~]/)[0];
      return prev[node] ? { ...prev, [node]: 'waiting' } : prev;
    }
    case 'approval_resolved': {
      const node = String(e.id).split(/[[~]/)[0];
      return prev[node] === 'waiting' ? { ...prev, [node]: 'running' } : prev;
    }
    default:
      return prev;
  }
}
