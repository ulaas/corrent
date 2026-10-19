export const SRC: string;
export const CATALOGS: string[];
export const WEB_CATALOG: string;
export function keysIn(text: string): string[];
export function usedKeys(dir?: string): Map<string, Set<string>>;
export function catalog(files?: string[]): Record<string, string>;
export function placeholders(s: string): string[];
