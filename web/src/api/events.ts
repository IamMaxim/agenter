import type { BrowserServerMessage } from './types';

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

  socket.addEventListener('open', () => {
    socket.send(
      JSON.stringify({
        type: 'subscribe_session',
        request_id: crypto.randomUUID(),
        session_id: sessionId
      })
    );
    handlers.onOpen?.();
  });

  socket.addEventListener('message', (event) => {
    handlers.onMessage(JSON.parse(event.data as string) as BrowserServerMessage);
  });
  socket.addEventListener('close', () => handlers.onClose?.());
  socket.addEventListener('error', (event) => handlers.onError?.(event));

  return {
    close: () => socket.close()
  };
}
