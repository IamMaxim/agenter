import { describe, expect, it } from 'vitest';

import { MAX_OPEN_TABS, parseSavedTabs, serializeTabs } from './sessionTabs';

describe('session tabs persistence', () => {
  it('parses a plain array payload', () => {
    const raw = JSON.stringify([
      { session_id: 's1', title: 'First' },
      { session_id: 's2', title: 'Second' },
      { session_id: '', title: '' },
      { foo: 'bar' },
      { sessionId: 's1', title: 'Duplicate' }
    ]);
    expect(parseSavedTabs(raw)).toEqual([
      { sessionId: 's1', title: 'First' },
      { sessionId: 's2', title: 'Second' }
    ]);
  });

  it('parses new document payloads and truncates to max tabs', () => {
    const raw = JSON.stringify({
      version: 1,
      tabs: Array.from({ length: MAX_OPEN_TABS + 3 }, (_, index) => ({
        sessionId: `session-${index}`,
        title: `Session ${index}`
      }))
    });

    const parsed = parseSavedTabs(raw);
    expect(parsed).toHaveLength(MAX_OPEN_TABS);
    expect(parsed[0]).toEqual({ sessionId: 'session-0', title: 'Session 0' });
    expect(parsed.at(-1)).toEqual({ sessionId: `session-${MAX_OPEN_TABS - 1}`, title: `Session ${MAX_OPEN_TABS - 1}` });
  });

  it('serializes deterministic deduplicated payloads', () => {
    expect(serializeTabs([{ sessionId: 's1', title: 'One' }, { sessionId: 's1', title: 'Dup' }])).toBe(
      JSON.stringify({
        version: 1,
        tabs: [{ sessionId: 's1', title: 'One' }]
      })
    );
  });

  it('serializes fallback title when title is missing or empty', () => {
    expect(serializeTabs([{ sessionId: 's1', title: '' }])).toBe(
      JSON.stringify({
        version: 1,
        tabs: [{ sessionId: 's1', title: 'Untitled session' }]
      })
    );
  });
});
