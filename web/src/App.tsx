import { useEffect, useMemo, useState, type ComponentType, type SVGProps } from 'react';
import { PlugZapIcon, UnplugIcon } from 'lucide-react';
import { CommandMenu, type CommandEntry } from '@/components/command-menu';
import { TabBar } from '@/components/fields';
import { Avatar, AvatarFallback } from '@/components/ui/avatar';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Item, ItemActions, ItemContent, ItemDescription, ItemMedia, ItemTitle } from '@/components/ui/item';
import { Kbd } from '@/components/ui/kbd';
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupContent,
  SidebarHeader,
  SidebarInset,
  SidebarMenu,
  SidebarMenuBadge,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarProvider,
  SidebarTrigger,
} from '@/components/ui/sidebar';
import { Toaster } from '@/components/ui/sonner';
import { Spinner } from '@/components/ui/spinner';
import { api, getApiKey, getOrg, setApiKey, setOrg, type Me, type OrgSettings } from './api';
import { SelectField } from '@/components/fields';
import { KeyDialog } from './KeyDialog';
import { modelLabel } from './labels';
import { applyTheme, isDark, setDevMode, useDevMode, useTheme, type Theme } from './prefs';
import { navigate, useRoute } from './router';
import { IconAgent, IconBook, IconData, IconChat, IconFlow, IconGear, IconMoon, IconRuns, IconSearch, IconStack, IconSun } from './icons';
import { MediaStudio } from './pages/MediaStudio';
import { Triggers } from './pages/Triggers';
import { Connect } from './pages/Connect';
import { Playground } from './pages/Playground';
import { Runs } from './pages/Runs';
import { RunDetail } from './pages/RunDetail';
import { Compare } from './pages/Compare';
import { Agents } from './pages/Agents';
import { Tools } from './pages/Tools';
import { UsagePage } from './pages/Usage';
import { Keys } from './pages/Keys';
import { Approvals } from './pages/Approvals';
import { Knowledge } from './pages/Knowledge';
import { Memory } from './pages/Memory';
import { Personas } from './pages/Personas';
import { Evals } from './pages/Evals';
import { Audit } from './pages/Audit';
import { People } from './pages/People';
import { DataPage } from './pages/Data';
import { AgentAnalytics, ConversationSearch } from './pages/Insights';
import { Skills } from './pages/Skills';
import { Workflows } from './pages/Workflows';
import { Welcome, welcomeDone } from './pages/Welcome';
import { ModelsSettings, Preferences } from './pages/Settings';

type Icon = ComponentType<SVGProps<SVGSVGElement>>;

/** Top-level sections. Each owns some route ids (old ones keep working). */
const NAV: { id: string; label: string; icon: Icon; routes: string[]; tabs?: { id: string; label: string }[] }[] = [
  { id: 'chat', label: 'Chat', icon: IconChat, routes: ['chat', 'playground'] },
  {
    id: 'assistants',
    label: 'Assistants',
    icon: IconAgent,
    routes: ['assistants', 'agents', 'personas', 'memory', 'skills', 'evals'],
    tabs: [
      { id: 'assistants', label: 'Assistants' },
      { id: 'personas', label: 'Personas' },
      { id: 'memory', label: 'Memory' },
      { id: 'skills', label: 'Skills' },
      { id: 'evals', label: 'Tests' },
    ],
  },
  {
    id: 'automations',
    label: 'Automations',
    icon: IconFlow,
    routes: ['automations', 'workflows', 'schedules', 'triggers'],
    tabs: [
      { id: 'automations', label: 'Workflows' },
      { id: 'schedules', label: 'Schedules & webhooks' },
    ],
  },
  { id: 'knowledge', label: 'Knowledge', icon: IconStack, routes: ['knowledge'] },
  { id: 'data', label: 'Data', icon: IconData, routes: ['data'] },
  {
    id: 'activity',
    label: 'Activity',
    icon: IconRuns,
    routes: ['activity', 'runs', 'search', 'analytics', 'approvals', 'usage'],
    tabs: [
      { id: 'activity', label: 'History' },
      { id: 'search', label: 'Search chats' },
      { id: 'analytics', label: 'By assistant' },
      { id: 'approvals', label: 'Needs your OK' },
      { id: 'usage', label: 'Spending' },
    ],
  },
  {
    id: 'settings',
    label: 'Settings',
    icon: IconGear,
    routes: ['settings', 'models', 'tools', 'media', 'people', 'keys', 'audit', 'connect', 'preferences'],
    tabs: [
      { id: 'settings', label: 'AI models' },
      { id: 'tools', label: 'Tools & connections' },
      { id: 'media', label: 'Voice & images' },
      { id: 'people', label: 'People & teams' },
      { id: 'keys', label: 'Access keys' },
      { id: 'audit', label: 'Audit & data' },
      { id: 'connect', label: 'Use from other apps' },
      { id: 'preferences', label: 'Preferences' },
    ],
  },
];

/** Route ids that are one tab's alias for another. */
const TAB_ALIAS: Record<string, string> = {
  agents: 'assistants',
  workflows: 'automations',
  triggers: 'schedules',
  runs: 'activity',
  models: 'settings',
  playground: 'chat',
};

type Boot = 'loading' | 'ready';

export function App() {
  const route = useRoute();
  const raw = route.parts[0] ?? 'chat';
  const [boot, setBoot] = useState<Boot>('loading');
  const [health, setHealth] = useState<{ ok: boolean; version?: string; model?: string | null; auth?: string }>({ ok: true });
  const [pending, setPending] = useState(0);
  const [palette, setPalette] = useState(false);
  const [agents, setAgents] = useState<string[]>([]);
  const [dev] = useDevMode();
  const [me, setMe] = useState<Me>();
  const [theme, setTheme] = useTheme();

  useEffect(() => {
    applyTheme();
  }, []);

  // A new page starts at the top (the window scrolls, not a panel).
  const path = route.parts.join('/');
  useEffect(() => {
    window.scrollTo(0, 0);
  }, [path]);

  // Sign this browser in when the server allows it, then send a brand-new
  // install to the welcome screen. Pages load only after this, so they never
  // trip the key dialog on the way.
  useEffect(() => {
    (async () => {
      try {
        let status = await api.setupStatus();
        if (status.needs_key && status.can_claim) {
          const claimed = await api.claim();
          setApiKey(claimed.secret);
          status = await api.setupStatus();
        }
        if (status.needs_key) {
          window.dispatchEvent(new CustomEvent('gai:unauthorized'));
        } else if (!status.has_chat_model && !welcomeDone() && (raw === 'chat' || raw === 'playground')) {
          navigate('/welcome');
        }
      } catch {
        /* server offline: the footer says so */
      }
      setBoot('ready');
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Who this is, and how their organization brands the dashboard.
  useEffect(() => {
    if (boot !== 'ready') return;
    api
      .me()
      .then((m) => {
        setMe(m);
        const accent = (m.org.settings as OrgSettings | null)?.branding?.accent;
        if (accent && CSS.supports('color', accent)) document.documentElement.style.setProperty('--primary', accent);
        document.title = brandName(m);
      })
      .catch(() => {});
  }, [boot]);

  useEffect(() => {
    if (boot !== 'ready') return;
    const poll = () =>
      api
        .approvals()
        .then((a) => setPending(a.length))
        .catch(() => {});
    poll();
    const t = setInterval(poll, 4000);
    return () => clearInterval(t);
  }, [boot]);

  useEffect(() => {
    const check = () =>
      api
        .health()
        .then((h) => setHealth({ ok: true, version: h.version, model: h.default_model, auth: h.auth }))
        .catch(() => setHealth({ ok: false }));
    check();
    const t = setInterval(check, 15_000);
    return () => clearInterval(t);
  }, [route]);

  useEffect(() => {
    if (palette)
      api
        .agents()
        .then((a) => setAgents(a.map((x) => x.summary.name)))
        .catch(() => {});
  }, [palette]);

  const section = NAV.find((n) => n.routes.includes(raw))?.id ?? (raw === 'welcome' ? 'welcome' : 'chat');
  const tab = TAB_ALIAS[raw] ?? raw;
  const nav = NAV.find((n) => n.id === section);

  const commands = useMemo<CommandEntry[]>(() => {
    const go = (to: string) => () => navigate(to);
    const pages: CommandEntry[] = NAV.flatMap((n) =>
      (n.tabs ?? [{ id: n.id, label: n.label }]).map((t) => ({
        id: `go-${t.id}`,
        group: 'Go to',
        label: n.tabs && t.label !== n.label ? `${n.label} › ${t.label}` : n.label,
        onSelect: go(`/${t.id}`),
      })),
    );
    const themes: { id: Theme; label: string }[] = [
      { id: 'system', label: 'Theme: match the system' },
      { id: 'light', label: 'Theme: light' },
      { id: 'dark', label: 'Theme: dark' },
    ];
    return [
      { id: 'new-chat', group: 'Do', label: 'New chat', onSelect: go('/chat'), keywords: ['message', 'talk'] },
      { id: 'new-assistant', group: 'Do', label: 'New assistant', onSelect: go('/assistants?new=1'), keywords: ['agent', 'create'] },
      { id: 'new-persona', group: 'Do', label: 'New persona', onSelect: go('/personas?new=1'), keywords: ['voice', 'tone', 'character'] },
      {
        id: 'add-knowledge',
        group: 'Do',
        label: 'Add documents to Knowledge',
        onSelect: go('/knowledge'),
        keywords: ['upload', 'files', 'rag'],
      },
      {
        id: 'add-model',
        group: 'Do',
        label: 'Connect an AI model',
        onSelect: go('/welcome'),
        keywords: ['key', 'ollama', 'provider', 'setup'],
      },
      ...pages,
      ...agents.map((a) => ({
        id: `agent-${a}`,
        group: 'Chat with',
        label: a,
        onSelect: go(`/chat?agent=${encodeURIComponent(a)}`),
      })),
      {
        id: 'dev',
        group: 'Preferences',
        label: dev ? 'Hide developer details' : 'Show developer details',
        keywords: ['ids', 'json', 'yaml', 'advanced', 'debug'],
        onSelect: () => setDevMode(!dev),
      },
      ...themes.map((t) => ({ id: `theme-${t.id}`, group: 'Preferences', label: t.label, onSelect: () => setTheme(t.id) })),
    ];
  }, [agents, dev, setTheme]);

  let page;
  if (boot === 'loading') {
    page = (
      <div className="flex flex-1 items-center justify-center" aria-busy="true">
        <Spinner />
      </div>
    );
  } else {
    switch (tab) {
      case 'welcome':
        page = <Welcome />;
        break;
      case 'activity':
        page =
          route.parts[2] === 'compare' && route.parts[3] ? (
            <Compare a={route.parts[1]} b={route.parts[3]} />
          ) : route.parts[1] ? (
            <RunDetail id={route.parts[1]} />
          ) : (
            <Runs />
          );
        break;
      case 'assistants':
        page = (
          <Agents selected={route.parts[1] ? decodeURIComponent(route.parts[1]) : undefined} creating={route.query.get('new') === '1'} />
        );
        break;
      case 'automations':
        page = <Workflows selected={route.parts[1] ? decodeURIComponent(route.parts[1]) : undefined} />;
        break;
      case 'data':
        page = <DataPage selected={route.parts[1] ? decodeURIComponent(route.parts[1]) : undefined} />;
        break;
      case 'knowledge':
        page = <Knowledge selected={route.parts[1] ? decodeURIComponent(route.parts[1]) : undefined} />;
        break;
      case 'personas':
        page = (
          <Personas selected={route.parts[1] ? decodeURIComponent(route.parts[1]) : undefined} creating={route.query.get('new') === '1'} />
        );
        break;
      case 'memory':
        page = <Memory />;
        break;
      case 'skills':
        page = <Skills />;
        break;
      case 'evals':
        page = <Evals parts={route.parts.slice(1)} />;
        break;
      case 'media':
        page = <MediaStudio />;
        break;
      case 'schedules':
        page = <Triggers selected={route.parts[1]} />;
        break;
      case 'connect':
        page = <Connect />;
        break;
      case 'settings':
        page = <ModelsSettings />;
        break;
      case 'preferences':
        page = <Preferences />;
        break;
      case 'tools':
        page = <Tools />;
        break;
      case 'usage':
        page = <UsagePage />;
        break;
      case 'people':
        page = <People />;
        break;
      case 'search':
        page = <ConversationSearch />;
        break;
      case 'analytics':
        page = <AgentAnalytics />;
        break;
      case 'keys':
        page = <Keys />;
        break;
      case 'audit':
        page = <Audit />;
        break;
      case 'approvals':
        page = <Approvals />;
        break;
      default:
        page = <Playground route={route} />;
    }
  }

  // Sub-pages (a run, an agent) keep their section's tabs but no tab is lit.
  const tabs = nav?.tabs && boot === 'ready' && (
    <TabBar
      variant="underline"
      aria-label={nav.label}
      items={nav.tabs.map((t) => ({
        id: t.id,
        label: t.label,
        badge: t.id === 'approvals' && pending > 0 ? <Badge>{pending}</Badge> : undefined,
      }))}
      value={tab}
      onValueChange={(id) => navigate(`/${id}`)}
    />
  );

  const signedIn = !!getApiKey() || !!me?.user;
  const brand = brandName(me);
  const logo = (me?.org.settings as OrgSettings | null)?.branding?.logo_url;
  // A signed-in user in several organizations picks one; a key has its own.
  const orgChoices = !getApiKey() && me && me.memberships.length > 1 ? me.memberships : null;

  return (
    <SidebarProvider>
      <Sidebar>
        <SidebarHeader>
          <SidebarMenu>
            <SidebarMenuItem>
              <SidebarMenuButton size="lg" asChild>
                <a href="#/chat">
                  <Avatar className="rounded-lg">
                    {logo ? (
                      <img src={logo} alt="" className="size-full rounded-lg object-contain" />
                    ) : (
                      <AvatarFallback className="rounded-lg bg-primary font-semibold text-primary-foreground">
                        {brand.charAt(0).toLowerCase()}
                      </AvatarFallback>
                    )}
                  </Avatar>
                  <span className="truncate text-base font-semibold">{brand}</span>
                </a>
              </SidebarMenuButton>
            </SidebarMenuItem>
            {orgChoices && (
              <SidebarMenuItem>
                <SelectField
                  aria-label="Organization"
                  options={orgChoices.map((m) => ({ value: m.org_slug, label: m.org_name }))}
                  value={getOrg() ?? me?.org.slug ?? ''}
                  onValueChange={(v) => {
                    setOrg(String(v));
                    location.assign('#/chat');
                    location.reload();
                  }}
                />
              </SidebarMenuItem>
            )}
            <SidebarMenuItem>
              <SidebarMenuButton variant="outline" onClick={() => setPalette(true)}>
                <IconSearch />
                <span>Search or jump to…</span>
                <Kbd className="ml-auto">Ctrl K</Kbd>
              </SidebarMenuButton>
            </SidebarMenuItem>
          </SidebarMenu>
        </SidebarHeader>
        <SidebarContent>
          <SidebarGroup>
            <SidebarGroupContent>
              <SidebarMenu aria-label="Main">
                {NAV.map(({ id, label, icon: Icon }) => (
                  <SidebarMenuItem key={id}>
                    <SidebarMenuButton asChild isActive={section === id} tooltip={label}>
                      <a
                        href={`#/${id}`}
                        aria-current={section === id ? 'page' : undefined}
                        onClick={(e) => {
                          e.preventDefault();
                          navigate(`/${id}`);
                        }}
                      >
                        <Icon />
                        <span>{label}</span>
                      </a>
                    </SidebarMenuButton>
                    {id === 'activity' && pending > 0 && (
                      <SidebarMenuBadge title={`${pending} waiting for your OK`}>{pending}</SidebarMenuBadge>
                    )}
                  </SidebarMenuItem>
                ))}
                {dev && (
                  <SidebarMenuItem>
                    <SidebarMenuButton asChild>
                      <a href="/docs" target="_blank" rel="noreferrer">
                        <IconBook />
                        <span>API docs</span>
                      </a>
                    </SidebarMenuButton>
                  </SidebarMenuItem>
                )}
              </SidebarMenu>
            </SidebarGroupContent>
          </SidebarGroup>
        </SidebarContent>
        <SidebarFooter>
          <Item size="xs">
            <ItemMedia variant="icon">{health.ok ? <PlugZapIcon /> : <UnplugIcon className="text-destructive" />}</ItemMedia>
            <ItemContent className="min-w-0">
              <ItemTitle>{health.ok ? 'Connected' : 'Server offline'}</ItemTitle>
              {health.model && (
                <ItemDescription className="truncate">
                  <a href="#/settings" title={dev ? health.model : 'Change in Settings'}>
                    {dev ? health.model : modelLabel(health.model)}
                  </a>
                </ItemDescription>
              )}
              {me?.user && <ItemDescription className="truncate">{me.user.email}</ItemDescription>}
              {dev && (
                <ItemDescription>
                  {health.version && `v${health.version} · `}
                  {health.auth === 'open' ? 'open (no keys yet)' : signedIn ? 'signed in with a key' : 'key required'}
                </ItemDescription>
              )}
            </ItemContent>
            <ItemActions>
              {((dev && getApiKey()) || me?.user) && (
                <Button
                  variant="link"
                  size="xs"
                  onClick={async () => {
                    setApiKey(null);
                    setOrg(null);
                    if (me?.user) await api.logout().catch(() => {});
                    location.reload();
                  }}
                >
                  Sign out
                </Button>
              )}
              <Button
                variant="ghost"
                size="icon-sm"
                aria-label={`Theme: ${theme}. Switch theme`}
                title="Switch light / dark"
                onClick={() => setTheme(isDark(theme) ? 'light' : 'dark')}
              >
                {isDark(theme) ? <IconSun /> : <IconMoon />}
              </Button>
            </ItemActions>
          </Item>
        </SidebarFooter>
      </Sidebar>
      <SidebarInset className="min-w-0">
        <header className="flex h-12 items-center gap-2 border-b px-3 md:hidden">
          <SidebarTrigger />
          <span className="font-semibold">{brand}</span>
        </header>
        {tabs && <div className="sticky top-0 z-10 border-b bg-background px-6 pt-2 md:px-8">{tabs}</div>}
        {page}
      </SidebarInset>
      <CommandMenu open={palette} onOpenChange={setPalette} items={commands} placeholder="Search pages, assistants and actions…" />
      <Toaster position="bottom-right" theme={theme} />
      <KeyDialog />
    </SidebarProvider>
  );
}

/** The product name the organization shows, else gai. */
function brandName(me?: Me) {
  return (me?.org.settings as OrgSettings | null)?.branding?.product_name?.trim() || 'gai';
}
