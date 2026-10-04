// One WebSocket connection to a relay, speaking just enough NIP-01 for the survey: publish and wait for
// the relay's OK, subscribe, and a keep-alive that mirrors what nostr-tools does in the web app (a REQ
// that can match nothing, answered by EOSE) because browsers can't send WebSocket pings.
import crypto from 'node:crypto';

const NOTHING = { ids: ['0'.repeat(64)], limit: 1 }; // matches no event, so the relay just says EOSE

export class RelayConn {
  constructor(url) {
    this.url = url;
    this.ws = null;
    this.openedAt = null;
    this.acks = new Map(); // event id -> resolve
    this.eose = new Map(); // sub id -> resolve
    this.subs = new Map(); // sub id -> onEvent
    this.onClose = () => {};
    this.notices = [];
  }

  /** Resolves with the connect time in ms, rejects with an Error (timeout, refused, ...). */
  open(timeoutMs = 8000) {
    return new Promise((resolve, reject) => {
      const t0 = Date.now();
      let settled = false;
      const done = (fn, v) => { if (!settled) { settled = true; clearTimeout(timer); fn(v); } };
      const timer = setTimeout(() => { try { this.ws?.close(); } catch {} done(reject, new Error('connect timeout')); }, timeoutMs);
      try { this.ws = new WebSocket(this.url); } catch (e) { return done(reject, e); }
      this.ws.addEventListener('open', () => { this.openedAt = Date.now(); done(resolve, Date.now() - t0); });
      this.ws.addEventListener('error', (e) => done(reject, new Error(e.error?.cause?.code || e.error?.message || e.message || 'connect error')));
      this.ws.addEventListener('close', (e) => {
        done(reject, new Error(`closed (code ${e.code})`));
        this.openedAt = null;
        for (const r of this.acks.values()) r({ ok: false, reason: 'connection closed' });
        this.acks.clear();
        this.onClose(e);
      });
      this.ws.addEventListener('message', (m) => this.#message(m.data));
    });
  }

  get isOpen() { return this.ws?.readyState === 1; }

  #message(data) {
    let msg;
    try { msg = JSON.parse(typeof data === 'string' ? data : String(data)); } catch { return; }
    const [type, a, b, c] = msg;
    if (type === 'OK') this.acks.get(a)?.({ ok: b === true, reason: c || '' });
    else if (type === 'EOSE') this.eose.get(a)?.();
    else if (type === 'EVENT') this.subs.get(a)?.(b);
    else if (type === 'NOTICE') this.notices.push(String(a).slice(0, 200));
    else if (type === 'CLOSED') this.eose.get(a)?.('closed: ' + (b || ''));
  }

  send(msg) { this.ws.send(JSON.stringify(msg)); }

  /** Publishes and waits for the relay's OK. Resolves { ok, reason, ms } (never rejects). */
  publish(event, timeoutMs = 10000) {
    return new Promise((resolve) => {
      if (!this.isOpen) return resolve({ ok: false, reason: 'not connected', ms: 0 });
      const t0 = Date.now();
      const timer = setTimeout(() => { this.acks.delete(event.id); resolve({ ok: false, reason: 'no OK (timeout)', ms: Date.now() - t0 }); }, timeoutMs);
      this.acks.set(event.id, (r) => { clearTimeout(timer); this.acks.delete(event.id); resolve({ ...r, ms: Date.now() - t0 }); });
      try { this.send(['EVENT', event]); } catch (e) { clearTimeout(timer); resolve({ ok: false, reason: String(e.message), ms: 0 }); }
    });
  }

  /** Subscribes; resolves { id, eoseMs } once the relay says EOSE (or { id, eoseMs: null, reason } on CLOSED/timeout). */
  subscribe(filters, onEvent, timeoutMs = 8000) {
    return new Promise((resolve) => {
      const id = 'sv' + crypto.randomBytes(4).toString('hex');
      const t0 = Date.now();
      this.subs.set(id, onEvent);
      const timer = setTimeout(() => { this.eose.delete(id); resolve({ id, eoseMs: null, reason: 'no EOSE (timeout)' }); }, timeoutMs);
      this.eose.set(id, (closed) => {
        clearTimeout(timer); this.eose.delete(id);
        resolve(closed ? { id, eoseMs: null, reason: closed } : { id, eoseMs: Date.now() - t0 });
      });
      try { this.send(['REQ', id, ...filters]); } catch (e) { clearTimeout(timer); resolve({ id, eoseMs: null, reason: String(e.message) }); }
    });
  }

  /** Keep-alive round trip in ms, or null if the relay didn't answer in time. */
  async ping(timeoutMs = 8000) {
    if (!this.isOpen) return null;
    const id = 'pg' + crypto.randomBytes(4).toString('hex');
    const t0 = Date.now();
    const r = await new Promise((resolve) => {
      const timer = setTimeout(() => { this.eose.delete(id); resolve(null); }, timeoutMs);
      this.eose.set(id, () => { clearTimeout(timer); this.eose.delete(id); resolve(Date.now() - t0); });
      try { this.send(['REQ', id, NOTHING]); } catch { clearTimeout(timer); resolve(null); }
    });
    try { if (this.isOpen) this.send(['CLOSE', id]); } catch {}
    return r;
  }

  close() { try { this.ws?.close(); } catch {} }
}

/** Asks a relay for its NIP-11 information document over HTTPS. Resolves the parsed JSON or null. */
export async function nip11(url, timeoutMs = 6000) {
  try {
    const res = await fetch(url.replace(/^wss:/, 'https:').replace(/^ws:/, 'http:'), { headers: { accept: 'application/nostr+json' }, signal: AbortSignal.timeout(timeoutMs) });
    if (!res.ok) return null;
    const text = await res.text();
    return JSON.parse(text.slice(0, 200_000));
  } catch { return null; }
}
