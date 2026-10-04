// Phase 3: turns out/probe.jsonl and out/soak.jsonl into a ranking (out/report.md, out/ranked.json) and a
// proposed pool that avoids putting several relays on the same host or network.
import { writeFileSync } from 'node:fs';
import dns from 'node:dns/promises';
import { readJsonl, outFile, hostOf, pct, log } from './lib/util.mjs';

const GATE = { uptime: 0.95, accept: 0.99, delivery: 0.99, ping: 0.98 };

function soakStats(events, nowMs) {
  const s = { drops: 0, connectFailures: 0, pubs: 0, accepted: 0, reasons: {}, delivered: 0, lost: 0, hbMs: [], pingMs: [], pingFail: 0, pings: 0, offers: 0, offersOk: 0, offerDelivered: 0, upMs: 0, totalMs: 0 };
  let first = null, last = null, upSince = null;
  for (const e of events) {
    first ??= e.t; last = e.t;
    if (e.type === 'up') upSince ??= e.t;
    else if (e.type === 'down') { s.drops++; if (upSince != null) { s.upMs += e.t - upSince; upSince = null; } }
    else if (e.type === 'connect_failed') s.connectFailures++;
    else if (e.type === 'end') { if (upSince != null) { s.upMs += e.t - upSince; upSince = null; } }
    else if (e.type === 'pub' && !e.skipped) {
      if (e.kind === 'offer') { s.pubs++; if (e.ok) s.accepted++; s.offers++; if (e.ok) s.offersOk++; }
      if (!e.ok) s.reasons[e.reason] = (s.reasons[e.reason] || 0) + 1;
    } else if (e.type === 'delivered') { s.delivered++; if (e.kind === 'hb') s.hbMs.push(e.ms); else s.offerDelivered++; }
    else if (e.type === 'lost') s.lost++;
    else if (e.type === 'ping') { /* counted in the agg lines; the individual line only shows when */ }
    else if (e.type === 'agg') {
      s.pubs += e.pubs; s.accepted += e.ok; s.delivered += e.hb.length; s.hbMs.push(...e.hb);
      s.pings += e.pingMs.length + e.pingFail; s.pingFail += e.pingFail; s.pingMs.push(...e.pingMs);
    }
  }
  if (upSince != null) s.upMs += (last ?? nowMs) - upSince;
  s.totalMs = Math.max(1, (last ?? nowMs) - (first ?? nowMs));
  s.hbMs.sort((a, b) => a - b); s.pingMs.sort((a, b) => a - b);
  return s;
}

async function addressOf(url) {
  try { const [ip] = await dns.resolve4(hostOf(url)); return ip; } catch { return null; }
}
const net24 = (ip) => (ip ? ip.split('.').slice(0, 3).join('.') : null);
const domainOf = (url) => hostOf(url).split('.').slice(-2).join('.');

export async function report({ pool = 20 } = {}) {
  const probes = new Map(readJsonl(outFile('probe.jsonl')).map((p) => [p.url, p]));
  const soakEvents = readJsonl(outFile('soak.jsonl'));
  const byRelay = new Map();
  for (const e of soakEvents) if (e.url) byRelay.set(e.url, [...(byRelay.get(e.url) || []), e]);
  const hasSoak = byRelay.size > 0;
  const urls = hasSoak ? [...byRelay.keys()] : [...probes.values()].filter((p) => p.ok).map((p) => p.url);
  const rows = [];
  for (const url of urls) {
    const p = probes.get(url);
    const st = hasSoak ? soakStats(byRelay.get(url), Date.now()) : null;
    const ip = await addressOf(url);
    const hours = st ? st.totalMs / 3_600_000 : null;
    const uptime = st ? st.upMs / st.totalMs : null;
    const accept = st ? (st.pubs ? st.accepted / st.pubs : 0) : null;
    const delivery = st ? (st.delivered + st.lost ? st.delivered / (st.delivered + st.lost) : 0) : null;
    const pingOk = st ? (st.pings ? 1 - st.pingFail / st.pings : 0) : null;
    const p50 = st ? pct(st.hbMs, 50) : (p?.sizes?.heartbeat?.deliveredMs ?? null);
    const p95 = st ? pct(st.hbMs, 95) : null;
    const failedGates = [];
    if (st) {
      if (uptime < GATE.uptime) failedGates.push(`uptime ${(uptime * 100).toFixed(1)}%`);
      if (accept < GATE.accept) failedGates.push(`accepted ${(accept * 100).toFixed(1)}%`);
      if (delivery < GATE.delivery) failedGates.push(`delivered ${(delivery * 100).toFixed(1)}%`);
      if (pingOk < GATE.ping) failedGates.push(`keep-alive ${(pingOk * 100).toFixed(1)}%`);
      if (st.offers && st.offersOk < st.offers) failedGates.push('rejected offer-sized events');
    }
    const score = st ? Math.round((100 * uptime * accept * delivery - (p95 ?? 0) / 50 - (st.drops / Math.max(hours, 0.1)) * 2) * 10) / 10 : (p50 != null ? 100 - p50 / 50 : 0);
    rows.push({
      url, score, passes: failedGates.length === 0, failedGates, ip, net: net24(ip), domain: domainOf(url),
      software: p?.nip11?.software?.split('/').pop() || null,
      sizes: { offer17k: !!p?.checks?.offer17k, offer33k: !!p?.checks?.offer33k, offer65k: !!p?.checks?.offer65k },
      connectMs: p?.connectMs ?? null, hours: hours && Math.round(hours * 10) / 10,
      uptimePct: uptime != null ? Math.round(uptime * 1000) / 10 : null, drops: st?.drops ?? null,
      acceptPct: accept != null ? Math.round(accept * 1000) / 10 : null, deliveredPct: delivery != null ? Math.round(delivery * 1000) / 10 : null,
      p50, p95, pingP95: st ? pct(st.pingMs, 95) : null, rejections: st ? Object.entries(st.reasons).sort((a, b) => b[1] - a[1]).slice(0, 3) : [],
    });
  }
  rows.sort((a, b) => b.score - a.score);

  // Greedy pick by score, but never two relays on the same /24 or the same registered domain.
  const chosen = [], skipped = [], nets = new Set(), domains = new Set();
  for (const r of rows.filter((x) => x.passes)) {
    if (chosen.length >= pool) break;
    if ((r.net && nets.has(r.net)) || domains.has(r.domain)) { skipped.push({ url: r.url, why: 'same network/domain as a better relay' }); continue; }
    chosen.push(r.url); if (r.net) nets.add(r.net); domains.add(r.domain);
  }
  writeFileSync(outFile('ranked.json'), JSON.stringify({ generated: new Date().toISOString(), basedOn: hasSoak ? 'soak' : 'probe only', proposedPool: chosen, rows }, null, 2));

  const md = [];
  md.push(`# Relay survey report`, '', `Generated ${new Date().toISOString()} — based on ${hasSoak ? 'the overnight soak' : '**the probe only (no soak data yet)**'}; ${rows.length} relays ranked, ${rows.filter((r) => r.passes).length} pass the gates.`, '');
  md.push(`Gates: uptime ≥ ${GATE.uptime * 100}%, publishes accepted ≥ ${GATE.accept * 100}%, deliveries ≥ ${GATE.delivery * 100}%, keep-alive ≥ ${GATE.ping * 100}%, no rejected offer-sized events.`, '');
  md.push(`## Proposed pool (${chosen.length})`, '', '```json', JSON.stringify(chosen, null, 2), '```', '');
  if (skipped.length) md.push(`Skipped for diversity: ${skipped.map((s) => s.url).join(', ')}`, '');
  md.push('## All relays', '', '| # | relay | score | uptime | drops | accepted | delivered | p50 / p95 ms | keep-alive p95 | 33k | 65k | software | gates failed |', '|---|---|---|---|---|---|---|---|---|---|---|---|---|');
  rows.forEach((r, i) => md.push(`| ${i + 1} | ${r.url.replace('wss://', '')} | ${r.score} | ${r.uptimePct ?? '–'}% | ${r.drops ?? '–'} | ${r.acceptPct ?? '–'}% | ${r.deliveredPct ?? '–'}% | ${r.p50 ?? '–'} / ${r.p95 ?? '–'} | ${r.pingP95 ?? '–'} | ${r.sizes.offer33k ? '✓' : '✗'} | ${r.sizes.offer65k ? '✓' : '✗'} | ${r.software ?? ''} | ${r.failedGates.join('; ')} |`));
  const withRejections = rows.filter((r) => r.rejections.length);
  if (withRejections.length) { md.push('', '## Rejection reasons seen', ''); withRejections.forEach((r) => md.push(`- ${r.url}: ${r.rejections.map(([k, n]) => `${n}× “${k}”`).join(', ')}`)); }
  const failedProbes = [...probes.values()].filter((p) => !p.ok);
  md.push('', `## Failed the probe (${failedProbes.length})`, '');
  failedProbes.forEach((p) => md.push(`- ${p.url}: ${(p.limits?.[0] || p.errors?.[0] || 'failed')}`));
  writeFileSync(outFile('report.md'), md.join('\n') + '\n');
  log(`report -> out/report.md, out/ranked.json (${chosen.length} relays proposed)`);
}
