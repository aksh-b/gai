// The workflow designer: steps on an xyflow canvas. Drag steps to arrange
// them, drag from one step to another to make it wait, select a line or a
// step and press Delete to remove it, click a step to edit it. Everything
// is written back to the spec (positions under `layout`, waits under
// `needs`), so it round-trips with the YAML. Loaded on demand.

import { useEffect, useMemo, useState } from 'react';
import {
  applyEdgeChanges,
  applyNodeChanges,
  Background,
  Controls,
  Handle,
  MarkerType,
  Position,
  ReactFlow,
  type Connection,
  type Edge,
  type EdgeChange,
  type Node as FlowNode,
  type NodeChange,
  type NodeProps,
} from '@xyflow/react';
import '@xyflow/react/dist/style.css';
import { toast } from 'sonner';

import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from '@/components/ui/card';
import { cn } from '@/lib/utils';
import type { GraphNode } from './api';
import { H, layout, W } from './DagFlow';
import { IconPlus } from './icons';
import { isDark, useDevMode, useTheme } from './prefs';
import { KINDS, newStep, nodeKind, refsFor, Step, useStepChoices, type Node, type Spec } from './WorkflowForm';

const REFS = /nodes\s*(?:\.\s*([A-Za-z_][A-Za-z0-9_]*)|\[\s*["']([^"']+)["']\s*\])/g;

/** Every template string in a step, as the server finds references. */
function templates(n: Node): string[] {
  const out: string[] = [];
  const walk = (v: unknown) => {
    if (typeof v === 'string') out.push(v);
    else if (Array.isArray(v)) v.forEach(walk);
    else if (v && typeof v === 'object') Object.values(v).forEach(walk);
  };
  [n.if, n.input, n.template, n.human, n.map].forEach((t) => typeof t === 'string' && out.push(t));
  walk(n.args);
  walk(n.value);
  return out;
}

/** The steps a step waits for: named in `needs`, or used in its text. */
export function dependencies(n: Node, ids: Set<string>): { needs: string[]; uses: string[] } {
  const needs = ((n.needs as string[] | undefined) ?? []).filter((d) => ids.has(d));
  const uses = new Set<string>();
  for (const t of templates(n))
    for (const m of t.matchAll(REFS)) {
      const r = m[1] ?? m[2];
      if (ids.has(r) && r !== n.id) uses.add(r);
    }
  return { needs, uses: [...uses].filter((u) => !needs.includes(u)) };
}

const KIND_LABEL: Record<string, string> = {
  agent: 'Assistant',
  tool: 'Tool',
  human: 'Ask a person',
  template: 'Text',
  value: 'Data',
};

type StepData = { node: Node; selected: boolean };

function StepBox({ data }: NodeProps<FlowNode<StepData>>) {
  const { node: n, selected } = data;
  const kind = nodeKind(n);
  const what =
    kind === 'agent' ? (typeof n.agent === 'string' ? n.agent : 'one-off helper') : kind === 'tool' ? n.tool : undefined;
  return (
    <div
      className={cn(
        'flex h-full w-full cursor-pointer flex-col justify-center rounded-lg border-[1.5px] bg-card px-3 transition-colors hover:border-primary',
        selected ? 'border-primary ring-2 ring-primary/40' : 'border-border',
      )}
    >
      <Handle type="target" position={Position.Left} className="!size-2.5 !border-2 !border-background !bg-muted-foreground" />
      <span className="truncate font-mono text-[13px] font-semibold text-foreground">{n.id}</span>
      <span className="truncate text-[11px] text-muted-foreground">
        {n.map ? 'for each · ' : ''}
        {KIND_LABEL[kind ?? ''] ?? 'step'}
        {what ? ` · ${what}` : ''}
      </span>
      <Handle type="source" position={Position.Right} className="!size-2.5 !border-2 !border-background !bg-primary" />
    </div>
  );
}

const nodeTypes = { step: StepBox };
const GAP_X = 64;

export default function WorkflowCanvas({ spec, onChange }: { spec: Spec; onChange: (s: Spec) => void }) {
  const [theme] = useTheme();
  const [dev] = useDevMode();
  const choices = useStepChoices();
  const steps = useMemo(() => spec.nodes ?? [], [spec.nodes]);
  const [selected, setSelected] = useState<string | undefined>(steps[0]?.id);
  const saved = (spec.layout ?? {}) as Record<string, [number, number]>;

  const ids = useMemo(() => new Set(steps.map((n) => n.id as string)), [steps]);
  const deps = useMemo(
    () => Object.fromEntries(steps.map((n) => [n.id as string, dependencies(n, ids)])) as Record<string, ReturnType<typeof dependencies>>,
    [steps, ids],
  );

  // Steps without a saved place go where their dependencies put them.
  const auto = useMemo(
    () =>
      layout(
        steps.map((n) => ({ id: n.id, kind: nodeKind(n) ?? 'agent', map: !!n.map, deps: [...deps[n.id].needs, ...deps[n.id].uses] }) as GraphNode),
      ),
    [steps, deps],
  );
  const [nodes, setNodes] = useState<FlowNode<StepData>[]>([]);
  useEffect(() => {
    setNodes(
      steps.map((n) => {
        const p = saved[n.id];
        return {
          id: n.id,
          type: 'step',
          position: p ? { x: p[0], y: p[1] } : (auto[n.id] ?? { x: 0, y: 0 }),
          width: W,
          height: H,
          data: { node: n, selected: selected === n.id },
        };
      }),
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [steps, auto, selected, spec.layout]);

  // Kept as state so xyflow can mark a line selected (for Delete).
  const [edges, setEdges] = useState<Edge[]>([]);
  useEffect(
    () =>
      setEdges(
      steps.flatMap((n) => [
        ...deps[n.id].needs.map((d) => ({
          id: `${d}->${n.id}`,
          source: d,
          target: n.id,
          markerEnd: { type: MarkerType.ArrowClosed, width: 16, height: 16, color: 'var(--primary)' },
          style: { stroke: 'var(--primary)', strokeWidth: 1.8 },
        })),
        // Waiting because the step uses the result: removed by editing the step's text.
        ...deps[n.id].uses.map((d) => ({
          id: `${d}~>${n.id}`,
          source: d,
          target: n.id,
          deletable: false,
          markerEnd: { type: MarkerType.ArrowClosed, width: 16, height: 16, color: 'var(--muted-foreground)' },
          style: { stroke: 'var(--muted-foreground)', strokeWidth: 1.6, strokeDasharray: '5 4' },
        })),
      ]),
      ),
    [steps, deps],
  );

  const setSteps = (next: Node[], extra: Partial<Spec> = {}) => onChange({ ...spec, nodes: next, ...extra });

  /** Whether `from` already waits, directly or not, for `on`. */
  const waitsFor = (from: string, on: string, seen = new Set<string>()): boolean => {
    if (from === on) return true;
    if (seen.has(from)) return false;
    seen.add(from);
    const d = deps[from];
    return !!d && [...d.needs, ...d.uses].some((x) => waitsFor(x, on, seen));
  };

  function connect(c: Connection) {
    const { source, target } = c;
    if (!source || !target || source === target) return;
    if (waitsFor(target, source)) return;
    if (waitsFor(source, target)) {
      toast.error(`${source} already waits for ${target}, so ${target} can't wait for it.`);
      return;
    }
    setSteps(steps.map((n) => (n.id === target ? { ...n, needs: [...((n.needs as string[]) ?? []), source] } : n)));
  }

  function removeEdges(gone: Edge[]) {
    let next = steps;
    for (const e of gone)
      next = next.map((n) => {
        if (n.id !== e.target) return n;
        const needs = ((n.needs as string[]) ?? []).filter((d) => d !== e.source);
        const { needs: _old, ...rest } = n;
        void _old;
        return needs.length ? { ...rest, needs } : rest;
      });
    setSteps(next);
  }

  function removeSteps(gone: string[]) {
    const layoutLeft = Object.fromEntries(Object.entries(saved).filter(([k]) => !gone.includes(k)));
    const next = steps
      .filter((n) => !gone.includes(n.id))
      .map((n) => {
        const needs = ((n.needs as string[]) ?? []).filter((d) => !gone.includes(d));
        const { needs: _old, ...rest } = n;
        void _old;
        return needs.length ? { ...rest, needs } : rest;
      });
    if (gone.includes(selected ?? '')) setSelected(undefined);
    setSteps(next, { layout: Object.keys(layoutLeft).length ? layoutLeft : undefined });
  }

  /** Keep every step where it is now, once one has been moved. */
  function placeAll(moved: FlowNode[]) {
    const at = Object.fromEntries(nodes.map((n) => [n.id, [Math.round(n.position.x), Math.round(n.position.y)]]));
    for (const m of moved) at[m.id] = [Math.round(m.position.x), Math.round(m.position.y)];
    onChange({ ...spec, layout: at });
  }

  function add(kind: string) {
    const step = newStep(kind, spec, choices.agents[0]?.name);
    let extra: Partial<Spec> = {};
    if (Object.keys(saved).length) {
      const right = Math.max(0, ...nodes.map((n) => n.position.x));
      const last = nodes[nodes.length - 1]?.position.y ?? 0;
      extra = { layout: { ...saved, [step.id]: [right + W + GAP_X, last] } };
    }
    setSteps([...steps, step], extra);
    setSelected(step.id);
  }

  function editStep(i: number, n: Node | null) {
    const old = steps[i];
    if (!n) return removeSteps([old.id]);
    if (n.id === old.id) return setSteps(steps.map((x, j) => (j === i ? n : x)));
    // Renamed: keep its place and the steps that wait for it.
    const next = steps.map((x, j) =>
      j === i ? n : x.needs ? { ...x, needs: (x.needs as string[]).map((d) => (d === old.id ? n.id : d)) } : x,
    );
    const lay = { ...saved };
    if (lay[old.id]) {
      lay[n.id] = lay[old.id];
      delete lay[old.id];
    }
    setSelected(n.id);
    setSteps(next, Object.keys(lay).length ? { layout: lay } : {});
  }

  const index = steps.findIndex((n) => n.id === selected);
  const current = index >= 0 ? steps[index] : undefined;
  const editable = current && nodeKind(current) !== null && nodeKind(current) !== 'value';
  const bottom = Math.max(0, ...nodes.map((n) => n.position.y + H));

  return (
    <Card>
      <CardHeader>
        <CardTitle>Steps</CardTitle>
        <CardDescription>
          Drag from a step's right edge to another step to make it wait for the first. A dashed line means the step uses that result in
          its text. Select a step or a solid line and press Delete to remove it; click a step to edit it.
        </CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        <div
          className="w-full overflow-hidden rounded-md border"
          style={{ height: Math.min(Math.max(bottom + 120, 320), 560) }}
          aria-label="Workflow designer"
        >
          <ReactFlow
            nodes={nodes}
            edges={edges}
            onEdgesChange={(ch: EdgeChange[]) => setEdges((es) => applyEdgeChanges(ch, es))}
            nodeTypes={nodeTypes}
            colorMode={isDark(theme) ? 'dark' : 'light'}
            fitView
            fitViewOptions={{ padding: 0.2, maxZoom: 1 }}
            minZoom={0.25}
            maxZoom={1.75}
            onNodesChange={(ch: NodeChange<FlowNode<StepData>>[]) => setNodes((ns) => applyNodeChanges(ch, ns))}
            onNodeDragStop={(_, n, moved) => placeAll(moved.length ? moved : [n])}
            onConnect={connect}
            onEdgesDelete={removeEdges}
            onNodesDelete={(gone) => removeSteps(gone.map((n) => n.id))}
            onNodeClick={(_, n) => setSelected(n.id)}
            onPaneClick={() => setSelected(undefined)}
            deleteKeyCode={['Backspace', 'Delete']}
            proOptions={{ hideAttribution: true }}
            style={{ background: 'transparent' }}
          >
            <Background gap={16} size={1} color="var(--border)" />
            <Controls showInteractive={false} position="bottom-right" />
          </ReactFlow>
        </div>
        {current &&
          (editable ? (
            <Step
              index={index}
              node={current}
              count={steps.length}
              agents={choices.agents}
              tools={choices.tools}
              refs={refsFor(
                spec,
                steps.filter((n) => n.id !== current.id && !waitsFor(n.id, current.id)),
              )}
              dev={dev}
              onChange={(n) => editStep(index, n)}
            />
          ) : (
            <p className="text-sm text-muted-foreground">Step {current.id} is edited in YAML.</p>
          ))}
      </CardContent>
      <CardFooter className="flex-wrap gap-2">
        <span className="text-sm text-muted-foreground">Add a step:</span>
        {KINDS.map((k) => (
          <Button variant="outline" size="sm" key={k.value} onClick={() => add(k.value)}>
            <IconPlus data-icon="inline-start" />
            {k.label}
          </Button>
        ))}
      </CardFooter>
    </Card>
  );
}
