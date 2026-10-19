// Playing a list: tracks go to the music player; videos open the player at the first one.
import { go } from './router.svelte';
import { music, toEntry } from './player.svelte';
import type { Item } from './types';

export function playList(items: Item[], start = 0, shuffle = false) {
  if (!items.length) return;
  if (items.every((i) => i.kind === 'track' || i.kind === 'book')) {
    const s = shuffle ? Math.floor(Math.random() * items.length) : start;
    if (shuffle !== music.shuffle) music.toggleShuffle();
    music.play(items.map((t) => toEntry(t)), s);
  } else {
    go(`/play/${items[start]?.id ?? items[0].id}`);
  }
}
