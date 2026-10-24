// Persona bits shared by the Personas page, the assistant editor and chat:
// the avatar, and working out which persona a chat speaks as.

import { useCallback, useEffect, useState } from 'react';
import { Avatar, AvatarFallback, AvatarImage } from '@/components/ui/avatar';
import { cn } from '@/lib/utils';
import { api, type AgentSpec, type PersonaDefaults, type PersonaEntry, type PersonaSpec } from './api';

/** The name a persona speaks as. */
export const personaName = (p: Pick<PersonaSpec, 'name' | 'display_name'>) => p.display_name?.trim() || p.name;

const isImage = (a: string) => /^(https?:\/\/|\/|data:image\/)/.test(a);

/** A persona's avatar: an emoji, an image URL, or its first letter. */
export function PersonaAvatar({ persona, className }: { persona: Pick<PersonaSpec, 'name' | 'display_name' | 'avatar'>; className?: string }) {
  const a = persona.avatar?.trim() ?? '';
  const name = personaName(persona);
  return (
    <Avatar className={cn('size-8', className)}>
      {a && isImage(a) && <AvatarImage src={a} alt="" />}
      <AvatarFallback className="text-base" aria-label={name}>
        {a && !isImage(a) ? a : name.charAt(0).toUpperCase()}
      </AvatarFallback>
    </Avatar>
  );
}

/** Stored personas and the server-wide default, loaded once. */
export function usePersonas() {
  const [list, setList] = useState<PersonaEntry[]>([]);
  const [defaults, setDefaults] = useState<PersonaDefaults | null>(null);
  const reload = useCallback(() => {
    api
      .personas()
      .then(setList)
      .catch(() => {});
    api
      .personaDefaults()
      .then(setDefaults)
      .catch(() => {});
  }, []);
  useEffect(reload, [reload]);
  return { list, defaults, reload };
}

/** `name@v3` / `name@prod` → `name`. */
const baseName = (ref: string) => ref.split('@')[0];

/**
 * The persona a chat speaks as: the one picked for it, else its assistant's,
 * else the default. `null` when there is none (or it is turned off).
 */
export function activePersona(
  picked: string,
  agent: AgentSpec | null,
  list: PersonaEntry[],
  defaults: PersonaDefaults | null,
): PersonaSpec | null {
  const ref = picked || agent?.persona || defaults?.persona || '';
  if (!ref || ref === 'none') return null;
  return list.find((p) => p.summary.name === baseName(ref))?.spec ?? null;
}
