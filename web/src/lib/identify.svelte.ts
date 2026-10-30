// "Identify…" from anywhere (item pages, card menus): the host in App.svelte shows the dialog.
export interface IdentifyRequest {
  id: number;
  title: string;
  year?: number | null;
  /** Called after a candidate was applied. */
  done?: () => void;
}

export const identifying = $state<{ current: IdentifyRequest | null }>({ current: null });

export function identify(req: IdentifyRequest) {
  identifying.current = req;
}
