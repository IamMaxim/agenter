export type AppRoute =
  | { name: 'login' }
  | { name: 'home' }
  | { name: 'chat'; sessionId: string };

export function parseRoute(hash: string): AppRoute {
  const path = hash.replace(/^#/, '') || '/';
  const parts = path.split('/').filter(Boolean);

  if (parts.length === 0) {
    return { name: 'home' };
  }

  if (parts[0] === 'login') {
    return { name: 'login' };
  }

  if (parts[0] === 'sessions' && parts[1]) {
    return { name: 'chat', sessionId: decodeURIComponent(parts[1]) };
  }

  if (parts[0] === 'sessions' || parts[0] === 'workspaces') {
    return { name: 'home' };
  }

  return { name: 'home' };
}

export function routeHref(route: AppRoute): string {
  if (route.name === 'chat') {
    return `#/sessions/${encodeURIComponent(route.sessionId)}`;
  }
  if (route.name === 'login') {
    return '#/login';
  }
  return '#/';
}
