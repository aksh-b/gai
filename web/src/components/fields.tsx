// Labelled form controls used across the dashboard, composed from the shadcn
// primitives in ./ui. Pages pass data (options, tab items) instead of
// repeating the Field / Select / Tabs structure each time.

import { tr } from '@/i18n';
import { useId, type ComponentProps, type ReactNode } from 'react';

import { Checkbox } from '@/components/ui/checkbox';
import { Field, FieldContent, FieldDescription, FieldError, FieldLabel, FieldTitle } from '@/components/ui/field';
import { Input } from '@/components/ui/input';
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';
import { Switch } from '@/components/ui/switch';
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group';
import { cn } from '@/lib/utils';

type Labelled = {
  label?: ReactNode;
  description?: ReactNode;
};

export type TextFieldProps = ComponentProps<typeof Input> &
  Labelled & {
    error?: ReactNode;
  };

/** An input with its label, help text and error. */
export function TextField({ label, description, error, id, className, ...rest }: TextFieldProps) {
  const auto = useId();
  const fid = id ?? auto;
  return (
    <Field data-invalid={error ? true : undefined} className={className}>
      {label && <FieldLabel htmlFor={fid}>{tr(label)}</FieldLabel>}
      <Input id={fid} aria-invalid={error ? true : undefined} {...rest} />
      {description && <FieldDescription>{tr(description)}</FieldDescription>}
      {error && <FieldError>{error}</FieldError>}
    </Field>
  );
}

export interface SelectOption {
  value: string;
  label: ReactNode;
  description?: ReactNode;
  disabled?: boolean;
}

// Radix Select reserves "" for "nothing chosen", so an option whose value
// is "" (e.g. "Server default") travels under this stand-in.
const NONE = '__none__';
const enc = (v: string) => (v === '' ? NONE : v);
const dec = (v: string) => (v === NONE ? '' : v);

export type SelectFieldProps = Labelled & {
  options: SelectOption[];
  value?: string | null;
  onValueChange?: (value: string) => void;
  placeholder?: string;
  disabled?: boolean;
  className?: string;
  'aria-label'?: string;
};

/** A dropdown from a list of options, labelled when `label` is given. */
export function SelectField({
  label,
  description,
  options,
  value,
  onValueChange,
  placeholder,
  disabled,
  className,
  ...aria
}: SelectFieldProps) {
  const id = useId();
  const hasEmpty = options.some((o) => o.value === '');
  const current = value == null || (value === '' && !hasEmpty) ? '' : enc(value);
  const chosen = options.find((o) => enc(o.value) === current);
  const control = (
    <Select value={current} onValueChange={(v) => onValueChange?.(dec(v))} disabled={disabled}>
      <SelectTrigger id={id} aria-label={aria['aria-label']} className={cn('w-full max-w-full', !label && className)}>
        {/* Only the label: descriptions are for the open list. */}
        <SelectValue placeholder={placeholder}>{chosen?.label}</SelectValue>
      </SelectTrigger>
      <SelectContent>
        <SelectGroup>
          {options.map((o) => (
            <SelectItem key={o.value} value={enc(o.value)} disabled={o.disabled}>
              {o.label}
              {o.description && <span className="text-muted-foreground">· {o.description}</span>}
            </SelectItem>
          ))}
        </SelectGroup>
      </SelectContent>
    </Select>
  );
  if (!label) return control;
  return (
    <Field className={className}>
      <FieldLabel htmlFor={id}>{tr(label)}</FieldLabel>
      {control}
      {description && <FieldDescription>{tr(description)}</FieldDescription>}
    </Field>
  );
}

type ToggleProps = Labelled & {
  checked?: boolean;
  onCheckedChange?: (checked: boolean) => void;
  disabled?: boolean;
  className?: string;
};

/** An on/off setting with its label beside it. */
export function SwitchField({ label, description, size, className, ...rest }: ToggleProps & { size?: 'sm' | 'default' }) {
  const id = useId();
  return (
    <Field orientation="horizontal" className={className}>
      <Switch id={id} size={size} {...rest} />
      <ToggleLabel id={id} label={label} description={description} />
    </Field>
  );
}

/** A checkbox with its label beside it. */
export function CheckboxField({ label, description, className, onCheckedChange, ...rest }: ToggleProps) {
  const id = useId();
  return (
    <Field orientation="horizontal" className={className}>
      <Checkbox id={id} onCheckedChange={(c) => onCheckedChange?.(c === true)} {...rest} />
      <ToggleLabel id={id} label={label} description={description} />
    </Field>
  );
}

function ToggleLabel({ id, label, description }: Labelled & { id: string }) {
  if (!label && !description) return null;
  if (!description) return <FieldLabel htmlFor={id}>{tr(label)}</FieldLabel>;
  return (
    <FieldContent>
      <FieldLabel htmlFor={id}>{tr(label)}</FieldLabel>
      <FieldDescription>{tr(description)}</FieldDescription>
    </FieldContent>
  );
}

export interface TabItem {
  id: string;
  label: ReactNode;
  icon?: ReactNode;
  badge?: ReactNode;
  disabled?: boolean;
}

export type TabBarProps = {
  items: TabItem[];
  value?: string;
  onValueChange?: (id: string) => void;
  /** `pill` for filters inside a page, `underline` for page sections. */
  variant?: 'pill' | 'underline';
  className?: string;
  'aria-label'?: string;
};

/** A row of tabs that switches what the page shows; the page renders the content. */
export function TabBar({ items, value, onValueChange, variant = 'pill', className, ...aria }: TabBarProps) {
  return (
    <Tabs value={value} onValueChange={onValueChange} className={className}>
      <TabsList variant={variant === 'underline' ? 'line' : 'default'} aria-label={aria['aria-label']}>
        {items.map((t) => (
          <TabsTrigger key={t.id} value={t.id} disabled={t.disabled}>
            {t.icon}
            {t.label}
            {t.badge}
          </TabsTrigger>
        ))}
      </TabsList>
    </Tabs>
  );
}

export type ToggleFieldProps = {
  label?: ReactNode;
  options: SelectOption[];
  value: string;
  onValueChange: (value: string) => void;
  className?: string;
  'aria-label'?: string;
};

/** One choice from two to five options, shown side by side. */
export function ToggleField({ label, options, value, onValueChange, className, ...aria }: ToggleFieldProps) {
  const id = useId();
  return (
    <Field className={className}>
      {label && <FieldTitle id={id}>{tr(label)}</FieldTitle>}
      <ToggleGroup
        type="single"
        variant="outline"
        spacing={0}
        aria-labelledby={label ? id : undefined}
        aria-label={aria['aria-label']}
        value={value}
        // Radix sends "" when the chosen item is clicked again; keep the choice.
        onValueChange={(v) => v && onValueChange(v)}
        className="w-fit"
      >
        {options.map((o) => (
          <ToggleGroupItem key={o.value} value={o.value} disabled={o.disabled}>
            {o.label}
          </ToggleGroupItem>
        ))}
      </ToggleGroup>
    </Field>
  );
}
