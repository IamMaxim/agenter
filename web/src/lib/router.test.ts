import { describe, expect, it } from 'vitest';

import { parseRoute, routeHref } from './router';

describe('router', () => {
  it('defaults to the session list', () => {
    expect(parseRoute('')).toEqual({ name: 'sessions' });
    expect(parseRoute('#/')).toEqual({ name: 'sessions' });
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
});
