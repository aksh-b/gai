// The dashboard's language.
//
// Text is written in English and looked up in a catalog per language, keyed
// by the English text itself, so anything not translated yet shows in
// English. Shared components (page headers, cards, buttons, empty states)
// translate the strings they are given; other code calls t().

import type { ReactNode } from 'react';
import de from './locales/de';
import es from './locales/es';
import fr from './locales/fr';
import hi from './locales/hi';
import ja from './locales/ja';
import pt from './locales/pt';
import zh from './locales/zh';

export const LANGUAGES: { code: string; name: string }[] = [
  { code: 'en', name: 'English' },
  { code: 'de', name: 'Deutsch' },
  { code: 'es', name: 'Español' },
  { code: 'fr', name: 'Français' },
  { code: 'pt', name: 'Português' },
  { code: 'hi', name: 'हिन्दी' },
  { code: 'ja', name: '日本語' },
  { code: 'zh', name: '中文' },
];

const CATALOGS: Record<string, Record<string, string>> = { de, es, fr, hi, ja, pt, zh };

/** Languages assistants can be told to answer in (any other code or name works too). */
export const REPLY_LANGUAGES: { value: string; label: string }[] = [
  { value: 'ar', label: 'العربية (Arabic)' },
  { value: 'zh', label: '中文 (Chinese)' },
  { value: 'nl', label: 'Nederlands (Dutch)' },
  { value: 'en', label: 'English' },
  { value: 'fr', label: 'Français (French)' },
  { value: 'de', label: 'Deutsch (German)' },
  { value: 'hi', label: 'हिन्दी (Hindi)' },
  { value: 'id', label: 'Bahasa Indonesia' },
  { value: 'it', label: 'Italiano (Italian)' },
  { value: 'ja', label: '日本語 (Japanese)' },
  { value: 'ko', label: '한국어 (Korean)' },
  { value: 'pl', label: 'Polski (Polish)' },
  { value: 'pt', label: 'Português (Portuguese)' },
  { value: 'pt-BR', label: 'Português do Brasil' },
  { value: 'ru', label: 'Русский (Russian)' },
  { value: 'es', label: 'Español (Spanish)' },
  { value: 'tr', label: 'Türkçe (Turkish)' },
  { value: 'uk', label: 'Українська (Ukrainian)' },
  { value: 'vi', label: 'Tiếng Việt (Vietnamese)' },
];

const KEY = 'gai.language';

function stored(): string | null {
  try {
    return localStorage.getItem(KEY);
  } catch {
    return null;
  }
}

function detect(): string {
  const choice = stored();
  if (choice && LANGUAGES.some((l) => l.code === choice)) return choice;
  for (const l of navigator.languages ?? [navigator.language]) {
    const base = l.toLowerCase().split('-')[0];
    if (LANGUAGES.some((x) => x.code === base)) return base;
  }
  return 'en';
}

/** The dashboard's language for this page load. */
export const language = detect();

if (typeof document !== 'undefined') document.documentElement.lang = language;

/** `text` in the dashboard's language; `{name}` takes `vars.name`. */
export function t(text: string, vars?: Record<string, string | number>): string {
  let out = CATALOGS[language]?.[text] ?? text;
  if (vars) for (const [k, v] of Object.entries(vars)) out = out.replaceAll(`{${k}}`, String(v));
  return out;
}

/** Translate a node when it is plain text. */
export function tr(node: ReactNode): ReactNode {
  return typeof node === 'string' ? t(node) : node;
}

/** Translate the text parts of children: `<Button><Icon /> Save</Button>`. */
export function trChildren(children: ReactNode): ReactNode {
  if (typeof children === 'string') return t(children);
  if (Array.isArray(children)) return children.map((c) => (typeof c === 'string' ? c.replace(c.trim(), t(c.trim())) : c));
  return children;
}

/** Whether the user chose a language here, rather than the browser's. */
export function chosenLanguage(): string | null {
  return stored();
}

/** Switch the dashboard's language (the page reloads to apply it). */
export function setLanguage(code: string | null) {
  try {
    if (code) localStorage.setItem(KEY, code);
    else localStorage.removeItem(KEY);
  } catch {
    /* private mode: it only lasts for this load */
  }
  location.reload();
}
