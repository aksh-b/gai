// A sortable table over plain rows, built on shadcn's Table.

import { useMemo, useState, type ReactNode } from 'react';
import { ArrowDownIcon, ArrowUpIcon } from 'lucide-react';

import { Button } from '@/components/ui/button';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table';
import { cn } from '@/lib/utils';

export type CellValue = string | number | Date | null | undefined;

export interface Column<T> {
  key: string;
  header: ReactNode;
  /** What the column sorts by (and shows, when there is no `cell`). */
  value?: (row: T) => CellValue;
  cell?: (row: T) => ReactNode;
  width?: number;
  minWidth?: number;
  align?: 'start' | 'end' | 'center';
  /** Columns with a `value` sort unless this is false. */
  sortable?: boolean;
}

export interface SortKey {
  key: string;
  dir: 'asc' | 'desc';
}

export type DataTableProps<T> = {
  rows: T[];
  columns: Column<T>[];
  getRowId?: (row: T) => string;
  /** Scroll inside this height (px) instead of growing the page. */
  height?: number;
  onRowAction?: (row: T) => void;
  defaultSort?: SortKey[];
  empty?: ReactNode;
  'aria-label'?: string;
};

const cmp = (a: CellValue, b: CellValue) => {
  if (a == null) return b == null ? 0 : 1;
  if (b == null) return -1;
  if (a instanceof Date && b instanceof Date) return a.getTime() - b.getTime();
  if (typeof a === 'number' && typeof b === 'number') return a - b;
  return String(a).localeCompare(String(b), undefined, { numeric: true });
};

const alignClass = { start: 'text-left', end: 'text-right', center: 'text-center' } as const;

export function DataTable<T>({ rows, columns, getRowId, height, onRowAction, defaultSort, empty, ...aria }: DataTableProps<T>) {
  const [sort, setSort] = useState<SortKey | undefined>(defaultSort?.[0]);

  const sorted = useMemo(() => {
    if (!sort) return rows;
    const col = columns.find((c) => c.key === sort.key);
    if (!col?.value) return rows;
    const get = col.value;
    const out = [...rows].sort((a, b) => cmp(get(a), get(b)));
    return sort.dir === 'desc' ? out.reverse() : out;
  }, [rows, columns, sort]);

  const toggle = (key: string) =>
    setSort((s) => (s?.key === key ? { key, dir: s.dir === 'asc' ? 'desc' : 'asc' } : { key, dir: 'asc' }));

  return (
    <div className="overflow-auto rounded-lg border" style={height ? { maxHeight: height } : undefined}>
      <Table aria-label={aria['aria-label']}>
        <TableHeader className="sticky top-0 z-[1] bg-background">
          <TableRow>
            {columns.map((c) => {
              const sortable = c.value && c.sortable !== false;
              const active = sort?.key === c.key;
              return (
                <TableHead
                  key={c.key}
                  className={alignClass[c.align ?? 'start']}
                  style={{ width: c.width, minWidth: c.minWidth }}
                  aria-sort={active ? (sort.dir === 'asc' ? 'ascending' : 'descending') : undefined}
                >
                  {sortable ? (
                    <Button variant="ghost" size="xs" className={cn('-mx-2', c.align === 'end' && 'flex-row-reverse')} onClick={() => toggle(c.key)}>
                      {c.header}
                      {active && (sort.dir === 'asc' ? <ArrowUpIcon data-icon="inline-end" /> : <ArrowDownIcon data-icon="inline-end" />)}
                    </Button>
                  ) : (
                    c.header
                  )}
                </TableHead>
              );
            })}
          </TableRow>
        </TableHeader>
        <TableBody>
          {sorted.length === 0 ? (
            <TableRow>
              <TableCell colSpan={columns.length} className="h-24 text-center text-muted-foreground">
                {empty ?? 'Nothing here yet.'}
              </TableCell>
            </TableRow>
          ) : (
            sorted.map((row, i) => (
              <TableRow
                key={getRowId ? getRowId(row) : i}
                className={onRowAction ? 'cursor-pointer' : undefined}
                tabIndex={onRowAction ? 0 : undefined}
                onClick={onRowAction ? () => onRowAction(row) : undefined}
                onKeyDown={
                  onRowAction
                    ? (e) => {
                        if (e.key === 'Enter') onRowAction(row);
                      }
                    : undefined
                }
              >
                {columns.map((c) => (
                  <TableCell key={c.key} className={cn(alignClass[c.align ?? 'start'], 'max-w-0 truncate')} style={{ width: c.width, minWidth: c.minWidth }}>
                    {c.cell ? c.cell(row) : formatValue(c.value?.(row))}
                  </TableCell>
                ))}
              </TableRow>
            ))
          )}
        </TableBody>
      </Table>
    </div>
  );
}

function formatValue(v: CellValue): ReactNode {
  if (v instanceof Date) return v.toLocaleString();
  return v ?? '';
}
