import { useCallback } from 'react';
import { QueryClient, keepPreviousData, useQuery } from '@tanstack/react-query';
import type { ContentBlock, RunStatus, Usage } from './api';

/** The dashboard's query cache. */
export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      // Pages show errors as they happen and poll where they need to.
      retry: false,
      refetchOnWindowFocus: false,
    },
  },
});

/**
 * Load data (and again on `reload`, or when `deps` change), tracking
 * loading and errors. Results are cached with TanStack Query by the loader
 * and its deps, so a page opened again shows its last data at once while
 * it refreshes, and two parts of a page asking the same thing share one
 * request.
 */
export function useLoad<T>(load: () => Promise<T>, deps: unknown[] = []) {
  const queryKey = ['load', load.toString(), ...deps];
  const q = useQuery({ queryKey, queryFn: load, placeholderData: keepPreviousData });
  const { refetch } = q;
  const reload = useCallback(() => void refetch(), [refetch]);
  const key = JSON.stringify(queryKey);
  const setData = useCallback(
    (d: T | undefined | ((old: T | undefined) => T | undefined)) => queryClient.setQueryData<T>(JSON.parse(key), d as never),
    [key],
  );
  return {
    data: q.data,
    error: q.error ? (q.error as Error).message : undefined,
    loading: q.isFetching,
    reload,
    setData,
  };
}

/** Load everything again (after a change that touches many pages). */
export function reloadAll() {
  void queryClient.invalidateQueries();
}

export function ago(iso: string | null | undefined): string {
  if (!iso) return '';
  const s = Math.max(0, (Date.now() - new Date(iso).getTime()) / 1000);
  if (s < 60) return `${Math.floor(s)}s ago`;
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86400)}d ago`;
}

export function duration(from: string, to: string | null | undefined): string {
  const ms = (to ? new Date(to).getTime() : Date.now()) - new Date(from).getTime();
  if (ms < 1000) return `${ms} ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)} s`;
  return `${Math.floor(ms / 60_000)}m ${Math.round((ms % 60_000) / 1000)}s`;
}

export const tokens = (n: number) => (n >= 10_000 ? `${(n / 1000).toFixed(1)}k` : n.toLocaleString());
export const totalIn = (u: Usage) => u.input_tokens + u.cache_read_tokens + u.cache_write_tokens;
export const money = (usd: number) => (usd === 0 ? '$0' : usd < 0.01 ? `$${usd.toFixed(4)}` : `$${usd.toFixed(2)}`);

export function blocksText(blocks: ContentBlock[] | Record<string, unknown> | undefined): string {
  if (!Array.isArray(blocks)) return '';
  return blocks
    .map((b) => (b.type === 'text' ? b.text : ''))
    .join('');
}

export const statusTone: Record<RunStatus, 'ok' | 'warn' | 'bad' | 'busy'> = {
  completed: 'ok',
  running: 'busy',
  queued: 'busy',
  awaiting_approval: 'warn',
  failed: 'bad',
  cancelled: 'warn',
  budget_exceeded: 'bad',
};

export function compactJson(v: unknown, max = 120): string {
  const s = typeof v === 'string' ? v : JSON.stringify(v);
  return s.length > max ? `${s.slice(0, max - 1)}…` : s;
}
