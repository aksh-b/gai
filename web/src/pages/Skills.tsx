import { useState } from 'react';
import { FileTextIcon, GraduationCapIcon } from 'lucide-react';
import { toast } from 'sonner';
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
import {
  Attachment,
  AttachmentAction,
  AttachmentActions,
  AttachmentContent,
  AttachmentGroup,
  AttachmentMedia,
  AttachmentTitle,
} from '@/components/ui/attachment';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardAction, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from '@/components/ui/card';
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible';
import { Field, FieldDescription, FieldLabel } from '@/components/ui/field';
import { Input } from '@/components/ui/input';
import { Spinner } from '@/components/ui/spinner';
import { Textarea } from '@/components/ui/textarea';
import { api, type SkillView } from '../api';
import { Disclosure, EmptyState, ErrorNote, Markdown, Page, PageHeader } from '@/components/shared';
import { IconPlus, IconTrash, IconX } from '../icons';
import { useLoad } from '../util';

const TEMPLATE = `---
name: my-skill
description: What this skill does, and when an agent should use it.
---
# My skill

Step-by-step instructions the agent follows once it loads this skill.
`;

export function Skills() {
  const list = useLoad(() => api.skills(), []);
  return (
    <Page>
      <PageHeader
        title="Skills"
        subtitle={
          <>
            Step-by-step know-how an assistant reads only when a task needs it, like "how we write release notes". Upload one below, then
            pick it in an assistant's settings.
          </>
        }
      />
      <ErrorNote error={list.error} />
      {list.data?.length === 0 && (
        <EmptyState title="No skills yet" icon={<GraduationCapIcon />}>
          Upload one below.
        </EmptyState>
      )}
      <div className="grid gap-3 md:grid-cols-2">
        {list.data?.map((s) => (
          <SkillCard key={s.name} s={s} onChange={list.reload} />
        ))}
      </div>
      <Card>
        <CardContent>
          <Disclosure summary="Upload a skill" open={list.data?.length === 0}>
            <Upload onSaved={list.reload} />
          </Disclosure>
        </CardContent>
      </Card>
    </Page>
  );
}

function SkillCard({ s, onChange }: { s: SkillView; onChange: () => void }) {
  const [open, setOpen] = useState(false);
  const detail = useLoad(() => (open ? api.skill(s.name) : Promise.resolve(undefined)), [open, s.name]);
  const stored = s.source.kind === 'stored';
  async function remove() {
    await api.deleteSkill(s.name).catch((e: Error) => toast.error(e.message));
    onChange();
  }
  return (
    <Card>
      <CardHeader>
        <CardTitle className="font-mono">{s.name}</CardTitle>
        <CardDescription>{s.description}</CardDescription>
        <CardAction className="flex items-center gap-1">
          <Badge variant="outline">{s.source.kind === 'stored' ? `Uploaded v${s.source.version}` : 'Folder'}</Badge>
          {stored && (
            <AlertDialog>
              <AlertDialogTrigger asChild>
                <Button variant="ghost" size="icon-sm" aria-label={`Delete ${s.name}`}>
                  <IconTrash />
                </Button>
              </AlertDialogTrigger>
              <AlertDialogContent>
                <AlertDialogHeader>
                  <AlertDialogTitle>Delete the skill {s.name}?</AlertDialogTitle>
                  <AlertDialogDescription>Assistants that use it lose it. Every uploaded version is removed.</AlertDialogDescription>
                </AlertDialogHeader>
                <AlertDialogFooter>
                  <AlertDialogCancel>Cancel</AlertDialogCancel>
                  <AlertDialogAction variant="destructive" onClick={remove}>
                    Delete
                  </AlertDialogAction>
                </AlertDialogFooter>
              </AlertDialogContent>
            </AlertDialog>
          )}
        </CardAction>
      </CardHeader>
      {(s.source.kind === 'dir' || s.files.length > 0) && (
        <CardContent className="flex flex-col gap-2">
          {s.source.kind === 'dir' && <p className="font-mono text-xs text-muted-foreground">{s.source.path}</p>}
          {s.files.length > 0 && (
            <div className="flex flex-wrap gap-1">
              {s.files.map((f) => (
                <Badge key={f} variant="secondary" className="font-mono">
                  {f}
                </Badge>
              ))}
            </div>
          )}
        </CardContent>
      )}
      <CardFooter className="flex-col items-start gap-3">
        <Collapsible open={open} onOpenChange={setOpen} className="flex w-full flex-col gap-3">
          <CollapsibleTrigger asChild>
            <Button variant="link" size="sm" className="w-fit px-0">
              {open ? 'Hide instructions' : 'Instructions'}
            </Button>
          </CollapsibleTrigger>
          <CollapsibleContent>{detail.data ? <Markdown text={detail.data.instructions} /> : <Spinner />}</CollapsibleContent>
        </Collapsible>
      </CardFooter>
    </Card>
  );
}

function Upload({ onSaved }: { onSaved: () => void }) {
  const [content, setContent] = useState(TEMPLATE);
  const [files, setFiles] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);

  async function addFiles(list: FileList | null) {
    if (!list) return;
    const next = { ...files };
    for (const f of Array.from(list)) {
      if (f.name === 'SKILL.md') setContent(await f.text());
      else next[f.webkitRelativePath.split('/').slice(1).join('/') || f.name] = await f.text();
    }
    setFiles(next);
  }

  async function save() {
    setBusy(true);
    try {
      const r = await api.saveSkill(content, files);
      toast.success(`Saved ${r.name} v${r.version}`);
      setContent(TEMPLATE);
      setFiles({});
      onSaved();
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="flex flex-col gap-4 pt-2">
      <Field>
        <FieldLabel htmlFor="skill-md">SKILL.md</FieldLabel>
        <Textarea id="skill-md" className="min-h-56 font-mono" value={content} onChange={(e) => setContent(e.target.value)} />
      </Field>
      <Field>
        <FieldLabel htmlFor="skill-files">Files</FieldLabel>
        <Input
          id="skill-files"
          type="file"
          multiple
          onChange={(e) => {
            addFiles(e.target.files);
            e.target.value = '';
          }}
        />
        <FieldDescription>Scripts, templates and references the skill uses. A SKILL.md here replaces the text above.</FieldDescription>
      </Field>
      {Object.keys(files).length > 0 && (
        <AttachmentGroup>
          {Object.keys(files).map((f) => (
            <Attachment key={f} size="xs">
              <AttachmentMedia variant="icon">
                <FileTextIcon />
              </AttachmentMedia>
              <AttachmentContent>
                <AttachmentTitle className="font-mono">{f}</AttachmentTitle>
              </AttachmentContent>
              <AttachmentActions>
                <AttachmentAction
                  aria-label={`Remove ${f}`}
                  onClick={() => setFiles(Object.fromEntries(Object.entries(files).filter(([k]) => k !== f)))}
                >
                  <IconX />
                </AttachmentAction>
              </AttachmentActions>
            </Attachment>
          ))}
        </AttachmentGroup>
      )}
      <Button className="w-fit" onClick={save} disabled={busy}>
        {busy ? <Spinner data-icon="inline-start" /> : <IconPlus data-icon="inline-start" />}
        Save skill
      </Button>
    </div>
  );
}
