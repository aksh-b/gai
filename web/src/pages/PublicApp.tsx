// An app as the people it's shared with see it: a form, a button, and the
// result. Opened from the app's link (#/app/<name>?key=…), outside the
// dashboard, so it needs no sign-in when the link carries the app's key or
// the app is public.

import { useEffect, useMemo, useRef, useState } from 'react';
import { CheckCircle2Icon, HourglassIcon } from 'lucide-react';
import { SelectField, SwitchField, TextField } from '@/components/fields';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from '@/components/ui/card';
import { FieldGroup } from '@/components/ui/field';
import { Spinner } from '@/components/ui/spinner';
import { ErrorNote, Json, Markdown } from '@/components/shared';
import { appClient, type AppField, type AppForm, type AppResult } from '../api';
import { TextArea } from '../forms';

type Client = ReturnType<typeof appClient>;

/** The standalone page at #/app/<name>. */
export function PublicApp({ name, query }: { name: string; query: URLSearchParams }) {
  const client = useMemo(() => appClient(name, query.get('key'), query.get('org')), [name, query]);
  const [form, setForm] = useState<AppForm>();
  const [error, setError] = useState<string>();
  useEffect(() => {
    client
      .form()
      .then((f) => {
        setForm(f);
        document.title = f.title;
      })
      .catch((e: Error) => setError(e.message));
  }, [client]);
  return (
    <main className="min-h-svh bg-muted/40 px-4 py-10">
      <div className="mx-auto flex max-w-2xl flex-col gap-4">
        {error && (
          <Alert variant="destructive">
            <AlertTitle>This app can't be opened</AlertTitle>
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        )}
        {!form && !error && (
          <div className="flex justify-center py-20" aria-busy="true">
            <Spinner />
          </div>
        )}
        {form && <AppRunner form={form} client={client} />}
      </div>
    </main>
  );
}

/** Starting values: each field's default. */
function initial(fields: AppField[]): Record<string, unknown> {
  return Object.fromEntries(
    fields.map((f) => [f.name, Array.isArray(f.default) ? f.default.join('\n') :(f.default ?? (f.type === 'boolean' ? false : ''))]),
  );
}

/** The form and its result. Also the builder's preview. */
export function AppRunner({ form, client }: { form: AppForm; client: Client }) {
  const [values, setValues] = useState<Record<string, unknown>>(() => initial(form.fields));
  const [result, setResult] = useState<AppResult>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const poll = useRef<ReturnType<typeof setTimeout>>(undefined);
  useEffect(() => () => clearTimeout(poll.current), []);
  useEffect(() => setValues(initial(form.fields)), [form.fields]);

  const missing = form.fields.filter(
    (f) => f.required && f.default == null && f.type !== 'boolean' && String(values[f.name] ?? '').trim() === '',
  );

  async function submit() {
    clearTimeout(poll.current);
    setBusy(true);
    setError(undefined);
    setResult(undefined);
    try {
      const { run_id } = await client.start(values);
      const check = async () => {
        try {
          const r = await client.result(run_id);
          setResult(r);
          if (r.done) setBusy(false);
          else poll.current = setTimeout(check, 1200);
        } catch (e) {
          setError((e as Error).message);
          setBusy(false);
        }
      };
      await check();
    } catch (e) {
      setError((e as Error).message);
      setBusy(false);
    }
  }

  return (
    <>
      <Card>
        <CardHeader>
          <CardTitle className="text-xl">{form.title}</CardTitle>
          {form.description && <CardDescription>{form.description}</CardDescription>}
        </CardHeader>
        <CardContent>
          <FieldGroup className="grid gap-4 sm:grid-cols-2">
            {form.fields.map((f) => (
              <Input key={f.name} field={f} value={values[f.name]} onChange={(v) => setValues((x) => ({ ...x, [f.name]: v }))} />
            ))}
          </FieldGroup>
        </CardContent>
        <CardFooter className="justify-end gap-2">
          {missing.length > 0 && (
            <span className="text-sm text-muted-foreground">Fill in {missing.map((f) => f.label ?? f.name).join(', ')}</span>
          )}
          <Button onClick={submit} disabled={busy || missing.length > 0}>
            {busy && <Spinner data-icon="inline-start" />}
            {form.submit_label}
          </Button>
        </CardFooter>
      </Card>
      <ErrorNote error={error} />
      {result && <Outcome result={result} structured={form.structured} />}
    </>
  );
}

function Input({ field: f, value, onChange }: { field: AppField; value: unknown; onChange: (v: unknown) => void }) {
  const label = `${f.label ?? f.name}${f.required || f.default != null || f.type === 'boolean' ? '' : ' (optional)'}`;
  const help = f.help ?? undefined;
  switch (f.type) {
    case 'list':
      return (
        <TextArea
          label={label}
          description={help ?? 'One per line'}
          rows={4}
          value={String(value ?? '')}
          placeholder={f.placeholder ?? undefined}
          onChange={onChange}
        />
      );
    case 'long_text':
      return <TextArea label={label} description={help} rows={5} value={String(value ?? '')} placeholder={f.placeholder ?? undefined} onChange={onChange} />;
    case 'boolean':
      return <SwitchField className="col-span-full" label={label} description={help} checked={value === true} onCheckedChange={onChange} />;
    case 'select':
      return (
        <SelectField
          label={label}
          description={help}
          options={(f.options ?? []).map((o) => ({ value: o, label: o }))}
          value={String(value ?? '')}
          placeholder={f.placeholder ?? 'Choose…'}
          onValueChange={onChange}
        />
      );
    default:
      return (
        <TextField
          label={label}
          description={help}
          type={f.type === 'number' ? 'number' : 'text'}
          placeholder={f.placeholder ?? undefined}
          value={String(value ?? '')}
          onChange={(e) => onChange(e.target.value)}
        />
      );
  }
}

function Outcome({ result, structured }: { result: AppResult; structured: boolean }) {
  if (!result.done) {
    const waiting = result.status === 'awaiting_approval';
    return (
      <Alert>
        {waiting ? <HourglassIcon /> : <Spinner />}
        <AlertTitle>{waiting ? 'Waiting for someone to OK a step' : 'Working on it…'}</AlertTitle>
        {waiting && <AlertDescription>It carries on by itself once it's approved. You can leave this page open.</AlertDescription>}
      </Alert>
    );
  }
  if (result.status !== 'completed') return <ErrorNote error={result.error ?? `It stopped: ${result.status}`} />;
  const out = result.output;
  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          <CheckCircle2Icon className="size-4 text-success" />
          Result
        </CardTitle>
      </CardHeader>
      <CardContent>
        {typeof out === 'string' && !structured ? <Markdown text={out} /> : <Json value={out} />}
      </CardContent>
    </Card>
  );
}
