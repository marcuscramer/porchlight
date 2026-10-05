// Test client mirroring android-app as closely as the medium allows: same
// theme (see styles.css), same screen flow and copy, same multi-contact
// data model and multi-pairing relay client. Several Kotlin files' worth of
// behavior live here in one JS file (no build step):
//   - Config.kt                    -> the "Config-equivalent" section (pairings list)
//   - NostrSignalingClient.kt      -> the "Relay client" section
//   - CameraAgentService.kt        -> the "Pairing (SPAKE2)" section
//   - MainActivity.kt/HomeScreens.kt/PassphrasePairingScreens.kt -> "Screens"
// The whole pairing-bootstrap state machine — SPAKE2 exchange, collision/
// redelivery/stash handling, timeout, name sanitization — lives entirely in
// call-core's WASM module, not a local "helpers" section. This file's job is
// purely the imperative shell: forward events in, execute the returned
// Effects.

// generateSecretKey is still needed for minting a brand-new pairing's own
// permanent identity keypair (startPairing) — everything else Nostr-crypto-
// related (gift-wrap construction/verification, NIP-44) lives in call-core's
// own `nostr_protocol` module now.
import { generateSecretKey, getPublicKey } from 'https://esm.sh/nostr-tools@2.25.2/pure';
import { SimplePool } from 'https://esm.sh/nostr-tools@2.25.2/pool';
import { bytesToHex, hexToBytes } from 'https://esm.sh/nostr-tools@2.25.2/utils';
// Namespace import, not individual named imports — every call-core function
// is reached as callCore.xxx(...), the same way CallCoreBridge.kt namespaces
// its own calls. A handful of names (requestCall/hangUp/handleOffer/
// acceptIncomingCall/currentHeartbeatIntervalMs) collide with this file's
// own like-named UI-facing wrapper below — the namespace import keeps those
// unambiguous without renaming either side.
import init, * as callCore from './wasm/call_core.js';
// AUTO-GENERATED from tokens/strings/*.json (tokens/build-strings.mjs) —
// see that file's own doc for the shared source this and android-app's
// res/values*/strings.xml both come from. STRINGS itself only has 'en'
// today (infra-only pass — see LOCALE_STORAGE_KEY's own doc); resolveLocale
// already falls back correctly once a second locale file exists, no code
// here needs to change when one does.
import { t as translate, resolveLocale } from './strings.js';
import { RELAYS, STUN_SERVERS, AUTO_DISMISS_DELAY_MS, PREVIEW_POSITIONS, PRESENCE_TICK_INTERVAL_MS, SUBSCRIPTION_RESUBSCRIBE_COOLDOWN_MS, HEARTBEAT_SPREAD_MS, SELF_VIEW_SHRINK_MS, MAX_PHRASE_LENGTH, CAPTURE, PAUSE_WHEN_HIDDEN_MS } from './shared-config.js';

// call-core's WASM module — a top-level await (legal since this file is
// loaded as type="module"), so nothing below can run a pairing attempt
// before it's ready.
await init();

const LOCALE_STORAGE_KEY = 'porchlight-locale';
// No language switcher yet (English-only infra pass) — this just means a
// browser already set to a language this app doesn't have a translation
// for falls back to English via resolveLocale, rather than needing this
// file to know that explicitly. A future switcher only has to write this
// key; everything downstream already reads it.
let locale = localStorage.getItem(LOCALE_STORAGE_KEY) || resolveLocale(navigator.language);
function t(key, params) { return translate(locale, key, params); }

/** Fills every element carrying a data-i18n* attribute from `t()` — for the
 * static markup in index.html (button labels, screen titles, and so on)
 * that doesn't otherwise pass through any JS string-building code. Run
 * once at startup, before the first render() — nothing here changes at
 * runtime (no language switcher yet), so there's no need to re-run it
 * later the way renderWaitingScreen etc. do for genuinely dynamic content. */
function applyStaticI18n() {
  for (const el of document.querySelectorAll('[data-i18n]')) el.textContent = t(el.getAttribute('data-i18n'));
  for (const el of document.querySelectorAll('[data-i18n-placeholder]')) el.placeholder = t(el.getAttribute('data-i18n-placeholder'));
  for (const el of document.querySelectorAll('[data-i18n-aria-label]')) el.setAttribute('aria-label', t(el.getAttribute('data-i18n-aria-label')));
  for (const el of document.querySelectorAll('[data-i18n-title]')) el.title = t(el.getAttribute('data-i18n-title'));
}
applyStaticI18n();

// Read once from call-core (a compile-time-constant Rust struct) instead of
// six independently hand-copied literals that could silently drift from
// NostrSignalingClient.kt/CameraAgentService.kt's own copies.
const PROTOCOL_CONSTANTS = JSON.parse(callCore.protocolConstants());
// One custom, unregistered kind for every message this app sends — used two
// ways: as the *inner*, NIP-44-encrypted payload of a confirmed pairing's
// gift-wrapped traffic (never appearing directly on the wire), and as the
// plain, unencrypted bootstrap message of an in-progress pairing attempt
// (appearing directly, tagged by rendezvous point). Ephemeral (20000-29999
// per NIP-01).
const SIGNAL_KIND = PROTOCOL_CONSTANTS.signal_kind;
// The gift-wrap kind — a second custom, unregistered, ephemeral kind.
const WRAP_KIND = PROTOCOL_CONSTANTS.wrap_kind;
// HEARTBEAT_INTERVAL_MS/FAST_HEARTBEAT_INTERVAL_MS/FAST_HEARTBEAT_WINDOW_MS/
// ONLINE_TIMEOUT_MS all live in call-core's `presence` module now — this is
// the one presence-related constant that stays here, purely this file's own
// polling cadence for calling callCore.checkOnlineTimeouts.
const ONLINE_CHECK_INTERVAL_MS = PRESENCE_TICK_INTERVAL_MS;
// "At least 120 seconds" per the pairing design doc.
const PAKE_LIVE_WINDOW_MS = PROTOCOL_CONSTANTS.pake_live_window_ms;
// Sanity bounds on untrusted network input are enforced inside
// call-core's signal router now, not here. The relay list (and the STUN
// servers, auto-dismiss delay and self-view position cycle) come from
// shared-config.js, generated from tokens/shared/shared-config.json — the one
// source both clients read.

const DEVICE_NAME_STORAGE_KEY = 'porchlight-device-name';
const RELAY_COOLDOWNS_STORAGE_KEY = 'porchlight-relay-cooldowns';
const PAIRINGS_STORAGE_KEY = 'porchlight-pairings';

// ---------------------------------------------------------------------------
// Config-equivalent (mirrors Config.kt) — deviceName + a list of pairings,
// each { id, ownPrivateKeyHex, peerPublicKey, peerName }, same shape as
// Kotlin's Pairing. [ownPrivateKeyHex] is *this device's own* permanent
// identity for *that one relationship* — there's no device-wide identity,
// every pairing mints its own keypair. "" (not null/undefined) for
// peerPublicKey/peerName, same convention as the Kotlin side.
// ---------------------------------------------------------------------------

let deviceName = localStorage.getItem(DEVICE_NAME_STORAGE_KEY) || '';
let pairings = loadPairings();

/**
 * Old-shaped or corrupted JSON is meant to simply fail to parse and come
 * back empty, "clean slate, deliberately" — mirrors Android's own
 * `parsePairings`: a single entry missing a required field
 * (`id`/`ownPrivateKeyHex`) invalidates the *whole* list, not just that one
 * entry, via `every` below rather than a per-entry check. Optional fields
 * are defaulted the same way Kotlin's `optString`/`optBoolean` do
 * (`""`/`false`, never `undefined`).
 */
function loadPairings() {
  let parsed;
  try { parsed = JSON.parse(localStorage.getItem(PAIRINGS_STORAGE_KEY) || '[]'); } catch { return []; }
  if (!Array.isArray(parsed)) return [];
  const allValid = parsed.every((p) => p && typeof p.id === 'string' && typeof p.ownPrivateKeyHex === 'string');
  if (!allValid) return [];
  // Older versions saved a pairing attempt as an unconfirmed contact (no peer yet), which a reload mid-attempt left
  // behind as a nameless row; attempts live in memory now, so those are dropped.
  return parsed.filter((p) => p.peerPublicKey).map((p) => ({
    id: p.id,
    ownPrivateKeyHex: p.ownPrivateKeyHex,
    peerPublicKey: p.peerPublicKey,
    peerName: p.peerName || '',
    // The newest gift-wrapped signal (by created_at, then by id — see
    // WrapEventCandidate::last_signal_created_at's own doc, nostr_protocol.rs,
    // for why both are needed) this pairing has actually processed ("" / 0
    // for one that's never received any yet). Persisted with the rest of
    // the pairing so the signal router can recognize a relay redelivery even
    // across a reload.
    lastSignalCreatedAt: p.lastSignalCreatedAt || 0,
    lastSignalEventId: p.lastSignalEventId || '',
  }));
}
function savePairings() {
  localStorage.setItem(PAIRINGS_STORAGE_KEY, JSON.stringify(pairings));
}
function findPairing(pairingId) { return pairings.find((p) => p.id === pairingId) || null; }

function addOrUpdatePairing(pairing) {
  pairings = pairings.filter((p) => p.id !== pairing.id).concat([pairing]);
  savePairings();
}
/** A human confirmed this pairing's candidate — pin it. */
function updatePairingPeer(pairingId, peerPublicKey, peerName) {
  pairings = pairings.map((p) => (p.id === pairingId ? { ...p, peerPublicKey, peerName } : p));
  savePairings();
}
/** Called from handleIncomingEvent right after a wrap event is actually
 * accepted — persists the new high-water mark immediately, not batched, so
 * it survives a reload even if the very next thing that happens is a
 * crash/close. */
function updateLastSignal(pairingId, createdAt, eventId) {
  pairings = pairings.map((p) => (p.id === pairingId ? { ...p, lastSignalCreatedAt: createdAt, lastSignalEventId: eventId } : p));
  savePairings();
}
/** "Forget this contact" — deletes the pairing entirely. */
function removePairing(pairingId) {
  pairings = pairings.filter((p) => p.id !== pairingId);
  savePairings();
  // See callCore.forgetPairing's own doc: clears a deferred call's
  // wants_call entry (a real leak otherwise) and, if this pairing owned the
  // active call slot, its ClosePeerConnection effect below tears down the
  // real pc *without* a bye — matches this function's own
  // no-notification-on-delete behavior.
  applyCallEffects(callCore.forgetPairing(pairingId));
  callCore.presenceRemovePairing(pairingId);
  contactUiState.delete(pairingId);
}
// This device's own pubkey for a given pairing's own key — cached, since
// every heartbeat tick touches every confirmed pairing's signer. Mirrors
// NostrSignalingClient.kt's ownPubkeyCache.
const ownPubkeyCache = new Map();
function ownPubkeyHexFor(ownPrivateKeyHex) {
  if (!ownPubkeyCache.has(ownPrivateKeyHex)) {
    ownPubkeyCache.set(ownPrivateKeyHex, getPublicKey(hexToBytes(ownPrivateKeyHex)));
  }
  return ownPubkeyCache.get(ownPrivateKeyHex);
}

// ---------------------------------------------------------------------------
// All pairing-bootstrap logic -- crypto (rendezvous tag, SPAKE2 exchange,
// key-confirmation HMAC + constant-time verification) and decision logic
// (collision/redelivery/stash handling, timeout, name sanitization) -- lives
// in the call-core WASM module imported above now. See the "Pairing
// (SPAKE2)" section below for the imperative-shell wiring.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Relay client (mirrors NostrSignalingClient.kt) — one relay subscription
// for every pairing, dispatching by sender pubkey (confirmed pairings, via
// gift-wrapped WRAP_KIND traffic) or rendezvous tag (pending pairing
// attempts, via plain unwrapped SIGNAL_KIND bootstrap messages).
// ---------------------------------------------------------------------------

let pool = null;
let wrapSub = null;
let bootstrapSub = null;
let heartbeatTimeoutHandle = null;
let onlineCheckTimer = null;

function confirmedPeers() {
  return pairings.map((p) => (
    {
      pairingId: p.id,
      ownPrivateKeyHex: p.ownPrivateKeyHex,
      peerPublicKey: p.peerPublicKey,
      lastSignalCreatedAt: p.lastSignalCreatedAt,
      lastSignalEventId: p.lastSignalEventId,
    }
  ));
}
/** Mirrors NostrSignalingClient.kt's PendingPairing: what call-core holds for each live pairing attempt (at most
 * one in practice), with `rendezvousTag`, `bootstrapPayloads` and `bootstrapTarget` as its snapshot gives them. */
function pendingPairingsList() {
  return JSON.parse(callCore.pendingAttemptIds()).map((pairingId) => {
    const json = callCore.buildBootstrapPayload(pairingId); // undefined if the attempt ended in the meantime
    const snapshot = json ? JSON.parse(json) : null;
    return {
      pairingId,
      ownPrivateKeyHex: snapshot ? snapshot.own_private_key_hex : null,
      rendezvousTag: snapshot ? snapshot.rendezvous_tag : null,
      bootstrapPayloads: snapshot ? snapshot.payloads : [],
      bootstrapTarget: snapshot ? snapshot.candidate_pubkey : null,
    };
  });
}

/**
 * Builds the current subscription filter set from scratch — the *what to
 * filter for* decision is call-core's own `nostr_protocol::build_relay_filters`
 * (see its own doc); this function translates that plain data into
 * `nostr-tools`' own filter-object shape and decides how many
 * `pool.subscribe()` calls to make — web keeps two independent subscription
 * handles rather than Android's single combined `client.subscribe`, tied to
 * each platform's own relay-client library.
 */
// A relay actively terminating our subscription (NIP-01 CLOSED - rate
// limiting, auth required, PoW required on the filter, etc.) leaves the
// WebSocket connection itself untouched, so nothing else here ever notices:
// found live, 2026-10-01, by deliberately auditing for more instances of
// the exact shape a prior bug took (a listener callback never wired to any
// state or recovery) - this is the same gap, cross-platform. Re-subscribing
// is the fix; debounced so a relay that keeps closing for a persistent
// reason doesn't get hammered in a tight loop.
let lastCloseResubscribeMs = 0;
const SUBSCRIPTION_CLOSE_RESUBSCRIBE_COOLDOWN_MS = SUBSCRIPTION_RESUBSCRIBE_COOLDOWN_MS;
function onSubscriptionClosed(reasons) {
  console.warn('relay closed our subscription:', reasons);
  const now = Date.now();
  if (now - lastCloseResubscribeMs < SUBSCRIPTION_CLOSE_RESUBSCRIBE_COOLDOWN_MS) return;
  lastCloseResubscribeMs = now;
  resubscribe();
}

function resubscribe() {
  if (wrapSub) { wrapSub.close(); wrapSub = null; }
  if (bootstrapSub) { bootstrapSub.close(); bootstrapSub = null; }
  if (!pool) return;
  const ownPubkeys = confirmedPeers().map((p) => ownPubkeyHexFor(p.ownPrivateKeyHex));
  const rendezvousTags = pendingPairingsList().map((p) => p.rendezvousTag).filter(Boolean);
  const relayFilters = JSON.parse(callCore.buildRelayFilters(ownPubkeys, rendezvousTags));
  if (relayFilters.wrap_filter) {
    const f = relayFilters.wrap_filter;
    wrapSub = pool.subscribe(RELAYS, { kinds: [f.kind], [`#${f.tag_name}`]: f.tag_values }, { onevent: handleIncomingEvent, oneose() {}, onclose: onSubscriptionClosed });
  }
  if (relayFilters.bootstrap_filter) {
    const f = relayFilters.bootstrap_filter;
    bootstrapSub = pool.subscribe(RELAYS, { kinds: [f.kind], [`#${f.tag_name}`]: f.tag_values }, { onevent: handleIncomingEvent, oneose() {}, onclose: onSubscriptionClosed });
  }
}

// nostr-tools never says which relay an event or message came from, nor why
// a connection failed or dropped (including every automatic reconnect), so the
// per-relay "last message" and connection errors on the Status screen
// come from the sockets themselves: a plain WebSocket that records what
// happens to it. Browsers keep a failed handshake's HTTP status to themselves,
// so the reason is only what the socket events carry.
class TrackedWebSocket extends WebSocket {
  constructor(url, ...rest) {
    super(url, ...rest);
    const key = String(url).replace(/\/$/, '');
    let failedAt = 0;
    this.addEventListener('message', () => callCore.noteRelayMessage(key, Date.now()));
    this.addEventListener('open', () => callCore.noteRelayConnected(key));
    this.addEventListener('error', () => { failedAt = Date.now(); callCore.noteRelayConnectError(key, 'connection failed', failedAt); });
    this.addEventListener('close', (e) => {
      // A failed handshake fires 'error' then 'close': keep it one entry, with the code.
      if (failedAt && Date.now() - failedAt < 2000) {
        callCore.noteRelayConnectError(key, `connection failed (code ${e.code})`, failedAt);
      } else {
        callCore.noteRelayConnectError(key, `closed (code ${e.code}${e.reason ? ': ' + e.reason : ''})`, Date.now());
      }
    });
  }
}

function connectRelayClient() {
  // Relays that rejected us recently stay paused across a reload instead of
  // being poked again right away (see call-core's signal_retry).
  try {
    callCore.importRelayMemory(localStorage.getItem(RELAY_COOLDOWNS_STORAGE_KEY) || '{}', Date.now());
  } catch { /* storage unavailable or unreadable: start clean */ }
  // enableReconnect: nostr-tools' SimplePool defaults this to false — once
  // any relay connection fails or drops, it just gives up on that relay
  // forever, which a tab left open across a transient relay hiccup would
  // hit. With this on, a dropped/failed relay retries with the library's
  // own backoff instead of needing a page reload to recover. enablePing: a
  // relay socket that died silently (NAT mapping expired, Wi-Fi roam, a
  // phone waking from sleep) still reports readyState OPEN, so without a
  // ping neither reconnect nor anything else ever notices — publishes
  // vanish and nothing arrives. nostr-tools pings every ~29s and closes a
  // socket that doesn't answer, which hands it to the reconnect above.
  pool = new SimplePool({ enableReconnect: true, enablePing: true, websocketImplementation: TrackedWebSocket });
  // nostr-tools' SimplePool has no separate "connect" step — a relay's
  // WebSocket only actually opens once something subscribes or publishes to
  // it. resubscribe() below skips calling pool.subscribe() at all when
  // there's nothing to filter for yet, so a brand-new profile with no
  // contacts would otherwise never open any relay connection at all.
  // ensureRelay() opens the connection directly, independent of any
  // subscription.
  for (const url of RELAYS) pool.ensureRelay(url).catch(() => { /* the socket itself records why (TrackedWebSocket) */ });
  resubscribe();
  onlineCheckTimer = setInterval(monitorOnlineTimeouts, ONLINE_CHECK_INTERVAL_MS);
  scheduleHeartbeat(0);
}

/** Everything about an incoming event — dedup, unwrap and verify, parsing,
 * presence, the per-message decisions — is call-core's signal router; this
 * only hands it a fresh snapshot of the pairings and executes the ordered
 * result. Mirrors NostrSignalingClient.kt's routeIncoming. */
function handleIncomingEvent(event) {
  const context = {
    device_name: deviceName,
    confirmed: confirmedPeers().map((p) => (
      {
        pairing_id: p.pairingId,
        own_private_key_hex: p.ownPrivateKeyHex,
        peer_public_key: p.peerPublicKey,
        last_signal_created_at: p.lastSignalCreatedAt,
        last_signal_event_id: p.lastSignalEventId,
        // Auto-answer isn't a web feature — every incoming call rings for a
        // manual Accept/Decline.
        auto_answer: false,
      }
    )),
    pending: pendingPairingsList().filter((p) => p.rendezvousTag).map((p) => ({ pairing_id: p.pairingId, rendezvous_tag: p.rendezvousTag })),
  };
  // try/catch: this runs inside SimplePool's own onevent callback, where an
  // uncaught throw would silently abort the event half-processed.
  let result;
  try {
    result = JSON.parse(callCore.routeEvent(JSON.stringify(event), JSON.stringify(context), Date.now()));
  } catch (err) {
    console.warn('handleIncomingEvent: callCore.routeEvent failed, dropping event', err);
    return;
  }
  // Persisted before anything else, so a redelivery of this exact event (or
  // anything older) is rejected on any later attempt, reload included.
  if (result.signal) updateLastSignal(result.signal.pairing_id, result.signal.created_at, result.signal.event_id);
  applyPresenceUpdate(JSON.stringify({ presence_effects: result.presence_effects, call_effects: result.call_effects }));
  for (const action of result.actions) applyRouteAction(action);
  if (result.bootstrap_effects.length) applyEffects(JSON.stringify(result.bootstrap_effects));
}

function applyRouteAction(action) {
  switch (action.kind) {
    case 'SendHeartbeat': {
      const peer = confirmedPeers().find((p) => p.pairingId === action.pairing_id);
      if (peer) sendToConfirmedPeer(peer.pairingId, peer.ownPrivateKeyHex, peer.peerPublicKey, () => action.payload_json);
      break;
    }
    case 'UpdatePeerName': {
      const p = findPairing(action.pairing_id);
      if (p && action.name !== p.peerName) { updatePairingPeer(action.pairing_id, p.peerPublicKey, action.name); render(); }
      break;
    }
    case 'ApplyRemoteAnswer':
      handleAnswer(action.sdp).catch((err) => failCallAttempt('handleAnswer failed (bad SDP?)', err));
      break;
    case 'AddRemoteIce':
      applyIceCandidate(action.sdp_mid, action.sdp_m_line_index, action.candidate);
      break;
    default:
      console.error('applyRouteAction: unknown action kind from call-core', action.kind);
  }
}

/**
 * Applies a JSON-encoded PresenceUpdateResult from call-core's `presence`
 * module — mirrors CameraAgentService.kt's onPresenceUpdate. `presence`
 * calls directly into `call_arbitration` itself, so there's only ever one
 * combined result to apply: presence_effects into contactUiState and
 * call_effects via the existing applyCallEffects.
 */
function applyPresenceUpdate(json) {
  const result = JSON.parse(json);
  for (const effect of result.presence_effects) {
    switch (effect.kind) {
      case 'SetStatus':
        // effect.status is one of 'offline'/'online'/'busy' (call-core's
        // own PresenceStatus) — stored as the raw wire string, no separate
        // JS enum needed.
        contactUiState.set(effect.pairing_id, { ...uiState(effect.pairing_id), status: effect.status });
        break;
      default:
        console.error('applyPresenceUpdate: unknown presence effect kind from call-core', effect.kind);
    }
  }
  applyCallEffects(JSON.stringify(result.call_effects));
}

// What the "Status" screen shows that nothing else tracks: when
// the last heartbeat went out.
let lastHeartbeatSentAt = null;
const expandedRelayRows = new Set(); // relay rows tapped open to show the full message
function recordResult(eventId, relay, accepted, reason, now) {
  callCore.recordPublishResult(eventId, relay, accepted, reason, now);
  if (accepted) return;
  try {
    localStorage.setItem(RELAY_COOLDOWNS_STORAGE_KEY, callCore.exportRelayMemory(now));
  } catch { /* storage unavailable */ }
}

function heartbeatTick() {
  lastHeartbeatSentAt = Date.now();
  // callCore.isCallActive(): this device's own single call slot, broadcast
  // identically to every contact regardless of who (if anyone) it's
  // actually occupied by. Read once per tick, not once per peer.
  const busy = callCore.isCallActive();
  // Built once per tick, not once per peer: whether this heartbeat carries
  // `hello` is consumed by the build itself (call-core's presence::take_hello),
  // so one payload has to serve every peer.
  const heartbeat = callCore.buildHeartbeatPayload(deviceName, busy);
  // One event per contact, spread out rather than all in the same instant:
  // relays throttle bursts ("rate-limited: slow down"), and a rejection that
  // arrives before the next send pauses that relay for the rest of them.
  if (heartbeat) {
    confirmedPeers().forEach((peer, i) => {
      const send = () => sendToConfirmedPeer(peer.pairingId, peer.ownPrivateKeyHex, peer.peerPublicKey, () => heartbeat);
      if (i === 0) send(); else setTimeout(send, i * HEARTBEAT_SPREAD_MS);
    });
  }
  for (const pending of pendingPairingsList()) {
    if (!pending.rendezvousTag) continue;
    for (const payload of pending.bootstrapPayloads) sendPairingBootstrap(pending.ownPrivateKeyHex, pending.rendezvousTag, pending.bootstrapTarget, payload);
  }
}

/** The delay until the next tick — steady, or shorter while a live pairing
 * attempt republishes its bootstrap messages on it. See call-core's own
 * `presence::current_heartbeat_interval_ms` doc: anything else that wants a
 * prompt heartbeat asks for exactly one (kickHeartbeat + hello) instead. */
function currentHeartbeatIntervalMs() {
  return callCore.currentHeartbeatIntervalMs();
}

function scheduleHeartbeat(delayMs) {
  clearTimeout(heartbeatTimeoutHandle);
  heartbeatTimeoutHandle = setTimeout(() => {
    heartbeatTick();
    scheduleHeartbeat(currentHeartbeatIntervalMs());
  }, delayMs);
}

/** Sends a heartbeat/bootstrap message right now, switches to the fast
 * cadence immediately, and re-subscribes with a freshly-built filter set —
 * mirrors NostrSignalingClient.kt's kickHeartbeat exactly. */
function kickHeartbeat() {
  resubscribe();
  scheduleHeartbeat(0);
}

function monitorOnlineTimeouts() {
  maybePauseConnection();
  const now = Date.now();
  applyPresenceUpdate(callCore.checkOnlineTimeouts(now));
  applyCallEffects(callCore.checkCallTimeout(now));
  retryPendingPublishes();
}

// ---------------------------------------------------------------------------
// Publish retry. nostr-tools' SimplePool.publish() returns one promise per
// relay that's a single ensureRelay()+publish() attempt and nothing more --
// no retry, no resend when a relay that was briefly down comes back. The
// queue bookkeeping (what's still outstanding, what's too old, the size
// bound, and the per-relay rejection cooldown) lives in call-core's
// signal_retry module, shared with android-app,
// whose quartz NostrClient has the identical gap; this file supplies only
// what's genuinely web-native: the real publish() calls, and translating
// each settled promise into callCore.recordPublishResult. Resending the same
// already-signed event is safe either way -- a relay that already has it just
// gets a harmless redelivery, and the receiving side's own callCore.
// markSeenOrIsDuplicate already guards against acting on it twice.
// ---------------------------------------------------------------------------

/** Publishes `event` to every relay call-core says isn't cooling down from a
 * recent rejection, handing its retry queue whichever ones reject it (or were
 * skipped for cooling down) instead of just discarding the failure. */
async function publishToRelays(event, payloadJson) {
  // Paused while the page is hidden (see pauseConnection): nothing to send to.
  if (!pool) return;
  const now = Date.now();
  const targets = callCore.availableRelays(RELAYS, now);
  const failedRelays = RELAYS.filter(url => !targets.includes(url));
  const settled = await Promise.allSettled(pool.publish(targets, event));
  const rejections = [];
  settled.forEach((result, i) => {
    if (result.status === 'rejected') {
      failedRelays.push(targets[i]);
      rejections.push([targets[i], String(result.reason)]);
      console.warn('publish failed, will retry:', targets[i], result.reason);
    } else {
      recordResult(event.id, targets[i], true, '', now);
    }
  });
  if (failedRelays.length === 0) return;
  callCore.recordPendingPublish(event.id, JSON.stringify(event), payloadJson, failedRelays, now);
  for (const [relay, reason] of rejections) recordResult(event.id, relay, false, reason, now);
}

/** Republishes whatever call-core says is still outstanding (it already
 * leaves out relays cooling down from a rejection). */
async function retryPendingPublishes() {
  if (!pool) return;
  const now = Date.now();
  for (const entry of JSON.parse(callCore.dueForRetry(now))) {
    const event = JSON.parse(entry.event_json);
    const settled = await Promise.allSettled(pool.publish(entry.relays, event));
    settled.forEach((result, i) => {
      if (result.status === 'fulfilled') recordResult(event.id, entry.relays[i], true, '', now);
      else recordResult(event.id, entry.relays[i], false, String(result.reason), now);
    });
  }
}

/**
 * Builds, gift-wraps, and publishes one signal message for a *confirmed*
 * pairing. Two layers:
 *
 * 1. **Inner event** — the actual payload, NIP-44 encrypted and signed by
 *    this pairing's own permanent identity.
 * 2. **Gift wrap** — that entire signed inner event, JSON-encoded, NIP-44
 *    re-encrypted under a *fresh, random, one-time keypair* generated just
 *    for this one message, and signed by that random key. This is what
 *    actually gets published: relays see only a random pubkey they've
 *    never seen before and will never see again, tagged at the recipient.
 *
 * Deliberately a simplified two-layer wrap, not NIP-59's full rumor/seal/
 * wrap — see call-core's own `nostr_protocol` module doc for the full
 * reasoning. callCore.buildWrappedEvent does the actual construction; this
 * function just builds the plain payload, hands it to call-core, and
 * publishes whatever comes back.
 */
async function publish(ownPrivateKeyHex, targetPubkeyHex, payloadJson) {
  try {
    const wrapJson = callCore.buildWrappedEvent(ownPrivateKeyHex, targetPubkeyHex, payloadJson);
    if (!wrapJson) return;
    await publishToRelays(JSON.parse(wrapJson), payloadJson);
  } catch (err) {
    console.error('failed to encrypt/publish', err);
  }
}
// buildPayload is one of the callCore.build*Payload functions — each
// payload is built by call-core itself instead of a hand-assembled object
// literal per message type. Can return undefined on a panic inside
// call-core; a no-op, same as every other "call-core couldn't
// build/interpret this" case here.
function sendToConfirmedPeer(pairingId, ownPrivateKeyHex, peerPublicKey, buildPayload) {
  const payload = buildPayload();
  if (payload) publish(ownPrivateKeyHex, peerPublicKey, payload);
}
function sendConfirmedOrPending(pairingId, buildPayload) {
  const peer = findPairing(pairingId);
  if (peer) sendToConfirmedPeer(pairingId, peer.ownPrivateKeyHex, peer.peerPublicKey, buildPayload);
}

/** Publishes one bootstrap-phase event — plain, unencrypted JSON content
 * (no encryption is possible or needed at this phase: neither side knows
 * the other's real pubkey until this exchange itself reveals it, and a
 * SPAKE2 blinded message is safe to publish in the open by construction),
 * self-signed by the attempt's own keypair, tagged with the rendezvous `d`
 * tag and (once known) the candidate's own `p` tag —
 * callCore.buildBootstrapEvent's job now (see its own doc). */
async function sendPairingBootstrap(ownPrivateKeyHex, tag, targetPubkeyHex, payloadObj) {
  try {
    const eventJson = callCore.buildBootstrapEvent(ownPrivateKeyHex, tag, targetPubkeyHex ?? null, JSON.stringify(payloadObj));
    if (!eventJson) return;
    await publishToRelays(JSON.parse(eventJson), JSON.stringify(payloadObj));
  } catch (err) {
    console.error('failed to publish bootstrap message', err);
  }
}

/** Tells every confirmed peer this client is going away, so they don't have
 * to wait out the online timeout — the "leaving" analog of the old Ably
 * presence.leave. In-progress pairing attempts aren't told anything (see
 * NostrSignalingClient.kt's close() doc). Best-effort: fire-and-forget,
 * racing page teardown. */
function announceLeaving() {
  for (const peer of confirmedPeers()) sendToConfirmedPeer(peer.pairingId, peer.ownPrivateKeyHex, peer.peerPublicKey, () => callCore.buildLeavingPayload());
}
window.addEventListener('beforeunload', announceLeaving);

// Two tabs of this same client, open at once, otherwise silently clobber
// each other: `pairings` is loaded into memory once at page load and every
// mutation (addOrUpdatePairing/removePairing) writes that whole in-memory
// array back to localStorage with no re-read first - found live, 2026-10-01,
// via a deliberate hunt for bugs in this shape after finding an unrelated
// one. The `storage` event only ever fires in *other* tabs of the same
// origin (never the tab that made the change), which is exactly what's
// needed here: pick up the other tab's write before this tab's own next
// save would otherwise overwrite it with a stale copy.
window.addEventListener('storage', (event) => {
  if (event.key !== PAIRINGS_STORAGE_KEY) return;
  pairings = loadPairings();
  render();
});

// Mobile browsers throttle/pause background tabs — timers (including
// scheduleHeartbeat's own setTimeout) can stall for far longer than their
// nominal delay, and the relay WebSocket connection can drop entirely,
// while the phone is asleep or the tab is backgrounded. Without this, a
// contact looks stuck "offline" (and a call placed right away can fail to
// connect) for however long it takes the *next* naturally-scheduled
// heartbeat to fire after coming back — found live: calling right after
// waking the phone didn't connect, but trying again immediately after did,
// since that second attempt's own kickHeartbeat (sendCall's own call) was
// what actually got a fresh heartbeat out. kickHeartbeat() here does the
// same thing proactively, the moment the tab is actually looked at again,
// instead of waiting for a retry to stumble into it.
document.addEventListener('visibilitychange', () => {
  if (document.visibilityState !== 'visible') {
    // Hidden: after a grace period (so a quick look at another app doesn't
    // make this client look offline) and only if nothing is going on, stop
    // using the network until the page is looked at again.
    clearTimeout(pauseTimeoutHandle);
    pauseTimeoutHandle = setTimeout(maybePauseConnection, PAUSE_WHEN_HIDDEN_MS);
    return;
  }
  clearTimeout(pauseTimeoutHandle);
  if (connectionPaused) resumeConnection();
  else {
    callCore.requestHello();
    kickHeartbeat();
  }
  // iOS pauses a playing video when the page is backgrounded; resume it
  // rather than leaving a paused frame for Safari to decorate.
  for (const video of document.querySelectorAll('video')) {
    if (video.srcObject && video.paused) keepPlaying(video);
  }
});

// ---------------------------------------------------------------------------
// Pause while hidden. A hidden mobile page can't take a call anyway (browsers
// freeze it), so keeping the relay sockets, their pings and the 25 s heartbeat
// running only drains the battery. After PAUSE_WHEN_HIDDEN_MS hidden with
// nothing going on, tell the contacts we're leaving, close the relays and stop
// the timers; coming back reconnects and says hello (resumeConnection). Never
// while a call is ringing, being placed or live, a call is waiting for a
// contact to come online, or a pairing attempt is running.
// ---------------------------------------------------------------------------
let connectionPaused = false;
let pausingInFlight = null;
let pauseTimeoutHandle = null;

function connectionIsIdle() {
  const pairingAttemptLive = pendingPairingsList().some((p) => p.rendezvousTag);
  // isCallActive covers a ringing, placed or live call and one deferred until
  // the contact comes online (that holds the call slot too).
  return !callCore.isCallActive() && !pc && !pairingAttemptLive;
}

function maybePauseConnection() {
  if (document.visibilityState === 'visible' || connectionPaused || !pool || !connectionIsIdle()) return;
  pauseConnection();
}

async function pauseConnection() {
  connectionPaused = true;
  clearTimeout(heartbeatTimeoutHandle);
  clearInterval(onlineCheckTimer);
  onlineCheckTimer = null;
  // The "leaving" messages have to be out before the sockets close.
  pausingInFlight = (async () => {
    const sends = confirmedPeers().map((peer) => {
      const payload = callCore.buildLeavingPayload();
      return payload ? publish(peer.ownPrivateKeyHex, peer.peerPublicKey, payload) : null;
    });
    await Promise.allSettled(sends);
    if (wrapSub) { wrapSub.close(); wrapSub = null; }
    if (bootstrapSub) { bootstrapSub.close(); bootstrapSub = null; }
    try { pool.close(RELAYS); } catch { /* already closed */ }
    pool = null;
    // Everyone is unreachable from here until we reconnect; don't show the
    // pre-pause dots for the moment before the hellos come back.
    applyPresenceUpdate(callCore.checkOnlineTimeouts(Date.now() + 24 * 3600 * 1000));
  })();
  await pausingInFlight;
  pausingInFlight = null;
}

async function resumeConnection() {
  if (pausingInFlight) await pausingInFlight;
  if (!connectionPaused) return;
  connectionPaused = false;
  callCore.requestHello();
  connectRelayClient();
  render();
}

// The browser regained network (Wi-Fi back, airplane mode off): same staleness
// as coming back to a backgrounded tab — say hello so peers answer right away.
window.addEventListener('online', () => {
  callCore.requestHello();
  kickHeartbeat();
});

// ---------------------------------------------------------------------------
// Pairing (SPAKE2) — mirrors CameraAgentService.kt's PakeAttempt/
// beginPakeAttempt/confirmPeer. Fully symmetric,
// no generator/enterer roles: both sides run the exact same "Enter a
// phrase" flow (see the pairing design doc).
// ---------------------------------------------------------------------------

/** The one pairing attempt in progress, or null. It exists only in memory — nothing is saved until a person
 * confirms the match — and `call-core` owns the protocol state; this is what the screens show.
 * `status`: 'preparing' (turning the phrase into the meeting point), 'waiting', 'matched' (the exchange confirmed
 * `candidate`, awaiting the "Pair with [name]?" tap), 'collided' or 'timedOut'. */
let attempt = null; // { id, startedAt, status, candidate: { pubkeyHex, name } | null }

// AUTO_DISMISS_DELAY_MS (imported above) applies only to pairing-progress'
// collision/timeout screens and call-outcome — never confirm-delete or the
// name-confirm ("Pair with [name]?") screen, both real decisions, not passive
// outcomes.
let pairingProgressAutoDismissHandle = null;

/** Starts a passphrase pairing attempt for a brand-new contact — mirrors CameraAgentService.startPairing. The
 * passphrase and the attempt's fresh keypair are never persisted: call-core holds the attempt in memory (its
 * trim/NFC-normalization, rendezvous tag, live-window timeout all live there) and hands back the finished contact
 * when a person confirms the match. Fills in `mine` (the attempt object the screens show) with the new id. */
function startPairing(mine, passphrase) {
  const start = JSON.parse(callCore.startAttempt(deviceName, passphrase));
  mine.id = start.pairing_id;
  mine.startedAt = Date.now();
  mine.status = 'waiting';
  kickHeartbeat();
  // handleTimeout's own generation check makes this a no-op for an attempt that has already ended.
  setTimeout(() => applyEffects(callCore.handleTimeout(start.pairing_id, start.generation)), PAKE_LIVE_WINDOW_MS);
}

/** Gives up on the attempt in progress (cancel, retry, auto-dismiss). */
function discardAttempt() {
  if (attempt && attempt.id) callCore.cancelAttempt(attempt.id);
  attempt = null;
}

/**
 * Executes whatever call-core Effects a call into it returned — the entire
 * imperative-shell half of the pairing-bootstrap state machine lives in
 * this one function now, mirroring CallCoreBridge.kt's Android twin
 * (`applyEffects`) field-for-field. See each Effect variant's own doc
 * (call-core's `Effect` enum) for what it means. [effectsJson] is the raw
 * JSON array call-core's WASM functions always return (possibly empty,
 * never undefined).
 */
function applyEffects(effectsJson) {
  const effects = JSON.parse(effectsJson);
  if (effects.length === 0) return;
  for (const effect of effects) {
    switch (effect.kind) {
      case 'SendBootstrap': {
        sendPairingBootstrap(effect.own_private_key_hex, effect.rendezvous_tag, effect.target_pubkey, effect.payload);
        break;
      }
      case 'KickHeartbeat':
        kickHeartbeat();
        break;
      case 'SetCollision':
        // Dropping the candidate here disarms an already-shown confirm
        // screen's Confirm button and triggers render()'s reactive bail-back
        // to pairing-progress — see render()'s own doc for why both
        // directions of that swap matter.
        if (attempt && attempt.id === effect.pairing_id) {
          attempt.status = 'collided';
          attempt.candidate = null;
          schedulePairingProgressAutoDismiss(effect.pairing_id);
        }
        break;
      case 'SetTimedOut':
        if (attempt && attempt.id === effect.pairing_id) {
          attempt.status = 'timedOut';
          schedulePairingProgressAutoDismiss(effect.pairing_id);
        }
        break;
      case 'SetConfirmedCandidate':
        if (attempt && attempt.id === effect.pairing_id) {
          attempt.status = 'matched';
          attempt.candidate = { pubkeyHex: effect.pubkey_hex, name: effect.name };
        }
        break;
      default:
        console.error('applyEffects: unknown effect kind from call-core', effect.kind);
    }
  }
  render();
}

/**
 * Drives the entire SPAKE2 pairing state machine for one incoming bootstrap
 * message — mirrors CallCoreBridge.kt's Android twin
 * (`handleBootstrapMessage`) exactly: this function's entire job is
 * forwarding the event into call-core and executing whatever Effects come
 * back, no decision logic of its own left. [payload] is the already-
 * JSON.parse'd wire message (`{"type":"pake1",...}` or
 * `{"type":"pake-confirm",...}`).
 */
/** A human tapped "Pair with [name]?" for the candidate the SPAKE2 exchange already cryptographically confirmed —
 * mirrors CameraAgentService.confirmPeer. call-core ends the attempt and hands back the finished contact, which is
 * saved here for the first time. */
function confirmPeer() {
  if (!attempt || !attempt.candidate) return;
  const json = callCore.confirmAttempt(attempt.id, attempt.candidate.pubkeyHex);
  attempt = null;
  if (!json) return; // the attempt ended in the meantime; render() bails back
  const confirmed = JSON.parse(json);
  addOrUpdatePairing({
    id: confirmed.pairing_id,
    ownPrivateKeyHex: confirmed.own_private_key_hex,
    peerPublicKey: confirmed.peer_public_key,
    peerName: confirmed.peer_name,
    lastSignalCreatedAt: 0,
    lastSignalEventId: '',
  });
  kickHeartbeat();
  applyPresenceUpdate(callCore.markSeen(confirmed.pairing_id, ownPubkeyHexFor(confirmed.own_private_key_hex), confirmed.peer_public_key, Date.now(), null, false));
  screen = 'waiting';
  render();
}

// ---------------------------------------------------------------------------
// Call arbitration + WebRTC (shared across every pairing — only one call
// can be live at a time, mirrors CameraAgentService/WebRtcEngine).
//
// See call-core/src/call_arbitration.rs's module docs ("Invariants") before
// changing hangUp, requestCall,
// acceptIncomingCall, or anything else call-arbitration-related — the single
// write-up of the invariants that module enforces. This file is the
// imperative shell around it: forward events in, execute whatever
// CallEffects come back.
// ---------------------------------------------------------------------------

// activePairingId is purely a UI-facing mirror of call-core's own state,
// kept in sync by applyCallEffects/onPeerConnected below — not the source
// of truth. Every CallEffect that needs a callId carries it directly.
let activePairingId = null;
let pc = null;
// Which pairing/call the current `pc` belongs to — set once when a new
// RTCPeerConnection is created (ensurePeerConnection), read by every
// callback for that pc's whole lifetime, cleared in closePeerConnection.
// Mirrors WebRtcEngine.kt's pcPairingId/pcCallId: reading a value captured
// once when negotiation started, instead of a live mutable field an async
// callback might race, is a real correctness property, not just plumbing.
let pcPairingId = null;
let pcCallId = null;

/**
 * Non-null exactly while an incoming call for [activePairingId] is ringing
 * and hasn't been applied to the peer connection yet — mirrors
 * CameraAgentService's AgentState.incomingCall (a UI-facing mirror set by
 * applyCallEffects's StartRinging case; call-core owns the real pending
 * offer/ICE buffer internally). { pairingId, callId }. No auto-answer on
 * web — always a manual Accept/Decline choice.
 */
let pendingIncomingCall = null;
/** The person accepted the incoming call and the media isn't up yet — one of
 * the inputs to call-core's call phase. Reset with the call controls. */
let acceptedIncoming = false;
let incomingCallTickHandle = null;
// True from the moment StartRinging fires until acquireLocalStream()'s
// promise for *this* incoming call settles — disables Accept/Decline for
// exactly that window. getUserMedia()'s own permission prompt does not
// block the page underneath it from receiving clicks on every major
// browser, so a tap aimed at the browser's own "Allow"/"Block" UI can
// physically land on this page's real Accept or Decline button instead —
// found live (looked like unwanted auto-answer, which web has no such
// feature for). Guards both buttons, not just Accept: an accidental
// Decline on a wanted call is just as bad.
let incomingCallAwaitingMedia = false;

// Set by applyCallEffects's ShowCallOutcome case, cleared once the
// call-outcome screen is left (Call again or Back) — mirrors
// CameraAgentService's AgentState.pendingCallOutcome. { pairingId, callId, reason }.
let pendingCallOutcome = null;
let callOutcomeAutoDismissHandle = null;

function clearIncomingCall() {
  clearTimeout(incomingCallTickHandle);
  incomingCallTickHandle = null;
  pendingIncomingCall = null;
  incomingCallAwaitingMedia = false;
}

/** Accepts whatever is currently pending (manual Accept click — web has no
 * auto-answer) — see callCore.acceptIncomingCall's/AcceptOutcome's own docs
 * for the two things the result can mean: `kind: 'ApplyOffer'` (the peer's
 * SDP was already in hand — apply it, replaying any ICE candidates buffered
 * while it was ringing) or `kind: 'CreateOffer'` (this side won the pubkey
 * tie-break on a `"call"` message — nothing negotiated yet, so *now* is
 * when the real offer gets created and sent). A no-op if there's nothing
 * pending. */
async function acceptIncomingCall() {
  primeVideos();
  const json = callCore.acceptIncomingCall(Date.now());
  if (!json) return;
  const result = JSON.parse(json);
  clearIncomingCall();
  acceptedIncoming = true;
  render();
  activePairingId = result.pairing_id;
  if (result.kind === 'ApplyOffer') {
    await handleOffer(result.pairing_id, result.call_id, result.sdp).catch((err) => failCallAttempt('handleOffer failed (bad SDP?)', err));
    for (const ice of result.ice_buffer) applyIceCandidate(ice.sdp_mid, ice.sdp_m_line_index, ice.candidate);
  } else if (result.kind === 'CreateOffer') {
    await makeOffer(result.pairing_id, result.call_id).catch((err) => failCallAttempt('makeOffer failed', err));
  } else {
    console.error('acceptIncomingCall: unknown AcceptOutcome kind from call-core', result.kind);
  }
}
let localStream = null;
let callActive = false;

// In-flight acquisition, if any — requestCall() calls acquireLocalStream()
// eagerly (fire-and-forget, for the self-view) while ensurePeerConnection()
// awaits its own call moments later; without tracking this, both calls
// would see `localStream` still null and each kick off a *separate* real
// getUserMedia() call, leaking whichever stream loses the race.
let acquireLocalStreamPromise = null;
/** The name of the last getUserMedia failure ("NotAllowedError", "NotFoundError", "TimeoutError", ...), so the
 * call-ended screen can say what to do. Reset at the start of every attempt. */
let lastMediaErrorName = null;
/** True while the browser's camera/microphone permission request is taking suspiciously long. */
let mediaWaitNote = false;
const MEDIA_PERMISSION_NOTE_MS = 6000;
const MEDIA_PERMISSION_GIVE_UP_MS = 45000;

/** Assigns [stream] to [video] only if it isn't already there. Setting
 * `srcObject` re-runs the browser's media-element load algorithm even for the
 * identical stream, which visibly blanks/flickers the video — and render()
 * (called on every presence/heartbeat update) re-applies it constantly. */
function setVideoSource(video, stream) {
  if (video.srcObject === stream) return;
  video.srcObject = stream;
  // Not left to the `autoplay` attribute: iOS Safari refuses that outright
  // in Low Power Mode (even for a muted video), and then puts its own big
  // play button over the video.
  keepPlaying(video);
}

/** Asks every video to play right now, while the tap that started or
 * accepted the call is still fresh. iOS Safari only lets a page start video
 * after a user gesture, and the call's own video arrives seconds later, long
 * after the gesture has expired. A play() made inside the gesture is
 * remembered for the element, so when the stream is attached afterwards it
 * is allowed to start without another tap. Must run first in the tap
 * handler, before anything async. */
function primeVideos() {
  for (const video of document.querySelectorAll('video')) video.play().catch(() => {});
}

/** Starts [video], and if the browser refuses (Safari, for an unmuted stream
 * not started by a direct gesture) tries again on the next tap or click
 * instead of leaving it paused: a paused frame is what makes iOS Safari put
 * its own play control on top of the video. */
function keepPlaying(video) {
  video.play().catch(() => {
    const resume = () => {
      document.removeEventListener('touchend', resume, true);
      document.removeEventListener('click', resume, true);
      keepPlaying(video);
    };
    document.addEventListener('touchend', resume, true);
    document.addEventListener('click', resume, true);
  });
}

/** Opens the camera/mic for an actual call attempt — called right when one
 * starts (requestCall, or receiving an offer via ensurePeerConnection),
 * never before. Idempotent — same as WebRtcEngine.acquireMedia. */
async function acquireLocalStream() {
  if (localStream) return localStream;
  if (acquireLocalStreamPromise) return acquireLocalStreamPromise;
  lastMediaErrorName = null;
  acquireLocalStreamPromise = (async () => {
    // The browser's permission prompt can legitimately take a while (the person has to tap Allow), so a slow
    // answer first only shows a note; a prompt that never comes (iOS browsers without the phone-level
    // permission don't always ask or refuse) eventually fails the attempt instead of hanging forever.
    mediaWaitNote = false;
    const noteTimer = setTimeout(() => { mediaWaitNote = true; render(); }, MEDIA_PERMISSION_NOTE_MS);
    let giveUpTimer;
    const request = navigator.mediaDevices.getUserMedia({ video: { width: { ideal: CAPTURE.width }, height: { ideal: CAPTURE.height }, frameRate: { ideal: CAPTURE.fps } }, audio: true });
    const giveUp = new Promise((_, reject) => {
      giveUpTimer = setTimeout(() => reject(Object.assign(new Error('getUserMedia: no answer'), { name: 'TimeoutError' })), MEDIA_PERMISSION_GIVE_UP_MS);
    });
    // If the browser answers only after we gave up, release what it hands over (the camera light would stay on).
    request.then((late) => { if (lastMediaErrorName === 'TimeoutError') late.getTracks().forEach((tr) => tr.stop()); }, () => {});
    try {
      const stream = await Promise.race([request, giveUp]);
      // The call attempt that triggered this may already have been
      // cancelled/ended while getUserMedia was still pending (a fast
      // Call-then-Cancel) — shut it down immediately instead of adopting it.
      if (activePairingId === null) {
        stream.getTracks().forEach((t) => t.stop());
      } else {
        localStream = stream;
        setVideoSource(localVideoEl, localStream);
        render();
      }
    } catch (err) {
      // A denied/unavailable camera would otherwise leave the user stranded
      // on the calling/incoming-call screen forever with no explanation —
      // failCallAttempt tears the attempt down and lets call-core's own
      // outcome surface — told first that it was this device's camera or
      // microphone, so the message doesn't blame the network.
      lastMediaErrorName = err && err.name ? err.name : 'Error';
      callCore.noteMediaFailure();
      failCallAttempt('camera/mic access failed', err);
    } finally {
      clearTimeout(noteTimer);
      clearTimeout(giveUpTimer);
      mediaWaitNote = false;
      acquireLocalStreamPromise = null;
      render();
    }
    return localStream;
  })();
  return acquireLocalStreamPromise;
}

/** Closes the camera/mic the moment a call ends — mirrors
 * WebRtcEngine.releaseMedia, called from cleanupPeerConnection. */
function releaseLocalStream() {
  if (!localStream) return;
  localStream.getTracks().forEach((t) => t.stop());
  localStream = null;
  localVideoEl.srcObject = null;
}

/**
 * Executes whatever CallEffects a call into call-core returned — mirrors
 * applyEffects above (pairing-bootstrap's own twin) and
 * CameraAgentService.applyCallEffects on Android. activePairingId is set
 * directly here for every effect that means "the call slot is now claimed"
 * (CreateOffer/SendCall/ApplyRemoteOffer/StartRinging) — matches
 * call_arbitration invariant #1: the slot is claimed the moment a call *could*
 * happen, not once it's accepted. It's cleared the other way, inside
 * closePeerConnection, not here.
 */
function applyCallEffects(effectsJson) {
  const effects = JSON.parse(effectsJson);
  for (const effect of effects) {
    switch (effect.kind) {
      case 'AcquireMedia':
        acquireLocalStream();
        break;
      case 'CreateOffer':
        activePairingId = effect.pairing_id;
        // .catch, not fire-and-forget bare: a rejection here (WebRTC
        // negotiation failure) would otherwise be a silent unhandled
        // promise rejection. Also tears the attempt down via
        // failCallAttempt — otherwise the user is stranded on the calling
        // screen forever with no explanation.
        makeOffer(effect.pairing_id, effect.call_id).catch((err) => failCallAttempt('makeOffer failed', err));
        break;
      case 'SendCall':
        activePairingId = effect.pairing_id;
        sendConfirmedOrPending(effect.pairing_id, () => callCore.buildCallPayload(effect.call_id));
        break;
      case 'SendBusy':
        sendBusy(effect.pairing_id, effect.call_id);
        break;
      case 'ApplyRemoteOffer':
        activePairingId = effect.pairing_id;
        handleOffer(effect.pairing_id, effect.call_id, effect.sdp).catch((err) => failCallAttempt('handleOffer failed (bad SDP?)', err));
        break;
      case 'StartRinging':
        activePairingId = effect.pairing_id;
        // See incomingCallAwaitingMedia's own doc: disabled until this
        // exact call's media acquisition has settled, re-enabled only if
        // this is still the pending call by then — a stray hangup or a
        // fresh different incoming call must not have a late-arriving
        // re-enable touch buttons that aren't its own.
        acquireLocalStream().finally(() => {
          if (pendingIncomingCall?.pairingId === effect.pairing_id && pendingIncomingCall?.callId === effect.call_id) {
            incomingCallAwaitingMedia = false;
            render();
          }
        });
        pendingIncomingCall = {
          pairingId: effect.pairing_id,
          callId: effect.call_id,
        };
        callPairingId = effect.pairing_id;
        resetCallControls();
        screen = 'incoming-call';
        break;
      case 'SendBye':
        sendConfirmedOrPending(effect.pairing_id, () => callCore.buildByePayload(effect.call_id));
        break;
      case 'ClosePeerConnection':
        closePeerConnection();
        break;
      case 'ClearIncomingCallTimer':
        clearIncomingCall();
        break;
      case 'KickHeartbeat':
        // A call placed to a peer we think is offline: one immediate
        // heartbeat carrying `hello` (call-core already requested it).
        kickHeartbeat();
        break;
      case 'ShowCallOutcome':
        pendingCallOutcome = { pairingId: effect.pairing_id, callId: effect.call_id, reason: effect.reason };
        callPairingId = effect.pairing_id;
        screen = 'call-outcome';
        clearTimeout(callOutcomeAutoDismissHandle);
        callOutcomeAutoDismissHandle = setTimeout(() => {
          // Guards against a stale timer outliving the screen it was meant
          // for (the person already left it some other way).
          if (screen !== 'call-outcome') return;
          pendingCallOutcome = null;
          screen = 'waiting';
          render();
        }, AUTO_DISMISS_DELAY_MS);
        break;
      default:
        console.error('applyCallEffects: unknown effect kind from call-core', effect.kind);
    }
  }
  render();
}

/** Starts a call attempt — the only way any call ever starts. See
 * callCore.requestCall's own doc for the tie-break/deferred-call design (now
 * unified there, no longer split between this section and this file's old
 * setPeerOnline). Always attempted, regardless of known internet/signaling
 * state — deliberately not pre-checked: acquireLocalStream(), triggered
 * downstream via the StartRinging effect below, is what actually triggers
 * the browser's real camera/mic permission prompt when needed, and a
 * pre-check blocking this call would mean that prompt never fires at all,
 * leaving no way to grant access in the first place. */
function requestCall(pairingId) {
  primeVideos();
  const peer = findPairing(pairingId);
  if (!peer) return;
  const ownPubkeyHex = ownPubkeyHexFor(peer.ownPrivateKeyHex);
  const peerOnline = callCore.isOnline(pairingId);
  const result = JSON.parse(callCore.requestCall(pairingId, ownPubkeyHex, peer.peerPublicKey, peerOnline, Date.now()));
  if (result.call_id !== null) activePairingId = pairingId;
  applyCallEffects(JSON.stringify(result.effects));
  if (result.call_id !== null) startCallingUi(pairingId);
}

function hangUp() {
  applyCallEffects(callCore.hangUp());
}

function sendBusy(pairingId, callId) { sendConfirmedOrPending(pairingId, () => callCore.buildBusyPayload(callId)); }

// ICE-candidate evidence for the current PeerConnection, and the "why did this
// call never connect?" diagnosis ('no_direct_path' | 'udp_blocked' | undefined)
// drawn from it, live in call-core's ice_evidence module (shared with Android's
// WebRtcEngine) — this file only reports the PeerConnection's own events
// (iceReset/iceNoteLocalCandidate/iceNoteRemoteCandidate/iceNoteConnected) and
// its current state at teardown (iceRememberDiagnosis). renderCallOutcomeScreen
// reads callCore.iceLastDiagnosis().

/** [pairingId]/[callId] come from whichever CallEffect triggered this
 * (CreateOffer, ApplyRemoteOffer, or the accept flow) — captured into
 * pcPairingId/pcCallId here for this pc's whole lifetime, not read back
 * from the removed activePairingId/activeCallId globals later. Mirrors
 * WebRtcEngine.kt's newPeerConnection — see its own doc. */
async function ensurePeerConnection(pairingId, callId) {
  if (pc) return pc;
  await acquireLocalStream();
  // Re-validate after the await — acquireLocalStream can take real
  // wall-clock time (a permission prompt, or waiting on an already-in-flight
  // acquisition), and this exact call can have been hung up, or already had
  // its own PeerConnection built by a second, concurrent caller, while we
  // were waiting. Without this check, an already-ended call would still get
  // a live PeerConnection built for it here: orphaned, never torn down
  // until the page reloads.
  if (activePairingId !== pairingId || pc) return pc;
  callCore.iceReset();
  pcPairingId = pairingId;
  pcCallId = callId;
  pc = new RTCPeerConnection({
    iceServers: STUN_SERVERS.map((urls) => ({ urls })),
  });
  if (localStream) for (const track of localStream.getTracks()) pc.addTrack(track, localStream);
  pc.ontrack = (event) => {
    setVideoSource(remoteVideoEl, event.streams[0]);
    // Autoplay can be refused (Safari especially, for an unmuted stream not
    // started by a direct gesture) — which would otherwise leave a paused
    // frame with the browser's own play affordance on top of it.
    keepPlaying(remoteVideoEl);
  };
  pc.onconnectionstatechange = () => {
    callActive = pc.connectionState === 'connected';
    if (callActive) callCore.iceNoteConnected();
    // contactUiState's own `connected` flag mirrors Android's
    // WebRtcEngine.onPeerConnected, which sets it on the same
    // connectionState transition — without it the "Call" button never
    // disappears once a call to that exact contact is already connected.
    if (pcPairingId) contactUiState.set(pcPairingId, { ...uiState(pcPairingId), connected: callActive });
    if (callActive) {
      screen = 'call';
      if (pcPairingId && pcCallId) callCore.markConnected(pcPairingId, pcCallId);
    } else if (pc.connectionState === 'failed' || pc.connectionState === 'closed') {
      closePeerConnection();
    }
    render();
  };
  pc.onicecandidate = (event) => {
    if (event.candidate) {
      callCore.iceNoteLocalCandidate(event.candidate.candidate, event.candidate.type);
    }
    if (event.candidate && pcPairingId) {
      sendConfirmedOrPending(pcPairingId, () =>
        callCore.buildIcePayload(event.candidate.sdpMid, event.candidate.sdpMLineIndex, event.candidate.candidate, pcCallId));
    }
  };
  return pc;
}

async function makeOffer(pairingId, callId) {
  await ensurePeerConnection(pairingId, callId);
  // Re-checked here too, not just inside ensurePeerConnection: that
  // function's own await gap means it can come back with a stale/null pc,
  // or a pc that belongs to a different pairing/call than the one this
  // function was asked for.
  if (!pc || pcPairingId !== pairingId || pcCallId !== callId) return;
  const offer = await pc.createOffer();
  await pc.setLocalDescription(offer);
  sendConfirmedOrPending(pairingId, () => callCore.buildOfferPayload(offer.sdp, callId));
}
async function handleOffer(pairingId, callId, sdp) {
  await ensurePeerConnection(pairingId, callId);
  // See makeOffer's identical check just above for why.
  if (!pc || pcPairingId !== pairingId || pcCallId !== callId) return;
  await pc.setRemoteDescription({ type: 'offer', sdp });
  const answer = await pc.createAnswer();
  await pc.setLocalDescription(answer);
  sendConfirmedOrPending(pairingId, () => callCore.buildAnswerPayload(answer.sdp, callId));
}
// Set synchronously the instant an answer is accepted for applying, not
// left to pc.signalingState — that only flips once setRemoteDescription's
// async work actually completes, leaving a window where a near-simultaneous
// duplicate (redelivery is real under relay load) could pass a
// signalingState-only check too. A *second*, independent guard from
// call-core's own pairingId/callId check (done by its signal router before an
// answer ever reaches here): this one is about *this call's own negotiation
// state*, that one's about *which call* — call-core has no visibility into
// pc, so it can't replace this half.
let answerApplied = false;

async function handleAnswer(sdp) {
  if (!pc || pc.signalingState !== 'have-local-offer' || answerApplied) return;
  answerApplied = true;
  await pc.setRemoteDescription({ type: 'answer', sdp });
}

function applyIceCandidate(sdpMid, sdpMLineIndex, candidate) {
  // addIceCandidate returns a Promise that rejects asynchronously (e.g. if
  // this candidate raced ahead of the offer/answer's own
  // setRemoteDescription still in flight) — .catch(), not try/catch, which
  // only ever catches a synchronous throw.
  if (!pc) return;
  callCore.iceNoteRemoteCandidate(candidate);
  pc.addIceCandidate({ candidate, sdpMid, sdpMLineIndex }).catch((err) => console.warn('bad ice payload or candidate arrived before remote description', err));
}

/**
 * Shared failure path for anything that can strand a call attempt with no
 * user-facing explanation — a rejected getUserMedia, a bad/incompatible SDP
 * offer/answer. Reuses closePeerConnection() rather than inventing a
 * separate error UI: call-core's own active_pairing_id is already set by
 * this point (every caller is reached only after a CreateOffer/StartRinging
 * effect already claimed the call slot), so tearing the connection down
 * here makes call-core's existing peer_connection_closed() logic compute
 * ShowCallOutcome{NeverConnected} on its own — the same screen and copy a
 * genuine network failure would show.
 */
function failCallAttempt(label, err) {
  console.warn(label, err);
  if (activePairingId) closePeerConnection();
}

/**
 * The one place `pc` actually stops existing — tells call-core's own
 * bookkeeping this real PeerConnection is gone, for *any* reason, including
 * ones call-core never asked for. Also the decline path for an incoming
 * call that hasn't been accepted yet — `pc` is still null in that case; the
 * 'bye' this is called alongside still needs to go out so the caller isn't
 * left ringing forever (see hangUp()).
 */
function closePeerConnection() {
  clearIncomingCall();
  if (pc) { callCore.iceRememberDiagnosis(pc.iceConnectionState); pc.close(); pc = null; }
  // Explicit, not just left to onconnectionstatechange's own final event —
  // same belt-and-suspenders reasoning as Android's WebRtcEngine.
  if (pcPairingId) contactUiState.set(pcPairingId, { ...uiState(pcPairingId), connected: false });
  pcPairingId = null;
  pcCallId = null;
  // Applied, not discarded: a ShowCallOutcome effect (if any) sets `screen`
  // to 'call-outcome' below, before the fallback on the next line runs — so
  // that fallback correctly no-ops whenever an outcome screen was just set.
  applyCallEffects(callCore.peerConnectionClosed());
  callActive = false;
  activePairingId = null;
  answerApplied = false;
  releaseLocalStream();
  // A peer ending a call needs its next heartbeat kicked out immediately,
  // not left to the slow idle cadence — being on an active call isn't one
  // of currentHeartbeatIntervalMs's own fast-cadence conditions, so nothing
  // else speeds this up.
  kickHeartbeat();
  if (screen === 'calling' || screen === 'call' || screen === 'incoming-call') { screen = 'waiting'; render(); }
}

// ---------------------------------------------------------------------------
// Screens (mirrors AppRoot's `when` in MainActivity.kt +
// HomeScreens.kt/PassphrasePairingScreens.kt). One `screen` variable, one
// render() that shows exactly one <section>, same shape as the Compose
// `when` blocks it's porting.
// ---------------------------------------------------------------------------

let screen = 'name';
let callPairingId = null; // which pairing the call screens (calling, ringing, live) are about
let pendingDeletePairingId = null; // which pairing 'confirm-delete' applies to
const el = (id) => document.getElementById(id);
// A plain <input> outside a <form> does nothing on Enter by default — this
// is the one place that forwarding to a submit button's own click handler
// is wired, instead of each of the three text-entry screens (name entry,
// rename, add-contact) hand-duplicating the identical keydown listener.
const submitOnEnter = (inputId, buttonId) => {
  el(inputId).addEventListener('keydown', (e) => { if (e.key === 'Enter') el(buttonId).click(); });
};
const screens = {
  name: el('screenName'), waiting: el('screenWaiting'), settings: el('screenSettings'),
  rename: el('screenRename'), connection: el('screenConnection'), 'confirm-delete': el('screenConfirmDelete'),
  'enter-phrase': el('screenEnterPhrase'), 'pairing-progress': el('screenPairingProgress'),
  pair: el('screenPair'), calling: el('screenCall'), 'incoming-call': el('screenCall'), call: el('screenCall'),
  'call-outcome': el('screenCallOutcome'),
};
const remoteVideoEl = el('remoteVideo');
const localVideoEl = el('localVideo');

/**
 * The self-view's position during a call — mirrors the Android app's own
 * PreviewCorner cycle: one "Position self-view" button press advances to
 * the next entry, wrapping around, with a fifth "invisible" position after
 * the four corners. Index 0 is the CSS default (bottom-start) — every new
 * call starts there (see resetCallControls).
 */
let previewPositionIndex = 0;

/** Each corner square's own size, in the position button's own 24x24
 * viewBox — sized so its outward corner lands exactly on the outer
 * square's edge while its inward corner stays at 10 or 14. */
const PREVIEW_ICON_SQUARE_SIZE = 8;

/** Where each corner's square sits — shared by both the filled (active) and
 * outlined (inactive) variant, see buildPositionIconInner. */
const PREVIEW_ICON_SQUARES = {
  'top-start': [2, 2], 'top-end': [14, 2], 'bottom-start': [2, 14], 'bottom-end': [14, 14],
};

/** Stroke width for both the outer square and each inactive corner square —
 * shared so PREVIEW_ICON_OUTLINE_INSET (below) can size an outlined square's
 * own rect correctly against it. */
const PREVIEW_ICON_STROKE_WIDTH = 1.3;

/** An outer square (sharp corners) with a square at each corner: the one
 * matching [activeCorner] filled solid, the rest outlined, sized so an
 * *outlined* square's stroke lands exactly on the same footprint a *filled*
 * one occupies — SVG centers a stroke on its path by default, so a plain
 * full-size outlined rect at this stroke-width would paint larger than a
 * filled rect of the same nominal size; inset by half the stroke-width on
 * every side first so the stroke's own outer edge lands on the true
 * bounds. Built as a plain SVG-fragment string, not a static icon, since
 * which corner is filled has to track live state. */
function buildPositionIconInner(activeCorner) {
  let svg = `<rect x="2" y="2" width="20" height="20" fill="none" stroke="currentColor" stroke-width="1.5"/>`;
  for (const [corner, [x, y]] of Object.entries(PREVIEW_ICON_SQUARES)) {
    if (corner === activeCorner) {
      svg += `<rect x="${x}" y="${y}" width="${PREVIEW_ICON_SQUARE_SIZE}" height="${PREVIEW_ICON_SQUARE_SIZE}" fill="currentColor"/>`;
    } else {
      const inset = PREVIEW_ICON_STROKE_WIDTH / 2;
      const size = PREVIEW_ICON_SQUARE_SIZE - PREVIEW_ICON_STROKE_WIDTH;
      svg += `<rect x="${x + inset}" y="${y + inset}" width="${size}" height="${size}" fill="none" stroke="currentColor" stroke-width="${PREVIEW_ICON_STROKE_WIDTH}"/>`;
    }
  }
  return svg;
}

function applyPreviewPositionClass() {
  localVideoEl.classList.remove(...PREVIEW_POSITIONS.map((p) => `preview-${p}`));
  localVideoEl.classList.add(`preview-${PREVIEW_POSITIONS[previewPositionIndex]}`);
}

// #callControls itself jumps to the opposite bottom corner right as this
// runs, whenever the new position crosses into/out of 'bottom-end' — a
// second click aimed at the button's old spot lands on plain video instead,
// which the outside-click handler below would otherwise read as "dismiss."
// This grace window lets that stray next click land harmlessly instead of
// closing the whole panel.
let lastPositionChangeAt = 0;
const POSITION_CHANGE_DISMISS_GRACE_MS = 500;

function cyclePreviewPosition() {
  previewPositionIndex = (previewPositionIndex + 1) % PREVIEW_POSITIONS.length;
  applyPreviewPositionClass();
  renderCallControls();
  lastPositionChangeAt = Date.now();
}

// Real WebRTC mute, not just a local preview change — disabling a
// MediaStreamTrack makes it emit silence/black frames on its own (spec
// behavior), so this is all either toggle needs: no renegotiation, and it
// mutes for the peer too, not just this device's own preview.
let audioEnabled = true;
let videoEnabled = true;

function setAudioEnabled(enabled) {
  audioEnabled = enabled;
  if (localStream) for (const t of localStream.getAudioTracks()) t.enabled = enabled;
}
function setVideoEnabled(enabled) {
  videoEnabled = enabled;
  if (localStream) for (const t of localStream.getVideoTracks()) t.enabled = enabled;
}

/** Whether the call-controls overlay (#callControls) is currently up — see
 * showCallControls/hideCallControls. */
let callControlsVisible = false;

/** Once per call, when its UI first appears (startCallingUi / an incoming
 * call's ring) — a fresh call always starts with the
 * self-view in its default spot, both mute toggles on, and the controls
 * overlay open (it stays open once connected until dismissed), undoing
 * whatever a previous call left them at. */
function resetCallControls() {
  acceptedIncoming = false;
  previewPositionIndex = 0;
  applyPreviewPositionClass();
  audioEnabled = true;
  videoEnabled = true;
  callControlsVisible = true;
  el('callControls').hidden = false;
}

function renderCallControls() {
  const audioBtn = el('toggleAudio');
  audioBtn.classList.toggle('checked', audioEnabled);
  audioBtn.setAttribute('aria-checked', String(audioEnabled));
  const videoBtn = el('toggleVideo');
  videoBtn.classList.toggle('checked', videoEnabled);
  videoBtn.setAttribute('aria-checked', String(videoEnabled));
  const activeCorner = PREVIEW_POSITIONS[previewPositionIndex];
  el('previewPositionIcon').innerHTML = buildPositionIconInner(activeCorner);
}

function showCallControls() {
  callControlsVisible = true;
  el('callControls').hidden = false;
  renderCallControls();
}
function hideCallControls() {
  callControlsVisible = false;
  el('callControls').hidden = true;
}

// Click anywhere on the live-call screen to reveal the controls; click
// anywhere on it that isn't one of the controls themselves to dismiss them
// again — mirrors Android's Select-to-reveal/Back-to-dismiss. The grace
// check is cyclePreviewPosition's own doc.
el('screenCall').addEventListener('click', (e) => {
  if (screen !== 'call') return;
  if (callControlsVisible) {
    const inGracePeriod = Date.now() - lastPositionChangeAt < POSITION_CHANGE_DISMISS_GRACE_MS;
    if (!el('callControls').contains(e.target) && !inGracePeriod) hideCallControls();
  } else {
    showCallControls();
  }
});
// Keyboard equivalent of the click handler above: Enter / Space toggle the
// controls (like a click on the video), Escape / Backspace hide them, and Tab
// opens them and moves focus in. While focus is on one of the controls
// themselves, Enter / Space just press that control as usual — only Escape /
// Backspace (and a click on the video) hide them from there.
document.addEventListener('keydown', (e) => {
  if (screen !== 'call') return;
  const toggles = e.key === 'Enter' || e.key === ' ' || e.key === 'Spacebar';
  const tab = e.key === 'Tab';
  const inControls = el('callControls').contains(e.target);
  if (!callControlsVisible && (toggles || tab)) {
    e.preventDefault();
    showCallControls();
    // Tab moves into the controls (the last one for Shift+Tab); Enter / Space
    // leave focus alone, so pressing them again hides the controls.
    if (tab) el(e.shiftKey ? 'callDisconnect' : 'cyclePreviewPosition').focus();
  } else if (callControlsVisible && (e.key === 'Backspace' || e.key === 'Escape' || (toggles && !inControls))) {
    e.preventDefault();
    hideCallControls();
  }
});
el('cyclePreviewPosition').addEventListener('click', () => cyclePreviewPosition());
el('toggleAudio').addEventListener('click', () => { setAudioEnabled(!audioEnabled); renderCallControls(); });
el('toggleVideo').addEventListener('click', () => { setVideoEnabled(!videoEnabled); renderCallControls(); });
el('callDisconnect').addEventListener('click', () => hangUp());

// Per-contact UI state (status/connected), fed by the relay client above —
// mirrors CameraAgentService.ContactState. `status` is one of call-core's
// own PresenceStatus wire strings ('offline'/'online'/'busy') — exactly one
// at a time, never a combination. Live and continuously correct — no
// hand-rolled reset timer here; call-core's own presence module owns every
// transition.
const contactUiState = new Map();
function uiState(pairingId) { return contactUiState.get(pairingId) || { status: 'offline', connected: false }; }

function render() {
  // Reactive swap: the moment a candidate is cryptographically confirmed
  // for the pairing attempt currently in progress, jump straight to the
  // name-confirm screen.
  if (screen === 'pairing-progress' && attempt && attempt.status === 'matched') {
    screen = 'pair';
  }
  // The reverse case, just as load-bearing: if we're already showing the
  // confirm screen but its candidate has since been withdrawn (a late-
  // arriving second party collides the attempt), bail back to
  // pairing-progress so it shows the actual outcome instead of leaving a
  // stale, now-nonfunctional "Pair with [Name]?" screen with a Confirm
  // button that quietly does nothing. Android's equivalent has no such gap
  // by construction (one reactive `when` over live state); this is that
  // same guarantee, written explicitly for JS's non-reactive DOM.
  if (screen === 'pair' && attempt && attempt.status !== 'matched') {
    screen = 'pairing-progress';
  }
  // No attempt at all (cancelled from elsewhere): nothing left to show.
  if ((screen === 'pair' || screen === 'pairing-progress') && !attempt) screen = 'waiting';

  // Calling, ringing and live all show the one #screenCall (so the self-view
  // can animate between them), hence a Set rather than one entry per name.
  for (const node of new Set(Object.values(screens))) node.hidden = screens[screen] !== node;

  if (screen === 'waiting') renderWaitingScreen();
  if (screen === 'settings') renderSettingsScreen();
  if (screen === 'connection') renderConnectionScreen();
  // The connection page refreshes itself while it's showing, nothing else does.
  if (screen === 'connection' && !connectionRefreshTimer) connectionRefreshTimer = setInterval(() => { if (screen === 'connection') renderConnectionScreen(); }, 1000);
  if (screen !== 'connection' && connectionRefreshTimer) { clearInterval(connectionRefreshTimer); connectionRefreshTimer = null; }
  // The pairing progress screen counts up, so it refreshes itself while it's showing.
  if (screen === 'pairing-progress' && !pairingRefreshTimer) pairingRefreshTimer = setInterval(() => { if (screen === 'pairing-progress') renderPairingProgressScreen(); }, 1000);
  if (screen !== 'pairing-progress' && pairingRefreshTimer) { clearInterval(pairingRefreshTimer); pairingRefreshTimer = null; }
  if (screen === 'confirm-delete') renderConfirmDeleteScreen();
  if (screen === 'pairing-progress') renderPairingProgressScreen();
  if (screen === 'pair') renderPairScreen();
  if (screen === 'calling' || screen === 'incoming-call' || screen === 'call') renderCallScreen();
  if (screen === 'call-outcome') renderCallOutcomeScreen();
}

// --- Name entry (mirrors NameEntryScreen) -----------------------------

// sanitizeName + slice here too, not just on receipt: this name becomes a
// *peer's* self-reported name the moment it heartbeats out — capping/
// cleaning it at the source means every other device that ever sees it
// sees the same sanitized value this device shows.
el('nameContinue').addEventListener('click', async () => {
  const name = callCore.sanitizeName(el('nameInput').value.trim().slice(0, PROTOCOL_CONSTANTS.max_name_length)) || t('nameEntry.defaultNameWeb');
  deviceName = name;
  localStorage.setItem(DEVICE_NAME_STORAGE_KEY, deviceName);
  await startApp();
});
submitOnEnter('nameInput', 'nameContinue');

el('renameSave').addEventListener('click', () => {
  const name = callCore.sanitizeName(el('renameInput').value.trim().slice(0, PROTOCOL_CONSTANTS.max_name_length));
  if (!name) return;
  deviceName = name;
  localStorage.setItem(DEVICE_NAME_STORAGE_KEY, deviceName);
  heartbeatTick(); // push the new name out immediately, don't wait up to 25s
  // Same place renameCancel returns to — this screen is only ever reached
  // from Settings, so Save should land there too, not drop to the waiting
  // screen.
  screen = 'settings'; render();
});
submitOnEnter('renameInput', 'renameSave');
el('renameCancel').addEventListener('click', () => { screen = 'settings'; render(); });

// --- Waiting screen (mirrors WaitingScreen + ContactRow) ----------------

el('settingsBtn').addEventListener('click', () => { screen = 'settings'; render(); });

// The contact list is built once and then kept in step with `pairings` and
// the per-contact UI state: a status flip touches one dot, a rename one name,
// and only an added or removed contact adds or removes that contact's row.
// Rebuilding everything on each render() (presence updates and the status
// poll land here) dropped keyboard focus and any press in progress.
let waitingTopSpacer = null;
let waitingAddBtn = null;
const waitingRows = new Map(); // pairing id -> { group, dot, name, call, del }

function setAttr(node, name, value) { if (node.getAttribute(name) !== value) node.setAttribute(name, value); }
function setClass(node, value) { if (node.className !== value) node.className = value; }
function setText(node, value) { if (node.textContent !== value) node.textContent = value; }

function buildWaitingList(listEl) {
  listEl.innerHTML = '';
  // The first of .contact-list's two fade spacers — see its own doc
  // (styles.css) for why an empty row here, not scroll-position tracking,
  // keeps the permanent top fade from showing when there's nothing above to
  // scroll to.
  waitingTopSpacer = document.createElement('div');
  waitingTopSpacer.className = 'contact-list-fade-spacer top';
  waitingTopSpacer.setAttribute('aria-hidden', 'true');
  listEl.appendChild(waitingTopSpacer);
  // A blank row, not just a bigger gap — sets "Add contact" apart as its
  // own action rather than one more entry in the contact list.
  const spacer = document.createElement('div');
  spacer.className = 'contact-list-spacer';
  listEl.appendChild(spacer);
  // Trailing row, not a separate Settings destination. .add-contact spans
  // every grid column and centers itself within that span — a standalone
  // action, not one more row sized to a single contact-list column.
  waitingAddBtn = document.createElement('button');
  waitingAddBtn.className = 'tv-button add-contact';
  // A person-with-"+" icon — same Material "person_add" glyph path as
  // Android's PersonAddIcon.
  waitingAddBtn.innerHTML = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M15 12c2.21 0 4-1.79 4-4s-1.79-4-4-4-4 1.79-4 4 1.79 4 4 4zm-9-2V7H4v3H1v2h3v3h2v-3h3v-2H6zm9 4c-2.67 0-8 1.34-8 4v2h16v-2c0-2.66-5.33-4-8-4z"/></svg>';
  waitingAddBtn.addEventListener('click', () => openEnterPhrase());
  listEl.appendChild(waitingAddBtn);
  // The second of .contact-list's two fade spacers — see the top one's own
  // doc, and .contact-list's (styles.css).
  const bottomFadeSpacer = document.createElement('div');
  bottomFadeSpacer.className = 'contact-list-fade-spacer bottom';
  bottomFadeSpacer.setAttribute('aria-hidden', 'true');
  listEl.appendChild(bottomFadeSpacer);
}

function createWaitingRow(pairingId) {
  // No wrapping row div — nameGroup/Call/Delete are direct children of
  // #contactList (a CSS grid), three per contact, so grid-template-columns
  // sizes each column once across every row.
  const group = document.createElement('span');
  group.className = 'contact-name-group';
  // A plain filled circle, not a text word — the color alone is the whole
  // signal; role="img" + aria-label carry the same meaning for a screen
  // reader.
  const dot = document.createElement('span');
  dot.setAttribute('role', 'img');
  group.appendChild(dot);
  const name = document.createElement('span');
  name.className = 'contact-name';
  group.appendChild(name);
  // Always present, even when this contact isn't callable right now —
  // hidden via .tv-button.call:disabled { visibility: hidden }, not left
  // out of the grid, so column 2 keeps the same width on every row.
  // `disabled` also takes it out of tab order.
  const call = document.createElement('button');
  call.className = 'tv-button call';
  // A phone-handset icon, not the word "Call" — same Material glyph path
  // as Android's Icons.Filled.Call.
  call.innerHTML = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6.62 10.79c1.44 2.83 3.76 5.14 6.59 6.59l2.2-2.2c.27-.27.67-.36 1.02-.24 1.12.37 2.33.57 3.57.57.55 0 1 .45 1 1V20c0 .55-.45 1-1 1-9.39 0-17-7.61-17-17 0-.55.45-1 1-1h3.5c.55 0 1 .45 1 1 0 1.25.2 2.45.57 3.57.11.35.03.74-.25 1.02l-2.2 2.2z"/></svg>';
  call.addEventListener('click', () => requestCall(pairingId));
  // Destructive and irreversible (the pairing's own key is gone, not
  // just unlinked) — confirms via #screenConfirmDelete before actually
  // removing it. A real trash-can silhouette, same path data as
  // Android's DeleteBinIcon.
  const del = document.createElement('button');
  del.className = 'delete-icon-btn';
  del.innerHTML = '<svg viewBox="0 0 1024 1024" aria-hidden="true"><path d="M266.2 256l47.2 581.4c0 32.4 26.2 58.6 58.6 58.6h282c32.4 0 58.6-26.2 58.6-58.6L759.2 256H266.2z m123.2 530L376 320h37l13.8 466h-37.4z m140.6 0h-36V320h36v466z m104.6 0h-37.2l13.6-466H648l-13.4 466zM728 184h-72l-52.6-46c-7.4-6.4-16.8-10-26.4-10h-129.6c-9.8 0-19.4 3.6-26.8 10L368 184h-72c-35.2 0-60 16.8-60 52h552c0-35.2-24.8-52-60-52z"/></svg>';
  del.addEventListener('click', () => {
    pendingDeletePairingId = pairingId;
    screen = 'confirm-delete';
    render();
  });
  return { group, dot, name, call, del };
}

/**
 * Mirrors WaitingScreen's own contact list (HomeScreens.kt): per-contact
 * Delete lives right on each row (no auto-answer toggle here — that's
 * Android/TV-only), and the list ends with a plain "Add contact" row.
 */
function renderWaitingScreen() {
  const listEl = el('contactList');
  if (!waitingTopSpacer) buildWaitingList(listEl);
  const present = new Set(pairings.map((p) => p.id));
  for (const [id, row] of waitingRows) {
    if (present.has(id)) continue;
    row.group.remove(); row.call.remove(); row.del.remove();
    waitingRows.delete(id);
  }
  // Rows sit between the top spacer and the blank row before "Add contact",
  // in `pairings` order; a row is only moved when it isn't already there.
  let cursor = waitingTopSpacer;
  for (const pairing of pairings) {
    let row = waitingRows.get(pairing.id);
    if (!row) { row = createWaitingRow(pairing.id); waitingRows.set(pairing.id, row); }
    if (cursor.nextSibling !== row.group) {
      const before = cursor.nextSibling;
      listEl.insertBefore(row.group, before);
      listEl.insertBefore(row.call, before);
      listEl.insertBefore(row.del, before);
    }
    cursor = row.del;
    const state = uiState(pairing.id);
    const label = pairing.peerName || t('common.thisContact');
    // Busy/Online/Offline are purely informational — Call stays available
    // regardless (see requestCall's own doc): tapping Call on an offline
    // contact just defers, and on a busy one resolves via the real
    // busy-signal exchange.
    const [dotClass, dotLabel] = state.status === 'busy' ? ['contact-status-dot busy', t('contacts.statusBusy')]
      : state.status === 'online' ? ['contact-status-dot ok', t('contacts.statusOnline')]
      : ['contact-status-dot danger', t('contacts.statusOffline')];
    setClass(row.dot, dotClass);
    setAttr(row.dot, 'aria-label', dotLabel);
    setText(row.name, pairing.peerName || t('common.unnamedContact'));
    // The rule itself lives in call-core (call_arbitration::can_place_call),
    // shared with Android.
    const canCall = callCore.canPlaceCall(state.connected);
    if (row.call.disabled === canCall) row.call.disabled = !canCall;
    setAttr(row.call, 'aria-label', t('contacts.callContact', { name: label }));
    setAttr(row.del, 'aria-label', t('contacts.deleteContact', { name: label }));
  }
  setAttr(waitingAddBtn, 'aria-label', t('contacts.addContact'));
  updateContactListFadeShape();
}

/**
 * See .contact-list's own doc (styles.css) for what this decides and why
 * — measures the real, live gap between the list's right edge and the
 * settings icon's left edge, rather than assuming either fade shape for a
 * fixed screen size, since this viewport is resizable (unlike Android's
 * fixed-size Portal TV screen, which nonetheless runs the identical live
 * onGloballyPositioned-driven check in HomeScreens.kt, since Porchlight
 * isn't guaranteed to run only on one screen size there either). Half an
 * icon-width of clearance is the bar, read live from the same design
 * token the icon's own size comes from rather than a separate hardcoded
 * number.
 */
function updateContactListFadeShape() {
  if (screen !== 'waiting') return;
  const listEl = el('contactList');
  const listRect = listEl.getBoundingClientRect();
  const fabRect = el('settingsBtn').getBoundingClientRect();
  // A zero-width rect means one of the two isn't actually laid out (this
  // screen isn't the visible one, most likely) — nothing to conclude.
  if (listRect.width === 0 || fabRect.width === 0) return;
  const minGapPx = (parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--size-settings-fab')) || 0) / 2;
  listEl.classList.toggle('top-fade-plain', fabRect.left - listRect.right >= minGapPx);
}

// Coalesced onto one rAF per resize burst rather than firing per event —
// the browser can dispatch many 'resize' events across a single drag.
let fadeShapeResizeHandle = null;
window.addEventListener('resize', () => {
  if (fadeShapeResizeHandle) return;
  fadeShapeResizeHandle = requestAnimationFrame(() => {
    fadeShapeResizeHandle = null;
    updateContactListFadeShape();
  });
});

// --- Device settings (mirrors AdminChoiceScreen) ------------------------

function renderSettingsScreen() { el('renameInput').value = deviceName; }
el('renameBtn').addEventListener('click', () => { screen = 'rename'; render(); });
el('connectionBtn').addEventListener('click', () => { screen = 'connection'; render(); });
el('connectionBack').addEventListener('click', () => { screen = 'settings'; render(); });

// --- Status (mirrors ConnectionInfoScreen) --------------------

let connectionRefreshTimer = null;

function agoText(atMs, now) {
  return formatAgo(JSON.parse(callCore.ago(atMs ?? undefined, now)));
}

function formatAgo(ago) {
  switch (ago.unit) {
    case 'seconds': return t('connectionInfo.agoSeconds', { seconds: ago.n });
    case 'minutes': return t('connectionInfo.agoMinutes', { minutes: ago.n });
    case 'hours': return t('connectionInfo.agoHours', { hours: ago.n });
    default: return t('connectionInfo.never');
  }
}

function renderConnectionScreen() {
  const now = Date.now();
  const rows = [];
  const row = (label, value, tone = '') => rows.push({ label, text: value, tone });

  // Not navigator.connection.effectiveType: that is a speed class ("4g" just
  // means fast, even on Wi-Fi), and browsers don't say Wi-Fi vs. mobile.
  const online = navigator.onLine;
  row(t('connectionInfo.network'), online ? t('connectionInfo.networkOther') : t('connectionInfo.networkOffline'), online ? 'ok' : 'bad');

  // The pool keys relays by their normalized URL (a trailing slash), RELAYS
  // doesn't have one — call-core compares without it. Which dot, which error,
  // how it is trimmed and how long ago all come from call-core.
  const connectedUrls = pool ? [...pool.listConnectionStatus()].filter(([, up]) => up).map(([url]) => url) : [];
  const views = JSON.parse(callCore.relayView(RELAYS, connectedUrls, now));
  const up = views.filter((v) => v.state !== 'down').length;
  row(t('connectionInfo.relays'), t('connectionInfo.relaysSummary', { up, total: RELAYS.length }), up > 0 ? 'ok' : 'bad');
  for (const v of views) {
    const dot = { cls: { connected: 'ok', paused: 'busy', down: 'danger' }[v.state], label: v.state === 'down' ? t('connectionInfo.relayNotConnected') : t('connectionInfo.relayConnected') };
    const counts = t('connectionInfo.relayCounts', { accepted: v.accepted, rejected: v.rejected });
    const when = formatAgo(v.ago);
    rows.push({
      label: v.host,
      dot,
      id: v.host,
      text: v.error ? `${counts} · ${when}: ` : `${counts} · ${when}`,
      italic: v.error || '',
    });
  }

  row(t('connectionInfo.lastHeartbeat'), agoText(lastHeartbeatSentAt, now));
  const peers = confirmedPeers();
  row(t('connectionInfo.contactsOnline'), t('connectionInfo.contactsOnlineValue', { online: peers.filter((p) => uiState(p.pairingId).status !== 'offline').length, total: peers.length }));

  const container = el('connectionRows');
  container.replaceChildren(...rows.map(({ label, dot, id, text, italic, tone }) => {
    const node = document.createElement('div');
    node.className = 'info-row';
    const l = document.createElement('span'); l.className = 'label';
    if (dot) {
      const d = document.createElement('span');
      d.className = 'contact-status-dot ' + dot.cls;
      d.setAttribute('role', 'img');
      d.setAttribute('aria-label', dot.label);
      l.append(d);
    }
    l.append(label);
    const v = document.createElement('span');
    v.className = 'value' + (tone ? ' ' + tone : '') + (dot ? ' dim relay' : '');
    v.append(text);
    if (italic) {
      const em = document.createElement('em');
      em.textContent = italic;
      v.append(em);
      // One line, ellipsized; the full text on hover (title) and, for touch,
      // on tap (toggles wrapping).
      v.title = text + italic;
      if (expandedRelayRows.has(id)) v.classList.add('expanded');
      v.addEventListener('click', () => {
        if (expandedRelayRows.has(id)) expandedRelayRows.delete(id); else expandedRelayRows.add(id);
        v.classList.toggle('expanded');
      });
    }
    node.append(l, v);
    return node;
  }));
}
el('settingsCancel').addEventListener('click', () => { screen = 'waiting'; render(); });

// --- Delete confirmation (mirrors WaitingScreen's own OutcomeScreen use) --

function renderConfirmDeleteScreen() {
  const pairing = findPairing(pendingDeletePairingId);
  el('confirmDeleteTitle').textContent = t('contacts.deleteConfirmTitle', { name: (pairing && pairing.peerName) || t('common.thisContact') });
}
el('confirmDeleteAction').addEventListener('click', () => {
  if (pendingDeletePairingId) removePairing(pendingDeletePairingId);
  pendingDeletePairingId = null;
  screen = 'waiting';
  render();
});
el('confirmDeleteCancel').addEventListener('click', () => {
  pendingDeletePairingId = null;
  screen = 'waiting';
  render();
});

// --- Enter a phrase (mirrors EnterPhraseScreen) --------------------------
//
// The one and only pairing entry screen, for a brand-new contact — there's
// no "Reconnect" anymore (see the pairing design doc: a stale contact is
// just Delete + Add contact now, not a separate flow). Nothing to generate
// or show/read here, so nothing for either side to be "first" at.

function openEnterPhrase() {
  el('phraseInput').value = '';
  screen = 'enter-phrase';
  render();
}
// Guards against a double-submit: a second click (or Enter firing right
// after a click) arriving before render() flips `screen` away from
// 'enter-phrase' used to be indistinguishable from a first, legitimate
// click — every start mints a fresh attempt, so two rapid submits for the same
// passphrase created two independent attempts at the same rendezvous tag, with
// only the first ever resolving.
let phraseSubmitInFlight = false;
el('phraseSubmit').addEventListener('click', async () => {
  if (phraseSubmitInFlight) return;
  const phrase = el('phraseInput').value.trim();
  if (!phrase) return;
  phraseSubmitInFlight = true;
  try {
    // Show the progress screen first: turning the phrase into the meeting
    // point (Argon2id) takes a moment, noticeably longer on a phone, and
    // blocks the page while it runs.
    const mine = { id: null, startedAt: Date.now(), status: 'preparing', candidate: null };
    attempt = mine;
    screen = 'pairing-progress';
    render();
    await new Promise((r) => setTimeout(r, 60));
    if (attempt !== mine) return; // cancelled while the screen was coming up
    startPairing(mine, phrase);
    render();
  } finally {
    phraseSubmitInFlight = false;
  }
});
submitOnEnter('phraseInput', 'phraseSubmit');
// Falls straight back to the waiting screen (where this was always reached
// from), not into Settings.
el('phraseCancel').addEventListener('click', () => { screen = 'waiting'; render(); });

// --- Pairing progress (mirrors PairingProgressScreen) --------------------

// Called from applyEffects' SetCollision/SetTimedOut cases — both outcomes
// dismiss the same way (give up on the attempt), same as tapping Cancel.
// Never used for the name-confirm ("Pair with [name]?") screen — that one
// stays manual, a real decision to actually trust a peer.
function schedulePairingProgressAutoDismiss(pairingId) {
  clearTimeout(pairingProgressAutoDismissHandle);
  pairingProgressAutoDismissHandle = setTimeout(() => {
    // Guards against a stale timer outliving the attempt it was meant for
    // (already retried, cancelled, or navigated away some other way).
    if (screen !== 'pairing-progress' || !attempt || attempt.id !== pairingId) return;
    discardAttempt();
    screen = 'waiting';
    render();
  }, AUTO_DISMISS_DELAY_MS);
}

let pairingRefreshTimer = null;

function connectedRelayCount() {
  return pool ? [...pool.listConnectionStatus()].filter(([, up]) => up).length : 0;
}

function formatMinutesSeconds(ms) {
  const total = Math.max(0, Math.ceil(ms / 1000));
  return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, '0')}`;
}

function renderPairingProgressScreen() {
  if (!attempt) return;
  const collided = attempt.status === 'collided';
  const timedOut = attempt.status === 'timedOut';
  el('progressWaiting').hidden = collided || timedOut;
  el('progressCollision').hidden = !collided;
  el('progressTimeout').hidden = !timedOut;
  if (collided || timedOut) return;
  // Which step this is is call-core's call (shared with Android); this only reports what it can see.
  const pending = pendingPairingsList().find((p) => p.pairingId === attempt.id);
  const hasAttempt = attempt.status !== 'preparing' && !!(pending && pending.rendezvousTag);
  const connected = connectedRelayCount();
  const view = JSON.parse(callCore.pairingPhase(hasAttempt, connected, !!(pending && pending.bootstrapTarget)));
  el('progressTitle').textContent = t(view.label_key);
  // Always two short lines (non-breaking spaces while preparing), so the spinner never moves between steps.
  const preparing = view.phase === 'preparing';
  el('progressRelays').textContent = preparing ? '\u00a0' : t('pairing.progressRelays', { connected, total: RELAYS.length });
  el('progressCancelsIn').textContent = preparing ? '\u00a0' : t('pairing.progressCancelsIn', { left: formatMinutesSeconds(PAKE_LIVE_WINDOW_MS - (Date.now() - attempt.startedAt)) });
}
function retryPairing() {
  // The attempt so far is forgotten entirely, same as an explicit Cancel —
  // the next one is a new attempt with its own id and key.
  discardAttempt();
  openEnterPhrase();
}
el('progressRetryCollision').addEventListener('click', retryPairing);
el('progressRetryTimeout').addEventListener('click', retryPairing);
el('progressCancel').addEventListener('click', () => { discardAttempt(); screen = 'waiting'; render(); });

// --- Name confirm (mirrors NameConfirmScreen) ----------------------------
//
// SPAKE2 already cryptographically proved both sides typed the same phrase
// by the time this shows — this tap is a cheap final sanity check, not a
// fingerprint comparison.

function renderPairScreen() {
  const candidate = attempt && attempt.candidate;
  el('pairTitle').textContent = t('pairing.confirmTitle', { name: (candidate && candidate.name) || t('common.thisDevice') });
}
el('pairConfirm').addEventListener('click', () => { confirmPeer(); });
el('pairCancel').addEventListener('click', () => { discardAttempt(); screen = 'waiting'; render(); });

// --- Calling (mirrors CallingScreen) -------------------------------------

function startCallingUi(pairingId) {
  callPairingId = pairingId;
  resetCallControls();
  screen = 'calling';
  render();
}
/** The one screen for calling, ringing and live: mode classes decide where
 * the self-view sits (full-screen, then shrinking to its corner on connect),
 * whether the centered message shows, and whether Accept is on the controls. */
let lastCallMode = null;
function renderCallScreen() {
  // Which phase the call is in is call-core's decision (shared with Android);
  // this only maps it onto the page's three layouts.
  const view = JSON.parse(callCore.callPhase(true, false, !!pendingIncomingCall, acceptedIncoming, screen === 'call' || callActive));
  const mode = view.phase === 'live' ? 'connected' : view.phase === 'ringing' ? 'incoming' : 'calling';
  const section = el('screenCall');
  for (const m of ['calling', 'incoming', 'connected']) section.classList.toggle(`mode-${m}`, m === mode);

  const pairing = findPairing(callPairingId);
  const name = (pairing && pairing.peerName) || t('common.unnamedContact');
  const ringing = mode !== 'connected';
  el('callMessage').hidden = !ringing;
  el('callSpinner').hidden = mode !== 'calling';
  if (view.label_key) el('callLabel').textContent = t(view.label_key);
  el('callName').textContent = name;
  el('callName').title = name;
  el('callNote').hidden = !mediaWaitNote;
  if (mediaWaitNote) el('callNote').textContent = t('call.mediaPermissionWait');

  // Until connected the controls can't be dismissed; afterwards they stay up
  // until the person dismisses them (see the click/keydown handlers).
  if (ringing) { callControlsVisible = true; el('callControls').hidden = false; }
  renderCallControls();

  // Accept: only while ringing, slides away once the call is answered. See
  // incomingCallAwaitingMedia's own doc for the disabled state — it guards
  // against a browser permission-prompt tap landing on a button instead.
  const accept = el('incomingAccept');
  el('acceptSlot').classList.toggle('gone', mode !== 'incoming');
  accept.tabIndex = mode === 'incoming' ? 0 : -1;
  accept.disabled = mode === 'incoming' && incomingCallAwaitingMedia;
  el('callDisconnect').disabled = mode === 'incoming' && incomingCallAwaitingMedia;
  if (mode !== lastCallMode) {
    if (mode === 'incoming') accept.focus({ preventScroll: true });
    else if (document.activeElement === accept) el('cyclePreviewPosition').focus({ preventScroll: true });
  }
  lastCallMode = mode;
}
el('incomingAccept').addEventListener('click', () => { acceptIncomingCall(); });

// --- Live call --------------------------------------------------------------
// See the call-controls setup right after makeOffer/ensurePeerConnection's
// own section above (PREVIEW_POSITIONS, resetCallControls,
// showCallControls/hideCallControls, and the click/keydown listeners) —
// #callControls' buttons are wired there, next to the state they read.

// --- Call outcome (mirrors CallOutcomeScreen in HomeScreens.kt) -----------
//
// Shown whenever a call ends for a reason the person didn't just cause
// themselves. The "why did this call end" decision itself is made once in
// call-core (see CallOutcomeReason's own doc there), not re-derived here.

// Which text an ended call gets (including the ICE refinement of "never
// connected") is call-core's decision; this maps each case to its strings.
const CALL_OUTCOME_COPY = {
  peer_ended: ['call.outcome.peerEndedTitle', 'call.outcome.peerEndedMessage'],
  declined: ['call.outcome.declinedTitle', 'call.outcome.declinedMessage'],
  cancelled: ['call.outcome.cancelledTitle', 'call.outcome.cancelledMessage'],
  no_answer: ['call.outcome.noAnswerTitle', 'call.outcome.noAnswerMessage'],
  unreachable: ['call.outcome.unreachableTitle', 'call.outcome.unreachableMessage'],
  busy: ['call.outcome.busyTitle', 'call.outcome.busyMessage'],
  camera_failed: ['call.outcome.cameraFailedTitle', 'call.outcome.cameraFailedMessage'],
  never_connected: ['call.outcome.neverConnectedTitle', 'call.outcome.neverConnectedMessage'],
  udp_blocked: ['call.outcome.neverConnectedTitle', 'call.outcome.udpBlockedMessage'],
  no_direct_path: ['call.outcome.neverConnectedTitle', 'call.outcome.noDirectPathMessage'],
  dropped: ['call.outcome.droppedTitle', 'call.outcome.droppedMessage'],
};

const CAMERA_ERROR_MESSAGE = {
  NotAllowedError: 'call.outcome.cameraDeniedMessage',
  SecurityError: 'call.outcome.cameraDeniedMessage',
  NotFoundError: 'call.outcome.cameraMissingMessage',
  OverconstrainedError: 'call.outcome.cameraMissingMessage',
  NotReadableError: 'call.outcome.cameraBusyMessage',
  AbortError: 'call.outcome.cameraBusyMessage',
  TimeoutError: 'call.outcome.cameraTimeoutMessage',
};

function renderCallOutcomeScreen() {
  const pairing = findPairing(pendingCallOutcome && pendingCallOutcome.pairingId);
  const name = (pairing && pairing.peerName) || t('common.unnamedContact');
  const text = callCore.outcomeText(pendingCallOutcome.reason, callCore.iceLastDiagnosis() ?? null);
  let [titleKey, messageKey] = CALL_OUTCOME_COPY[text];
  // What went wrong with the camera or microphone is known only here (browser error names), so it refines the
  // generic message the same way the ICE evidence refines "never connected".
  if (text === 'camera_failed' && lastMediaErrorName) messageKey = CAMERA_ERROR_MESSAGE[lastMediaErrorName] || messageKey;
  const title = t(titleKey);
  const message = t(messageKey, { name });
  el('callOutcomeTitle').textContent = title;
  el('callOutcomeMessage').textContent = message;
}
el('callOutcomeCancel').addEventListener('click', () => {
  pendingCallOutcome = null;
  screen = 'waiting';
  render();
});

// ---------------------------------------------------------------------------
// Bootstrap
// ---------------------------------------------------------------------------

async function startApp() {
  screen = 'waiting';
  render();
  connectRelayClient();
}

document.documentElement.style.setProperty('--self-view-shrink', `${SELF_VIEW_SHRINK_MS}ms`);
el('phraseInput').maxLength = MAX_PHRASE_LENGTH;

if (deviceName) {
  screen = 'waiting';
  startApp();
} else {
  screen = 'name';
  render();
}
