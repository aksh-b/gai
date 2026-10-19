import { useEffect, useRef, useState } from 'react';
import { Button } from '@/components/ui/button';
import { TextField } from '@/components/fields';
import { getApiKey, setApiKey } from './api';

/** Asks for an API key whenever the server answers 401. */
export function KeyDialog() {
  const ref = useRef<HTMLDialogElement>(null);
  const [value, setValue] = useState('');
  const [hadKey, setHadKey] = useState(false);

  useEffect(() => {
    const open = () => {
      if (ref.current?.open) return;
      setHadKey(!!getApiKey());
      setValue('');
      ref.current?.showModal();
    };
    window.addEventListener('gai:unauthorized', open);
    return () => window.removeEventListener('gai:unauthorized', open);
  }, []);

  return (
    <dialog ref={ref} className="key-dialog" aria-labelledby="key-title">
      <form
        method="dialog"
        className="stack"
        onSubmit={() => {
          setApiKey(value);
          location.reload();
        }}
      >
        <h2 id="key-title">{hadKey ? 'That key was not accepted' : 'Sign in'}</h2>
        <p className="muted">
          This gai server is shared, so it needs an access key. Ask whoever runs it for one, and paste it here. It is kept in this browser
          only.
        </p>
        <details className="small muted">
          <summary>I run this server</summary>
          <p>
            Make a key on the server with <span className="mono">gai keys create --name me --scope admin</span> (in the repo:{' '}
            <span className="mono">scripts/gai.sh keys create --name me --scope admin</span>). Opening the dashboard on the server's own
            computer, at <span className="mono">http://localhost</span>, signs you in automatically.
          </p>
        </details>
        <TextField label="Access key" placeholder="gai_…" value={value} onChange={(e) => setValue(e.target.value)} autoFocus />
        <div className="row gap end">
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
        </div>
      </form>
    </dialog>
  );
}
