// A sortable table over plain rows: shadcn's Data Table pattern (TanStack
// Table v9 over the Table component). Pages describe columns with `Column`.

import { useMemo, useState, type ReactNode } from 'react';
import {
  createColumnHelper,
  createSortedRowModel,
  rowSortingFeature,
  sortFn_alphanumeric,
  sortFn_basic,
  sortFn_datetime,
  sortFn_text,
  tableFeatures,
  useTable,
  type ColumnDef,
  type SortingState,
} from '@tanstack/react-table';
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

const features = tableFeatures({
  rowSortingFeature,
  sortedRowModel: createSortedRowModel(),
  sortFns: { alphanumeric: sortFn_alphanumeric, basic: sortFn_basic, datetime: sortFn_datetime, text: sortFn_text },
});

const alignClass = { start: 'text-left', end: 'text-right', center: 'text-center' } as const;

function formatValue(v: CellValue): ReactNode {
  if (v instanceof Date) return v.toLocaleString();
  return v ?? '';
}

export function DataTable<T extends object>({
  rows,
  columns,
  getRowId,
  height,
  onRowAction,
  defaultSort,
  empty,
  ...aria
}: DataTableProps<T>) {
  const [sorting, setSorting] = useState<SortingState>(() => (defaultSort ?? []).map((s) => ({ id: s.key, desc: s.dir === 'desc' })));

  const defs = useMemo((): ColumnDef<typeof features, T>[] => {
    const helper = createColumnHelper<typeof features, T>();
    return columns.map((c) => {
      const meta = { align: c.align ?? 'start', width: c.width, minWidth: c.minWidth };
      if (!c.value) {
        return helper.display({ id: c.key, header: () => c.header, cell: ({ row }) => c.cell?.(row.original), meta }) as ColumnDef<
          typeof features,
          T
        >;
      }
      const get = c.value;
      return helper.accessor((row: T) => get(row), {
        id: c.key,
        header: () => c.header,
        cell: ({ row, getValue }) => (c.cell ? c.cell(row.original) : formatValue(getValue() as CellValue)),
        enableSorting: c.sortable !== false,
        sortFn: 'auto',
        sortUndefined: 'last',
        meta,
      }) as ColumnDef<typeof features, T>;
    });
  }, [columns]);

  const table = useTable({
    features,
    data: rows,
    columns: defs,
    getRowId: getRowId ? (row: T) => getRowId(row) : undefined,
    onSortingChange: setSorting,
    state: { sorting },
  });

  const metaOf = (m: unknown) => (m ?? {}) as { align: 'start' | 'end' | 'center'; width?: number; minWidth?: number };

  return (
    <div className="overflow-auto rounded-lg border" style={height ? { maxHeight: height } : undefined}>
      <Table aria-label={aria['aria-label']} className="min-w-[42rem]">
        <TableHeader className="sticky top-0 bg-background">
          {table.getHeaderGroups().map((group) => (
            <TableRow key={group.id}>
              {group.headers.map((header) => {
                const meta = metaOf(header.column.columnDef.meta);
                const sorted = header.column.getIsSorted();
                return (
                  <TableHead
                    key={header.id}
                    className={alignClass[meta.align]}
                    style={{ width: meta.width, minWidth: meta.minWidth }}
                    aria-sort={sorted ? (sorted === 'asc' ? 'ascending' : 'descending') : undefined}
                  >
                    {header.isPlaceholder ? null : header.column.getCanSort() ? (
                      <Button
                        variant="ghost"
                        size="xs"
                        className={cn('-mx-2', meta.align === 'end' && 'flex-row-reverse')}
                        onClick={() => header.column.toggleSorting(sorted === 'asc')}
                      >
                        <table.FlexRender header={header} />
                        {sorted === 'asc' ? (
                          <ArrowUpIcon data-icon="inline-end" />
                        ) : sorted === 'desc' ? (
                          <ArrowDownIcon data-icon="inline-end" />
                        ) : null}
                      </Button>
                    ) : (
                      <table.FlexRender header={header} />
                    )}
                  </TableHead>
                );
              })}
            </TableRow>
          ))}
        </TableHeader>
        <TableBody>
          {table.getRowModel().rows.length ? (
            table.getRowModel().rows.map((row) => (
              <TableRow
                key={row.id}
                className={onRowAction ? 'cursor-pointer' : undefined}
                tabIndex={onRowAction ? 0 : undefined}
                onClick={onRowAction ? () => onRowAction(row.original) : undefined}
                onKeyDown={
                  onRowAction
                    ? (e) => {
                        if (e.key === 'Enter') onRowAction(row.original);
                      }
                    : undefined
                }
              >
                {row.getAllCells().map((cell) => {
                  const meta = metaOf(cell.column.columnDef.meta);
                  return (
                    <TableCell
                      key={cell.id}
                      className={cn(alignClass[meta.align], 'max-w-0 truncate')}
                      style={{ width: meta.width, minWidth: meta.minWidth }}
                    >
                      <table.FlexRender cell={cell} />
                    </TableCell>
                  );
                })}
              </TableRow>
            ))
          ) : (
            <TableRow>
              <TableCell colSpan={columns.length} className="h-24 text-center text-muted-foreground">
                {empty ?? 'Nothing here yet.'}
              </TableCell>
            </TableRow>
          )}
        </TableBody>
      </Table>
    </div>
  );
}
