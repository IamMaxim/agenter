import DOMPurify from 'dompurify';
import { marked } from 'marked';

marked.use({
  async: false,
  gfm: true,
  breaks: false
});

export function renderMarkdown(source: string): string {
  const html = marked.parse(source, { async: false });
  return DOMPurify.sanitize(html, {
    USE_PROFILES: { html: true }
  });
}
