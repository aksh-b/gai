import { useState } from 'react';
import { CopyIcon, KeyRoundIcon } from 'lucide-react';
import { toast } from 'sonner';
import { CheckboxField, TextField } from '@/components/fields';
import { EmptyState, ErrorNote, Page, PageHeader } from '@/components/shared';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardFooter, CardHeader, CardTitle } from '@/components/ui/card';
import { FieldGroup, FieldLegend, FieldSet } from '@/components/ui/field';
import { InputGroup, InputGroupAddon, InputGroupButton, InputGroupInput } from '@/components/ui/input-group';
import { Spinner } from '@/components/ui/spinner';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table';
import { api, getApiKey, setApiKey, type Scope } from '../api';
import { IconPlus, IconTrash } from '../icons';
import { ago, money, useLoad } from '../util';

const SCOPES: { id: Scope; label: string; description: string }[] = [
  { id: 'read', label: 'Read', description: 'View runs, sessions, agents and usage' },
  { id: 'run', label: 'Run', description: 'Start runs, chat and answer approvals' },
  { id: 'admin', label: 'Admin', description: 'Everything, including agents and keys' },
];

export function Keys() {
  const list = useLoad(() => api.keys(), []);
  const [name, setName] = useState('');
  const [scopes, setScopes] = useState<Scope[]>(['run']);
  const [budget, setBudget] = useState('');
  const [creating, setCreating] = useState(false);
  const [secret, setSecret] = useState<string | null>(null);
  const [revoking, setRevoking] = useState<{ id: string; name: string } | null>(null);

  async function create() {
    if (!name.trim() || scopes.length === 0) return;
    setCreating(true);
    try {
      const r = await api.createKey(name.trim(), scopes, budget ? Number(budget) : undefined);
      // The first key switches authentication on. Keep this browser signed
      // in by adopting the new key rather than locking it out.
      if (!getApiKey()) {
        setApiKey(r.secret);
        toast.info('This browser now signs in with the new key.');
      }
      setSecret(r.secret);
      setName('');
      setBudget('');
      list.reload();
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setCreating(false);
    }
  }

  async function revoke(id: string) {
    setRevoking(null);
    await api.revokeKey(id).catch((e: Error) => toast.error(e.message));
    list.reload();
  }

  const active = list.data?.filter((k) => !k.revoked_at) ?? [];
  const revoked = list.data?.filter((k) => k.revoked_at) ?? [];

  return (
    <Page>
      <PageHeader
        title="Access keys"
        subtitle={
          <>
            Keys let apps and scripts use gai (the API, and OpenAI-compatible clients at <code>/openai/v1</code>). This browser has its own
            key, made when you first opened the dashboard.
          </>
        }
      />
      <ErrorNote error={list.error} />

      {secret && (
        <Alert>
          <KeyRoundIcon />
          <AlertTitle>Your new key</AlertTitle>
          <AlertDescription className="flex flex-col gap-2">
            Copy it now. It will not be shown again.
            <InputGroup>
              <InputGroupInput readOnly value={secret} className="font-mono" aria-label="New key" onFocus={(e) => e.target.select()} />
              <InputGroupAddon align="inline-end">
                <InputGroupButton
                  onClick={() =>
                    navigator.clipboard.writeText(secret).then(
                      () => toast.success('Copied'),
                      () => toast.error('Copy failed'),
                    )
                  }
                >
                  <CopyIcon data-icon="inline-start" />
                  Copy
                </InputGroupButton>
              </InputGroupAddon>
            </InputGroup>
            <Button variant="ghost" size="sm" className="w-fit" onClick={() => setSecret(null)}>
              Done
            </Button>
          </AlertDescription>
        </Alert>
      )}

      <div className="grid items-start gap-4 lg:grid-cols-[22rem_minmax(0,1fr)]">
        <Card>
          <CardHeader>
            <CardTitle>Create a key</CardTitle>
          </CardHeader>
          <CardContent>
            <FieldGroup>
              <TextField label="Name" placeholder="e.g. ci, laptop, slack-bot" value={name} onChange={(e) => setName(e.target.value)} />
              <FieldSet>
                <FieldLegend variant="label">Scopes</FieldLegend>
                <FieldGroup className="gap-3">
                  {SCOPES.map((s) => (
                    <CheckboxField
                      key={s.id}
                      label={s.label}
                      description={s.description}
                      checked={scopes.includes(s.id)}
                      onCheckedChange={(checked) => setScopes((cur) => (checked ? [...cur, s.id] : cur.filter((x) => x !== s.id)))}
                    />
                  ))}
                </FieldGroup>
              </FieldSet>
              <TextField
                label="Budget (USD, optional)"
                description="New runs are refused once the key has spent this much."
                inputMode="decimal"
                value={budget}
                onChange={(e) => setBudget(e.target.value.replace(/[^0-9.]/g, ''))}
              />
            </FieldGroup>
          </CardContent>
          <CardFooter>
            <Button className="w-full" onClick={create} disabled={creating || !name.trim() || !scopes.length}>
              {creating ? <Spinner data-icon="inline-start" /> : <IconPlus data-icon="inline-start" />}
              Create key
            </Button>
          </CardFooter>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle>Active</CardTitle>
          </CardHeader>
          <CardContent>
            {active.length === 0 ? (
              <EmptyState title="No keys yet" icon={<KeyRoundIcon />}>
                Anyone who can reach this server can use it until a key exists.
              </EmptyState>
            ) : (
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Key</TableHead>
                    <TableHead>Scopes</TableHead>
                    <TableHead className="text-right">Spent</TableHead>
                    <TableHead>Last used</TableHead>
                    <TableHead className="w-10">
                      <span className="sr-only">Actions</span>
                    </TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {active.map((k) => (
                    <TableRow key={k.id}>
                      <TableCell>
                        <div className="font-medium">{k.name}</div>
                        <div className="font-mono text-xs text-muted-foreground">{k.prefix}…</div>
                      </TableCell>
                      <TableCell>
                        <div className="flex flex-wrap gap-1">
                          {k.scopes.map((s) => (
                            <Badge key={s} variant="outline">
                              {s}
                            </Badge>
                          ))}
                        </div>
                      </TableCell>
                      <TableCell className="text-right tabular-nums">
                        {money(k.spent_usd)}
                        {k.budget_usd != null && <span className="text-muted-foreground"> / {money(k.budget_usd)}</span>}
                      </TableCell>
                      <TableCell className="text-muted-foreground">{k.last_used_at ? ago(k.last_used_at) : 'Never'}</TableCell>
                      <TableCell>
                        <Button
                          variant="ghost"
                          size="icon-sm"
                          aria-label={`Revoke ${k.name}`}
                          onClick={() => setRevoking({ id: k.id, name: k.name })}
                        >
                          <IconTrash />
                        </Button>
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            )}
          </CardContent>
          {revoked.length > 0 && (
            <CardFooter className="text-sm text-muted-foreground">
              {revoked.length} revoked key{revoked.length === 1 ? '' : 's'} not shown.
            </CardFooter>
          )}
        </Card>
      </div>

      <AlertDialog open={revoking !== null} onOpenChange={(o) => !o && setRevoking(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Revoke {revoking?.name}?</AlertDialogTitle>
            <AlertDialogDescription>Anything using this key stops working straight away. This can't be undone.</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction variant="destructive" onClick={() => revoking && revoke(revoking.id)}>
              Revoke
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </Page>
  );
}
