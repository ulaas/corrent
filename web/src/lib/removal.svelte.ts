// "Remove from library…" from anywhere (item pages, card menus): the host in App.svelte shows
// the dialog (as the desktop's, src/app/media/removal.rs).
export interface RemovalRequest {
  id: number;
  title: string;
  /** Called after the item was removed. */
  done?: () => void;
}

export const removing = $state<{ current: RemovalRequest | null }>({ current: null });

export function removeFromLibrary(req: RemovalRequest) {
  removing.current = req;
}
