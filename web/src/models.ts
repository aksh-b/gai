// The chat models someone can actually pick, grouped the way people think
// about them: on this computer, in the cloud, or not connected yet.

import { useEffect, useState } from 'react';
import { api, type CatalogEntry, type SetupStatus } from './api';
import { isLocalProvider, modelLabel, providerLabel } from './labels';

export interface ModelChoice {
  id: string;
  label: string;
  /** Why it can't be used yet, if it can't. */
  blocked?: string;
  tools: boolean;
}

export interface ModelGroups {
  local: ModelChoice[];
  cloud: ModelChoice[];
  automatic: ModelChoice[];
  unavailable: ModelChoice[];
  demo: ModelChoice[];
  defaultModel: string | null;
  status: SetupStatus | null;
  loaded: boolean;
}

const EMPTY: ModelGroups = { local: [], cloud: [], automatic: [], unavailable: [], demo: [], defaultModel: null, status: null, loaded: false };

export function groupModels(catalog: CatalogEntry[], status: SetupStatus | null): ModelGroups {
  const g: ModelGroups = { ...EMPTY, local: [], cloud: [], automatic: [], unavailable: [], demo: [], status, loaded: true, defaultModel: status?.default_model ?? null };
  const installed = new Set(status?.ollama.models ?? []);
  for (const m of catalog) {
    if (m.kind !== 'chat') continue;
    const choice: ModelChoice = { id: m.id, label: modelLabel(m.id), tools: m.capabilities.tools };
    if (m.provider === 'mock') {
      if (m.id === 'mock/echo') g.demo.push(choice);
    } else if (m.provider === 'router') {
      g.automatic.push(choice);
    } else if (isLocalProvider(m.provider)) {
      if (m.provider !== 'ollama' || installed.has(m.model) || installed.has(`${m.model}:latest`)) g.local.push(choice);
      else g.unavailable.push({ ...choice, blocked: 'not downloaded yet' });
    } else if (m.key.status === 'missing') {
      g.unavailable.push({ ...choice, blocked: `needs a ${providerLabel(m.provider)} key` });
    } else {
      g.cloud.push(choice);
    }
  }
  if (!g.demo.length) g.demo.push({ id: 'mock/echo', label: modelLabel('mock/echo'), tools: true });
  return g;
}

/** Models grouped for pickers; reloads when `version` changes. */
export function useModelGroups(version = 0): ModelGroups {
  const [groups, setGroups] = useState<ModelGroups>(EMPTY);
  useEffect(() => {
    Promise.all([api.models(), api.setupStatus().catch(() => null)])
      .then(([catalog, status]) => setGroups(groupModels(catalog, status)))
      .catch(() => setGroups({ ...EMPTY, loaded: true }));
  }, [version]);
  return groups;
}

/** Options for SelectField, best groups first. Unusable models are left out. */
export function modelOptions(g: ModelGroups, dev: boolean) {
  const opt = (m: ModelChoice, where: string) => ({
    value: m.id,
    label: dev ? m.id : m.label,
    description: `${where}${m.tools ? '' : ' · chat only, no tools'}`,
  });
  return [
    ...g.automatic.map((m) => opt(m, 'Automatic')),
    ...g.cloud.map((m) => opt(m, 'Cloud')),
    ...g.local.map((m) => opt(m, 'On this computer')),
    ...g.demo.map((m) => opt(m, 'Demo')),
  ];
}
