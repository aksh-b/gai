import { useEffect, useState } from 'react';
import { SelectField, TextField } from '@/components/fields';
import { Disclosure, ErrorNote, Snippet } from '@/components/shared';
import { Button } from '@/components/ui/button';
import { Card, CardContent } from '@/components/ui/card';
import { Field, FieldDescription } from '@/components/ui/field';
import { Textarea } from '@/components/ui/textarea';
import { api, type PersonaSpec } from './api';
import { Section, TextArea, setIn } from './forms';
import { IconPlus, IconTrash } from './icons';
import { PersonaAvatar } from './Persona';

type Example = { user: string; assistant: string };

const FORMALITY = [
  { value: '', label: 'Not set' },
  { value: 'casual', label: 'Casual' },
  { value: 'neutral', label: 'Neutral' },
  { value: 'formal', label: 'Formal' },
];

const VERBOSITY = [
  { value: '', label: 'Not set' },
  { value: 'terse', label: 'Short', description: 'as brief as possible' },
  { value: 'balanced', label: 'Balanced', description: 'as long as needed' },
  { value: 'detailed', label: 'Detailed', description: 'thorough, with examples' },
];

/** Every part of a persona as a form, with the prompt it makes. */
export function PersonaForm({ spec, onChange, isNew }: { spec: PersonaSpec; onChange: (s: PersonaSpec) => void; isNew: boolean }) {
  const set = (key: keyof PersonaSpec, v: unknown) => onChange(setIn(spec, [key], v));
  const examples = (spec.examples ?? []) as Example[];
  const setExample = (i: number, e: Example | null) => {
    const next = [...examples];
    if (e) next[i] = e;
    else next.splice(i, 1);
    set('examples', next);
  };

  return (
    <>
      <Section title="Identity" hint="How the persona shows up in chat. None of this goes to the model except the name.">
        <TextField
          label="Name"
          value={spec.name ?? ''}
          disabled={!isNew}
          description={isNew ? 'Letters, digits, - and _. It cannot be changed later.' : 'Changes are kept as versions, so you can go back.'}
          onChange={(e) => set('name', e.target.value)}
        />
        <TextField
          label="Display name"
          value={spec.display_name ?? ''}
          placeholder={spec.name || 'Friendly tutor'}
          description="Shown in chat; the persona introduces itself by it."
          onChange={(e) => set('display_name', e.target.value)}
        />
        <Field>
          <div className="flex items-end gap-3">
            <PersonaAvatar persona={spec} className="size-9" />
            <TextField
              className="flex-1"
              label="Avatar"
              value={spec.avatar ?? ''}
              placeholder="🦉 or https://…/face.png"
              onChange={(e) => set('avatar', e.target.value)}
            />
          </div>
          <FieldDescription>An emoji, or a link to an image.</FieldDescription>
        </Field>
        <TextField
          label="Description"
          value={spec.description ?? ''}
          description="One sentence about the voice. The model reads it too."
          onChange={(e) => set('description', e.target.value)}
        />
        <TextArea
          label="Greeting"
          rows={2}
          value={spec.greeting ?? ''}
          placeholder="Hi! What would you like to learn today?"
          description="Shown when a new chat starts."
          onChange={(v) => set('greeting', v)}
        />
        <Lines
          label="Conversation starters"
          description="Suggested first messages, one per line."
          value={spec.starters ?? []}
          onChange={(v) => set('starters', v)}
        />
      </Section>

      <Section title="Voice" hint="How it talks. Leave anything empty to let the assistant decide.">
        <TextField
          label="Tone"
          value={spec.tone ?? ''}
          placeholder="warm and encouraging"
          onChange={(e) => set('tone', e.target.value)}
        />
        <TextField
          label="Language"
          value={spec.language ?? ''}
          placeholder="the user's language"
          onChange={(e) => set('language', e.target.value)}
        />
        <SelectField label="Formality" options={FORMALITY} value={spec.formality ?? ''} onValueChange={(v) => set('formality', v)} />
        <SelectField label="Length" options={VERBOSITY} value={spec.verbosity ?? ''} onValueChange={(v) => set('verbosity', v)} />
        <TextArea
          label="Anything else about the voice"
          rows={3}
          value={spec.instructions ?? ''}
          placeholder="Uses British spelling. Signs off with a short tip."
          onChange={(v) => set('instructions', v)}
        />
        <Lines label="Do" description="One per line." value={spec.dos ?? []} onChange={(v) => set('dos', v)} wide={false} />
        <Lines label="Don't" description="One per line." value={spec.donts ?? []} onChange={(v) => set('donts', v)} wide={false} />
      </Section>

      <Section
        title="Example replies"
        hint="A message and how this persona would answer it. One or two show the voice better than any description."
        aside={
          <Button variant="ghost" size="sm" onClick={() => set('examples', [...examples, { user: '', assistant: '' }])}>
            <IconPlus data-icon="inline-start" />
            Add example
          </Button>
        }
      >
        {examples.map((e, i) => (
          <Card key={i} size="sm" className="col-span-full">
            <CardContent className="flex gap-2">
              <div className="flex flex-1 flex-col gap-2">
                <Textarea
                  rows={1}
                  aria-label="Someone says"
                  placeholder="Someone says…"
                  value={e.user}
                  onChange={(x) => setExample(i, { ...e, user: x.target.value })}
                />
                <Textarea
                  rows={2}
                  aria-label="The persona answers"
                  placeholder="The persona answers…"
                  value={e.assistant}
                  onChange={(x) => setExample(i, { ...e, assistant: x.target.value })}
                />
              </div>
              <Button variant="ghost" size="icon" aria-label="Remove example" onClick={() => setExample(i, null)}>
                <IconTrash />
              </Button>
            </CardContent>
          </Card>
        ))}
        {examples.length === 0 && <FieldDescription className="col-span-full">No examples yet.</FieldDescription>}
      </Section>

      <Disclosure summary="What the model is told">
        <RenderedPrompt spec={spec} />
      </Disclosure>
    </>
  );
}

/** The persona as it goes into the system prompt, as the server renders it. */
function RenderedPrompt({ spec }: { spec: PersonaSpec }) {
  const [text, setText] = useState('');
  const [error, setError] = useState<string>();
  const key = JSON.stringify(spec);
  useEffect(() => {
    const t = setTimeout(() => {
      api
        .renderPersona({ ...spec, name: spec.name || 'persona' })
        .then((r) => {
          setText(r.text);
          setError(undefined);
        })
        .catch((e: Error) => setError(e.message));
    }, 300);
    return () => clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);
  return (
    <div className="flex flex-col gap-2">
      <ErrorNote error={error} />
      {text && <Snippet title="Added to the assistant's instructions" code={text} />}
    </div>
  );
}

/**
 * A list edited as lines of text. Blank lines are dropped from the list but
 * kept while typing, so Enter starts a new item.
 */
function Lines({
  label,
  description,
  value,
  onChange,
  wide = true,
}: {
  label: string;
  description?: string;
  value: string[];
  onChange: (v: string[]) => void;
  wide?: boolean;
}) {
  const [text, setText] = useState(value.join('\n'));
  const clean = (t: string) => t.split('\n').map((l) => l.trim()).filter(Boolean);
  // Follow changes made elsewhere (YAML tab, a loaded version).
  useEffect(() => {
    if (clean(text).join('\n') !== value.join('\n')) setText(value.join('\n'));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [value.join('\n')]);
  return (
    <TextArea
      label={label}
      description={description}
      rows={3}
      wide={wide}
      value={text}
      onChange={(t) => {
        setText(t);
        onChange(clean(t));
      }}
    />
  );
}
