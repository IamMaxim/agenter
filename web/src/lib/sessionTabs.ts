export interface PersistedSessionTab {
  sessionId: string;
  title: string;
  status?: string;
  workspaceId?: string;
  providerId?: string;
}

export const TAB_STORAGE_KEY = 'agenter.chat.tabs.v1';
export const MAX_OPEN_TABS = 12;
export interface SavedTabsDocument {
  version: 1;
  tabs: PersistedSessionTab[];
}

function normalizeSessionId(value: unknown): string | undefined {
  if (typeof value !== 'string') {
    return;
  }
  const trimmed = value.trim();
  if (!trimmed) {
    return;
  }
  return trimmed;
}

function normalizeTab(value: unknown): PersistedSessionTab | undefined {
  if (typeof value !== 'object' || value === null) {
    return undefined;
  }
  const record = value as Record<string, unknown>;
  const sessionId = normalizeSessionId(record.session_id ?? record.sessionId);
  if (!sessionId) {
    return undefined;
  }
  const title = normalizeSessionId(record.title) ?? sessionId;
  return {
    sessionId,
    title
  };
}

export function parseSavedTabs(raw: string | null): PersistedSessionTab[] {
  if (typeof raw !== 'string') {
    return [];
  }
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    return [];
  }

  const records = Array.isArray(value)
    ? value
    : typeof value === 'object' && value !== null && Array.isArray((value as { tabs?: unknown }).tabs)
      ? ((value as { tabs?: unknown }).tabs as unknown[])
      : [];
  const tabs: PersistedSessionTab[] = [];
  const seen = new Set<string>();

  for (const record of records) {
    const next = normalizeTab(record);
    if (!next || seen.has(next.sessionId)) {
      continue;
    }
    seen.add(next.sessionId);
    tabs.push(next);
    if (tabs.length >= MAX_OPEN_TABS) {
      break;
    }
  }

  return tabs;
}

export function serializeTabs(tabs: PersistedSessionTab[]): string {
  const next: PersistedSessionTab[] = [];
  const seen = new Set<string>();
  for (const tab of tabs) {
    if (!tab.sessionId) {
      continue;
    }
    if (seen.has(tab.sessionId)) {
      continue;
    }
    seen.add(tab.sessionId);
    next.push({
      sessionId: tab.sessionId,
      title: normalizeSessionId(tab.title) ?? tab.sessionId
    });
    if (next.length >= MAX_OPEN_TABS) {
      break;
    }
  }

  const payload: SavedTabsDocument = {
    version: 1,
    tabs: next
  };
  return JSON.stringify(payload);
}
