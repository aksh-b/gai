import { useEffect, useState } from 'react';
import { ThumbsDownIcon, ThumbsUpIcon } from 'lucide-react';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { Textarea } from '@/components/ui/textarea';
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group';
import { cn } from '@/lib/utils';
import { api } from './api';

const REASONS = ['Wrong', 'Incomplete', 'Too long', 'Off topic', 'Unsafe', 'Didn’t follow instructions'];

/**
 * Thumbs up or down on a reply. A thumbs down asks what went wrong; the
 * ratings show up on the run, in Assistants and can seed eval sets.
 */
export function ReplyRating({ runId }: { runId: string }) {
  const [rating, setRating] = useState<1 | -1 | 0>(0);
  const [asking, setAsking] = useState(false);
  const [tags, setTags] = useState<string[]>([]);
  const [comment, setComment] = useState('');

  useEffect(() => {
    let live = true;
    api
      .runFeedback(runId)
      .then((f) => {
        if (!live || !f[0]) return;
        setRating(f[0].rating > 0 ? 1 : -1);
        setTags(f[0].tags);
        setComment(f[0].comment ?? '');
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [runId]);

  async function rate(r: 1 | -1) {
    try {
      if (rating === r) {
        await api.unrate(runId);
        setRating(0);
        return;
      }
      await api.rate(runId, r);
      setRating(r);
      if (r === -1) setAsking(true);
    } catch (e) {
      toast.error((e as Error).message);
    }
  }

  async function explain() {
    try {
      await api.rate(runId, -1, comment.trim() || undefined, tags);
      setAsking(false);
      toast.success('Thanks, noted');
    } catch (e) {
      toast.error((e as Error).message);
    }
  }

  return (
    <>
      <Button
        variant="ghost"
        size="icon-sm"
        aria-label="Good reply"
        aria-pressed={rating === 1}
        title="Good reply"
        className={cn(rating === 1 && 'text-success')}
        onClick={() => rate(1)}
      >
        <ThumbsUpIcon className={cn(rating === 1 && 'fill-current')} />
      </Button>
      <Button
        variant="ghost"
        size="icon-sm"
        aria-label="Bad reply"
        aria-pressed={rating === -1}
        title="Bad reply"
        className={cn(rating === -1 && 'text-destructive')}
        onClick={() => rate(-1)}
      >
        <ThumbsDownIcon className={cn(rating === -1 && 'fill-current')} />
      </Button>
      <Dialog open={asking} onOpenChange={setAsking}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>What went wrong?</DialogTitle>
            <DialogDescription>Optional. It helps improve the assistant, and bad replies can become test cases.</DialogDescription>
          </DialogHeader>
          <ToggleGroup
            type="multiple"
            variant="outline"
            size="sm"
            className="flex-wrap justify-start"
            value={tags}
            onValueChange={setTags}
          >
            {REASONS.map((r) => (
              <ToggleGroupItem key={r} value={r.toLowerCase()}>
                {r}
              </ToggleGroupItem>
            ))}
          </ToggleGroup>
          <Textarea
            placeholder="What should it have said?"
            value={comment}
            onChange={(e) => setComment(e.target.value)}
            aria-label="Comment"
          />
          <DialogFooter>
            <Button variant="outline" onClick={() => setAsking(false)}>
              Skip
            </Button>
            <Button onClick={explain}>Send</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
