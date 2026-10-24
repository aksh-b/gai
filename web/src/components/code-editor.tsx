import { lazy, Suspense } from 'react';

import { cn } from '@/lib/utils';

export type CodeEditorProps = {
  value: string;
  onChange?: (text: string) => void;
  language?: 'yaml' | 'markdown' | 'text';
  /**
   * For YAML: the name of a schema in the server's OpenAPI document
   * (`AgentSpec`, `WorkflowSpec`, `PersonaSpec`). Keys and values are
   * checked against it, and top-level keys are offered as you type.
   */
  schema?: string;
  readOnly?: boolean;
  placeholder?: string;
  minHeight?: string;
  maxHeight?: string;
  className?: string;
  'aria-label'?: string;
};

const Impl = lazy(() => import('./code-editor-impl'));

/**
 * A code editor (CodeMirror) for YAML specs and Markdown, with syntax
 * highlighting, inline errors and, for specs, schema checks.
 */
export function CodeEditor(props: CodeEditorProps) {
  const className = cn(
    'overflow-auto rounded-md border border-input bg-transparent shadow-xs focus-within:border-ring focus-within:ring-3 focus-within:ring-ring/50 dark:bg-input/30',
    props.className,
  );
  return (
    <Suspense
      fallback={
        <pre className={cn(className, 'p-3 font-mono text-[13px] whitespace-pre-wrap')} style={{ minHeight: props.minHeight ?? '16rem' }}>
          {props.value}
        </pre>
      }
    >
      <Impl {...props} className={className} />
    </Suspense>
  );
}
