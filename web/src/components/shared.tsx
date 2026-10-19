// Small pieces every page uses, composed from the shadcn components in ./ui.

import { Fragment, type ReactElement, type ReactNode } from 'react';
import { AlertCircleIcon, ChevronRightIcon, CircleCheckIcon, CircleDashedIcon, CircleXIcon, ClockIcon } from 'lucide-react';

import { Alert, AlertDescription } from '@/components/ui/alert';
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from '@/components/ui/alert-dialog';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import {
  CodeBlock,
  CodeBlockActions,
  CodeBlockContent,
  CodeBlockCopyButton,
  CodeBlockHeader,
  CodeBlockTitle,
} from '@/components/ui/code-block';
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible';
import { Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from '@/components/ui/empty';
import { Item, ItemActions, ItemContent, ItemTitle } from '@/components/ui/item';
import { Spinner } from '@/components/ui/spinner';
import type { RunStatus } from '@/api';
import { statusLabel } from '@/labels';
import { statusTone } from '@/util';
import { cn } from '@/lib/utils';

/** A run's state as a badge: done, working, needs your OK, failed. */
export function StatusPill({ status }: { status: RunStatus }) {
  const tone = statusTone[status];
  const label = statusLabel[status] ?? status.replace('_', ' ');
  return (
    <Badge variant={tone === 'bad' ? 'destructive' : tone === 'ok' ? 'secondary' : 'outline'}>
      {tone === 'busy' ? (
        <Spinner data-icon="inline-start" />
      ) : tone === 'ok' ? (
        <CircleCheckIcon data-icon="inline-start" />
      ) : tone === 'bad' ? (
        <CircleXIcon data-icon="inline-start" />
      ) : status === 'awaiting_approval' ? (
        <ClockIcon data-icon="inline-start" />
      ) : (
        <CircleDashedIcon data-icon="inline-start" />
      )}
      {label}
    </Badge>
  );
}

/** A page's width and spacing. */
export function Page({ children, className, wide }: { children: ReactNode; className?: string; wide?: boolean }) {
  return <div className={cn('flex w-full flex-col gap-6 p-6 md:p-8', !wide && 'max-w-6xl', className)}>{children}</div>;
}

export function PageHeader({ title, subtitle, actions }: { title: ReactNode; subtitle?: ReactNode; actions?: ReactNode }) {
  return (
    <header className="flex flex-wrap items-end justify-between gap-4">
      <div className="flex flex-col gap-1">
        <h1 className="flex flex-wrap items-center gap-2 text-2xl font-semibold tracking-tight">{title}</h1>
        {subtitle && <p className="text-muted-foreground">{subtitle}</p>}
      </div>
      {actions && <div className="flex items-center gap-2">{actions}</div>}
    </header>
  );
}

/** Nothing to show yet, with what to do about it. */
export function EmptyState({
  title,
  children,
  icon,
  action,
  className,
}: {
  title: string;
  children?: ReactNode;
  icon?: ReactNode;
  action?: ReactNode;
  className?: string;
}) {
  return (
    <Empty className={className}>
      <EmptyHeader>
        {icon && <EmptyMedia variant="icon">{icon}</EmptyMedia>}
        <EmptyTitle>{title}</EmptyTitle>
        {children && <EmptyDescription>{children}</EmptyDescription>}
      </EmptyHeader>
      {action && <EmptyContent>{action}</EmptyContent>}
    </Empty>
  );
}

export function ErrorNote({ error }: { error?: string }) {
  if (!error) return null;
  return (
    <Alert variant="destructive">
      <AlertCircleIcon />
      <AlertDescription>{error}</AlertDescription>
    </Alert>
  );
}

/** A button that asks before doing something hard to undo. */
export function ConfirmAction({
  children,
  title,
  description,
  action,
  destructive = false,
  onConfirm,
}: {
  children: ReactElement;
  title: string;
  description: ReactNode;
  action: string;
  destructive?: boolean;
  onConfirm: () => void;
}) {
  return (
    <AlertDialog>
      <AlertDialogTrigger asChild>{children}</AlertDialogTrigger>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{title}</AlertDialogTitle>
          <AlertDialogDescription>{description}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction variant={destructive ? 'destructive' : 'default'} onClick={onConfirm}>
            {action}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

/** A row in a list of things to open: a link, highlighted when selected. */
export function LinkItem({
  href,
  active,
  onClick,
  title,
  action,
  children,
}: {
  href: string;
  active?: boolean;
  onClick?: () => void;
  title: ReactNode;
  action?: ReactNode;
  children?: ReactNode;
}) {
  return (
    <Item asChild variant={active ? 'muted' : 'outline'} size="sm">
      <a href={href} aria-current={active ? 'page' : undefined} onClick={onClick}>
        <ItemContent className="min-w-0">
          <ItemTitle className="w-full">{title}</ItemTitle>
          {children}
        </ItemContent>
        {action && <ItemActions>{action}</ItemActions>}
      </a>
    </Item>
  );
}

/** Label and value pairs, for a summary of facts. */
export function Details({ items }: { items: [ReactNode, ReactNode][] }) {
  return (
    <dl className="grid grid-cols-[max-content_minmax(0,1fr)] gap-x-4 gap-y-1 text-sm">
      {items.map(([k, v], i) => (
        <Fragment key={i}>
          <dt className="text-muted-foreground">{k}</dt>
          <dd>{v}</dd>
        </Fragment>
      ))}
    </dl>
  );
}

/** One number with its label, for rows of totals. */
export function Stat({ label, value }: { label: string; value: ReactNode }) {
  return (
    <Card size="sm">
      <CardHeader>
        <CardDescription>{label}</CardDescription>
        <CardTitle className="truncate text-xl tabular-nums">{value}</CardTitle>
      </CardHeader>
    </Card>
  );
}

/** A collapsible block, closed by default. */
export function Disclosure({ summary, children, open = false }: { summary: ReactNode; children: ReactNode; open?: boolean }) {
  return (
    <Collapsible defaultOpen={open} className="flex flex-col gap-2">
      <CollapsibleTrigger asChild>
        <Button variant="ghost" size="sm" className="group/disclosure w-fit">
          <ChevronRightIcon data-icon="inline-start" className="transition-transform group-data-[state=open]/disclosure:rotate-90" />
          {summary}
        </Button>
      </CollapsibleTrigger>
      <CollapsibleContent className="pl-6">{children}</CollapsibleContent>
    </Collapsible>
  );
}

export function Json({ value, className }: { value: unknown; className?: string }) {
  return (
    <CodeBlock size="sm">
      <CodeBlockContent wrap className={cn('max-h-90', className)}>
        {typeof value === 'string' ? value : JSON.stringify(value, null, 2)}
      </CodeBlockContent>
    </CodeBlock>
  );
}

/** A command or config to copy, with its title. */
export function Snippet({ title, code }: { title?: string; code: string }) {
  return (
    <CodeBlock size="sm">
      <CodeBlockHeader>
        {title && <CodeBlockTitle>{title}</CodeBlockTitle>}
        <CodeBlockActions>
          <CodeBlockCopyButton value={code} aria-label={title ? `Copy: ${title}` : 'Copy'} />
        </CodeBlockActions>
      </CodeBlockHeader>
      <CodeBlockContent>{code}</CodeBlockContent>
    </CodeBlock>
  );
}

/**
 * Small Markdown renderer for model output: fenced code, headings, lists,
 * bold, italics, inline code and links. Anything else is plain text.
 */
export function Markdown({ text }: { text: string }) {
  const blocks: ReactNode[] = [];
  const lines = text.split('\n');
  let i = 0;
  let key = 0;
  while (i < lines.length) {
    const line = lines[i];
    const fence = line.match(/^```(\S*)/);
    if (fence) {
      const body: string[] = [];
      i++;
      while (i < lines.length && !lines[i].startsWith('```')) body.push(lines[i++]);
      i++;
      blocks.push(
        <CodeBlock key={key++} size="sm">
          <CodeBlockContent wrap>{body.join('\n')}</CodeBlockContent>
        </CodeBlock>,
      );
      continue;
    }
    const heading = line.match(/^(#{1,4})\s+(.*)/);
    if (heading) {
      blocks.push(
        <h3 key={key++} className="mt-2 font-semibold">
          {inline(heading[2])}
        </h3>,
      );
      i++;
      continue;
    }
    if (/^\s*([-*]|\d+\.)\s+/.test(line)) {
      const ordered = /^\s*\d+\./.test(line);
      const items: string[] = [];
      while (i < lines.length && /^\s*([-*]|\d+\.)\s+/.test(lines[i])) {
        items.push(lines[i].replace(/^\s*([-*]|\d+\.)\s+/, ''));
        i++;
      }
      const L = ordered ? 'ol' : 'ul';
      blocks.push(
        <L key={key++} className={cn('flex flex-col gap-1 pl-6', ordered ? 'list-decimal' : 'list-disc')}>
          {items.map((it, j) => (
            <li key={j}>{inline(it)}</li>
          ))}
        </L>,
      );
      continue;
    }
    if (!line.trim()) {
      i++;
      continue;
    }
    const para: string[] = [];
    while (i < lines.length && lines[i].trim() && !/^```|^#{1,4}\s|^\s*([-*]|\d+\.)\s+/.test(lines[i])) para.push(lines[i++]);
    blocks.push(
      <p key={key++}>
        {para.map((p, j) => (
          <Fragment key={j}>
            {j > 0 && <br />}
            {inline(p)}
          </Fragment>
        ))}
      </p>,
    );
  }
  return <div className="flex flex-col gap-3 leading-relaxed">{blocks}</div>;
}

function inline(text: string): ReactNode[] {
  const out: ReactNode[] = [];
  const re = /(`[^`]+`|\*\*[^*]+\*\*|\*[^*\s][^*]*\*|\[[^\]]+\]\([^)\s]+\))/g;
  let last = 0;
  let m: RegExpExecArray | null;
  let k = 0;
  while ((m = re.exec(text))) {
    if (m.index > last) out.push(text.slice(last, m.index));
    const t = m[0];
    if (t.startsWith('`'))
      out.push(
        <code key={k++} className="rounded bg-muted px-1 py-0.5 font-mono text-[0.9em]">
          {t.slice(1, -1)}
        </code>,
      );
    else if (t.startsWith('**')) out.push(<strong key={k++}>{t.slice(2, -2)}</strong>);
    else if (t.startsWith('[')) {
      const [, label, href] = t.match(/\[([^\]]+)\]\(([^)]+)\)/)!;
      const safe = /^https?:\/\//.test(href) ? href : undefined;
      out.push(
        <a key={k++} href={safe} target="_blank" rel="noreferrer" className="font-medium underline underline-offset-4">
          {label}
        </a>,
      );
    } else out.push(<em key={k++}>{t.slice(1, -1)}</em>);
    last = m.index + t.length;
  }
  if (last < text.length) out.push(text.slice(last));
  return out;
}
