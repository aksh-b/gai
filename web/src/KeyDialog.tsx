import { useEffect, useState } from 'react';
import { TextField } from '@/components/fields';
import { Disclosure } from '@/components/shared';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { getApiKey, setApiKey } from './api';

/** Asks for an API key whenever the server answers 401. */
export function KeyDialog() {
  const [open, setOpen] = useState(false);
  const [value, setValue] = useState('');
  const [hadKey, setHadKey] = useState(false);

  useEffect(() => {
    const ask = () => {
      setHadKey(!!getApiKey());
      setValue('');
      setOpen(true);
    };
    window.addEventListener('gai:unauthorized', ask);
    return () => window.removeEventListener('gai:unauthorized', ask);
  }, []);

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogContent showCloseButton={false} onInteractOutside={(e) => e.preventDefault()}>
        <form
          className="flex flex-col gap-4"
          onSubmit={(e) => {
            e.preventDefault();
            setApiKey(value);
            location.reload();
          }}
        >
          <DialogHeader>
            <DialogTitle>{hadKey ? 'That key was not accepted' : 'Sign in'}</DialogTitle>
            <DialogDescription>
              This gai server is shared, so it needs an access key. Ask whoever runs it for one, and paste it here. It is kept in this
              browser only.
            </DialogDescription>
          </DialogHeader>
          <Disclosure summary="I run this server">
            <p className="text-sm text-muted-foreground">
              Make a key on the server with <code>gai keys create --name me --scope admin</code> (in the repo:{' '}
              <code>scripts/gai.sh keys create --name me --scope admin</code>). Opening the dashboard on the server's own computer, at{' '}
              <code>http://localhost</code>, signs you in automatically.
            </p>
          </Disclosure>
          <TextField label="Access key" placeholder="gai_…" value={value} onChange={(e) => setValue(e.target.value)} autoFocus />
          <DialogFooter>
            {hadKey && (
              <Button
                variant="ghost"
                type="button"
                onClick={() => {
                  setApiKey(null);
                  location.reload();
                }}
              >
                Forget saved key
              </Button>
            )}
            <Button type="submit" disabled={!value.trim().startsWith('gai_')}>
              Continue
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
