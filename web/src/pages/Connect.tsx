import { Page, PageHeader, Snippet } from '@/components/shared';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { api } from '../api';
import { useLoad } from '../util';

/** How MCP clients and other agents reach gai. */
export function Connect() {
  const agents = useLoad(() => api.agents(), []);
  const origin = location.origin;
  const names = (agents.data ?? []).map((a) => a.summary.name);

  return (
    <Page className="max-w-4xl">
      <PageHeader
        title="Use from other apps"
        subtitle="Let other AI apps (Claude Desktop, Claude Code, other agents) use your assistants. They sign in with an access key."
      />

      <Card>
        <CardHeader>
          <CardTitle>MCP</CardTitle>
          <CardDescription>
            Stored agents become <code>agent_&lt;name&gt;</code> tools, workflows become <code>workflow_&lt;name&gt;</code>, and knowledge
            collections are resources (plus a <code>search_knowledge</code> tool). Choose what is offered under <code>[server.mcp]</code> in
            gai.toml.
          </CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          <Snippet
            title="Claude Code"
            code={`claude mcp add --transport http gai ${origin}/mcp \\
  --header "Authorization: Bearer <your gai API key>"`}
          />
          <Snippet
            title="Any client that runs a command (stdio)"
            code={JSON.stringify({ mcpServers: { gai: { command: 'gai', args: ['mcp', 'serve'] } } }, null, 2)}
          />
          <p className="text-sm text-muted-foreground">
            On this machine gai runs in a container, so the command is <code>scripts/gai.sh mcp serve</code> from the gai folder. Calls that
            need a tool approval wait in the Approvals inbox.
          </p>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>A2A</CardTitle>
          <CardDescription>
            Other agents discover gai from its agent card and send tasks over JSON-RPC (<code>message/send</code>,{' '}
            <code>message/stream</code>, <code>tasks/get</code>, <code>tasks/cancel</code>). A task is a gai run; its context is the
            conversation.
          </CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          <Snippet title="Card for all agents" code={`${origin}/.well-known/agent-card.json`} />
          {names.length > 0 && (
            <Snippet title="One card per agent" code={names.map((n) => `${origin}/a2a/${n}/.well-known/agent-card.json`).join('\n')} />
          )}
          <Snippet
            title="Try it"
            code={`curl ${origin}/a2a/${names[0] ?? '<agent>'} \\
  -H "Authorization: Bearer <your gai API key>" \\
  -H 'content-type: application/json' \\
  -d '{"jsonrpc":"2.0","id":1,"method":"message/send","params":{"message":{"role":"user","messageId":"1","parts":[{"kind":"text","text":"Hello"}]}}}'`}
          />
          <Snippet
            title="Calling remote A2A agents from gai"
            code={`# gai.toml: each becomes a tool a2a__<name>
[a2a_agents.travel]
url = "https://agents.example.com/.well-known/agent-card.json"
headers = { Authorization = "Bearer \${TRAVEL_TOKEN}" }

# or, in an agent: delegates: [{ a2a: "<endpoint>", name: travel }]`}
          />
        </CardContent>
      </Card>
    </Page>
  );
}
