import { describe, expect, test } from 'vitest';

import { renderMarkdown } from './markdown';

describe('markdown rendering', () => {
  test('renders fenced code blocks with pre/code structure', () => {
    const html = renderMarkdown('```ts\nconst value = 1;\n```');

    expect(html).toContain('<pre>');
    expect(html).toContain('<code class="language-ts">');
    expect(html).toContain('const value = 1;');
  });
});
