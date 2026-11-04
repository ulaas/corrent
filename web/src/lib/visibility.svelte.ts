// Polling only while someone can see the page: a phone left on a settings page shouldn't keep
// waking the radio. Playback's own timers (position reports) don't go through this.

export const tab = $state({ visible: typeof document === 'undefined' || document.visibilityState === 'visible' });

if (typeof document !== 'undefined') {
  document.addEventListener('visibilitychange', () => (tab.visible = document.visibilityState === 'visible'));
}

/** Calls `f` every `ms` while the tab is visible, and once right when it comes back. Returns the
 *  function that stops it (so `$effect(() => everyVisible(load, 3000))` is all a page needs). */
export function everyVisible(f: () => void, ms: number): () => void {
  let timer: ReturnType<typeof setInterval> | null = null;
  const run = () => {
    if (timer === null) timer = setInterval(f, ms);
  };
  const halt = () => {
    if (timer !== null) clearInterval(timer);
    timer = null;
  };
  if (typeof document === 'undefined') {
    run();
    return halt;
  }
  const onchange = () => {
    if (document.visibilityState === 'visible') {
      f();
      run();
    } else halt();
  };
  document.addEventListener('visibilitychange', onchange);
  if (document.visibilityState === 'visible') run();
  return () => {
    halt();
    document.removeEventListener('visibilitychange', onchange);
  };
}
