import { appendFileSync, mkdirSync, existsSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export const here = path.dirname(fileURLToPath(import.meta.url));
export const root = path.join(here, '..');
export const outDir = path.join(root, 'out');
export function ensureOut() { mkdirSync(outDir, { recursive: true }); }
export const outFile = (name) => path.join(outDir, name);
export function appendJsonl(file, obj) { ensureOut(); appendFileSync(file, JSON.stringify(obj) + '\n'); }
export function readJsonl(file) {
  if (!existsSync(file)) return [];
  return readFileSync(file, 'utf8').split('\n').filter(Boolean).flatMap((l) => { try { return [JSON.parse(l)]; } catch { return []; } });
}
export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
export const hostOf = (url) => new URL(url).hostname;
export function pct(sorted, p) { return sorted.length ? sorted[Math.min(sorted.length - 1, Math.floor((p / 100) * sorted.length))] : null; }
export function log(...a) { console.log(new Date().toISOString().slice(11, 19), ...a); }

/** Runs `fn` over `items` with at most `n` in flight. */
export async function pool(items, n, fn) {
  let i = 0;
  await Promise.all(Array.from({ length: Math.min(n, items.length) }, async () => { while (i < items.length) { const item = items[i++]; await fn(item); } }));
}
