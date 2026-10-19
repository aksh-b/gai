import { useEffect, useId, useMemo, useRef, useState, type ReactNode } from 'react';
import { TabBar } from '@/components/fields';
import { Field, FieldDescription, FieldError, FieldLabel } from '@/components/ui/field';
import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';
import YAML from 'yaml';
import { ErrorNote } from './components';
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
    return (
      <div className="spec-editor">
        <div className="form-body">{children}</div>
      </div>
    );
  }

  return (
    <div className="spec-editor">
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
        <div className="form-body">{children}</div>
      ) : (
        <>
          <textarea
            className="code-editor"
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
    <fieldset className="form-section">
      <div className="row between">
        <legend>{title}</legend>
        {aside}
      </div>
      {hint && <p className="muted small form-hint">{hint}</p>}
      <div className="form-grid">{children}</div>
    </fieldset>
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
    <div className={wide ? 'span-all' : undefined}>
      <Field data-invalid={error ? true : undefined}>
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
    </div>
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
    <div className="picklist span-all">
      {items.length > 8 && (
        <input className="plain-input full" placeholder="Filter…" aria-label="Filter" value={q} onChange={(e) => setQ(e.target.value)} />
      )}
      <div className="picklist-items" style={{ maxHeight }}>
        {items.length === 0 && unknown.length === 0 && <p className="muted small">{empty}</p>}
        {unknown.map((v) => (
          <PickRow key={v} item={{ value: v, description: 'Not found here; kept as written.' }} on onToggle={() => toggle(v)} extra={extra} />
        ))}
        {groups.map((g) => (
          <div key={g}>
            {g && <div className="picklist-group">{g}</div>}
            {shown
              .filter((i) => (i.group ?? '') === g)
              .map((i) => (
                <PickRow key={i.value} item={i} on={selected.includes(i.value)} onToggle={() => toggle(i.value)} extra={extra} />
              ))}
          </div>
        ))}
      </div>
    </div>
  );
}

function PickRow({ item, on, onToggle, extra }: { item: PickItem; on: boolean; onToggle: () => void; extra?: (v: string) => ReactNode }) {
  return (
    <div className="pick-row" data-on={on || undefined}>
      <label className="pick-main">
        <input type="checkbox" checked={on} onChange={onToggle} />
        <span className="pick-text">
          <span className={item.label ? 'small' : 'mono small'}>{item.label ?? item.value}</span>
          {item.description && <span className="muted small pick-desc">{item.description}</span>}
        </span>
      </label>
      {on && extra?.(item.value)}
    </div>
  );
}

/** Buttons that insert template variables into a text area. */
export function TemplateVars({
  vars,
  onInsert,
}: {
  vars: { name: string; hint: string }[];
  onInsert: (text: string) => void;
}) {
  return (
    <div className="template-vars span-all">
      <span className="muted small">Insert:</span>
      {vars.map((v) => (
        <button key={v.name} type="button" className="chip mono small var-chip" title={v.hint} onClick={() => onInsert(`{{ ${v.name} }}`)}>
          {v.name}
        </button>
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
