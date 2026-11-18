// How a cast device reads in a list: its name, its model, the way it is reached.
import type { CastDevice } from './types';

/** "Living room · QN90 · AirPlay · not paired". `notPaired` is the translated "not paired";
 *  DLNA and AirPlay are protocol names and stay as they are. */
export function castDeviceLabel(
  d: Pick<CastDevice, 'name' | 'model' | 'kind' | 'needs_pairing'>,
  notPaired: string,
): string {
  const parts = [d.name];
  if (d.model) parts.push(d.model);
  if (d.kind === 'dlna') parts.push('DLNA');
  else if (d.kind === 'airplay') parts.push('AirPlay');
  if (d.needs_pairing) parts.push(notPaired);
  return parts.join(' · ');
}
