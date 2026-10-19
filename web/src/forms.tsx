import { useEffect, useId, useMemo, useRef, useState, type ReactNode } from 'react';
import { TabBar } from '@/components/fields';
import { Button } from '@/components/ui/button';
import { Card, CardAction, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Checkbox } from '@/components/ui/checkbox';
import { Field, FieldDescription, FieldError, FieldGroup, FieldLabel } from '@/components/ui/field';
import { Input } from '@/components/ui/input';
import { Item, ItemActions, ItemContent, ItemDescription, ItemGroup, ItemMedia, ItemTitle } from '@/components/ui/item';
import { ScrollArea } from '@/components/ui/scroll-area';
import { Textarea } from '@/components/ui/textarea';
import { cn } from '@/lib/utils';
import YAML from 'yaml';
import { ErrorNote } from '@/components/shared';
import { useDevMode } from './prefs';

/** Empty values are left out, so specs (and their YAML) stay short. */
function isEmpty(v: unknown): boolean {
  return (
    v === undefined ||
    v === null ||
    v === '' ||
    (Array.isArray(v) && v.length === 0) ||
    (typeof v === 'object' && !Array.isArray(v) && Object.keys(v as object).length === 0)
  );
}

/** A copy of `obj` with `path` set to `value` (or removed when empty). */
export function setIn<T extends object>(obj: T, path: (string | number)[], value: unknown): T {
  const [head, ...rest] = path;
  const copy: Record<string | number, unknown> = Array.isArray(obj) ? ([...obj] as never) : { ...(obj as object) };
  const next = rest.length ? setIn(((copy[head] as object) ?? (typeof rest[0] === 'number' ? [] : {})) as object, rest, value) : value;
  if (isEmpty(next) && !Array.isArray(obj)) delete copy[head];
  else copy[head] = next;
  return copy as T;
}

export function getIn(obj: unknown, path: (string | number)[]): unknown {
  return path.reduce<unknown>((o, k) => (o == null ? undefined : (o as Record<string | number, unknown>)[k]), obj);
}

/**
 * A spec edited as a form, with the same spec as YAML one tab over. Edits
 * on either side show on the other; YAML that does not parse keeps the last
 * good spec and says why.
 */
export function SpecEditor<T extends object>({
  value,
  onChange,
  children,
  label,
}: {
  value: T;
  onChange: (v: T) => void;
  children: ReactNode;
  label: string;
}) {
  const [dev] = useDevMode();
  const [tab, setTab] = useState('form');
  const [text, setText] = useState(() => YAML.stringify(value));
  const [error, setError] = useState<string>();
  const own = useRef(false);

  // Form edits (or a new spec) refresh the YAML, unless they came from it.
  useEffect(() => {
    if (own.current) {
      own.current = false;
      return;
    }
    setText(YAML.stringify(value));
    setError(undefined);
  }, [value]);

  // The YAML view is a developer detail.
  if (!dev && tab === 'form') {
    return <div className="flex flex-col gap-4">{children}</div>;
  }

  return (
    <div className="flex flex-col gap-4">
      <TabBar
        items={[
          { id: 'form', label: 'Form' },
          { id: 'yaml', label: 'YAML' },
        ]}
        value={tab}
        onValueChange={(t) => {
          if (t === 'form' && error) return;
          setTab(t);
        }}
        variant="underline"
        aria-label={`${label} editor`}
      />
      {tab === 'form' ? (
        <div className="flex flex-col gap-4">{children}</div>
      ) : (
        <>
          <Textarea
            className="min-h-96 font-mono"
            spellCheck={false}
            value={text}
            aria-label={`${label} (YAML)`}
            onChange={(e) => {
              setText(e.target.value);
              try {
                const v = YAML.parse(e.target.value);
                if (!v || typeof v !== 'object') throw new Error('The spec must be a YAML mapping.');
                setError(undefined);
                own.current = true;
                onChange(v as T);
              } catch (x) {
                setError((x as Error).message);
              }
            }}
          />
          <ErrorNote error={error && `Fix the YAML to go back to the form: ${error}`} />
        </>
      )}
    </div>
  );
}

/** A titled group of fields. */
export function Section({ title, hint, children, aside }: { title: string; hint?: ReactNode; children: ReactNode; aside?: ReactNode }) {
  return (
    <Card>
      <CardHeader>
        <CardTitle>{title}</CardTitle>
        {hint && <CardDescription>{hint}</CardDescription>}
        {aside && <CardAction>{aside}</CardAction>}
      </CardHeader>
      <CardContent>
        <FieldGroup className="grid gap-4 sm:grid-cols-2">{children}</FieldGroup>
      </CardContent>
    </Card>
  );
}

export function TextArea({
  label,
  description,
  error,
  value,
  onChange,
  rows = 4,
  mono = false,
  placeholder,
  wide = true,
  textareaRef,
}: {
  label: ReactNode;
  description?: ReactNode;
  error?: ReactNode;
  value: string;
  onChange: (v: string) => void;
  rows?: number;
  mono?: boolean;
  placeholder?: string;
  wide?: boolean;
  textareaRef?: React.Ref<HTMLTextAreaElement>;
}) {
  const id = useId();
  return (
    <Field data-invalid={error ? true : undefined} className={cn(wide && 'col-span-full')}>
      {label && <FieldLabel htmlFor={id}>{label}</FieldLabel>}
      <Textarea
        id={id}
        ref={textareaRef}
        className={mono ? 'font-mono' : undefined}
        rows={rows}
        value={value}
        placeholder={placeholder}
        aria-invalid={error ? true : undefined}
        onChange={(e) => onChange(e.target.value)}
      />
      {description && <FieldDescription>{description}</FieldDescription>}
      {error && <FieldError>{error}</FieldError>}
    </Field>
  );
}

/** A number input that maps empty to `undefined`. */
export function NumberInput({
  label,
  description,
  value,
  onChange,
  step,
  min,
  placeholder,
}: {
  label: ReactNode;
  description?: ReactNode;
  value: number | undefined;
  onChange: (v: number | undefined) => void;
  step?: number;
  min?: number;
  placeholder?: string;
}) {
  const id = useId();
  return (
    <Field>
      {label && <FieldLabel htmlFor={id}>{label}</FieldLabel>}
      <Input
        id={id}
        type="number"
        step={step}
        min={min}
        placeholder={placeholder}
        value={value ?? ''}
        onChange={(e) => onChange(e.target.value === '' ? undefined : Number(e.target.value))}
      />
      {description && <FieldDescription>{description}</FieldDescription>}
    </Field>
  );
}

export interface PickItem {
  value: string;
  label?: string;
  description?: string;
  group?: string;
}

/**
 * A searchable checklist. `extra` renders more controls for a chosen item
 * (e.g. a tool's policy).
 */
export function PickList({
  items,
  selected,
  onChange,
  empty = 'Nothing to choose from.',
  extra,
  maxHeight = 320,
}: {
  items: PickItem[];
  selected: string[];
  onChange: (v: string[]) => void;
  empty?: string;
  extra?: (value: string) => ReactNode;
  maxHeight?: number;
}) {
  const [q, setQ] = useState('');
  const shown = useMemo(() => {
    const needle = q.trim().toLowerCase();
    return items.filter(
      (i) =>
        !needle ||
        i.value.toLowerCase().includes(needle) ||
        (i.label ?? '').toLowerCase().includes(needle) ||
        (i.description ?? '').toLowerCase().includes(needle),
    );
  }, [items, q]);
  // Chosen values the list does not know (typed in YAML) stay visible.
  const unknown = selected.filter((s) => !items.some((i) => i.value === s));
  const groups = [...new Set(shown.map((i) => i.group ?? ''))];
  const toggle = (v: string) => onChange(selected.includes(v) ? selected.filter((x) => x !== v) : [...selected, v]);

  return (
    <div className="col-span-full flex flex-col gap-2">
      {items.length > 8 && <Input placeholder="Filter…" aria-label="Filter" value={q} onChange={(e) => setQ(e.target.value)} />}
      <ScrollArea className="rounded-lg border" style={{ maxHeight }}>
        {items.length === 0 && unknown.length === 0 && <p className="p-3 text-sm text-muted-foreground">{empty}</p>}
        <ItemGroup>
          {unknown.map((v) => (
            <PickRow
              key={v}
              item={{ value: v, description: 'Not found here; kept as written.' }}
              on
              onToggle={() => toggle(v)}
              extra={extra}
            />
          ))}
        </ItemGroup>
        {groups.map((g) => (
          <ItemGroup key={g}>
            {g && <p className="sticky top-0 border-b bg-muted px-3 py-1 text-xs font-medium text-muted-foreground">{g}</p>}
            {shown
              .filter((i) => (i.group ?? '') === g)
              .map((i) => (
                <PickRow key={i.value} item={i} on={selected.includes(i.value)} onToggle={() => toggle(i.value)} extra={extra} />
              ))}
          </ItemGroup>
        ))}
      </ScrollArea>
    </div>
  );
}

function PickRow({ item, on, onToggle, extra }: { item: PickItem; on: boolean; onToggle: () => void; extra?: (v: string) => ReactNode }) {
  const id = useId();
  return (
    <Item size="xs" variant={on ? 'muted' : 'default'} className="rounded-none border-b last:border-b-0">
      <ItemMedia>
        <Checkbox id={id} checked={on} onCheckedChange={onToggle} />
      </ItemMedia>
      <ItemContent className="min-w-0">
        <ItemTitle>
          <label htmlFor={id} className={cn('cursor-pointer', !item.label && 'font-mono')}>
            {item.label ?? item.value}
          </label>
        </ItemTitle>
        {item.description && <ItemDescription>{item.description}</ItemDescription>}
      </ItemContent>
      {on && extra && <ItemActions>{extra(item.value)}</ItemActions>}
    </Item>
  );
}

/** Buttons that insert template variables into a text area. */
export function TemplateVars({ vars, onInsert }: { vars: { name: string; hint: string }[]; onInsert: (text: string) => void }) {
  return (
    <div className="col-span-full flex flex-wrap items-center gap-1.5">
      <span className="text-sm text-muted-foreground">Insert:</span>
      {vars.map((v) => (
        <Button
          key={v.name}
          type="button"
          variant="secondary"
          size="xs"
          className="font-mono"
          title={v.hint}
          onClick={() => onInsert(`{{ ${v.name} }}`)}
        >
          {v.name}
        </Button>
      ))}
    </div>
  );
}

/** Insert `text` at the cursor of a text area, returning the new value. */
export function insertAt(el: HTMLTextAreaElement | null, value: string, text: string): string {
  if (!el) return value + text;
  const { selectionStart: a, selectionEnd: b } = el;
  const next = value.slice(0, a) + text + value.slice(b);
  requestAnimationFrame(() => {
    el.focus();
    el.selectionStart = el.selectionEnd = a + text.length;
  });
  return next;
}
