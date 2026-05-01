import { writable } from 'svelte/store';

export type ToastSeverity = 'error' | 'warning' | 'info';

export interface ToastMessage {
  id: string;
  severity: ToastSeverity;
  message: string;
}

export interface PushToastRequest {
  severity: ToastSeverity;
  message: string;
  timeoutMs?: number;
}

const DEFAULT_TIMEOUT_MS = 5000;
const timers = new Map<string, ReturnType<typeof setTimeout>>();

export const toasts = writable<ToastMessage[]>([]);

export function pushToast(request: PushToastRequest): string {
  const id = crypto.randomUUID();
  const toast: ToastMessage = {
    id,
    severity: request.severity,
    message: request.message
  };
  toasts.update((current) => [...current, toast]);

  const timeout = setTimeout(() => dismissToast(id), request.timeoutMs ?? DEFAULT_TIMEOUT_MS);
  timers.set(id, timeout);

  return id;
}

export function dismissToast(id: string) {
  const timer = timers.get(id);
  if (timer) {
    clearTimeout(timer);
    timers.delete(id);
  }
  toasts.update((current) => current.filter((toast) => toast.id !== id));
}
