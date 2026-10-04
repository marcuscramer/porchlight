// Builds the candidate list: online relay lists (if reachable) + seed-relays.txt + any --file, normalized
// to wss:// URLs. Writes out/candidates.json.
import { readFileSync, writeFileSync, existsSync } from 'node:fs';
import path from 'node:path';
import { root, ensureOut, outFile, log } from './lib/util.mjs';

const DEFAULT_LIST_URLS = ['https://api.nostr.watch/v1/online'];

export function normalize(raw) {
  try {
    const u = new URL(String(raw).trim());
    if (u.protocol !== 'wss:') return null;
    const h = u.hostname.toLowerCase();
    if (h === 'localhost' || h.endsWith('.onion') || h.endsWith('.local') || /^\d+\.\d+\.\d+\.\d+$/.test(h) || h.includes(':')) return null;
    return `wss://${h}${u.port ? ':' + u.port : ''}${u.pathname === '/' ? '' : u.pathname.replace(/\/$/, '')}`;
  } catch { return null; }
}

function parseList(text) {
  try {
    const j = JSON.parse(text);
    const arr = Array.isArray(j) ? j : Array.isArray(j.relays) ? j.relays : Object.keys(j);
    return arr.map((x) => (typeof x === 'string' ? x : x?.url)).filter(Boolean);
  } catch { return text.split(/\s+/).filter((l) => l.startsWith('wss://')); }
}

export async function discover({ listUrls = DEFAULT_LIST_URLS, files = [], max = 300 } = {}) {
  const found = new Map(); // url -> sources
  const add = (url, src) => { const n = normalize(url); if (n) found.set(n, [...(found.get(n) || []), src]); };
  for (const u of listUrls) {
    try {
      const res = await fetch(u, { signal: AbortSignal.timeout(15000) });
      if (!res.ok) throw new Error('HTTP ' + res.status);
      const urls = parseList(await res.text());
      urls.forEach((x) => add(x, u));
      log(`list ${u}: ${urls.length} entries`);
    } catch (e) { log(`list ${u}: unavailable (${e.message}) — continuing with the files`); }
  }
  for (const f of [path.join(root, 'seed-relays.txt'), ...files]) {
    if (!existsSync(f)) { log(`file ${f}: not found`); continue; }
    const urls = readFileSync(f, 'utf8').split('\n').map((l) => l.trim()).filter((l) => l && !l.startsWith('#'));
    urls.forEach((x) => add(x, path.basename(f)));
    log(`file ${path.basename(f)}: ${urls.length} entries`);
  }
  const candidates = [...found.keys()].slice(0, max);
  ensureOut();
  writeFileSync(outFile('candidates.json'), JSON.stringify(candidates, null, 2));
  log(`${candidates.length} candidates -> out/candidates.json`);
  return candidates;
}
