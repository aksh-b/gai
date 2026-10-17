import { useEffect, useRef, useState } from 'react';
import { Button, TextField } from '@flame';
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
        <h2 id="key-title">{hadKey ? 'That key was not accepted' : 'Sign in with an API key'}</h2>
        <p className="muted">
          This server requires an API key. Create one with <span className="mono">gai keys create --name me --scope admin</span> (in this repo:{' '}
          <span className="mono">scripts/gai.sh keys create --name me --scope admin</span>) and paste it here. It is kept in this browser only.
        </p>
        <TextField label="API key" placeholder="gai_…" value={value} onChange={(e) => setValue(e.target.value)} autoFocus />
        <div className="row gap end">
          {hadKey && (
            <Button
              type="button"
              variant="ghost"
              onClick={() => {
                setApiKey(null);
                location.reload();
              }}
            >
              Forget saved key
            </Button>
          )}
          <Button type="submit" tone="accent" disabled={!value.trim().startsWith('gai_')}>
            Continue
          </Button>
        </div>
      </form>
    </dialog>
  );
}
