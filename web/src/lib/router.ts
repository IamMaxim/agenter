export type AppRoute =
  | { name: 'login' }
  | { name: 'workspaces' }
  | { name: 'sessions' }
  | { name: 'chat'; sessionId: string };

export function parseRoute(hash: string): AppRoute {
  const path = hash.replace(/^#/, '') || '/sessions';
  const parts = path.split('/').filter(Boolean);

  if (parts.length === 0) {
    return { name: 'sessions' };
  }

  if (parts[0] === 'login') {
    return { name: 'login' };
  }

  if (parts[0] === 'workspaces') {
    return { name: 'workspaces' };
  }

  if (parts[0] === 'sessions' && parts[1]) {
    return { name: 'chat', sessionId: decodeURIComponent(parts[1]) };
  }

  return { name: 'sessions' };
}

export function routeHref(route: AppRoute): string {
  if (route.name === 'chat') {
    return `#/sessions/${encodeURIComponent(route.sessionId)}`;
  }

  return `#/${route.name}`;
}
