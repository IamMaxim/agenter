import { describe, expect, test } from 'vitest';

import { ansiToHtml } from './ansi';

describe('ANSI rendering', () => {
  test('converts common foreground colors and reset codes to safe HTML spans', () => {
    expect(ansiToHtml('\u001b[31mfailed\u001b[0m ok')).toBe(
      '<span class="ansi-fg-red">failed</span> ok'
    );
  });

  test('escapes command output while preserving line content', () => {
    expect(ansiToHtml('\u001b[32m<done>&\u001b[0m')).toBe(
      '<span class="ansi-fg-green">&lt;done&gt;&amp;</span>'
    );
  });
});
