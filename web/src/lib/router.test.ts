import { describe, expect, it } from 'vitest';

import { parseRoute, routeHref } from './router';

describe('router', () => {
  it('defaults to home when hash is empty or only slash', () => {
    expect(parseRoute('')).toEqual({ name: 'home' });
    expect(parseRoute('#/')).toEqual({ name: 'home' });
  });

  it('maps legacy list paths to home', () => {
    expect(parseRoute('#/sessions')).toEqual({ name: 'home' });
    expect(parseRoute('#/workspaces')).toEqual({ name: 'home' });
  });

  it('parses chat session ids from the hash route', () => {
    expect(parseRoute('#/sessions/11111111-1111-1111-1111-111111111111')).toEqual({
      name: 'chat',
      sessionId: '11111111-1111-1111-1111-111111111111'
    });
  });

  it('encodes chat session route hrefs', () => {
    expect(routeHref({ name: 'chat', sessionId: 'session/with/slash' })).toBe(
      '#/sessions/session%2Fwith%2Fslash'
    );
  });

  it('builds login and home hrefs', () => {
    expect(routeHref({ name: 'login' })).toBe('#/login');
    expect(routeHref({ name: 'home' })).toBe('#/');
  });
});
