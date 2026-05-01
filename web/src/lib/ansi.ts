const colorClasses = new Map<number, string>([
  [30, 'ansi-fg-black'],
  [31, 'ansi-fg-red'],
  [32, 'ansi-fg-green'],
  [33, 'ansi-fg-yellow'],
  [34, 'ansi-fg-blue'],
  [35, 'ansi-fg-magenta'],
  [36, 'ansi-fg-cyan'],
  [37, 'ansi-fg-white'],
  [90, 'ansi-fg-bright-black'],
  [91, 'ansi-fg-bright-red'],
  [92, 'ansi-fg-bright-green'],
  [93, 'ansi-fg-bright-yellow'],
  [94, 'ansi-fg-bright-blue'],
  [95, 'ansi-fg-bright-magenta'],
  [96, 'ansi-fg-bright-cyan'],
  [97, 'ansi-fg-bright-white']
]);

export function ansiToHtml(source: string): string {
  let html = '';
  let activeClass: string | undefined;
  let cursor = 0;
  const pattern = new RegExp(`${String.fromCharCode(27)}\\[([0-9;]*)m`, 'g');

  for (const match of source.matchAll(pattern)) {
    html += wrap(escapeHtml(source.slice(cursor, match.index)), activeClass);
    activeClass = nextAnsiClass(match[1], activeClass);
    cursor = match.index + match[0].length;
  }

  html += wrap(escapeHtml(source.slice(cursor)), activeClass);
  return html;
}

function nextAnsiClass(sequence: string, activeClass: string | undefined): string | undefined {
  const codes = sequence
    .split(';')
    .filter(Boolean)
    .map((code) => Number.parseInt(code, 10));
  const normalizedCodes = codes.length > 0 ? codes : [0];
  let next = activeClass;

  for (const code of normalizedCodes) {
    if (code === 0 || code === 39) {
      next = undefined;
      continue;
    }
    const colorClass = colorClasses.get(code);
    if (colorClass) {
      next = colorClass;
    }
  }

  return next;
}

function wrap(content: string, className: string | undefined): string {
  if (!content) {
    return '';
  }
  return className ? `<span class="${className}">${content}</span>` : content;
}

function escapeHtml(value: string): string {
  return value
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&#39;');
}
