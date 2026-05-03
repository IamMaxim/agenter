import type { BrowserServerMessage } from './types';
import { debugLog } from './debug';
import { normalizeBrowserServerMessage } from '../lib/normalizers';

export interface BrowserEventSocket {
  close: () => void;
}

export interface BrowserEventHandlers {
  onMessage: (message: BrowserServerMessage) => void;
  onOpen?: () => void;
  onClose?: () => void;
  onError?: (error: Error | Event) => void;
}

export interface BrowserEventSubscriptionOptions {
  afterSeq?: string;
  includeSnapshot?: boolean;
}

export function parseBrowserServerMessage(data: string): BrowserServerMessage {
  return normalizeBrowserServerMessage(JSON.parse(data));
}

export function connectSessionEvents(
  sessionId: string,
  optionsOrHandlers: BrowserEventSubscriptionOptions | BrowserEventHandlers,
  maybeHandlers?: BrowserEventHandlers
): BrowserEventSocket {
  const options = maybeHandlers ? (optionsOrHandlers as BrowserEventSubscriptionOptions) : {};
  const handlers = maybeHandlers ?? (optionsOrHandlers as BrowserEventHandlers);
  return connectSessionEventsInternal(sessionId, options, handlers);
}

function connectSessionEventsInternal(
  sessionId: string,
  options: BrowserEventSubscriptionOptions,
  handlers: BrowserEventHandlers
): BrowserEventSocket {
  const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  const socket = new WebSocket(`${protocol}//${window.location.host}/api/browser/ws`);
  debugLog('ws:create', { sessionId });

  socket.addEventListener('open', () => {
    const requestId = crypto.randomUUID();
    debugLog('ws:open', { sessionId, requestId });
    socket.send(
      JSON.stringify({
        type: 'subscribe_session',
        request_id: requestId,
        session_id: sessionId,
        ...(options.afterSeq ? { after_seq: options.afterSeq } : {}),
        ...(options.includeSnapshot ? { include_snapshot: true } : {})
      })
    );
    handlers.onOpen?.();
  });

  socket.addEventListener('message', (event) => {
    try {
      const message = parseBrowserServerMessage(event.data as string);
      debugLog('ws:message', { sessionId, type: message.type });
      handlers.onMessage(message);
    } catch (error) {
      debugLog('ws:parse-error', { sessionId });
      handlers.onError?.(error instanceof Error ? error : new Error('Invalid browser event.'));
    }
  });
  socket.addEventListener('close', (event) => {
    debugLog('ws:close', { sessionId, code: event.code, reason: event.reason });
    handlers.onClose?.();
  });
  socket.addEventListener('error', (event) => {
    debugLog('ws:error', { sessionId, eventType: event.type });
    handlers.onError?.(event);
  });

  return {
    close: () => {
      debugLog('ws:manual-close', { sessionId });
      socket.close();
    }
  };
}
