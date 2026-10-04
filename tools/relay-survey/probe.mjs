// Phase 1: a quick check of every candidate. NIP-11 limits, then — with two separate connections, one
// publishing and one subscribed — whether the relay accepts and *delivers* the event kinds and sizes
// Porchlight uses, and how fast. Writes one line per relay to out/probe.jsonl and the survivors to
// out/survivors.json.
import { writeFileSync } from 'node:fs';
import { RelayConn, nip11 } from './lib/relay.mjs';
import { newKeys, heartbeatEvent, offerEvent, bootstrapEvent, WRAP_KIND, BOOTSTRAP_KIND } from './lib/core.mjs';
import { appendJsonl, outFile, readJsonl, pool, sleep, log } from './lib/util.mjs';
import { unlinkSync, existsSync } from 'node:fs';
import crypto from 'node:crypto';

// SDP sizes giving roughly 17 KB, 33 KB and 65 KB events on the wire (a real offer is ~17 KB).
const OFFER_SIZES = [7400, 14500, 28500];

/** NIP-11 flags that rule a relay out for us, as a list of human-readable reasons. */
export function limitReasons(info) {
  const l = info?.limitation || {};
  const r = [];
  if (l.auth_required) r.push('auth required');
  if (l.payment_required) r.push('payment required');
  if (l.restricted_writes) r.push('restricted writes');
  if (l.min_pow_difficulty > 0) r.push(`proof-of-work ${l.min_pow_difficulty}`);
  if (l.max_message_length && l.max_message_length < 40_000) r.push(`max message ${l.max_message_length}`);
  if (l.max_event_tags && l.max_event_tags < 4) r.push(`max tags ${l.max_event_tags}`);
  return r;
}

async function probeOne(url) {
  const res = { type: 'probe', url, t: Date.now(), checks: {}, sizes: {}, errors: [] };
  const info = await nip11(url);
  res.nip11 = info ? { name: info.name, software: info.software, version: info.version, limitation: info.limitation } : null;
  res.limits = limitReasons(info);

  const A = new RelayConn(url);
  const B = new RelayConn(url);
  try {
    res.connectMs = await A.open();
    await B.open();
  } catch (e) {
    res.errors.push(e.message);
    res.ok = false;
    return res;
  }
  const sender = newKeys();
  const recipient = newKeys();
  const tag = crypto.randomBytes(16).toString('hex');
  const delivered = new Map(); // event id -> time received
  const onEvent = (ev) => delivered.set(ev.id, Date.now());
  const sub = await B.subscribe([{ kinds: [WRAP_KIND], '#p': [recipient.pk] }, { kinds: [BOOTSTRAP_KIND], '#d': [tag] }], onEvent);
  res.eoseMs = sub.eoseMs;
  if (sub.eoseMs == null) res.errors.push('subscribe: ' + sub.reason);

  const tests = [
    ['heartbeat', () => heartbeatEvent(sender, recipient.pk)],
    ...OFFER_SIZES.map((n) => [`offer${n}`, () => offerEvent(sender, recipient.pk, n)]),
    ['bootstrap', () => bootstrapEvent(sender, tag)],
  ];
  for (const [name, build] of tests) {
    const ev = build();
    const wireBytes = JSON.stringify(ev).length;
    const t0 = Date.now();
    const ack = await A.publish(ev);
    let rttMs = null;
    for (let i = 0; i < 100 && !delivered.has(ev.id); i++) await sleep(100);
    if (delivered.has(ev.id)) rttMs = delivered.get(ev.id) - t0;
    res.sizes[name] = { wireBytes, ok: ack.ok, reason: ack.reason, ackMs: ack.ms, deliveredMs: rttMs };
    if (!ack.ok) res.errors.push(`${name}: ${ack.reason}`);
    await sleep(400);
  }
  res.notices = [...A.notices, ...B.notices].slice(0, 5);
  A.close(); B.close();
  const s = res.sizes;
  const good = (k) => s[k]?.ok && s[k]?.deliveredMs != null;
  res.checks = { connect: true, eose: res.eoseMs != null, heartbeat: good('heartbeat'), offer17k: good('offer7400'), offer33k: good('offer14500'), offer65k: good('offer28500'), bootstrap: good('bootstrap') };
  // Survivors must handle everything the app sends except, possibly, the very largest offers.
  res.ok = res.checks.connect && res.checks.eose && res.checks.heartbeat && res.checks.offer17k && res.checks.bootstrap;
  return res;
}

export async function probe({ concurrency = 6, limit = Infinity, urls } = {}) {
  const candidates = urls ?? JSON.parse((await import('node:fs')).readFileSync(outFile('candidates.json'), 'utf8'));
  const list = candidates.slice(0, limit);
  if (existsSync(outFile('probe.jsonl'))) unlinkSync(outFile('probe.jsonl'));
  let done = 0;
  await pool(list, concurrency, async (url) => {
    let r;
    try { r = await probeOne(url); } catch (e) { r = { type: 'probe', url, ok: false, errors: ['probe crashed: ' + e.message] }; }
    appendJsonl(outFile('probe.jsonl'), r);
    log(`[${++done}/${list.length}] ${r.ok ? 'PASS' : 'fail'} ${url}${r.ok ? ` (connect ${r.connectMs} ms, heartbeat rtt ${r.sizes.heartbeat.deliveredMs} ms${r.checks.offer65k ? '' : ', no 65 KB'})` : ' — ' + (r.limits?.[0] || r.errors?.[0] || 'failed')}`);
  });
  // Best first (so a cap in `soak` drops the weakest, not the last to finish): handles bigger events, then faster delivery.
  // Size limits from NIP-11 are judged by the actual tests, not trusted.
  const quality = (r) => (r.checks.offer65k ? 0 : 1) * 1e6 + (r.checks.offer33k ? 0 : 1) * 1e5 + (r.sizes.heartbeat.deliveredMs ?? 99999);
  const survivors = readJsonl(outFile('probe.jsonl'))
    .filter((r) => r.ok && !(r.limits || []).some((x) => !x.startsWith('max ')))
    .sort((a, b) => quality(a) - quality(b))
    .map((r) => r.url);
  writeFileSync(outFile('survivors.json'), JSON.stringify(survivors, null, 2));
  log(`${survivors.length} of ${list.length} passed the probe -> out/survivors.json`);
  return survivors;
}
