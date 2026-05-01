import { get } from 'svelte/store';
import { afterEach, describe, expect, test, vi } from 'vitest';

import { dismissToast, pushToast, toasts } from './toasts';

describe('toast store', () => {
  afterEach(() => {
    vi.useRealTimers();
    for (const toast of get(toasts)) {
      dismissToast(toast.id);
    }
  });

  test('pushes severity and message and supports manual dismiss', () => {
    const id = pushToast({ severity: 'error', message: 'Could not load sessions.' });

    expect(get(toasts)).toEqual([
      expect.objectContaining({
        id,
        severity: 'error',
        message: 'Could not load sessions.'
      })
    ]);

    dismissToast(id);

    expect(get(toasts)).toEqual([]);
  });

  test('auto-dismisses after timeout', () => {
    vi.useFakeTimers();

    pushToast({ severity: 'warning', message: 'Connection lost.', timeoutMs: 1000 });

    expect(get(toasts)).toHaveLength(1);

    vi.advanceTimersByTime(1000);

    expect(get(toasts)).toEqual([]);
  });
});
