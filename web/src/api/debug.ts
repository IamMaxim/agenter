export function isDebugEnabled(): boolean {
  return import.meta.env.VITE_AGENTER_DEBUG === '1';
}

export function debugLog(event: string, fields: Record<string, unknown> = {}): void {
  if (!isDebugEnabled()) {
    return;
  }
  console.debug('[agenter]', event, fields);
}
