// Toast notifications with an optional action.
export type ToastKind = 'info' | 'success' | 'warning' | 'error';

export interface Toast {
  id: number;
  kind: ToastKind;
  text: string;
  action?: { label: string; run: () => void };
}

export const toasts = $state<Toast[]>([]);
let next = 1;

export function toast(text: string, kind: ToastKind = 'info', action?: Toast['action'], ttl = 5000): number {
  const id = next++;
  toasts.push({ id, kind, text, action });
  if (toasts.length > 5) toasts.shift();
  if (ttl > 0) setTimeout(() => dismiss(id), action ? ttl * 1.6 : ttl);
  return id;
}

export function dismiss(id: number) {
  const i = toasts.findIndex((t) => t.id === id);
  if (i >= 0) toasts.splice(i, 1);
}
