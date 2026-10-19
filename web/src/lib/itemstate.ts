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

export async function togglePlayed(item: Item) {
  const want = !item.state.played;
  item.state.played = want;
  try {
    Object.assign(
      item.state,
      await api.put<UserState>(`/items/${item.id}/state`, want ? { played: true, position_ms: 0 } : { played: false }),
    );
  } catch (e) {
    item.state.played = !want;
    toast(errorText(e), 'error');
  }
}
