// The workflow graph drawn with xyflow: pan, zoom and fit, laid out left to
// right by dependency depth. Loaded on demand from Dag.tsx.

import { useMemo } from 'react';
import {
  Background,
  Controls,
  Handle,
  MarkerType,
  Position,
  ReactFlow,
  type Edge,
  type Node,
  type NodeProps,
} from '@xyflow/react';
import '@xyflow/react/dist/style.css';

import { cn } from '@/lib/utils';
import type { GraphNode } from './api';
import type { DagProps, NodeStatus } from './Dag';
import { isDark, useTheme } from './prefs';

/** Node box colours by status, from the theme tokens. */
const NODE_TONE: Record<string, string> = {
  pending: 'bg-card border-border',
  running: 'bg-info/10 border-info',
  waiting: 'bg-warning/15 border-warning',
  completed: 'bg-success/10 border-success',
  failed: 'bg-destructive/10 border-destructive',
  skipped: 'bg-muted border-border border-dashed opacity-60',
};

export const W = 176;
export const H = 52;
const GAP_X = 64;
const GAP_Y = 20;

type StepData = { node: GraphNode; status: NodeStatus; selected: boolean; clickable: boolean };

function Step({ data }: NodeProps<Node<StepData>>) {
  const { node: n, status: st, selected, clickable } = data;
  return (
    <div
      className={cn(
        'flex h-full w-full flex-col justify-center rounded-lg border-[1.5px] px-3 transition-colors',
        NODE_TONE[st] ?? NODE_TONE.pending,
        selected && 'ring-2 ring-primary',
        clickable && 'cursor-pointer hover:border-primary',
      )}
      title={`${n.id} · ${n.map ? 'map over ' : ''}${n.kind} · ${st}`}
    >
      <Handle type="target" position={Position.Left} className="!size-1.5 !border-0 !bg-muted-foreground" isConnectable={false} />
      <div className="flex items-center gap-1.5">
        <span className="truncate font-mono text-[13px] font-semibold text-foreground">{n.id}</span>
        {st === 'running' && <span className="ml-auto size-2 shrink-0 animate-pulse rounded-full bg-info" />}
      </div>
      <span className="truncate text-[11px] text-muted-foreground">
        {n.map ? `map · ${n.kind}` : n.kind}
        {st !== 'pending' ? ` · ${st}` : ''}
      </span>
      <Handle type="source" position={Position.Right} className="!size-1.5 !border-0 !bg-muted-foreground" isConnectable={false} />
    </div>
  );
}

const nodeTypes = { step: Step };

/** Each node sits one column after the furthest node it depends on. */
export function layout(nodes: GraphNode[]): Record<string, { x: number; y: number }> {
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
  const tallest = Math.max(1, ...columns.map((c) => c.length));
  const height = tallest * H + (tallest - 1) * GAP_Y;
  const pos: Record<string, { x: number; y: number }> = {};
  columns.forEach((col, ci) => {
    const top = (height - (col.length * H + (col.length - 1) * GAP_Y)) / 2;
    col.forEach((id, ri) => {
      pos[id] = { x: ci * (W + GAP_X), y: top + ri * (H + GAP_Y) };
    });
  });
  return pos;
}

export default function DagFlow({ nodes, status = {}, selected, onSelect }: DagProps) {
  const [theme] = useTheme();
  const { flowNodes, edges, height } = useMemo(() => {
    const pos = layout(nodes);
    const flowNodes: Node<StepData>[] = nodes.map((n) => ({
      id: n.id,
      type: 'step',
      position: pos[n.id],
      width: W,
      height: H,
      draggable: false,
      connectable: false,
      data: { node: n, status: status[n.id] ?? 'pending', selected: selected === n.id, clickable: !!onSelect },
    }));
    const edges: Edge[] = nodes.flatMap((n) =>
      n.deps
        .filter((d) => pos[d])
        .map((d) => {
          const done = status[d] === 'completed';
          return {
            id: `${d}->${n.id}`,
            source: d,
            target: n.id,
            animated: status[n.id] === 'running',
            markerEnd: { type: MarkerType.ArrowClosed, width: 16, height: 16, color: done ? 'var(--success)' : 'var(--muted-foreground)' },
            style: { stroke: done ? 'var(--success)' : 'var(--border)', strokeWidth: 1.6 },
          };
        }),
    );
    const bottom = Math.max(0, ...Object.values(pos).map((p) => p.y + H));
    return { flowNodes, edges, height: Math.min(Math.max(bottom + 48, 140), 480) };
  }, [nodes, status, selected, onSelect]);

  return (
    <div className="w-full overflow-hidden rounded-md" style={{ height }} role="img" aria-label="Workflow graph">
      <ReactFlow
        nodes={flowNodes}
        edges={edges}
        nodeTypes={nodeTypes}
        colorMode={isDark(theme) ? 'dark' : 'light'}
        fitView
        fitViewOptions={{ padding: 0.15, maxZoom: 1 }}
        minZoom={0.25}
        maxZoom={1.75}
        nodesDraggable={false}
        nodesConnectable={false}
        elementsSelectable={!!onSelect}
        onNodeClick={(_, n) => onSelect?.(n.id)}
        proOptions={{ hideAttribution: true }}
        style={{ background: 'transparent' }}
      >
        <Background gap={16} size={1} color="var(--border)" />
        <Controls showInteractive={false} position="bottom-right" />
      </ReactFlow>
    </div>
  );
}
