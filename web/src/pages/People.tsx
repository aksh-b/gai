import { useState } from 'react';
import { toast } from 'sonner';
import { SelectField, TextField } from '@/components/fields';
import { ConfirmAction, EmptyState, ErrorNote, Page, PageHeader } from '@/components/shared';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from '@/components/ui/card';
import { FieldGroup } from '@/components/ui/field';
import { Spinner } from '@/components/ui/spinner';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table';
import { api, type Me, type Membership, type OrgRole, type OrgSettings } from '../api';
import { IconPlus, IconTrash } from '../icons';
import { useLoad } from '../util';

const ROLES: { value: OrgRole; label: string; description: string }[] = [
  { value: 'viewer', label: 'Viewer', description: 'sees everything' },
  { value: 'member', label: 'Member', description: 'also chats and runs' },
  { value: 'builder', label: 'Builder', description: 'also builds assistants, workflows, knowledge' },
  { value: 'admin', label: 'Admin', description: 'also keys, connections, people' },
  { value: 'owner', label: 'Owner', description: 'also the organization itself' },
];

const roleLabel = (r: OrgRole) => ROLES.find((x) => x.value === r)?.label ?? r;

function can(me: Me | undefined) {
  const p = me?.principal;
  const admin = !!p && p.scopes.includes('admin') && !p.team_id;
  return { admin, owner: p?.role === 'owner' || (!!p && !p.role && admin) };
}

/** Members, roles, teams and secrets of the organization you work in. */
export function People() {
  const me = useLoad(() => api.me(), []);
  const members = useLoad(() => api.members(), []);
  const teams = useLoad(() => api.teams(), []);
  const { admin, owner } = can(me.data);

  return (
    <Page>
      <PageHeader
        title="People & teams"
        subtitle={
          me.data ? (
            <>
              Who works in <strong>{me.data.org.name}</strong> and what they may do. Teams own assistants, workflows and knowledge: only
              their builders change them.
            </>
          ) : undefined
        }
      />
      <ErrorNote error={me.error ?? members.error ?? teams.error} />
      <MembersCard members={members.data} admin={admin} owner={owner} reload={members.reload} />
      <TeamsCard members={members.data ?? []} teams={teams.data} admin={admin} reload={teams.reload} />
      {admin && <SecretsCard />}
      {owner && me.data && <OrgCard me={me.data} reload={me.reload} />}
      {me.data?.org.id === 'default' && admin && <OrgsCard />}
    </Page>
  );
}

function MembersCard({
  members,
  admin,
  owner,
  reload,
}: {
  members?: Membership[];
  admin: boolean;
  owner: boolean;
  reload: () => void;
}) {
  const [email, setEmail] = useState('');
  const [role, setRole] = useState<OrgRole>('member');
  const [busy, setBusy] = useState(false);
  const roles = ROLES.filter((r) => owner || r.value !== 'owner');

  async function add() {
    setBusy(true);
    try {
      await api.setMember(email.trim(), role);
      toast.success(`${email.trim()} can sign in now`);
      setEmail('');
      reload();
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  }

  async function change(m: Membership, r: OrgRole) {
    await api.setMember(m.user_id, r).then(reload, (e: Error) => toast.error(e.message));
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle>People</CardTitle>
        <CardDescription>People join when they first sign in with the email you add here.</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        {!members ? (
          <Spinner />
        ) : (
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Email</TableHead>
                <TableHead>Name</TableHead>
                <TableHead className="w-56">Role</TableHead>
                {admin && <TableHead className="w-12" />}
              </TableRow>
            </TableHeader>
            <TableBody>
              {members.map((m) => (
                <TableRow key={m.user_id}>
                  <TableCell className="font-medium">{m.email}</TableCell>
                  <TableCell className="text-muted-foreground">{m.name ?? '—'}</TableCell>
                  <TableCell>
                    {admin && (owner || m.role !== 'owner') ? (
                      <SelectField
                        aria-label={`Role of ${m.email}`}
                        options={roles.map((r) => ({ value: r.value, label: r.label }))}
                        value={m.role}
                        onValueChange={(v) => change(m, v as OrgRole)}
                      />
                    ) : (
                      <Badge variant="secondary">{roleLabel(m.role)}</Badge>
                    )}
                  </TableCell>
                  {admin && (
                    <TableCell>
                      {(owner || m.role !== 'owner') && (
                        <ConfirmAction
                          title={`Remove ${m.email}?`}
                          description="They lose access to this organization and leave its teams."
                          action="Remove"
                          destructive
                          onConfirm={() => api.removeMember(m.user_id).then(reload, (e: Error) => toast.error(e.message))}
                        >
                          <Button variant="ghost" size="icon-sm" aria-label={`Remove ${m.email}`}>
                            <IconTrash />
                          </Button>
                        </ConfirmAction>
                      )}
                    </TableCell>
                  )}
                </TableRow>
              ))}
            </TableBody>
          </Table>
        )}
      </CardContent>
      {admin && (
        <CardFooter className="flex flex-wrap items-end gap-3">
          <TextField
            className="min-w-64 flex-1"
            label="Add someone"
            placeholder="name@example.com"
            type="email"
            value={email}
            onChange={(e) => setEmail(e.target.value)}
          />
          <SelectField
            className="w-48"
            label="Role"
            options={roles.map((r) => ({ value: r.value, label: r.label, description: r.description }))}
            value={role}
            onValueChange={(v) => setRole(v as OrgRole)}
          />
          <Button onClick={add} disabled={busy || !email.includes('@')}>
            {busy ? <Spinner data-icon="inline-start" /> : <IconPlus data-icon="inline-start" />}
            Add
          </Button>
        </CardFooter>
      )}
    </Card>
  );
}

function TeamsCard({
  members,
  teams,
  admin,
  reload,
}: {
  members: Membership[];
  teams?: { id: string; name: string; members: string[] }[];
  admin: boolean;
  reload: () => void;
}) {
  const [name, setName] = useState('');
  const owners = useLoad(() => api.owners(), [teams]);
  const email = (id: string) => members.find((m) => m.user_id === id)?.email ?? id;
  const fail = (e: Error) => toast.error(e.message);

  return (
    <Card>
      <CardHeader>
        <CardTitle>Teams</CardTitle>
        <CardDescription>
          Give an assistant to a team on its page (or anything else with <code>PUT /v1/owners/&lt;kind&gt;/&lt;name&gt;</code>): then only the team's builders
          change it.
        </CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        {teams?.length === 0 && <EmptyState title="No teams yet">Everyone with a builder role can change everything.</EmptyState>}
        {teams?.map((t) => {
          const owned = owners.data?.filter((o) => o.team_id === t.id) ?? [];
          const others = members.filter((m) => !t.members.includes(m.user_id));
          return (
            <div key={t.id} className="flex flex-col gap-2 rounded-lg border p-3">
              <div className="flex items-center gap-2">
                <span className="font-medium">{t.name}</span>
                <span className="text-sm text-muted-foreground">
                  {t.members.length} {t.members.length === 1 ? 'person' : 'people'}
                  {owned.length > 0 && ` · owns ${owned.map((o) => `${o.kind} ${o.name}`).join(', ')}`}
                </span>
                {admin && (
                  <ConfirmAction
                    title={`Delete the ${t.name} team?`}
                    description="What it owns goes back to the whole organization, and its keys stop working."
                    action="Delete team"
                    destructive
                    onConfirm={() => api.deleteTeam(t.id).then(reload, fail)}
                  >
                    <Button variant="ghost" size="icon-sm" className="ml-auto" aria-label={`Delete ${t.name}`}>
                      <IconTrash />
                    </Button>
                  </ConfirmAction>
                )}
              </div>
              <div className="flex flex-wrap items-center gap-2">
                {t.members.map((u) => (
                  <Badge key={u} variant="secondary" className="gap-1">
                    {email(u)}
                    {admin && (
                      <button
                        type="button"
                        className="ml-1 text-muted-foreground hover:text-foreground"
                        aria-label={`Take ${email(u)} out of ${t.name}`}
                        onClick={() => api.removeTeamMember(t.id, u).then(reload, fail)}
                      >
                        ×
                      </button>
                    )}
                  </Badge>
                ))}
                {admin && others.length > 0 && (
                  <SelectField
                    className="w-56"
                    aria-label={`Add someone to ${t.name}`}
                    placeholder="Add someone…"
                    options={others.map((m) => ({ value: m.user_id, label: m.email }))}
                    value=""
                    onValueChange={(v) => v && api.addTeamMember(t.id, String(v)).then(reload, fail)}
                  />
                )}
              </div>
            </div>
          );
        })}
      </CardContent>
      {admin && (
        <CardFooter className="flex items-end gap-3">
          <TextField className="flex-1" label="New team" placeholder="e.g. support" value={name} onChange={(e) => setName(e.target.value)} />
          <Button
            disabled={!name.trim()}
            onClick={() =>
              api.createTeam(name.trim()).then(() => {
                setName('');
                reload();
              }, fail)
            }
          >
            <IconPlus data-icon="inline-start" />
            Create team
          </Button>
        </CardFooter>
      )}
    </Card>
  );
}

function SecretsCard() {
  const list = useLoad(() => api.secrets(), []);
  const [name, setName] = useState('');
  const [value, setValue] = useState('');
  const fail = (e: Error) => toast.error(e.message);
  return (
    <Card>
      <CardHeader>
        <CardTitle>Secrets</CardTitle>
        <CardDescription>
          Tokens your sources read by name (their <code>token_env</code>). Values are never shown again.
        </CardDescription>
      </CardHeader>
      <CardContent className="flex flex-wrap gap-2">
        {list.data?.length === 0 && <span className="text-sm text-muted-foreground">None yet.</span>}
        {list.data?.map((n) => (
          <Badge key={n} variant="outline" className="gap-1 font-mono">
            {n}
            <button
              type="button"
              className="ml-1 text-muted-foreground hover:text-foreground"
              aria-label={`Delete ${n}`}
              onClick={() => api.deleteSecret(n).then(list.reload, fail)}
            >
              ×
            </button>
          </Badge>
        ))}
      </CardContent>
      <CardFooter className="flex flex-wrap items-end gap-3">
        <TextField
          className="w-56"
          label="Name"
          placeholder="GITHUB_TOKEN"
          value={name}
          onChange={(e) => setName(e.target.value.toUpperCase().replace(/[^A-Z0-9_]/g, '_'))}
        />
        <TextField className="min-w-64 flex-1" label="Value" type="password" value={value} onChange={(e) => setValue(e.target.value)} />
        <Button
          disabled={!name || !value}
          onClick={() =>
            api.putSecret(name, value).then(() => {
              setName('');
              setValue('');
              list.reload();
            }, fail)
          }
        >
          Save secret
        </Button>
      </CardFooter>
    </Card>
  );
}

function OrgCard({ me, reload }: { me: Me; reload: () => void }) {
  const settings = (me.org.settings ?? {}) as OrgSettings;
  const [name, setName] = useState(me.org.name);
  const [product, setProduct] = useState(settings.branding?.product_name ?? '');
  const [logo, setLogo] = useState(settings.branding?.logo_url ?? '');
  const [accent, setAccent] = useState(settings.branding?.accent ?? '');
  const [busy, setBusy] = useState(false);

  async function save() {
    setBusy(true);
    try {
      await api.updateOrg(me.org.slug, {
        name,
        settings: {
          ...settings,
          branding: { product_name: product || null, logo_url: logo || null, accent: accent || null },
        },
      });
      toast.success('Saved. Reload to see the new look.');
      reload();
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle>Organization</CardTitle>
        <CardDescription>
          How the dashboard looks for everyone in {me.org.name}.
          {settings.limits?.budget_usd_per_month != null && ` Monthly budget: $${settings.limits.budget_usd_per_month}.`}
        </CardDescription>
      </CardHeader>
      <CardContent>
        <FieldGroup className="grid gap-4 md:grid-cols-2">
          <TextField label="Name" value={name} onChange={(e) => setName(e.target.value)} />
          <TextField label="Product name" placeholder="gai" value={product} onChange={(e) => setProduct(e.target.value)} />
          <TextField label="Logo URL" placeholder="https://…/logo.svg" value={logo} onChange={(e) => setLogo(e.target.value)} />
          <TextField label="Accent colour" placeholder="#5b5bd6" value={accent} onChange={(e) => setAccent(e.target.value)} />
        </FieldGroup>
      </CardContent>
      <CardFooter>
        <Button onClick={save} disabled={busy || !name.trim()}>
          {busy && <Spinner data-icon="inline-start" />}
          Save
        </Button>
      </CardFooter>
    </Card>
  );
}

/** Server admins: every organization on this server. */
function OrgsCard() {
  const list = useLoad(() => api.orgs(), []);
  const [slug, setSlug] = useState('');
  const [name, setName] = useState('');
  const [owner, setOwner] = useState('');
  const fail = (e: Error) => toast.error(e.message);
  return (
    <Card>
      <CardHeader>
        <CardTitle>Organizations on this server</CardTitle>
        <CardDescription>Each gets its own database and files; they never see each other's data.</CardDescription>
      </CardHeader>
      <CardContent>
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Slug</TableHead>
              <TableHead>Name</TableHead>
              <TableHead className="w-12" />
            </TableRow>
          </TableHeader>
          <TableBody>
            {list.data?.map((o) => (
              <TableRow key={o.id}>
                <TableCell className="font-mono">{o.slug}</TableCell>
                <TableCell>{o.name}</TableCell>
                <TableCell>
                  {o.id !== 'default' && (
                    <ConfirmAction
                      title={`Delete ${o.name}?`}
                      description="Its people, teams and keys go and it stops being served. Its data stays on the server's disk."
                      action="Delete"
                      destructive
                      onConfirm={() => api.deleteOrg(o.slug).then(list.reload, fail)}
                    >
                      <Button variant="ghost" size="icon-sm" aria-label={`Delete ${o.name}`}>
                        <IconTrash />
                      </Button>
                    </ConfirmAction>
                  )}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </CardContent>
      <CardFooter className="flex flex-wrap items-end gap-3">
        <TextField
          className="w-40"
          label="Slug"
          placeholder="acme"
          value={slug}
          onChange={(e) => setSlug(e.target.value.toLowerCase().replace(/[^a-z0-9-]/g, ''))}
        />
        <TextField className="w-48" label="Name" placeholder="Acme" value={name} onChange={(e) => setName(e.target.value)} />
        <TextField className="min-w-56 flex-1" label="Owner's email" value={owner} onChange={(e) => setOwner(e.target.value)} />
        <Button
          disabled={slug.length < 2 || !name.trim()}
          onClick={() =>
            api.createOrg(slug, name.trim(), owner.trim() || undefined).then(() => {
              setSlug('');
              setName('');
              setOwner('');
              list.reload();
            }, fail)
          }
        >
          <IconPlus data-icon="inline-start" />
          Create
        </Button>
      </CardFooter>
    </Card>
  );
}
