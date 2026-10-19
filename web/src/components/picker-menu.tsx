// Pick one of a list from a button: a DropdownMenu with a radio group per
// section. Used for the chat's assistant and model pickers.

import { Fragment, type ReactNode } from 'react';
import { ChevronDownIcon } from 'lucide-react';

import { Button } from '@/components/ui/button';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';

export interface PickerOption {
  value: string;
  label: ReactNode;
  description?: ReactNode;
  /** Options with the same group are listed together under its name. */
  group?: string;
}

export function PickerMenu({
  value,
  options,
  onValueChange,
  'aria-label': ariaLabel,
  className,
}: {
  value: string;
  options: PickerOption[];
  onValueChange: (value: string) => void;
  'aria-label': string;
  className?: string;
}) {
  const current = options.find((o) => o.value === value) ?? options[0];
  const groups: [string, PickerOption[]][] = [];
  for (const o of options) {
    const g = o.group ?? '';
    const last = groups[groups.length - 1];
    if (last && last[0] === g) last[1].push(o);
    else groups.push([g, [o]]);
  }
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="outline" size="sm" aria-label={ariaLabel} className={className}>
          <span className="truncate">{current?.label}</span>
          <ChevronDownIcon data-icon="inline-end" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="max-h-96 min-w-64">
        <DropdownMenuRadioGroup value={value} onValueChange={onValueChange}>
          {groups.map(([group, opts], i) => (
            <Fragment key={group || i}>
              {i > 0 && <DropdownMenuSeparator />}
              <DropdownMenuGroup>
                {group && <DropdownMenuLabel>{group}</DropdownMenuLabel>}
                {opts.map((o) => (
                  <DropdownMenuRadioItem key={o.value} value={o.value} className="flex-col items-start gap-0">
                    <span>{o.label}</span>
                    {o.description && <span className="text-xs text-muted-foreground">{o.description}</span>}
                  </DropdownMenuRadioItem>
                ))}
              </DropdownMenuGroup>
            </Fragment>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
