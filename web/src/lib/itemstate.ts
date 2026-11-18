// Favorite / played toggles (PUT /items/{id}/state) with an optimistic update.
import { api, errorText } from './api';
import { toast } from './toast.svelte';
import type { Item, UserState } from './types';

export async function toggleFavorite(item: Item) {
  const want = !item.state.favorite;
  item.state.favorite = want;
  try {
    Object.assign(item.state, await api.put<UserState>(`/items/${item.id}/state`, { favorite: want }));
  } catch (e) {
    item.state.favorite = !want;
    toast(errorText(e), 'error');
  }
}

/** Played, a container (show, season, album) when nothing in it is left unplayed. */
export function isPlayed(item: Item): boolean {
  return item.state.played || item.state.unplayed === 0;
}

export async function togglePlayed(item: Item) {
  const before = { played: item.state.played, unplayed: item.state.unplayed };
  const want = !isPlayed(item);
  item.state.played = want;
  // A container's count of unplayed items: none when played, unknown (until the answer) when not.
  if (item.state.unplayed !== undefined) item.state.unplayed = want ? 0 : undefined;
  try {
    Object.assign(
      item.state,
      await api.put<UserState>(`/items/${item.id}/state`, want ? { played: true, position_ms: 0 } : { played: false }),
    );
  } catch (e) {
    Object.assign(item.state, before);
    toast(errorText(e), 'error');
  }
}
