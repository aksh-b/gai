// The CodeMirror editor behind <CodeEditor>. Loaded on demand: it is the
// largest part of the dashboard, and most pages never need it.

import { useEffect, useRef } from 'react';
import { autocompletion, type CompletionContext, type CompletionResult } from '@codemirror/autocomplete';
import { markdown } from '@codemirror/lang-markdown';
import { yaml } from '@codemirror/lang-yaml';
import { HighlightStyle, syntaxHighlighting } from '@codemirror/language';
import { linter, lintGutter, type Diagnostic } from '@codemirror/lint';
import { Compartment, EditorState, type Extension } from '@codemirror/state';
import { EditorView, placeholder as placeholderExt } from '@codemirror/view';
import { basicSetup } from 'codemirror';
import { tags as t } from '@lezer/highlight';
import Ajv2020, { type ErrorObject, type ValidateFunction } from 'ajv/dist/2020';
import { isMap, isNode, isScalar, parseDocument, type Document } from 'yaml';

import type { CodeEditorProps } from './code-editor';

/** Colours from the theme tokens, so the editor follows light and dark. */
const theme = EditorView.theme({
  '&': {
    color: 'var(--foreground)',
    backgroundColor: 'transparent',
    fontSize: '13px',
  },
  '.cm-content': { fontFamily: 'var(--font-mono)', caretColor: 'var(--foreground)' },
  '.cm-scroller': { fontFamily: 'var(--font-mono)', lineHeight: '1.55' },
  '&.cm-focused': { outline: 'none' },
  '.cm-gutters': {
    backgroundColor: 'transparent',
    color: 'var(--muted-foreground)',
    borderRight: '1px solid var(--border)',
  },
  '.cm-activeLine, .cm-activeLineGutter': { backgroundColor: 'color-mix(in oklch, var(--muted) 60%, transparent)' },
  '&.cm-focused .cm-selectionBackground, .cm-selectionBackground, ::selection': {
    backgroundColor: 'color-mix(in oklch, var(--primary) 22%, transparent) !important',
  },
  '.cm-cursor': { borderLeftColor: 'var(--foreground)' },
  '.cm-tooltip': {
    backgroundColor: 'var(--popover)',
    color: 'var(--popover-foreground)',
    border: '1px solid var(--border)',
    borderRadius: 'var(--radius-md)',
  },
  '.cm-tooltip-autocomplete ul li[aria-selected]': {
    backgroundColor: 'var(--accent)',
    color: 'var(--accent-foreground)',
  },
  '.cm-panels': { backgroundColor: 'var(--muted)', color: 'var(--foreground)' },
  '.cm-placeholder': { color: 'var(--muted-foreground)' },
});

/** Token colours from --code-* in index.css, set for light and dark. */
const highlight = syntaxHighlighting(
  HighlightStyle.define([
    { tag: [t.propertyName, t.definition(t.propertyName), t.attributeName], color: 'var(--code-key)' },
    { tag: [t.string, t.special(t.string), t.content], color: 'var(--code-string)' },
    { tag: [t.number, t.bool, t.null, t.atom], color: 'var(--code-number)' },
    { tag: [t.keyword, t.typeName, t.labelName, t.meta], color: 'var(--code-keyword)' },
    { tag: [t.comment, t.lineComment], color: 'var(--code-comment)', fontStyle: 'italic' },
    { tag: t.heading, color: 'var(--code-key)', fontWeight: '600' },
    { tag: [t.link, t.url], color: 'var(--code-string)', textDecoration: 'underline' },
    { tag: t.emphasis, fontStyle: 'italic' },
    { tag: t.strong, fontWeight: '600' },
    { tag: [t.punctuation, t.separator], color: 'var(--muted-foreground)' },
  ]),
);

// ---- schemas ----

type Schema = { properties?: Record<string, Schema & { description?: string }>; $ref?: string; [k: string]: unknown };

let openapi: Promise<{ components?: { schemas?: Record<string, Schema> } } | undefined> | undefined;
const validators = new Map<string, ValidateFunction | undefined>();
let ajv: Ajv2020 | undefined;

/** The server's OpenAPI document, which holds the spec schemas. */
function loadOpenapi() {
  openapi ??= fetch('/openapi.json')
    .then((r) => (r.ok ? r.json() : undefined))
    .catch(() => undefined);
  return openapi;
}

async function validator(name: string): Promise<ValidateFunction | undefined> {
  if (validators.has(name)) return validators.get(name);
  const doc = await loadOpenapi();
  let v: ValidateFunction | undefined;
  if (doc?.components?.schemas?.[name]) {
    try {
      if (!ajv) {
        ajv = new Ajv2020({ strict: false, allErrors: true, validateFormats: false });
        ajv.addSchema(doc, 'openapi');
      }
      v = ajv.compile({ $ref: `openapi#/components/schemas/${name}` });
    } catch {
      v = undefined;
    }
  }
  validators.set(name, v);
  return v;
}

/** The schema of `name`'s top-level keys, for completion. */
async function topKeys(name: string): Promise<Record<string, { description?: string }>> {
  const doc = await loadOpenapi();
  const all = doc?.components?.schemas ?? {};
  let s: Schema | undefined = all[name];
  for (let i = 0; i < 5 && s?.$ref; i++) s = all[String(s.$ref).split('/').pop() ?? ''];
  return s?.properties ?? {};
}

/** Where a schema error points in the YAML. */
function rangeOf(doc: Document, e: ErrorObject, length: number): [number, number] {
  const path = e.instancePath
    .split('/')
    .slice(1)
    .map((p) => p.replace(/~1/g, '/').replace(/~0/g, '~'))
    .map((p) => (/^\d+$/.test(p) ? Number(p) : p));
  // An unknown key: point at the key itself.
  if (e.keyword === 'additionalProperties') {
    const parent = path.length ? doc.getIn(path, true) : doc.contents;
    const key = (e.params as { additionalProperty?: string }).additionalProperty;
    if (isMap(parent)) {
      const pair = parent.items.find((p) => isScalar(p.key) && p.key.value === key);
      if (pair && isNode(pair.key) && pair.key.range) return [pair.key.range[0], pair.key.range[1]];
    }
  }
  for (let n = path.length; n >= 0; n--) {
    const node = n ? doc.getIn(path.slice(0, n), true) : doc.contents;
    if (isNode(node) && node.range) return [node.range[0], Math.max(node.range[0] + 1, Math.min(node.range[1], length))];
  }
  return [0, Math.min(1, length)];
}

function message(e: ErrorObject): string {
  switch (e.keyword) {
    case 'additionalProperties':
      return `Unknown key "${(e.params as { additionalProperty: string }).additionalProperty}"`;
    case 'required':
      return `Missing "${(e.params as { missingProperty: string }).missingProperty}"`;
    case 'enum':
      return `Must be one of: ${((e.params as { allowedValues: unknown[] }).allowedValues ?? []).join(', ')}`;
    default:
      return e.message ? e.message.charAt(0).toUpperCase() + e.message.slice(1) : 'Not valid here';
  }
}

function yamlLinter(schema?: string) {
  return linter(
    async (view) => {
      const text = view.state.doc.toString();
      if (!text.trim()) return [];
      const doc = parseDocument(text, { prettyErrors: false });
      const out: Diagnostic[] = doc.errors.map((e) => ({
        from: Math.min(e.pos[0], text.length),
        to: Math.min(Math.max(e.pos[1], e.pos[0] + 1), text.length),
        severity: 'error',
        message: e.message.split('\n')[0],
      }));
      if (out.length || !schema) return out;
      const v = await validator(schema);
      if (!v || v(doc.toJS())) return out;
      // anyOf/oneOf report every branch; the first few errors say enough.
      const seen = new Set<string>();
      for (const e of v.errors ?? []) {
        if (e.keyword === 'anyOf' || e.keyword === 'oneOf' || e.keyword === 'if') continue;
        const [from, to] = rangeOf(doc, e, text.length);
        const msg = message(e);
        const k = `${from}:${msg}`;
        if (seen.has(k)) continue;
        seen.add(k);
        out.push({ from, to, severity: 'warning', message: msg });
        if (out.length >= 8) break;
      }
      return out;
    },
    { delay: 400 },
  );
}

function keyCompletions(schema: string) {
  return async (ctx: CompletionContext): Promise<CompletionResult | null> => {
    const line = ctx.state.doc.lineAt(ctx.pos);
    const before = line.text.slice(0, ctx.pos - line.from);
    // Top-level keys only: the line starts at column 0 and has no colon yet.
    if (!/^[A-Za-z_]*$/.test(before)) return null;
    if (!before && !ctx.explicit) return null;
    const keys = await topKeys(schema);
    const used = new Set(
      ctx.state.doc
        .toString()
        .split('\n')
        .map((l) => /^([A-Za-z_][\w-]*):/.exec(l)?.[1])
        .filter(Boolean),
    );
    return {
      from: line.from,
      options: Object.entries(keys)
        .filter(([k]) => !used.has(k))
        .map(([k, s]) => ({ label: k, apply: `${k}: `, type: 'property', info: s.description })),
      validFor: /^[A-Za-z_]*$/,
    };
  };
}

function languageFor(p: CodeEditorProps): Extension[] {
  switch (p.language) {
    case 'yaml':
      return [
        yaml(),
        yamlLinter(p.schema),
        lintGutter(),
        ...(p.schema ? [autocompletion({ override: [keyCompletions(p.schema)] })] : []),
      ];
    case 'markdown':
      return [markdown(), EditorView.lineWrapping];
    default:
      return [EditorView.lineWrapping];
  }
}

export default function CodeEditorImpl(props: CodeEditorProps) {
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView>(null);
  const onChange = useRef(props.onChange);
  onChange.current = props.onChange;
  const readOnly = useRef(new Compartment());

  useEffect(() => {
    const v = new EditorView({
      parent: host.current!,
      state: EditorState.create({
        doc: props.value,
        extensions: [
          basicSetup,
          theme,
          highlight,
          ...languageFor(props),
          readOnly.current.of(EditorState.readOnly.of(!!props.readOnly)),
          EditorView.contentAttributes.of({ 'aria-label': props['aria-label'] ?? 'Code editor' }),
          ...(props.placeholder ? [placeholderExt(props.placeholder)] : []),
          EditorView.updateListener.of((u) => {
            if (u.docChanged) onChange.current?.(u.state.doc.toString());
          }),
        ],
      }),
    });
    view.current = v;
    return () => v.destroy();
    // The language and schema are fixed for an editor's life.
  }, [props.language, props.schema]);

  // Outside changes (a form edit, a reset) replace the text.
  useEffect(() => {
    const v = view.current;
    if (v && v.state.doc.toString() !== props.value) {
      v.dispatch({ changes: { from: 0, to: v.state.doc.length, insert: props.value } });
    }
  }, [props.value]);

  useEffect(() => {
    view.current?.dispatch({ effects: readOnly.current.reconfigure(EditorState.readOnly.of(!!props.readOnly)) });
  }, [props.readOnly]);

  return (
    <div
      ref={host}
      className={props.className}
      style={{ minHeight: props.minHeight ?? '16rem', maxHeight: props.maxHeight }}
      data-slot="code-editor"
    />
  );
}
