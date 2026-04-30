import type { BrowserServerMessage } from './types';
import { debugLog } from './debug';

export interface BrowserEventSocket {
  close: () => void;
}

export interface BrowserEventHandlers {
  onMessage: (message: BrowserServerMessage) => void;
  onOpen?: () => void;
  onClose?: () => void;
  onError?: (error: Event) => void;
}

export function connectSessionEvents(
  sessionId: string,
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
        session_id: sessionId
      })
    );
    handlers.onOpen?.();
  });

  socket.addEventListener('message', (event) => {
    const message = JSON.parse(event.data as string) as BrowserServerMessage;
    debugLog('ws:message', { sessionId, type: message.type });
    handlers.onMessage(message);
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
