// The Ctrl+K / ⌘K command palette, built on shadcn's Command in a dialog.

import { useEffect, type ReactNode } from 'react';

import { CommandDialog, Command, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList } from '@/components/ui/command';

export interface CommandEntry {
  id: string;
  label: string;
  group?: string;
  icon?: ReactNode;
  keywords?: string[];
  onSelect?: () => void;
}

export function CommandMenu({
  open,
  onOpenChange,
  items,
  placeholder,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  items: CommandEntry[];
  placeholder?: string;
}) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key.toLowerCase() === 'k' && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        onOpenChange(!open);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [open, onOpenChange]);

  const groups = new Map<string, CommandEntry[]>();
  for (const it of items) {
    const g = it.group ?? '';
    groups.set(g, [...(groups.get(g) ?? []), it]);
  }

  return (
    <CommandDialog open={open} onOpenChange={onOpenChange} title="Search" description="Search pages, assistants and actions">
      <Command>
        <CommandInput placeholder={placeholder} />
        <CommandList>
          <CommandEmpty>Nothing matches.</CommandEmpty>
          {[...groups].map(([group, entries]) => (
            <CommandGroup key={group} heading={group || undefined}>
              {entries.map((it) => (
                <CommandItem
                  key={it.id}
                  value={`${it.label} ${it.id}`}
                  keywords={it.keywords}
                  onSelect={() => {
                    onOpenChange(false);
                    it.onSelect?.();
                  }}
                >
                  {it.icon}
                  {it.label}
                </CommandItem>
              ))}
            </CommandGroup>
          ))}
        </CommandList>
      </Command>
    </CommandDialog>
  );
}
