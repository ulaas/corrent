// Promise-based confirmation dialogs (rendered by ConfirmHost.svelte).
export interface ConfirmRequest {
  title: string;
  text?: string;
  ok: string;
  danger?: boolean;
  /** Optional checkbox (e.g. "also delete the files"). */
  check?: string;
  resolve: (r: { ok: boolean; checked: boolean }) => void;
}

export const confirms = $state<ConfirmRequest[]>([]);

export function confirm(o: Omit<ConfirmRequest, 'resolve'>): Promise<{ ok: boolean; checked: boolean }> {
  return new Promise((resolve) => confirms.push({ ...o, resolve }));
}
