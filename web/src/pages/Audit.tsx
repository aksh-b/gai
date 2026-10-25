import { useState } from 'react';
import { DownloadIcon, EraserIcon, ShieldCheckIcon, TimerResetIcon } from 'lucide-react';
import { toast } from 'sonner';
import { TextField } from '@/components/fields';
import { ConfirmAction, EmptyState, ErrorNote, Page, PageHeader } from '@/components/shared';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from '@/components/ui/card';
import { Spinner } from '@/components/ui/spinner';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table';
import { api } from '../api';
import { ago, useLoad } from '../util';

/** Settings › Audit & data: who changed what, retention, forgetting a person. */
export function Audit() {
  const [actor, setActor] = useState('');
  const [target, setTarget] = useState('');
  const [filter, setFilter] = useState<{ actor?: string; target?: string }>({});
  const rows = useLoad(() => api.audit({ ...filter, limit: 200 }), [filter.actor, filter.target]);
  const [user, setUser] = useState('');
  const [busy, setBusy] = useState(false);

  async function download() {
    try {
      const blob = await api.auditCsv(filter);
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url;
      a.download = 'gai-audit.csv';
      a.click();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
    } catch (e) {
      toast.error((e as Error).message);
    }
  }

  async function sweep() {
    setBusy(true);
    try {
      const r = await api.sweepRetention();
      const n = r.agents.reduce((s, a) => s + a.sessions, 0);
      toast.success(n ? `Deleted ${n} old conversation${n === 1 ? '' : 's'}` : 'Nothing was old enough to delete');
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <Page wide>
      <PageHeader title="Audit & data" subtitle="Who changed what through the API and this dashboard, and the data gai keeps about people." />
      <Card>
        <CardHeader>
          <CardTitle>Changes</CardTitle>
          <CardDescription>Assistants, keys, settings, connections, approvals and more. Chats and runs are in Activity instead.</CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-4">
          <form
            className="grid items-end gap-3 sm:grid-cols-[1fr_1fr_auto_auto]"
            onSubmit={(e) => {
              e.preventDefault();
              setFilter({ actor: actor.trim() || undefined, target: target.trim() || undefined });
            }}
          >
            <TextField label="Who" placeholder="API key name" value={actor} onChange={(e) => setActor(e.target.value)} />
            <TextField label="What" placeholder="e.g. agents/ or keys/" value={target} onChange={(e) => setTarget(e.target.value)} />
            <Button type="submit" variant="outline">
              Filter
            </Button>
            <Button type="button" variant="outline" onClick={download}>
              <DownloadIcon data-icon="inline-start" />
              CSV
            </Button>
          </form>
          <ErrorNote error={rows.error} />
          {!rows.data ? (
            <Spinner />
          ) : rows.data.length === 0 ? (
            <EmptyState title="Nothing recorded" icon={<ShieldCheckIcon />}>
              Changes made through the API and the dashboard show up here.
            </EmptyState>
          ) : (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>When</TableHead>
                  <TableHead>Who</TableHead>
                  <TableHead>What</TableHead>
                  <TableHead>Result</TableHead>
                  <TableHead>Details</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {rows.data.map((r) => {
                  const detail = r.detail && typeof r.detail === 'object' && Object.keys(r.detail).length ? JSON.stringify(r.detail) : '';
                  return (
                    <TableRow key={r.id}>
                      <TableCell title={r.at}>{ago(r.at)}</TableCell>
                      <TableCell>{r.actor}</TableCell>
                      <TableCell>
                        <span className="font-mono text-xs">{r.action.split(' ')[0]}</span> {r.target}
                      </TableCell>
                      <TableCell>
                        <Badge variant={r.status < 400 ? 'secondary' : 'destructive'}>{r.status}</Badge>
                      </TableCell>
                      <TableCell className="max-w-xs truncate font-mono text-xs text-muted-foreground" title={detail}>
                        {detail}
                      </TableCell>
                    </TableRow>
                  );
                })}
              </TableBody>
            </Table>
          )}
        </CardContent>
      </Card>

      <div className="grid gap-4 md:grid-cols-2">
        <Card>
          <CardHeader>
            <CardTitle>Forget a person</CardTitle>
            <CardDescription>
              Deletes their conversations with every assistant, the runs and files in them, their memory and their ratings. It can’t be undone.
            </CardDescription>
          </CardHeader>
          <CardContent>
            <TextField label="User id" value={user} onChange={(e) => setUser(e.target.value)} placeholder="The user_id their chats use" />
          </CardContent>
          <CardFooter>
            <ConfirmAction
              title={`Forget ${user || 'this user'}?`}
              description="Everything gai keeps about them is deleted for good."
              action="Forget"
              destructive
              onConfirm={async () => {
                try {
                  const d = await api.forgetUser(user.trim());
                  toast.success(`Deleted ${d.sessions} conversations, ${d.memory_blocks} memory blocks and ${d.feedback} ratings`);
                  setUser('');
                  rows.reload();
                } catch (e) {
                  toast.error((e as Error).message);
                }
              }}
            >
              <Button variant="outline" disabled={!user.trim()}>
                <EraserIcon data-icon="inline-start" />
                Forget
              </Button>
            </ConfirmAction>
          </CardFooter>
        </Card>
        <Card>
          <CardHeader>
            <CardTitle>Retention</CardTitle>
            <CardDescription>
              Assistants with “Keep conversations for” set (or <code>retention_days</code> in gai.toml) have older conversations deleted every hour.
            </CardDescription>
          </CardHeader>
          <CardFooter>
            <Button variant="outline" onClick={sweep} disabled={busy}>
              {busy ? <Spinner data-icon="inline-start" /> : <TimerResetIcon data-icon="inline-start" />}
              Apply now
            </Button>
          </CardFooter>
        </Card>
      </div>
    </Page>
  );
}
