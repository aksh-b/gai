import { useState } from 'react';
import { Button, Select, TextField, toast } from '@flame';
import { api, type McpServerConfig, type McpServerInfo, type ToolPolicy } from '../api';
import { Disclosure, Empty, ErrorNote, Json, PageHeader } from '../components';
import { IconPlus, IconTrash } from '../icons';
import { useLoad } from '../util';

export function Tools() {
  const tools = useLoad(() => api.tools(), []);
  const mcp = useLoad(() => api.mcp(), []);
  const hooks = useLoad(() => api.hooks(), []);
  const reload = () => {
    tools.reload();
    mcp.reload();
  };

  const builtin = tools.data?.filter((t) => !t.spec.name.startsWith('mcp__')) ?? [];

  return (
    <div className="page">
      <PageHeader
        title="Tools"
        subtitle="What agents can call. Policies: auto runs freely, approve waits for a human, deny refuses. Agents can override them."
      />
      <ErrorNote error={tools.error ?? mcp.error} />

      <section className="card">
        <div className="row between">
          <h3>MCP servers</h3>
          {hooks.data && hooks.data.length > 0 && (
            <span className="muted small">
              Guardrail hooks:{' '}
              {hooks.data.map((h) => (
                <span key={h} className="chip mono small">
                  {h}
                </span>
              ))}
            </span>
          )}
        </div>
        <p className="muted small">
          Tools from Model Context Protocol servers appear as <span className="mono">mcp__server__tool</span>; give an agent{' '}
          <span className="mono">mcp__server__*</span> to use them all.
        </p>
        <div className="mcp-list">
          {mcp.data?.map((s) => (
            <McpCard key={s.name} s={s} onChange={reload} />
          ))}
          {mcp.data?.length === 0 && <Empty title="No MCP servers">Add one below, or declare it under [mcp_servers] in gai.toml.</Empty>}
        </div>
        <Disclosure summary={<strong>Add a server</strong>}>
          <AddMcp onAdded={reload} />
        </Disclosure>
      </section>

      <h3>Built-in tools</h3>
      <div className="tool-list">
        {builtin.map((t) => (
          <section key={t.spec.name} className="card">
            <div className="row between">
              <strong className="mono">{t.spec.name}</strong>
              <PolicyPill policy={t.policy} />
            </div>
            <p className="muted">{t.spec.description}</p>
            <Disclosure summary={<span className="muted small">Input schema</span>}>
              <Json value={t.spec.input_schema} />
            </Disclosure>
          </section>
        ))}
      </div>
    </div>
  );
}

function PolicyPill({ policy }: { policy: ToolPolicy }) {
  return (
    <span className="pill" data-tone={policy === 'auto' ? 'ok' : policy === 'approve' ? 'warn' : 'bad'}>
      {policy}
    </span>
  );
}

function McpCard({ s, onChange }: { s: McpServerInfo; onChange: () => void }) {
  const [busy, setBusy] = useState(false);
  const target = s.config.url ?? [s.config.command, ...(s.config.args ?? [])].filter(Boolean).join(' ');
  const state = s.status.state;
  const tone = state === 'ready' ? 'ok' : state === 'failed' ? 'bad' : state === 'connecting' ? 'busy' : 'warn';
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
    <div className="mcp-card">
      <div className="row between">
        <div className="row gap">
          <strong className="mono">{s.name}</strong>
          <span className="pill" data-tone={tone}>
            {state === 'needs_auth' ? 'needs sign-in' : state}
          </span>
          <span className="chip small">{s.source === 'config' ? 'gai.toml' : 'added here'}</span>
        </div>
        <div className="row gap">
          {state === 'needs_auth' ? (
            <Button size="sm" tone="accent" loading={busy} onClick={signIn}>
              Sign in
            </Button>
          ) : (
            <Button size="sm" variant="outline" loading={busy} onClick={() => act(() => api.reconnectMcp(s.name))}>
              Reconnect
            </Button>
          )}
          {isHttp && state === 'ready' && (
            <Button size="sm" variant="ghost" onClick={() => confirm(`Sign out of ${s.name}?`) && act(() => api.mcpSignOut(s.name))}>
              Sign out
            </Button>
          )}
          {s.source !== 'config' && (
            <Button
              size="sm"
              variant="ghost"
              tone="danger"
              leading={<IconTrash />}
              onClick={() => confirm(`Remove ${s.name}?`) && act(() => api.removeMcp(s.name))}
            >
              Remove
            </Button>
          )}
        </div>
      </div>
      <div className="muted small mono">{target}</div>
      {state === 'failed' && 'message' in s.status && <div className="error-note small">{String(s.status.message)}</div>}
      {state === 'needs_auth' && (
        <div className="muted small">This server needs you to sign in. Sign in opens its approval page in a new tab.</div>
      )}
      {s.tools.length > 0 && (
        <div className="row gap wrap">
          {s.tools.map((t) => (
            <span key={t} className="chip mono small">
              {t.replace(`mcp__${s.name}__`, '')}
            </span>
          ))}
        </div>
      )}
    </div>
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
      kind === 'stdio'
        ? { command: parts[0], args: parts.slice(1), env: lines(pairs) }
        : { url: url.trim(), headers: lines(pairs) };
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
    <div className="mcp-form">
      <TextField label="Name" placeholder="e.g. github" value={name} onChange={(e) => setName(e.target.value)} />
      <Select
        label="Transport"
        value={kind}
        onValueChange={setKind}
        options={[
          { value: 'stdio', label: 'stdio', description: 'Start a local program' },
          { value: 'http', label: 'Streamable HTTP', description: 'Connect to a URL' },
        ]}
      />
      {kind === 'stdio' ? (
        <TextField
          label="Command"
          placeholder="npx -y @modelcontextprotocol/server-filesystem /data"
          value={command}
          onChange={(e) => setCommand(e.target.value)}
        />
      ) : (
        <TextField label="URL" placeholder="https://example.com/mcp" value={url} onChange={(e) => setUrl(e.target.value)} />
      )}
      <div className="full">
        <label className="label" htmlFor="mcp-pairs">
          {kind === 'stdio' ? 'Environment' : 'Headers'} (one NAME=value per line; values may use {'${VAR}'})
        </label>
        <textarea id="mcp-pairs" className="code-editor small-editor" value={pairs} onChange={(e) => setPairs(e.target.value)} />
      </div>
      <Select
        label="Policy"
        value={policy}
        onValueChange={setPolicy}
        options={[
          { value: '', label: 'Default', description: 'auto, except tools the server marks destructive' },
          { value: 'auto', label: 'auto' },
          { value: 'approve', label: 'approve' },
          { value: 'deny', label: 'deny' },
        ]}
      />
      <div className="row end full">
        <Button tone="accent" leading={<IconPlus />} loading={busy} disabled={!name.trim() || (kind === 'stdio' ? !command.trim() : !url.trim())} onClick={add}>
          Add and connect
        </Button>
      </div>
    </div>
  );
}
