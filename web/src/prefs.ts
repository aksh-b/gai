// Per-browser preferences: developer details and the colour theme.

import { useEffect, useState } from 'react';

const DEV_KEY = 'gai.devMode';
const THEME_KEY = 'gai.theme';
const EVENT = 'gai:prefs';

function read(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function write(key: string, value: string | null) {
  try {
    if (value === null) localStorage.removeItem(key);
    else localStorage.setItem(key, value);
  } catch {
    /* storage unavailable: the choice lasts for this page only */
  }
  window.dispatchEvent(new CustomEvent(EVENT));
}

function usePref<T>(get: () => T): T {
  const [value, setValue] = useState(get);
  useEffect(() => {
    const on = () => setValue(get());
    window.addEventListener(EVENT, on);
    window.addEventListener('storage', on);
    return () => {
      window.removeEventListener(EVENT, on);
      window.removeEventListener('storage', on);
    };
  }, [get]);
  return value;
}

const getDev = () => read(DEV_KEY) === '1';

/** Developer details: IDs, raw JSON and YAML, events, token counts, CLI hints. */
export function useDevMode(): [boolean, (on: boolean) => void] {
  return [usePref(getDev), setDevMode];
}

export function setDevMode(on: boolean) {
  write(DEV_KEY, on ? '1' : null);
}

export function isDevMode() {
  return getDev();
}

export type Theme = 'system' | 'light' | 'dark';

const getTheme = (): Theme => {
  const t = read(THEME_KEY);
  return t === 'light' || t === 'dark' ? t : 'system';
};

/** Apply the saved theme to the page (flame reads `data-theme`). */
export function applyTheme(theme: Theme = getTheme()) {
  const root = document.documentElement;
  if (theme === 'system') root.removeAttribute('data-theme');
  else root.setAttribute('data-theme', theme);
}

export function useTheme(): [Theme, (t: Theme) => void] {
  return [
    usePref(getTheme),
    (t: Theme) => {
      write(THEME_KEY, t === 'system' ? null : t);
      applyTheme(t);
    },
  ];
}
