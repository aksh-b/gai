import { useRef, useState } from 'react';
import { DownloadIcon, PackageIcon, UploadIcon } from 'lucide-react';
import { toast } from 'sonner';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import { Spinner } from '@/components/ui/spinner';
import { SwitchField } from '@/components/fields';
import { api, type BundleReport } from './api';

/** Save a blob as a file. */
function download(blob: Blob, name: string) {
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = name;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

const KINDS: [keyof BundleReport['manifest']['contents'], string][] = [
  ['agents', 'Assistants'],
  ['workflows', 'Workflows'],
  ['personas', 'Personas'],
  ['prompts', 'Prompts'],
  ['skills', 'Skills'],
  ['mcp_servers', 'Connected apps'],
  ['knowledge', 'Knowledge'],
  ['triggers', 'Schedules (imported turned off)'],
];

/**
 * Share assistants as a .gaibundle file, or bring one in. A bundle carries
 * what the assistants use (personas, skills, knowledge, connected apps) but
 * never keys or tokens.
 */
export function BundleMenu({ selected, onImported }: { selected?: string; onImported?: () => void }) {
  const input = useRef<HTMLInputElement>(null);
  const [file, setFile] = useState<File>();
  const [peek, setPeek] = useState<BundleReport>();
  const [done, setDone] = useState<BundleReport>();
  const [skip, setSkip] = useState(false);
  const [busy, setBusy] = useState(false);

  async function exportBundle(agents?: string[]) {
    try {
      const name = agents?.[0] ?? 'gai';
      const blob = await api.exportBundle(agents ? { agents, name } : { all: true, name: 'All assistants' });
      download(blob, `${name}.gaibundle`);
    } catch (e) {
      toast.error((e as Error).message);
    }
  }

  async function choose(f: File | undefined) {
    if (!f) return;
    try {
      setPeek(await api.bundle(f));
      setFile(f);
      setDone(undefined);
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      if (input.current) input.current.value = '';
    }
  }

  async function install() {
    if (!file) return;
    setBusy(true);
    try {
      const report = await api.bundle(file, { skip_existing: skip });
      setDone(report);
      setPeek(undefined);
      onImported?.();
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  }

  const report = done ?? peek;
  const close = () => {
    setPeek(undefined);
    setDone(undefined);
    setFile(undefined);
  };

  return (
    <>
      <input ref={input} type="file" accept=".gaibundle,.zip,application/zip" hidden onChange={(e) => choose(e.target.files?.[0])} />
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button variant="outline">
            <PackageIcon data-icon="inline-start" />
            Share
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          {selected && (
            <DropdownMenuItem onSelect={() => exportBundle([selected])}>
              <DownloadIcon />
              Export “{selected}”
            </DropdownMenuItem>
          )}
          <DropdownMenuItem onSelect={() => exportBundle()}>
            <DownloadIcon />
            Export everything
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem onSelect={() => input.current?.click()}>
            <UploadIcon />
            Import a bundle…
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

      <Dialog open={!!report} onOpenChange={(o) => !o && close()}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{done ? `Imported ${report?.manifest.name}` : `Import ${report?.manifest.name}?`}</DialogTitle>
            <DialogDescription>
              {report?.manifest.description ?? `Made with gai ${report?.manifest.gai_version}.`}
            </DialogDescription>
          </DialogHeader>
          {report && !done && (
            <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-sm">
              {KINDS.filter(([k]) => (report.manifest.contents[k] as string[] | undefined)?.length).map(([k, label]) => (
                <div key={k} className="contents">
                  <dt className="text-muted-foreground">{label}</dt>
                  <dd>{(report.manifest.contents[k] as string[]).join(', ')}</dd>
                </div>
              ))}
            </dl>
          )}
          {done && (
            <ul className="flex flex-col gap-0.5 text-sm">
              {done.installed.map((i) => (
                <li key={i}>+ {i}</li>
              ))}
              {done.skipped.map((i) => (
                <li key={i} className="text-muted-foreground">
                  = {i} (already here)
                </li>
              ))}
            </ul>
          )}
          {report && report.missing_env.length > 0 && (
            <Alert>
              <AlertTitle>Needs keys this server doesn’t have</AlertTitle>
              <AlertDescription>
                <p>Set these on the server (they were left out of the file on purpose):</p>
                <ul className="mt-1 font-mono text-xs">
                  {report.missing_env.map((v) => (
                    <li key={v.name}>
                      {v.name} <span className="font-sans text-muted-foreground">· {v.used_by}</span>
                    </li>
                  ))}
                </ul>
              </AlertDescription>
            </Alert>
          )}
          {report && report.missing_models.length > 0 && (
            <Alert>
              <AlertTitle>Some AI models aren’t connected here</AlertTitle>
              <AlertDescription>{report.missing_models.join(', ')}. Connect them in Settings → AI models, or pick another model.</AlertDescription>
            </Alert>
          )}
          {done && done.warnings.length > 0 && (
            <Alert variant="destructive">
              <AlertDescription>{done.warnings.join(' · ')}</AlertDescription>
            </Alert>
          )}
          {!done && (
            <SwitchField
              label="Keep what’s already here"
              description="Otherwise items with the same name get a new version (the old ones stay in their history)."
              checked={skip}
              onCheckedChange={setSkip}
            />
          )}
          <DialogFooter>
            {done ? (
              <Button onClick={close}>Done</Button>
            ) : (
              <>
                <Button variant="outline" onClick={close}>
                  Cancel
                </Button>
                <Button onClick={install} disabled={busy}>
                  {busy && <Spinner data-icon="inline-start" />}
                  Import
                </Button>
              </>
            )}
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
