// Builds the candidate list: online relay lists (if reachable) + seed-relays.txt + any --file, normalized
// to wss:// URLs. Writes out/candidates.json.
import { readFileSync, writeFileSync, existsSync } from 'node:fs';
import path from 'node:path';
import { root, ensureOut, outFile, log } from './lib/util.mjs';

const DEFAULT_LIST_URLS = ['https://api.nostr.watch/v1/online'];
// A CSV of online relays kept by the bitchat project (first column: host without scheme).
const DEFAULT_CSV_URLS = ['https://raw.githubusercontent.com/permissionlesstech/bitchat/main/relays/online_relays_gps.csv'];
// Relays that publish NIP-66 monitor events (kind 30166, one per relay, `d` tag = relay URL).
const DEFAULT_NIP66 = ['wss://relay.damus.io', 'wss://nostr.oxtr.dev', 'wss://relay.snort.social'];

/** Asks a relay for recent NIP-66 relay-discovery events and returns the relay URLs they describe. */
function nip66(url, timeoutMs = 15000) {
  return new Promise((resolve) => {
    const urls = new Set();
    let ws;
    const done = () => { clearTimeout(timer); try { ws?.close(); } catch {} resolve([...urls]); };
    const timer = setTimeout(done, timeoutMs);
    try { ws = new WebSocket(url); } catch { return done(); }
    ws.addEventListener('open', () => ws.send(JSON.stringify(['REQ', 'disc', { kinds: [30166], limit: 800, since: Math.floor(Date.now() / 1000) - 3 * 86400 }])));
    ws.addEventListener('message', (m) => {
      let d; try { d = JSON.parse(m.data); } catch { return; }
      if (d[0] === 'EVENT') { const t = d[2]?.tags?.find((x) => x[0] === 'd'); if (t?.[1]) urls.add(t[1]); }
      else if (d[0] === 'EOSE' || d[0] === 'CLOSED') done();
    });
    ws.addEventListener('error', done);
  });
}

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

export async function discover({ listUrls = DEFAULT_LIST_URLS, csvUrls = DEFAULT_CSV_URLS, nip66Relays = DEFAULT_NIP66, files = [], max = 400 } = {}) {
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
  for (const u of csvUrls) {
    try {
      const res = await fetch(u, { signal: AbortSignal.timeout(15000) });
      if (!res.ok) throw new Error('HTTP ' + res.status);
      const rows = (await res.text()).split('\n').slice(1).map((l) => l.split(',')[0].trim()).filter(Boolean);
      rows.forEach((h) => add(/^wss?:\/\//.test(h) ? h : `wss://${h}`, u));
      log(`csv ${u}: ${rows.length} entries`);
    } catch (e) { log(`csv ${u}: unavailable (${e.message})`); }
  }
  for (const r of nip66Relays) {
    const urls = await nip66(r);
    urls.forEach((x) => add(x, 'nip66:' + r));
    log(`NIP-66 monitor events from ${r}: ${urls.length} relays`);
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
