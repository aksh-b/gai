import { useState } from 'react';
import { AlertCircleIcon, PlugIcon } from 'lucide-react';
import { toast } from 'sonner';
import { SelectField, TextField, ToggleField } from '@/components/fields';
import { Alert, AlertDescription } from '@/components/ui/alert';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardAction, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { FieldGroup } from '@/components/ui/field';
import { Item, ItemActions, ItemContent, ItemDescription, ItemFooter, ItemGroup, ItemTitle } from '@/components/ui/item';
import { Spinner } from '@/components/ui/spinner';
import { api, type McpServerConfig, type McpServerInfo, type ToolPolicy } from '../api';
import { ConfirmAction, Disclosure, EmptyState, ErrorNote, Json, Page, PageHeader } from '@/components/shared';
import { IconPlus, IconTrash } from '../icons';
import { TextArea } from '../forms';
import { useLoad } from '../util';
import { toolLabel } from '../labels';
import { useDevMode } from '../prefs';

export function Tools() {
  const [dev] = useDevMode();
  const tools = useLoad(() => api.tools(), []);
  const mcp = useLoad(() => api.mcp(), []);
  const hooks = useLoad(() => api.hooks(), []);
  const templates = useLoad(() => api.templates(), []);
  const ready = (templates.data ?? []).filter((t) => t.kind === 'mcp' && !t.installed);
  const reload = () => {
    tools.reload();
    mcp.reload();
    templates.reload();
  };

  async function connect(id: string) {
    try {
      await api.installTemplate(id);
      toast.success('Connected. If it needs a token, its status says so.');
      reload();
    } catch (e) {
      toast.error((e as Error).message);
    }
  }

  const builtin = tools.data?.filter((t) => !t.spec.name.startsWith('mcp__')) ?? [];

  return (
    <Page>
      <PageHeader
        title="Tools & connections"
        subtitle="What assistants can do, and the apps they can work with. Choose whether each tool acts on its own or asks you first."
      />
      <ErrorNote error={tools.error ?? mcp.error} />

      <Card>
        <CardHeader>
          <CardTitle>Connected apps</CardTitle>
          <CardDescription>
            Apps that speak MCP (the Model Context Protocol) add their own tools. Pick “Everything from …” in an assistant's abilities to
            use them all.
          </CardDescription>
          {hooks.data && hooks.data.length > 0 && (
            <CardAction className="flex flex-wrap items-center gap-1 text-sm text-muted-foreground">
              Guardrail hooks:
              {hooks.data.map((h) => (
                <Badge key={h} variant="outline" className="font-mono">
                  {h}
                </Badge>
              ))}
            </CardAction>
          )}
        </CardHeader>
        <CardContent className="flex flex-col gap-4">
          {mcp.data?.length === 0 ? (
            <EmptyState title="No apps connected" icon={<PlugIcon />}>
              Connect one below (any MCP server: Jira, GitHub, a database, …).
            </EmptyState>
          ) : (
            <ItemGroup className="gap-2">
              {mcp.data?.map((s) => (
                <McpCard key={s.name} s={s} onChange={reload} />
              ))}
            </ItemGroup>
          )}
          {ready.length > 0 && (
            <ItemGroup className="gap-2">
              {ready.map((t) => (
                <Item key={t.id} variant="outline" size="sm">
                  <ItemContent>
                    <ItemTitle>{t.title}</ItemTitle>
                    <ItemDescription>{t.description}</ItemDescription>
                  </ItemContent>
                  <ItemActions>
                    <Button size="sm" variant="outline" onClick={() => connect(t.id)}>
                      <IconPlus data-icon="inline-start" />
                      Connect
                    </Button>
                  </ItemActions>
                </Item>
              ))}
            </ItemGroup>
          )}
          <Disclosure summary="Connect an app">
            <AddMcp onAdded={reload} />
          </Disclosure>
        </CardContent>
      </Card>

      <h2 className="text-lg font-semibold">Built-in abilities</h2>
      <div className="grid gap-3 md:grid-cols-2">
        {builtin.map((t) => (
          <Card key={t.spec.name}>
            <CardHeader>
              <CardTitle title={t.spec.name}>{dev ? <span className="font-mono">{t.spec.name}</span> : toolLabel(t.spec.name)}</CardTitle>
              <CardDescription>{t.spec.description}</CardDescription>
              <CardAction>
                <PolicyPill policy={t.policy} />
              </CardAction>
            </CardHeader>
            {dev && (
              <CardContent>
                <Disclosure summary="Input schema">
                  <Json value={t.spec.input_schema} />
                </Disclosure>
              </CardContent>
            )}
          </Card>
        ))}
      </div>
    </Page>
  );
}

function PolicyPill({ policy }: { policy: ToolPolicy }) {
  return (
    <Badge variant={policy === 'auto' ? 'secondary' : policy === 'approve' ? 'outline' : 'destructive'}>
      {policy === 'auto' ? 'Acts on its own' : policy === 'approve' ? 'Asks first' : 'Never'}
    </Badge>
  );
}

function McpCard({ s, onChange }: { s: McpServerInfo; onChange: () => void }) {
  const [busy, setBusy] = useState(false);
  const target = s.config.url ?? [s.config.command, ...(s.config.args ?? [])].filter(Boolean).join(' ');
  const state = s.status.state;
  const isHttp = !!s.config.url;

  // OAuth: open the provider's consent page; its redirect lands on the
  // server, which connects and tells this window when it is done.
  async function signIn() {
    // Open the tab now, inside the click, so pop-up blockers allow it; it is
    // pointed at the provider once the server has the sign-in URL.
    const popup = window.open('about:blank', '_blank');
    setBusy(true);
    try {
      const { auth_url } = await api.mcpSignIn(s.name);
      if (popup) popup.location.href = auth_url;
      else window.location.assign(auth_url);
      const done = (e: MessageEvent) => {
        if (e.origin !== location.origin || e.data?.type !== 'gai-mcp-signed-in') return;
        window.removeEventListener('message', done);
        toast.success(`Signed in to ${e.data.server}`);
        onChange();
      };
      window.addEventListener('message', done);
      // Also poll, for browsers that cut the link to the opener.
      for (let i = 0; i < 150; i++) {
        await new Promise((r) => setTimeout(r, 2000));
        const now = (await api.mcp()).find((x) => x.name === s.name);
        if (now && now.status.state !== 'needs_auth') {
          window.removeEventListener('message', done);
          onChange();
          break;
        }
      }
    } catch (e) {
      popup?.close();
      toast.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  }

  async function act(fn: () => Promise<unknown>) {
    setBusy(true);
    try {
      await fn();
      onChange();
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <Item variant="outline">
      <ItemContent className="min-w-0">
        <ItemTitle className="flex-wrap">
          <span className="font-mono">{s.name}</span>
          <Badge variant={state === 'ready' ? 'secondary' : state === 'failed' ? 'destructive' : 'outline'}>
            {state === 'connecting' && <Spinner data-icon="inline-start" />}
            {state === 'needs_auth' ? 'Needs sign-in' : state.charAt(0).toUpperCase() + state.slice(1)}
          </Badge>
          <Badge variant="outline">{s.source === 'config' ? 'gai.toml' : 'Added here'}</Badge>
        </ItemTitle>
        <ItemDescription className="truncate font-mono">{target}</ItemDescription>
      </ItemContent>
      <ItemActions className="flex-wrap">
        {state === 'needs_auth' ? (
          <Button size="sm" onClick={signIn} disabled={busy}>
            {busy && <Spinner data-icon="inline-start" />}
            Sign in
          </Button>
        ) : (
          <Button variant="outline" size="sm" onClick={() => act(() => api.reconnectMcp(s.name))} disabled={busy}>
            {busy && <Spinner data-icon="inline-start" />}
            Reconnect
          </Button>
        )}
        {isHttp && state === 'ready' && (
          <ConfirmAction
            title={`Sign out of ${s.name}?`}
            description="Its tools stop working until you sign in again."
            action="Sign out"
            onConfirm={() => act(() => api.mcpSignOut(s.name))}
          >
            <Button variant="ghost" size="sm">
              Sign out
            </Button>
          </ConfirmAction>
        )}
        {s.source !== 'config' && (
          <ConfirmAction
            title={`Remove ${s.name}?`}
            description="Assistants lose its tools. You can connect it again later."
            action="Remove"
            destructive
            onConfirm={() => act(() => api.removeMcp(s.name))}
          >
            <Button variant="destructive" size="sm">
              <IconTrash data-icon="inline-start" />
              Remove
            </Button>
          </ConfirmAction>
        )}
      </ItemActions>
      {(state === 'failed' || state === 'needs_auth' || s.tools.length > 0) && (
        <ItemFooter className="flex-col items-stretch gap-2">
          {state === 'failed' && 'message' in s.status && (
            <Alert variant="destructive">
              <AlertCircleIcon />
              <AlertDescription>{String(s.status.message)}</AlertDescription>
            </Alert>
          )}
          {state === 'needs_auth' && (
            <p className="text-sm text-muted-foreground">This server needs you to sign in. Sign in opens its approval page in a new tab.</p>
          )}
          {s.tools.length > 0 && (
            <div className="flex flex-wrap gap-1">
              {s.tools.map((t) => (
                <Badge key={t} variant="secondary" className="font-mono">
                  {t.replace(`mcp__${s.name}__`, '')}
                </Badge>
              ))}
            </div>
          )}
        </ItemFooter>
      )}
    </Item>
  );
}

function lines(text: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const l of text.split('\n')) {
    const i = l.indexOf('=');
    if (i > 0) out[l.slice(0, i).trim()] = l.slice(i + 1).trim();
  }
  return out;
}

/** Split a command line on spaces, respecting simple quotes. */
function argv(cmd: string): string[] {
  return (cmd.match(/"[^"]*"|'[^']*'|\S+/g) ?? []).map((a) => a.replace(/^["']|["']$/g, ''));
}

function AddMcp({ onAdded }: { onAdded: () => void }) {
  const [name, setName] = useState('');
  const [kind, setKind] = useState('stdio');
  const [command, setCommand] = useState('');
  const [url, setUrl] = useState('');
  const [pairs, setPairs] = useState('');
  const [policy, setPolicy] = useState('');
  const [busy, setBusy] = useState(false);

  async function add() {
    const parts = argv(command);
    const config: McpServerConfig =
      kind === 'stdio' ? { command: parts[0], args: parts.slice(1), env: lines(pairs) } : { url: url.trim(), headers: lines(pairs) };
    if (policy) config.policy = policy as ToolPolicy;
    setBusy(true);
    try {
      const info = await api.addMcp(name.trim(), config);
      if (info.status.state === 'ready') toast.success(`${info.name}: ${info.tools.length} tools`);
      else toast.error(`${info.name} was added but did not connect`);
      setName('');
      setCommand('');
      setUrl('');
      setPairs('');
      onAdded();
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <FieldGroup className="grid gap-4 pt-2 sm:grid-cols-2">
      <TextField label="Name" placeholder="e.g. github" value={name} onChange={(e) => setName(e.target.value)} />
      <ToggleField
        label="Connects by"
        value={kind}
        onValueChange={setKind}
        options={[
          { value: 'stdio', label: 'Starting a program' },
          { value: 'http', label: 'A URL' },
        ]}
      />
      {kind === 'stdio' ? (
        <TextField
          label="Command"
          className="col-span-full"
          placeholder="npx -y @modelcontextprotocol/server-filesystem /data"
          value={command}
          onChange={(e) => setCommand(e.target.value)}
        />
      ) : (
        <TextField
          label="URL"
          className="col-span-full"
          placeholder="https://example.com/mcp"
          value={url}
          onChange={(e) => setUrl(e.target.value)}
        />
      )}
      <TextArea
        label={`${kind === 'stdio' ? 'Environment' : 'Headers'} (one NAME=value per line; values may use \${VAR})`}
        mono
        rows={3}
        value={pairs}
        onChange={setPairs}
      />
      <SelectField
        label="Policy"
        value={policy}
        onValueChange={setPolicy}
        options={[
          { value: '', label: 'Usual', description: 'Acts on its own, but asks first for anything the app marks as risky' },
          { value: 'auto', label: 'Always acts on its own' },
          { value: 'approve', label: 'Always asks first' },
          { value: 'deny', label: 'Never' },
        ]}
      />
      <div className="col-span-full flex justify-end">
        <Button onClick={add} disabled={busy || !name.trim() || (kind === 'stdio' ? !command.trim() : !url.trim())}>
          {busy ? <Spinner data-icon="inline-start" /> : <IconPlus data-icon="inline-start" />}
          Add and connect
        </Button>
      </div>
    </FieldGroup>
  );
}
