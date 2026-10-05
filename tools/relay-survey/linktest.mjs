// Link test for the relay-management discussion (not a product feature): on a chosen set of relays, measure
//   latency  - publish -> relay OK, publish -> delivered to the other device, and a full ack round trip, per relay
//   traffic  - how many bytes a device really moves with N relays open, for three ways of sending heartbeats:
//              idle (listen only), all (every heartbeat to every relay, as the app does now), one (one relay at a
//              time with an end-to-end ack and failover, the idea under discussion)
// Throwaway keys only. Usage:
//   node linktest.mjs latency [--rounds 30] [--urls a,b,...]
//   node linktest.mjs traffic [--minutes 6] [--modes idle,all,one] [--urls a,b,...]
// Default relays: the first ten rows of out/report.md.
import { readFileSync, existsSync } from 'node:fs';
import { spawn, execFileSync } from 'node:child_process';
import { RelayConn } from './lib/relay.mjs';
import { newKeys, keysFrom, plainEvent, wrappedBytes, heartbeatEvent, offerEvent, BOOTSTRAP_KIND } from './lib/core.mjs';
import { outFile, sleep, pct, log, appendJsonl } from './lib/util.mjs';

const args = process.argv.slice(2);
const cmd = args[0];
const opt = (name, dflt) => { const i = args.indexOf('--' + name); return i >= 0 ? args[i + 1] : dflt; };

function defaultRelays() {
  const rows = readFileSync(new URL('./out/report.md', import.meta.url), 'utf8').split('\n').filter((l) => /^\| \d+ \|/.test(l));
  return rows.slice(0, 10).map((l) => 'wss://' + l.split('|')[2].trim());
}
const urls = opt('urls', '') ? opt('urls').split(',') : defaultRelays();
const stats = (xs) => { const s = xs.filter((x) => x != null).sort((a, b) => a - b); return { n: s.length, p50: pct(s, 50), p95: pct(s, 95), max: s[s.length - 1] ?? null }; };

// ---------------------------------------------------------------------------------------------- latency
async function latencyRelay(url, rounds) {
  const A = newKeys(), B = newKeys();
  const ca = new RelayConn(url), cb = new RelayConn(url);
  const res = { url, connectMs: null, eoseMs: null, ok: [], oneWay: [], rtt: [], ackOk: [], lost: 0, offerOneWay: [], error: null };
  try {
    const t0 = Date.now();
    await Promise.all([ca.open(), cb.open()]);
    res.connectMs = Date.now() - t0;
  } catch (e) { res.error = e.message; return res; }
  const sent = new Map(); // event id -> { t0, tB, tA }
  const acks = new Map(); // ack event id -> original id
  const hbBytes = JSON.stringify(heartbeatEvent(A, B.pk)).length;
  const ackBytes = wrappedBytes('{"type":"ack","id":"0123456789abcdef"}');
  const sb = await cb.subscribe([{ kinds: [BOOTSTRAP_KIND], '#p': [B.pk] }], (ev) => {
    const now = Date.now();
    const m = sent.get(ev.id);
    if (!m || m.tB) return;
    m.tB = now;
    const ack = plainEvent(B, A.pk, { type: 'ack', id: ev.id }, ackBytes);
    acks.set(ack.id, ev.id);
    cb.publish(ack).then((r) => { m.ackOk = r.ms; });
  });
  const sa = await ca.subscribe([{ kinds: [BOOTSTRAP_KIND], '#p': [A.pk] }], (ev) => {
    const orig = acks.get(ev.id);
    const m = orig && sent.get(orig);
    if (m && !m.tA) m.tA = Date.now();
  });
  res.eoseMs = sa.eoseMs;
  if (sa.eoseMs == null || sb.eoseMs == null) { res.error = 'subscribe: ' + (sa.reason || sb.reason); ca.close(); cb.close(); return res; }
  for (let i = 0; i < rounds; i++) {
    const ev = plainEvent(A, B.pk, { type: 'hb' }, hbBytes);
    const m = { t0: Date.now() };
    sent.set(ev.id, m);
    const r = await ca.publish(ev);
    m.okMs = r.ok ? r.ms : null;
    for (let w = 0; w < 100 && !m.tA; w++) await sleep(100); // up to 10 s for the ack to come back
    if (!m.tB) res.lost++;
    res.ok.push(m.okMs);
    if (m.tB) res.oneWay.push(m.tB - m.t0);
    if (m.tA) res.rtt.push(m.tA - m.t0);
    if (m.ackOk != null) res.ackOk.push(m.ackOk);
    await sleep(2000);
  }
  // A few offer-sized messages (a real offer is ~17 KB on the wire), one way only.
  for (let i = 0; i < 5; i++) {
    const ev = plainEvent(A, B.pk, { type: 'offer' }, 17000);
    const m = { t0: Date.now() };
    sent.set(ev.id, m);
    await ca.publish(ev);
    for (let w = 0; w < 100 && !m.tB; w++) await sleep(100);
    if (m.tB) res.offerOneWay.push(m.tB - m.t0);
    await sleep(2000);
  }
  ca.close(); cb.close();
  return res;
}

async function latency() {
  const rounds = Number(opt('rounds', 30));
  log(`latency: ${urls.length} relays, ${rounds} rounds each, one every 2 s`);
  const results = await Promise.all(urls.map((u) => latencyRelay(u, rounds)));
  const rows = results.map((r) => ({
    relay: new URL(r.url).hostname, connect: r.connectMs, eose: r.eoseMs,
    ok_p50: stats(r.ok).p50, ok_p95: stats(r.ok).p95,
    oneway_p50: stats(r.oneWay).p50, oneway_p95: stats(r.oneWay).p95,
    rtt_p50: stats(r.rtt).p50, rtt_p95: stats(r.rtt).p95, rtt_max: stats(r.rtt).max,
    lost: r.lost, offer_p50: stats(r.offerOneWay).p50, error: r.error,
  }));
  console.table(rows);
  const all = (k) => results.flatMap((r) => r[k]);
  const ms = (xs) => { const s = stats(xs); return `p50 ${s.p50} ms, p95 ${s.p95} ms (n=${s.n})`; };
  console.log('ALL relays: publish->OK', ms(all('ok')));
  console.log('ALL relays: publish->delivered (one way)', ms(all('oneWay')));
  console.log('ALL relays: publish->ack back (round trip)', ms(all('rtt')));
  appendJsonl(outFile('linktest-latency.jsonl'), { t: Date.now(), results });
}

// ---------------------------------------------------------------------------------------------- device
async function device() {
  const me = opt('me');
  const mode = opt('mode');
  const minutes = Number(opt('minutes', 6));
  const labels = ['D1', 'D2', 'D3'];
  const contacts = labels.filter((l) => l !== me).concat(['X']); // X never answers: an offline contact
  const myKey = Object.fromEntries(contacts.map((c) => [c, keysFrom(`k:${me}:${c}`)]));
  const peerPk = (c) => keysFrom(c === 'X' ? `k:X:${me}` : `k:${c}:${me}`).pk;
  const contactOf = Object.fromEntries(contacts.map((c) => [myKey[c].pk, c]));
  const hbBytes = JSON.stringify(heartbeatEvent(newKeys(), newKeys().pk)).length;
  const ackBytes = wrappedBytes('{"type":"ack","id":"0123456789abcdef"}');
  const c = { bytesIn: 0, bytesOut: 0, framesIn: 0, framesOut: 0, hb: 0, acks: 0, acksRecv: 0, hops: 0, closes: 0, connFail: 0, notices: 0, rejects: 0, ackMs: [] };
  const conns = [];
  const waiting = new Map(); // event id -> resolve
  const state = Object.fromEntries(contacts.map((x, i) => [x, { idx: (i * 3) % urls.length, misses: 0 }]));
  const pks = Object.values(myKey).map((k) => k.pk);
  let stopped = false;

  async function connectRelay(url) {
    let backoff = 1000;
    while (!stopped) {
      const conn = new RelayConn(url);
      const origSend = conn.send.bind(conn);
      conn.send = (msg) => { const s = JSON.stringify(msg); c.bytesOut += s.length; c.framesOut++; conn.ws.send(s); };
      try {
        await conn.open();
        conn.ws.addEventListener('message', (m) => { c.bytesIn += String(m.data).length; c.framesIn++; });
        const sub = await conn.subscribe([{ kinds: [BOOTSTRAP_KIND], '#p': pks }], (ev) => onEvent(conn, ev));
        if (sub.eoseMs == null) throw new Error('subscribe ' + sub.reason);
        conn.url = url;
        conns.push(conn);
        await new Promise((resolve) => {
          conn.onClose = () => { c.closes++; const i = conns.indexOf(conn); if (i >= 0) conns.splice(i, 1); resolve(); };
          (async () => { while (!stopped && conn.isOpen) { await sleep(29_000); if (conn.isOpen) await conn.ping(); } })();
        });
        backoff = 1000;
      } catch (e) {
        c.connFail++;
        conn.close();
        await sleep(backoff);
        backoff = Math.min(backoff * 2, 60_000);
      }
      c.notices += conn.notices.length;
    }
  }

  function onEvent(conn, ev) {
    let msg; try { msg = JSON.parse(ev.content); } catch { return; }
    const pTag = ev.tags.find((t) => t[0] === 'p')?.[1];
    const contact = contactOf[pTag];
    if (msg.type === 'hb' && mode === 'one' && contact) {
      const ack = plainEvent(myKey[contact], ev.pubkey, { type: 'ack', id: ev.id }, ackBytes);
      c.acks++;
      conn.publish(ack).then((r) => { if (!r.ok) c.rejects++; });
    } else if (msg.type === 'ack') {
      c.acksRecv++;
      waiting.get(msg.id)?.();
    }
  }

  const waitAck = (id, ms) => new Promise((resolve) => {
    const t = setTimeout(() => { waiting.delete(id); resolve(false); }, ms);
    waiting.set(id, () => { clearTimeout(t); waiting.delete(id); resolve(true); });
  });

  async function heartbeat(ct) {
    const ev = plainEvent(myKey[ct], peerPk(ct), { type: 'hb', from: me }, hbBytes);
    c.hb++;
    if (mode === 'idle') return;
    if (mode === 'all') { for (const conn of [...conns]) conn.publish(ev).then((r) => { if (!r.ok) c.rejects++; }); return; }
    // mode one: sticky relay, end-to-end ack, failover to the next relay; a contact that has not answered 3 times in a
    // row is only probed on one rotating relay per heartbeat
    const st = state[ct];
    const silent = st.misses >= 3;
    const ordered = urls.map((_, k) => urls[(st.idx + k) % urls.length]);
    let tries = 0;
    for (const url of ordered) {
      const conn = conns.find((x) => x.url === url);
      if (!conn) continue;
      if (silent && tries >= 1) break;
      tries++; c.hops++;
      const t0 = Date.now();
      conn.publish(ev).then((r) => { if (!r.ok) c.rejects++; });
      if (await waitAck(ev.id, 2000)) { st.idx = urls.indexOf(url); st.misses = 0; c.ackMs.push(Date.now() - t0); return; }
    }
    st.misses++;
    if (silent) st.idx = (st.idx + 1) % urls.length;
  }

  urls.forEach((u) => connectRelay(u));
  await sleep(8000);
  const t0 = Date.now();
  contacts.forEach((ct, i) => setTimeout(function loop() { if (stopped) return; heartbeat(ct); setTimeout(loop, 25_000); }, i * 400));
  const report = (final) => {
    const a = stats(c.ackMs);
    console.log(JSON.stringify({ me, mode, final: !!final, sec: Math.round((Date.now() - t0) / 1000), open: conns.length, ...c, ackMs: undefined, ackP50: a.p50, ackP95: a.p95 }));
  };
  const timer = setInterval(() => report(false), 60_000);
  await sleep(minutes * 60_000);
  stopped = true; clearInterval(timer);
  report(true);
  conns.forEach((x) => x.close());
  process.exit(0);
}

// ---------------------------------------------------------------------------------------------- traffic
function nettopBytes(pid) {
  const out = execFileSync('nettop', ['-P', '-L', '1', '-J', 'bytes_in,bytes_out'], { encoding: 'utf8' });
  const line = out.split('\n').find((l) => l.startsWith(`node.${pid},`));
  if (!line) return null;
  const [, i, o] = line.split(',');
  return { in: Number(i), out: Number(o) };
}

async function traffic() {
  const minutes = Number(opt('minutes', 6));
  const modes = opt('modes', 'idle,all,one').split(',');
  const summary = [];
  for (const mode of modes) {
    log(`traffic: mode ${mode}, 3 devices x ${urls.length} relays, ${minutes} min`);
    const procs = ['D1', 'D2', 'D3'].map((me) => {
      const p = spawn(process.execPath, [new URL(import.meta.url).pathname, 'device', '--me', me, '--mode', mode, '--minutes', String(minutes), '--urls', urls.join(',')], { stdio: ['ignore', 'pipe', 'inherit'] });
      p.last = null; p.me = me;
      p.stdout.on('data', (d) => { for (const l of String(d).split('\n').filter(Boolean)) { try { p.last = JSON.parse(l); } catch {} } });
      return p;
    });
    await sleep(25_000); // connected and past the first heartbeats
    const before = procs.map((p) => nettopBytes(p.pid));
    const t0 = Date.now();
    await sleep(Math.max(1000, minutes * 60_000 - 40_000)); // sample again shortly before the devices stop
    const after = procs.map((p) => nettopBytes(p.pid));
    const secs = (Date.now() - t0) / 1000;
    await Promise.all(procs.map((p) => new Promise((r) => p.on('exit', r))));
    for (const [k, p] of procs.entries()) {
      const l = p.last || {};
      const wire = before[k] && after[k] ? { in: (after[k].in - before[k].in) / 1024 / (secs / 60), out: (after[k].out - before[k].out) / 1024 / (secs / 60) } : null;
      summary.push({ mode, device: p.me, sockets: l.open, sec: l.sec, 'wire KB in/min': wire && +wire.in.toFixed(1), 'wire KB out/min': wire && +wire.out.toFixed(1), 'app KB in/min': +(l.bytesIn / 1024 / (l.sec / 60)).toFixed(1), 'app KB out/min': +(l.bytesOut / 1024 / (l.sec / 60)).toFixed(1), frames_in: l.framesIn, frames_out: l.framesOut, hb: l.hb, acks_sent: l.acks, acks_recv: l.acksRecv, hops: l.hops, ack_p50: l.ackP50, ack_p95: l.ackP95, closes: l.closes, connFail: l.connFail, rejects: l.rejects });
    }
    appendJsonl(outFile('linktest-traffic.jsonl'), { t: Date.now(), mode, devices: procs.map((p) => p.last) });
    await sleep(5000);
  }
  console.table(summary);
}

if (cmd === 'latency') await latency();
else if (cmd === 'traffic') await traffic();
else if (cmd === 'device') await device();
else console.log('usage: node linktest.mjs latency|traffic [--urls a,b] [--rounds N] [--minutes N] [--modes idle,all,one]');
