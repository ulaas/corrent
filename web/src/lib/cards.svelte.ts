// Card sizes from the "card size" preference.
import { prefs } from './prefs.svelte';
import type { Shape } from './media';

const BASE: Record<Shape, number> = { poster: 160, square: 176, landscape: 280 };
const SCALE = { small: 0.82, medium: 1, large: 1.22 } as const;

export function cardWidth(shape: Shape): number {
  const phone = typeof innerWidth === 'number' && innerWidth < 600 ? 0.78 : 1;
  return Math.round(BASE[shape] * SCALE[prefs.cardSize] * phone);
}
