// Phase 2: the overnight soak. For every surviving relay, hold one publishing and one subscribed
// connection for hours and act like the app does: three heartbeats (to three different "contacts",
// 400 ms apart) every 25 s, an offer-sized event every 10 minutes, and a keep-alive every 29 s. Everything
// goes to out/soak.jsonl as one line per observation; `report` turns that into the ranking.
import { readFileSync } from 'node:fs';
import { RelayConn } from './lib/relay.mjs';
import { newKeys, heartbeatEvent, offerEvent, WRAP_KIND } from './lib/core.mjs';
import { appendJsonl, outFile, sleep, log } from './lib/util.mjs';

const HEARTBEAT_MS = 25_000;
const SPREAD_MS = 400;
const OFFER_EVERY_MS = 10 * 60_000;
const PING_MS = 29_000;
const DELIVERY_TIMEOUT_MS = 20_000;

class Soaker {
  constructor(url, log_) {
    this.url = url;
    this.log = log_;
    this.sender = newKeys();
    this.contacts = [newKeys(), newKeys(), newKeys()];
    this.pending = new Map(); // event id -> { t0, kind }
    this.A = null; this.B = null;
    this.stopped = false;
    this.backoff = 1000;
    this.agg = this.#freshAgg();
  }

  #freshAgg() { return { pubs: 0, ok: 0, hb: [], pingMs: [], pingFail: 0 }; }

  /** Routine results are summed into 5-minute `agg` lines (a 20-hour run would otherwise log millions of lines); failures and anything unusual are logged individually. */
  flush() {
    if (this.agg.pubs || this.agg.hb.length || this.agg.pingMs.length || this.agg.pingFail) this.event('agg', this.agg);
    this.agg = this.#freshAgg();
  }

  event(type, extra = {}) { this.log({ t: Date.now(), url: this.url, type, ...extra }); }

  async connect() {
    while (!this.stopped) {
      const A = new RelayConn(this.url);
      const B = new RelayConn(this.url);
      try {
        await A.open();
        await B.open();
        const sub = await B.subscribe([{ kinds: [WRAP_KIND], '#p': this.contacts.map((c) => c.pk) }], (ev) => this.#delivered(ev));
        if (sub.eoseMs == null) throw new Error('subscribe: ' + sub.reason);
        A.onClose = () => this.#dropped('A');
        B.onClose = () => this.#dropped('B');
        this.A = A; this.B = B;
        this.backoff = 1000;
        this.event('up');
        return;
      } catch (e) {
        A.close(); B.close();
        this.event('connect_failed', { reason: e.message });
        await sleep(this.backoff);
        this.backoff = Math.min(this.backoff * 2, 60_000);
      }
    }
  }

  #dropped(which) {
    if (this.stopped || !this.A) return;
    const A = this.A, B = this.B;
    this.A = null; this.B = null;
    A.close(); B.close();
    this.event('down', { conn: which });
    this.connect();
  }

  #delivered(ev) {
    const p = this.pending.get(ev.id);
    if (!p) return;
    this.pending.delete(ev.id);
    if (p.kind === 'hb') this.agg.hb.push(Date.now() - p.t0);
    else this.event('delivered', { kind: p.kind, ms: Date.now() - p.t0 });
  }

  async publish(kind, ev) {
    if (!this.A?.isOpen) return this.event('pub', { kind, ok: false, reason: 'not connected', ms: 0, skipped: true });
    const t0 = Date.now();
    this.pending.set(ev.id, { t0, kind });
    setTimeout(() => { if (this.pending.delete(ev.id)) this.event('lost', { kind }); }, DELIVERY_TIMEOUT_MS);
    const ack = await this.A.publish(ev);
    if (kind === 'hb') { this.agg.pubs++; if (ack.ok) this.agg.ok++; }
    if (kind !== 'hb' || !ack.ok) this.event('pub', { kind, ok: ack.ok, reason: ack.reason, ms: ack.ms });
  }

  async run(untilMs) {
    await this.connect();
    let lastOffer = Date.now();
    const pinger = setInterval(async () => {
      for (const [name, c] of [['A', this.A], ['B', this.B]]) {
        if (!c) continue;
        const ms = await c.ping();
        if (ms == null) { this.agg.pingFail++; this.event('ping', { conn: name, ms: null }); } else this.agg.pingMs.push(ms);
      }
    }, PING_MS);
    const flusher = setInterval(() => this.flush(), 5 * 60_000);
    while (!this.stopped && Date.now() < untilMs) {
      const tick = Date.now();
      for (let i = 0; i < this.contacts.length; i++) {
        this.publish('hb', heartbeatEvent(this.sender, this.contacts[i].pk));
        await sleep(SPREAD_MS);
      }
      if (Date.now() - lastOffer >= OFFER_EVERY_MS) { lastOffer = Date.now(); this.publish('offer', offerEvent(this.sender, this.contacts[0].pk)); }
      await sleep(Math.max(0, HEARTBEAT_MS - (Date.now() - tick)));
    }
    clearInterval(pinger);
    clearInterval(flusher);
    this.flush();
    this.stopped = true;
    this.A?.close(); this.B?.close();
    this.event('end');
  }
}

export async function soak({ hours = 8, top = 40, urls } = {}) {
  const survivors = urls ?? JSON.parse(readFileSync(outFile('survivors.json'), 'utf8'));
  const list = survivors.slice(0, top);
  const untilMs = Date.now() + hours * 3_600_000;
  log(`soaking ${list.length} relays for ${hours} h (until ${new Date(untilMs).toLocaleTimeString()}) -> out/soak.jsonl`);
  const soakers = list.map((u) => new Soaker(u, (o) => appendJsonl(outFile('soak.jsonl'), o)));
  const stop = () => { log('stopping...'); soakers.forEach((s) => { s.stopped = true; }); };
  process.once('SIGINT', stop);
  process.once('SIGTERM', stop);
  appendJsonl(outFile('soak.jsonl'), { t: Date.now(), type: 'start', relays: list, hours });
  // Staggered starts so the relays aren't all hit at the same instant.
  await Promise.all(soakers.map(async (s, i) => { await sleep((i * HEARTBEAT_MS) / soakers.length); await s.run(untilMs); }));
  appendJsonl(outFile('soak.jsonl'), { t: Date.now(), type: 'finish' });
  log('soak finished');
}
