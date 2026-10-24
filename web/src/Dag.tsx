import { lazy, Suspense } from 'react';

import type { GraphNode } from './api';

/** Node status shown on the graph. */
export type NodeStatus = 'pending' | 'running' | 'completed' | 'failed' | 'skipped' | 'waiting';

export type DagProps = {
  nodes: GraphNode[];
  status?: Record<string, NodeStatus>;
  selected?: string;
  onSelect?: (id: string) => void;
};

const Flow = lazy(() => import('./DagFlow'));

/**
 * A workflow graph, left to right: each node sits one column after the
 * furthest node it depends on. Nodes are coloured by status when given, and
 * the graph can be panned and zoomed.
 */
export function Dag(props: DagProps) {
  if (!props.nodes.length) return <p className="text-sm text-muted-foreground">No steps yet.</p>;
  return (
    <Suspense fallback={<div className="h-36 w-full animate-pulse rounded-md bg-muted/50" />}>
      <Flow {...props} />
    </Suspense>
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
