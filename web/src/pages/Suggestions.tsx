import { LightbulbIcon } from 'lucide-react';
import { toast } from 'sonner';
import { EmptyState, ErrorNote, Markdown, Page, PageHeader } from '@/components/shared';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from '@/components/ui/card';
import { api, type Suggestion } from '../api';
import { navigate } from '../router';
import { ago, useLoad } from '../util';

function actionLabel(s: Suggestion): string {
  switch (s.action?.kind) {
    case 'run_agent':
      return `Ask ${s.action.agent}`;
    case 'install_template':
      return 'Add it';
    case 'open':
      return 'Open';
    default:
      return 'Done';
  }
}

/** Activity → Suggestions: what gai and proactive assistants suggest doing. */
export function SuggestionsPage() {
  const list = useLoad(() => api.suggestions(), []);

  async function accept(s: Suggestion) {
    try {
      const a = await api.acceptSuggestion(s.id);
      if (a.run_id) toast.success(`Started: ${s.action?.kind === 'run_agent' ? s.action.agent : 'run'}`);
      else if (a.installed) toast.success(`Added ${a.installed}`);
      list.reload();
      if (a.link) navigate(a.link);
    } catch (e) {
      toast.error((e as Error).message);
    }
  }

  async function dismiss(s: Suggestion) {
    try {
      await api.dismissSuggestion(s.id);
      list.reload();
    } catch (e) {
      toast.error((e as Error).message);
    }
  }

  return (
    <Page className="max-w-4xl">
      <PageHeader
        title="Suggestions"
        subtitle="Things worth doing, from how you use gai and from assistants that watch your data on a schedule."
      />
      <ErrorNote error={list.error} />
      {list.data?.length === 0 && (
        <EmptyState title="Nothing to suggest" icon={<LightbulbIcon />}>
          Add the Proactive watcher assistant and give it a schedule: it checks your data and suggests what to do here.
        </EmptyState>
      )}
      {list.data?.map((s) => (
        <Card key={s.id}>
          <CardHeader>
            <CardTitle>{s.title}</CardTitle>
            <CardDescription className="flex flex-wrap items-center gap-2">
              <Badge variant="secondary">{s.source === 'gai' ? 'From your usage' : `From ${s.source}`}</Badge>
              {ago(s.created_at)}
              {s.run_id && <a href={`#/runs/${s.run_id}`}>See how it found this</a>}
            </CardDescription>
          </CardHeader>
          {s.details && (
            <CardContent>
              <Markdown text={s.details} />
            </CardContent>
          )}
          <CardFooter className="gap-2">
            <Button onClick={() => accept(s)}>{actionLabel(s)}</Button>
            <Button variant="ghost" onClick={() => dismiss(s)}>
              Dismiss
            </Button>
          </CardFooter>
        </Card>
      ))}
    </Page>
  );
}
