/** Relative luminance (WCAG) of a #rrggbb colour. */
export function luminance(hex: string): number {
  const n = parseInt(hex.replace('#', '').slice(0, 6), 16) || 0;
  const ch = [(n >> 16) & 255, (n >> 8) & 255, n & 255].map((v) => {
    const c = v / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * ch[0] + 0.7152 * ch[1] + 0.0722 * ch[2];
}

/** Text colour that reads on top of `hex` (same rule as the tokens plugin). */
export function onColor(hex: string): string {
  return luminance(hex) > 0.36 ? '#0B0D12' : '#FFFFFF';
}
