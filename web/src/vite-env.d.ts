/// <reference types="svelte" />
/// <reference types="vite/client" />

declare module 'virtual:tokens.css';
declare module 'virtual:tokens' {
  export const presets: string[];
  export const fixedAccentPresets: string[];
  export const accents: Record<string, { dark: string; light: string }>;
  export const durations: Record<'micro' | 'short' | 'page' | 'long', number>;
}
declare module 'virtual:i18n/tr' {
  const catalog: Record<string, string>;
  export default catalog;
}
declare module '*.svg?raw' {
  const svg: string;
  export default svg;
}
declare module '*.woff2' {
  const url: string;
  export default url;
}
declare module 'hls.js/dist/hls.light.mjs' {
  import Hls from 'hls.js';
  export default Hls;
}
