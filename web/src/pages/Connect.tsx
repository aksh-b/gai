import { api } from '../api';
import { PageHeader } from '../components';
import { useLoad } from '../util';

/** How MCP clients and other agents reach gai. */
export function Connect() {
  const agents = useLoad(() => api.agents(), []);
  const origin = location.origin;
  const names = (agents.data ?? []).map((a) => a.summary.name);

  return (
    <div className="page">
      <PageHeader
        title="Use from other apps"
        subtitle="Let other AI apps (Claude Desktop, Claude Code, other agents) use your assistants. They sign in with an access key."
      />

      <section className="card stack">
        <h3>MCP</h3>
        <p className="muted">
          Stored agents become <span className="mono">agent_&lt;name&gt;</span> tools, workflows become{' '}
          <span className="mono">workflow_&lt;name&gt;</span>, and knowledge collections are resources (plus a{' '}
          <span className="mono">search_knowledge</span> tool). Choose what is offered under <span className="mono">[server.mcp]</span> in gai.toml.
        </p>
        <span className="label">Claude Code</span>
        <pre className="code">{`claude mcp add --transport http gai ${origin}/mcp \\
  --header "Authorization: Bearer <your gai API key>"`}</pre>
        <span className="label">Any client that runs a command (stdio)</span>
        <pre className="code">{JSON.stringify({ mcpServers: { gai: { command: 'gai', args: ['mcp', 'serve'] } } }, null, 2)}</pre>
        <p className="muted small">
          On this machine gai runs in a container, so the command is <span className="mono">scripts/gai.sh mcp serve</span> from the gai folder.
          Calls that need a tool approval wait in the Approvals inbox.
        </p>
      </section>

      <section className="card stack">
        <h3>A2A</h3>
        <p className="muted">
          Other agents discover gai from its agent card and send tasks over JSON-RPC (<span className="mono">message/send</span>,{' '}
          <span className="mono">message/stream</span>, <span className="mono">tasks/get</span>, <span className="mono">tasks/cancel</span>). A task is a
          gai run; its context is the conversation.
        </p>
        <span className="label">Card for all agents</span>
        <pre className="code">{`${origin}/.well-known/agent-card.json`}</pre>
        {names.length > 0 && (
          <>
            <span className="label">One card per agent</span>
            <pre className="code">{names.map((n) => `${origin}/a2a/${n}/.well-known/agent-card.json`).join('\n')}</pre>
          </>
        )}
        <span className="label">Try it</span>
        <pre className="code">{`curl ${origin}/a2a/${names[0] ?? '<agent>'} \\
  -H "Authorization: Bearer <your gai API key>" \\
  -H 'content-type: application/json' \\
  -d '{"jsonrpc":"2.0","id":1,"method":"message/send","params":{"message":{"role":"user","messageId":"1","parts":[{"kind":"text","text":"Hello"}]}}}'`}</pre>
        <span className="label">Calling remote A2A agents from gai</span>
        <pre className="code">{`# gai.toml: each becomes a tool a2a__<name>
[a2a_agents.travel]
url = "https://agents.example.com/.well-known/agent-card.json"
headers = { Authorization = "Bearer \${TRAVEL_TOKEN}" }

# or, in an agent: delegates: [{ a2a: "<endpoint>", name: travel }]`}</pre>
      </section>
    </div>
  );
}
