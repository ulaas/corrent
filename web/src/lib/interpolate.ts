/** Replaces `{name}` placeholders; unknown names stay visible so mistakes are noticed. */
export function interpolate(text: string, params?: Record<string, unknown>): string {
  if (!params) return text;
  return text.replace(/\{(\w+)\}/g, (m, k: string) => (k in params ? String(params[k]) : m));
}
