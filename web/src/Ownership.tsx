import { toast } from 'sonner';
import { SelectField } from '@/components/fields';
import { api } from './api';
import { ago, useLoad } from './util';

const PLURAL: Record<string, string> = {
  agent: 'agents',
  workflow: 'workflows',
  persona: 'personas',
  collection: 'collections',
};

/**
 * Which team owns a resource (only its builders change it), and who
 * changed it last.
 */
export function Ownership({ kind, name }: { kind: 'agent' | 'workflow' | 'persona' | 'collection'; name: string }) {
  const teams = useLoad(() => api.teams(), []);
  const owners = useLoad(() => api.owners(), [name]);
  const changes = useLoad(() => api.changes(`${PLURAL[kind]}/${name}`), [name]);
  const owner = owners.data?.find((o) => o.kind === kind && o.name === name);
  const last = changes.data?.find((c) => c.status < 400);

  return (
    <div className="flex flex-wrap items-center gap-2 text-sm text-muted-foreground">
      {last && (
        <span>
          Changed by {last.actor} {ago(last.at)}
        </span>
      )}
      {!!teams.data?.length && (
        <SelectField
          className="w-48"
          aria-label="Team that owns it"
          options={[{ value: '', label: 'Everyone can change it' }, ...teams.data.map((t) => ({ value: t.id, label: `Team ${t.name}` }))]}
          value={owner?.team_id ?? ''}
          onValueChange={(v) =>
            api.setOwner(kind, name, v ? String(v) : null).then(
              () => {
                owners.reload();
                toast.success(v ? 'Only that team’s builders can change it now' : 'Everyone with a builder role can change it');
              },
              (e: Error) => toast.error(e.message),
            )
          }
        />
      )}
    </div>
  );
}
