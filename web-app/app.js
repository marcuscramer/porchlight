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

// call-core's WASM module — a top-level await (legal since this file is
// loaded as type="module"), so nothing below can run a pairing attempt
// before it's ready.
await init();

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
const ONLINE_CHECK_INTERVAL_MS = 10000;
// "At least 120 seconds" per the pairing design doc.
const PAKE_LIVE_WINDOW_MS = PROTOCOL_CONSTANTS.pake_live_window_ms;
// Sanity bounds on untrusted network input are enforced inside
// callCore.parseSignalPayload now, not here. Validated via a standalone
// spike: all three relays round-trip a message in a few hundred ms.
const RELAYS = ['wss://relay.damus.io', 'wss://nos.lol', 'wss://relay.primal.net'];

const DEVICE_NAME_STORAGE_KEY = 'porchlight-device-name';
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
  return parsed.map((p) => ({
    id: p.id,
    ownPrivateKeyHex: p.ownPrivateKeyHex,
    peerPublicKey: p.peerPublicKey || '',
    peerName: p.peerName || '',
    // The newest gift-wrapped signal (by created_at, then by id — see
    // WrapEventCandidate::last_signal_created_at's own doc, nostr_protocol.rs,
    // for why both are needed) this pairing has actually processed ("" / 0
    // for one that's never received any yet). Persisted with the rest of
    // the pairing so handleWrapEvent can recognize a relay redelivery even
    // across a reload.
    lastSignalCreatedAt: p.lastSignalCreatedAt || 0,
    lastSignalEventId: p.lastSignalEventId || '',
  }));
}
function savePairings() {
  localStorage.setItem(PAIRINGS_STORAGE_KEY, JSON.stringify(pairings));
}
function isConfirmed(pairing) { return !!pairing.peerPublicKey; }
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
/** Called from handleWrapEvent right after a wrap event is actually
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
  callCore.cancelAttempt(pairingId);
  confirmedCandidate.delete(pairingId);
  pairingCollision.delete(pairingId);
  pairingTimedOut.delete(pairingId);
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
let signalingOk = false;

function confirmedPeers() {
  return pairings.filter(isConfirmed).map((p) => (
    {
      pairingId: p.id,
      ownPrivateKeyHex: p.ownPrivateKeyHex,
      peerPublicKey: p.peerPublicKey,
      lastSignalCreatedAt: p.lastSignalCreatedAt,
      lastSignalEventId: p.lastSignalEventId,
    }
  ));
}
/** Mirrors NostrSignalingClient.kt's PendingPairing — rendezvousTag/
 * bootstrapPayload/bootstrapTarget are null when there's no *live* attempt
 * right now (a fresh page load, or a timed-out/collided attempt): nothing
 * to publish or subscribe for until a human starts a new one. */
function pendingPairingsList() {
  return pairings.filter((p) => !isConfirmed(p)).map((p) => {
    const json = callCore.buildBootstrapPayload(p.id); // undefined if there's no live attempt
    const snapshot = json ? JSON.parse(json) : null;
    return {
      pairingId: p.id,
      ownPrivateKeyHex: p.ownPrivateKeyHex,
      rendezvousTag: snapshot ? snapshot.rendezvous_tag : null,
      bootstrapPayload: snapshot ? snapshot.payload : null,
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
function resubscribe() {
  if (wrapSub) { wrapSub.close(); wrapSub = null; }
  if (bootstrapSub) { bootstrapSub.close(); bootstrapSub = null; }
  if (!pool) return;
  const ownPubkeys = confirmedPeers().map((p) => ownPubkeyHexFor(p.ownPrivateKeyHex));
  const rendezvousTags = pendingPairingsList().map((p) => p.rendezvousTag).filter(Boolean);
  const relayFilters = JSON.parse(callCore.buildRelayFilters(ownPubkeys, rendezvousTags));
  if (relayFilters.wrap_filter) {
    const f = relayFilters.wrap_filter;
    wrapSub = pool.subscribe(RELAYS, { kinds: [f.kind], [`#${f.tag_name}`]: f.tag_values }, { onevent: handleIncomingEvent, oneose() {} });
  }
  if (relayFilters.bootstrap_filter) {
    const f = relayFilters.bootstrap_filter;
    bootstrapSub = pool.subscribe(RELAYS, { kinds: [f.kind], [`#${f.tag_name}`]: f.tag_values }, { onevent: handleIncomingEvent, oneose() {} });
  }
}

function connectRelayClient() {
  // enableReconnect: nostr-tools' SimplePool defaults this to false — once
  // any relay connection fails or drops, it just gives up on that relay
  // forever, which a tab left open across a transient relay hiccup would
  // hit. With this on, a dropped/failed relay retries with the library's
  // own backoff instead of needing a page reload to recover.
  pool = new SimplePool({ enableReconnect: true });
  // nostr-tools' SimplePool has no separate "connect" step — a relay's
  // WebSocket only actually opens once something subscribes or publishes to
  // it. resubscribe() below skips calling pool.subscribe() at all when
  // there's nothing to filter for yet, so a brand-new profile with no
  // contacts would otherwise never open any relay connection and
  // "Connected" would incorrectly read red. ensureRelay() opens the
  // connection directly, independent of any subscription.
  for (const url of RELAYS) pool.ensureRelay(url).catch(() => {});
  resubscribe();
  onlineCheckTimer = setInterval(monitorOnlineTimeouts, ONLINE_CHECK_INTERVAL_MS);
  setInterval(pollSignalingStatus, 3000);
  scheduleHeartbeat(0);
  pollSignalingStatus();
}

function pollSignalingStatus() {
  if (!pool) return;
  const nowOk = [...pool.listConnectionStatus().values()].some(Boolean);
  if (nowOk !== signalingOk) { signalingOk = nowOk; render(); }
}

function handleIncomingEvent(event) {
  if (!callCore.markSeenOrIsDuplicate(event.id)) return;
  if (event.kind === WRAP_KIND) handleWrapEvent(event);
  else if (event.kind === SIGNAL_KIND) handleBootstrapEvent(event);
}

/** Unwraps a gift-wrapped signal event and dispatches it to a *confirmed*
 * pairing — see publish()'s doc for the wrap's shape. The wrap's own `p`
 * tag is one of this client's per-pairing pubkeys, which directly (and
 * uniquely) identifies which pairing this is before anything is even
 * decrypted. callCore.unwrapWrappedEventForAny does that routing plus the
 * entire verify-decrypt-verify-decrypt chain (including the pinned-peer
 * identity check) — see its own doc. */
function handleWrapEvent(event) {
  const candidates = confirmedPeers();
  const candidatesJson = JSON.stringify(
    candidates.map((p) => (
      {
        pairing_id: p.pairingId,
        own_private_key_hex: p.ownPrivateKeyHex,
        peer_public_key: p.peerPublicKey,
        last_signal_created_at: p.lastSignalCreatedAt,
        last_signal_event_id: p.lastSignalEventId,
      }
    )),
  );
  const routedJson = callCore.unwrapWrappedEventForAny(JSON.stringify(event), candidatesJson);
  if (!routedJson) return;
  const routed = JSON.parse(routedJson);
  const peer = candidates.find((p) => p.pairingId === routed.pairing_id);
  if (!peer) return;
  // See WrapEventCandidate::last_signal_created_at's own doc
  // (nostr_protocol.rs) — persist immediately, before this event is even
  // dispatched, so a redelivery of the *same* event (or anything older) is
  // rejected by unwrap_wrapped_event_for_any itself on any subsequent
  // attempt, reload included.
  updateLastSignal(routed.pairing_id, routed.signal_created_at, routed.signal_event_id);
  // Parsing (type extraction, per-type field shape/length validation) is
  // callCore.parseSignalPayload's job — dispatchFromConfirmedPeer still gets
  // called even when this is undefined: a validly wrapped/decrypted/
  // signature-verified message from a confirmed peer must still register as
  // "this peer is alive" regardless of whether its own content parses.
  const messageJson = callCore.parseSignalPayload(routed.payload_json);
  dispatchFromConfirmedPeer(peer, messageJson ? JSON.parse(messageJson) : null);
}

/** Dispatches an unwrapped, unencrypted bootstrap-phase event — matched to
 * a pending pairing purely by its `d` tag (the passphrase-derived
 * rendezvous point), since neither side's real pubkey is known yet.
 * Signature verification is callCore.verifyBootstrapEvent's job now (see
 * its own doc) — still required even though bootstrap content is
 * plaintext. */
function handleBootstrapEvent(event) {
  const verifiedJson = callCore.verifyBootstrapEvent(JSON.stringify(event));
  if (!verifiedJson) return;
  const verified = JSON.parse(verifiedJson);
  const pending = pendingPairingsList().find((p) => p.rendezvousTag === verified.rendezvous_tag);
  if (!pending) return;
  let payload;
  try { payload = JSON.parse(verified.payload_json); } catch { return; }
  if (!payload.type) return;
  onPairingBootstrapMessage(pending.pairingId, verified.sender_pubkey_hex, payload.type, payload);
}

/**
 * [message] is undefined/null for an unrecognized type or a malformed
 * payload — callCore.markSeen still runs unconditionally first regardless
 * (a validly wrapped/decrypted/signature-verified message from a confirmed
 * peer means they're alive, independent of whether this client can make
 * sense of what they actually said).
 */
function dispatchFromConfirmedPeer(peer, message) {
  const pairingId = peer.pairingId;
  const ownPubkeyHex = ownPubkeyHexFor(peer.ownPrivateKeyHex);
  const peerBusy = message && message.type === 'heartbeat' && typeof message.busy === 'boolean' ? message.busy : null;
  applyPresenceUpdate(callCore.markSeen(pairingId, ownPubkeyHex, peer.peerPublicKey, Date.now(), peerBusy));
  if (!message) return;
  switch (message.type) {
    case 'heartbeat':
      if (message.name) {
        const p = findPairing(pairingId);
        if (p && message.name !== p.peerName) { updatePairingPeer(pairingId, p.peerPublicKey, message.name); render(); }
      }
      break;
    case 'leaving':
      applyPresenceUpdate(callCore.handleLeavingMessage(pairingId));
      break;
    case 'bye':
      // Gated on callId, not just pairingId — inside callCore.handlePeerHangup:
      // a "bye" from an attempt this client has already moved on from must
      // not tear down a call that isn't the one it's actually about.
      onPeerHangup(pairingId, message.callId);
      break;
    case 'busy':
      // Releases this device's own claimed call slot immediately, not just
      // waiting for the peer's next heartbeat — see
      // callCore.handlePeerBusyReply's own doc. The busy badge itself comes
      // from live presence status (applyPresenceUpdate's SetStatus
      // handling), not a one-shot flag.
      applyPresenceUpdate(callCore.handlePeerBusyReply(pairingId, ownPubkeyHex, peer.peerPublicKey, message.callId));
      break;
    case 'call': {
      // The pubkey tie-break lives inside callCore.handleShouldOffer now —
      // this always forwards when there's a real peer, same as every other
      // message type.
      const peer2 = findPairing(pairingId);
      if (peer2) onShouldOffer(pairingId, message.callId, ownPubkeyHexFor(peer2.ownPrivateKeyHex), peer2.peerPublicKey);
      break;
    }
    case 'offer':
      onOfferReceived(pairingId, message.sdp, message.callId);
      break;
    case 'answer':
      onAnswer(pairingId, message.sdp, message.callId);
      break;
    case 'ice':
      onRemoteIce(pairingId, message.callId, message);
      break;
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

function heartbeatTick() {
  callCore.pruneStalePending(pendingPairingsList().map((p) => p.pairingId));
  // callCore.isCallActive(): this device's own single call slot, broadcast
  // identically to every contact regardless of who (if anyone) it's
  // actually occupied by. Read once per tick, not once per peer.
  const busy = callCore.isCallActive();
  for (const peer of confirmedPeers()) {
    sendToConfirmedPeer(peer.pairingId, peer.ownPrivateKeyHex, peer.peerPublicKey, () => callCore.buildHeartbeatPayload(deviceName, busy));
  }
  for (const pending of pendingPairingsList()) {
    if (!pending.rendezvousTag || !pending.bootstrapPayload) continue;
    sendPairingBootstrap(pending.ownPrivateKeyHex, pending.rendezvousTag, pending.bootstrapTarget, pending.bootstrapPayload);
  }
}

/** Adaptive cadence, not a flat interval — see call-core's own
 * `presence::current_heartbeat_interval_ms` doc for the full rationale
 * (the decision itself, including every constant, now lives entirely
 * there). Applies globally (every pairing gets the faster rate while *any*
 * one needs it), not per-pairing — a deliberate simplification, same
 * trade-off as the Kotlin side. */
function currentHeartbeatIntervalMs() {
  return callCore.currentHeartbeatIntervalMs(pendingPairingsList().map((p) => p.pairingId), Date.now());
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
  applyPresenceUpdate(callCore.checkOnlineTimeouts(Date.now()));
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
    await Promise.allSettled(pool.publish(RELAYS, JSON.parse(wrapJson)));
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
  if (peer && isConfirmed(peer)) sendToConfirmedPeer(pairingId, peer.ownPrivateKeyHex, peer.peerPublicKey, buildPayload);
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
    await Promise.allSettled(pool.publish(RELAYS, JSON.parse(eventJson)));
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

// ---------------------------------------------------------------------------
// Pairing (SPAKE2) — mirrors CameraAgentService.kt's PakeAttempt/
// beginPakeAttempt/onPairingBootstrapMessage/confirmPeer. Fully symmetric,
// no generator/enterer roles: both sides run the exact same "Enter a
// phrase" flow (see the pairing design doc).
// ---------------------------------------------------------------------------

/** At most one entry: the peer a SPAKE2 exchange has already
 * cryptographically confirmed for this pending pairing, awaiting the final
 * human "Pair with [name]?" tap. `call-core` owns the actual attempt
 * registry — this file only keeps the UI-facing state a confirmed/
 * collided/timed-out attempt needs to render, mirroring Android's own
 * ContactState. */
const confirmedCandidate = new Map(); // pairingId -> { pubkeyHex, name }
// Mirrors ContactState.pairingCollision/pairingTimedOut.
const pairingCollision = new Set();
const pairingTimedOut = new Set();

/** Starts a passphrase pairing attempt for a brand-new contact — mirrors
 * CameraAgentService.startPairing. [passphrase] is never persisted — only
 * this attempt's own fresh keypair is, immediately, so it's ready to become
 * the pairing's permanent identity the moment SPAKE2 succeeds. */
function startPairing(passphrase) {
  const fresh = { id: crypto.randomUUID(), ownPrivateKeyHex: bytesToHex(generateSecretKey()), peerPublicKey: '', peerName: '' };
  addOrUpdatePairing(fresh);
  beginPakeAttempt(fresh, passphrase);
  return fresh;
}

/** Actually starts the SPAKE2 exchange for [pairing] — calls into
 * call-core, which trims/NFC-normalizes the raw typed passphrase and
 * derives the rendezvous tag internally (via pake-bridge), registers the
 * live attempt in call-core's own registry, and schedules its live-window
 * timeout. Mirrors CameraAgentService.beginPakeAttempt. */
function beginPakeAttempt(pairing, passphrase) {
  const ownPubkeyHex = getPublicKey(hexToBytes(pairing.ownPrivateKeyHex));
  const start = JSON.parse(callCore.startAttempt(pairing.id, ownPubkeyHex, deviceName, passphrase));
  pairingCollision.delete(pairing.id);
  pairingTimedOut.delete(pairing.id);
  kickHeartbeat();
  // handleTimeout's own generation check makes this a no-op for a fresh
  // retry or an already-resolved match.
  setTimeout(() => applyEffects(callCore.handleTimeout(pairing.id, start.generation)), PAKE_LIVE_WINDOW_MS);
  render();
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
        const pairing = findPairing(effect.pairing_id);
        if (pairing) sendPairingBootstrap(pairing.ownPrivateKeyHex, effect.rendezvous_tag, effect.target_pubkey, effect.payload);
        break;
      }
      case 'KickHeartbeat':
        kickHeartbeat();
        break;
      case 'SetCollision':
        // Deleting the confirmed candidate here (not just flagging
        // pairingCollision) disarms an already-shown confirm screen's
        // Confirm button and triggers render()'s reactive bail-back to
        // pairing-progress — see render()'s own doc for why both directions
        // of that swap matter.
        confirmedCandidate.delete(effect.pairing_id);
        pairingCollision.add(effect.pairing_id);
        break;
      case 'SetTimedOut':
        pairingTimedOut.add(effect.pairing_id);
        break;
      case 'SetConfirmedCandidate':
        confirmedCandidate.set(effect.pairing_id, { pubkeyHex: effect.pubkey_hex, name: effect.name });
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
function onPairingBootstrapMessage(pairingId, senderPubkey, type, payload) {
  applyEffects(callCore.handleBootstrapMessage(pairingId, senderPubkey, type, JSON.stringify(payload)));
}

/** A human tapped "Pair with [name]?" for a candidate the SPAKE2 exchange
 * already cryptographically confirmed — mirrors
 * CameraAgentService.confirmPeer. */
function confirmPeer(pairingId, publicKeyHex) {
  const candidate = confirmedCandidate.get(pairingId);
  if (!candidate || candidate.pubkeyHex !== publicKeyHex) return;
  confirmedCandidate.delete(pairingId);
  // The SPAKE2 exchange was already consumed by finish() inside
  // handleBootstrapMessage (call-core) by the time a candidate exists to
  // confirm here — this just tells call-core to stop tracking the attempt.
  callCore.cancelAttempt(pairingId);
  updatePairingPeer(pairingId, publicKeyHex, candidate.name || '');
  kickHeartbeat();
  const peer = findPairing(pairingId);
  if (peer) applyPresenceUpdate(callCore.markSeen(pairingId, ownPubkeyHexFor(peer.ownPrivateKeyHex), publicKeyHex, Date.now()));
  screen = 'waiting';
  render();
}

// ---------------------------------------------------------------------------
// Call arbitration + WebRTC (shared across every pairing — only one call
// can be live at a time, mirrors CameraAgentService/WebRtcEngine).
//
// See /CALL_STATE.md at the repo root before changing onOfferReceived,
// onShouldOffer, hangUp, requestCall, acceptIncomingCall, or anything else
// call-arbitration-related — it's the single write-up of the invariants
// call-core's `call_arbitration` module enforces. This file is the
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
  const json = callCore.acceptIncomingCall();
  if (!json) return;
  const result = JSON.parse(json);
  clearIncomingCall();
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
// Whether the one-time startup probe (see checkPermissions) confirmed
// camera/mic access works — drives the waiting screen's "Camera & mic"
// check now that localStream itself is no longer held open at idle.
let permissionsOk = false;

/**
 * Reflects whether camera/mic access is already granted, for the waiting
 * screen's checkmark — via the read-only Permissions API, which reports
 * state without ever showing a dialog, rather than calling getUserMedia()
 * just to find out. That distinction matters on Safari specifically: unlike
 * Chrome/Firefox, Safari doesn't durably cache a grant once every track
 * from a prior getUserMedia() call has been stopped, so a real probe here
 * would be an extra, needless prompt on top of whatever the first real call
 * triggers.
 */
async function checkPermissions() {
  if (navigator.permissions?.query) {
    try {
      const status = await navigator.permissions.query({ name: 'camera' });
      permissionsOk = status.state === 'granted';
      status.onchange = () => { permissionsOk = status.state === 'granted'; render(); };
      render();
      return;
    } catch (err) {
      // Older Safari/WebKit doesn't support querying 'camera' this way —
      // fall through to the real-probe fallback below.
    }
  }
  try {
    const probe = await navigator.mediaDevices.getUserMedia({ video: true, audio: true });
    probe.getTracks().forEach((t) => t.stop());
    permissionsOk = true;
  } catch (err) {
    console.error('camera/mic permission check failed', err);
    permissionsOk = false;
  }
  render();
}

// In-flight acquisition, if any — requestCall() calls acquireLocalStream()
// eagerly (fire-and-forget, for the self-view) while ensurePeerConnection()
// awaits its own call moments later; without tracking this, both calls
// would see `localStream` still null and each kick off a *separate* real
// getUserMedia() call, leaking whichever stream loses the race.
let acquireLocalStreamPromise = null;

/** Opens the camera/mic for an actual call attempt — called right when one
 * starts (requestCall, or receiving an offer via ensurePeerConnection),
 * never before. Idempotent — same as WebRtcEngine.acquireMedia. */
async function acquireLocalStream() {
  if (localStream) return localStream;
  if (acquireLocalStreamPromise) return acquireLocalStreamPromise;
  acquireLocalStreamPromise = (async () => {
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ video: true, audio: true });
      // The call attempt that triggered this may already have been
      // cancelled/ended while getUserMedia was still pending (a fast
      // Call-then-Cancel) — shut it down immediately instead of adopting it.
      if (activePairingId === null) {
        stream.getTracks().forEach((t) => t.stop());
      } else {
        localStream = stream;
        localVideoEl.srcObject = localStream;
        callingLocalVideoEl.srcObject = localStream;
        render();
      }
    } catch (err) {
      // A denied/unavailable camera would otherwise leave the user stranded
      // on the calling/incoming-call screen forever with no explanation —
      // failCallAttempt tears the attempt down and lets call-core's own
      // NeverConnected outcome surface, same as any other reason this call
      // never got anywhere.
      failCallAttempt('camera/mic access failed', err);
    } finally {
      acquireLocalStreamPromise = null;
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
  callingLocalVideoEl.srcObject = null;
}

/**
 * Executes whatever CallEffects a call into call-core returned — mirrors
 * applyEffects above (pairing-bootstrap's own twin) and
 * CameraAgentService.applyCallEffects on Android. activePairingId is set
 * directly here for every effect that means "the call slot is now claimed"
 * (CreateOffer/SendCall/ApplyRemoteOffer/StartRinging) — matches
 * CALL_STATE.md invariant #1: the slot is claimed the moment a call *could*
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
        currentPairingId = effect.pairing_id;
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
      case 'ShowCallOutcome':
        pendingCallOutcome = { pairingId: effect.pairing_id, callId: effect.call_id, reason: effect.reason };
        currentPairingId = effect.pairing_id;
        screen = 'call-outcome';
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
 * setPeerOnline). */
function requestCall(pairingId) {
  const peer = findPairing(pairingId);
  if (!peer) return;
  const ownPubkeyHex = ownPubkeyHexFor(peer.ownPrivateKeyHex);
  const peerOnline = callCore.isOnline(pairingId);
  const result = JSON.parse(callCore.requestCall(pairingId, ownPubkeyHex, peer.peerPublicKey, peerOnline));
  if (result.call_id !== null) activePairingId = pairingId;
  applyCallEffects(JSON.stringify(result.effects));
  if (result.call_id !== null) startCallingUi(pairingId);
}

function hangUp() {
  applyCallEffects(callCore.hangUp());
}

function sendBusy(pairingId, callId) { sendConfirmedOrPending(pairingId, () => callCore.buildBusyPayload(callId)); }

/** See callCore.handleShouldOffer's own doc for the guard sequence (pubkey
 * tie-break first, then busy, then redelivery). This function's whole job
 * is forwarding the event in and executing whatever CallEffects come back.
 * `autoAnswer` isn't a web feature — always `false` here, same as
 * onOfferReceived. */
function onShouldOffer(pairingId, callId, ownPubkeyHex, peerPubkeyHex) {
  applyCallEffects(callCore.handleShouldOffer(pairingId, callId, ownPubkeyHex, peerPubkeyHex, false));
}

/**
 * See callCore.handleOffer's own doc for the full guard sequence (busy,
 * already-active redelivery, already-ringing redelivery, then the
 * pubkey-tie-break fast-path that applies immediately with no ring —
 * CALL_STATE.md invariant #3). Auto-answer isn't a web feature — every
 * incoming call rings for a manual Accept/Decline. Mirrors
 * CameraAgentService.onOffer.
 */
function onOfferReceived(pairingId, sdp, callId) {
  applyCallEffects(callCore.handleOffer(pairingId, callId, sdp, false));
}

function onPeerHangup(pairingId, callId) {
  // Gated on callId, not just pairingId — inside callCore.handlePeerHangup:
  // a "bye" from an attempt this client has already moved on from must not
  // tear down a call that isn't the one it's actually about.
  applyCallEffects(callCore.handlePeerHangup(pairingId, callId));
}

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
  // Once per call, here (not inside onconnectionstatechange below, which
  // can fire more than once per call if the connection blips and
  // recovers) — see resetCallControls' own doc.
  resetCallControls();
  pcPairingId = pairingId;
  pcCallId = callId;
  pc = new RTCPeerConnection({
    iceServers: [{ urls: 'stun:stun.l.google.com:19302' }, { urls: 'stun:stun1.l.google.com:19302' }],
  });
  if (localStream) for (const track of localStream.getTracks()) pc.addTrack(track, localStream);
  pc.ontrack = (event) => { remoteVideoEl.srcObject = event.streams[0]; };
  pc.onconnectionstatechange = () => {
    callActive = pc.connectionState === 'connected';
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
// callCore.shouldApplyAnswer's own pairingId/callId check below: this one
// is about *this call's own negotiation state*, that one's about *which
// call* — call-core has no visibility into pc, so it can't replace this
// half.
let answerApplied = false;

async function handleAnswer(sdp) {
  if (!pc || pc.signalingState !== 'have-local-offer' || answerApplied) return;
  answerApplied = true;
  await pc.setRemoteDescription({ type: 'answer', sdp });
}

/** Mirrors CameraAgentService.onAnswer: the *which call* guard
 * (call-core's own answer_applied flag) lives in
 * callCore.shouldApplyAnswer; handleAnswer's own answerApplied flag above
 * is the *second*, independent WebRTC-signalingState-level guard. */
async function onAnswer(pairingId, sdp, callId) {
  if (callCore.shouldApplyAnswer(pairingId, callId)) {
    await handleAnswer(sdp).catch((err) => failCallAttempt('handleAnswer failed (bad SDP?)', err));
  }
}

function applyIceCandidate(sdpMid, sdpMLineIndex, candidate) {
  // addIceCandidate returns a Promise that rejects asynchronously (e.g. if
  // this candidate raced ahead of the offer/answer's own
  // setRemoteDescription still in flight) — .catch(), not try/catch, which
  // only ever catches a synchronous throw.
  if (!pc) return;
  pc.addIceCandidate({ candidate, sdpMid, sdpMLineIndex }).catch((err) => console.warn('bad ice payload or candidate arrived before remote description', err));
}

/** See callCore.handleRemoteIce's own doc — folds the id/callId gating into
 * one call-core decision: buffer if it matches the pending ring, apply if
 * it matches the active call, drop silently otherwise. try/catch, not a
 * bare call: this is one of the few call-core exports that can genuinely
 * throw — without it, a malformed/corrupted ICE payload from a confirmed
 * peer could throw uncaught inside SimplePool's own onevent callback,
 * silently aborting dispatchFromConfirmedPeer mid-message. */
function onRemoteIce(pairingId, callId, payload) {
  let outcome;
  try {
    outcome = JSON.parse(callCore.handleRemoteIce(pairingId, callId, payload.sdpMid ?? null, payload.sdpMLineIndex, payload.candidate));
  } catch (err) {
    console.warn('onRemoteIce: callCore.handleRemoteIce failed, dropping candidate', err);
    return;
  }
  if (outcome.outcome === 'Apply') applyIceCandidate(outcome.sdp_mid, outcome.sdp_m_line_index, outcome.candidate);
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
  if (pc) { pc.close(); pc = null; }
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
let currentPairingId = null; // which pairing 'pair'/'pairing-progress' applies to
let pendingDeletePairingId = null; // which pairing 'confirm-delete' applies to
const el = (id) => document.getElementById(id);
const screens = {
  name: el('screenName'), waiting: el('screenWaiting'), settings: el('screenSettings'),
  rename: el('screenRename'), 'confirm-delete': el('screenConfirmDelete'),
  'enter-phrase': el('screenEnterPhrase'), 'pairing-progress': el('screenPairingProgress'),
  pair: el('screenPair'), calling: el('screenCalling'), 'incoming-call': el('screenIncomingCall'), call: el('screenCall'),
  'call-outcome': el('screenCallOutcome'),
};
const remoteVideoEl = el('remoteVideo');
const localVideoEl = el('localVideo');
const callingLocalVideoEl = el('callingLocalVideo');

/**
 * The self-view's position during a call — mirrors the Android app's own
 * PreviewCorner cycle: one "Position self-view" button press advances to
 * the next entry, wrapping around, with a fifth "invisible" position after
 * the four corners. Index 0 is the CSS default (bottom-start) — every new
 * call starts there (see resetCallControls).
 */
const PREVIEW_POSITIONS = ['bottom-start', 'top-start', 'top-end', 'bottom-end', 'invisible'];
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

/** Once per call, mirroring the old localVideoDrag.reset()'s own timing
 * (see ensurePeerConnection) — a fresh call always starts with the
 * self-view in its default spot, both mute toggles on, and the controls
 * overlay closed, undoing whatever a previous call left them at. */
function resetCallControls() {
  previewPositionIndex = 0;
  applyPreviewPositionClass();
  audioEnabled = true;
  videoEnabled = true;
  callControlsVisible = false;
  el('callControls').hidden = true;
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
  // Lower-right by default — lower-left only for the one self-view position
  // that would otherwise sit right underneath it, 'bottom-end'.
  const controlsOnLeft = activeCorner === 'bottom-end';
  el('callControls').classList.toggle('position-left', controlsOnLeft);
  el('callControls').classList.toggle('position-right', !controlsOnLeft);
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
  if (callControlsVisible) {
    const inGracePeriod = Date.now() - lastPositionChangeAt < POSITION_CHANGE_DISMISS_GRACE_MS;
    if (!el('callControls').contains(e.target) && !inGracePeriod) hideCallControls();
  } else {
    showCallControls();
  }
});
// Enter/Space opens, Backspace/Escape closes — the keyboard-only
// equivalent of the click handler above.
document.addEventListener('keydown', (e) => {
  if (screen !== 'call') return;
  if (!callControlsVisible && (e.key === 'Enter' || e.key === ' ' || e.key === 'Spacebar')) {
    e.preventDefault();
    showCallControls();
  } else if (callControlsVisible && (e.key === 'Backspace' || e.key === 'Escape')) {
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
  if (screen === 'pairing-progress' && currentPairingId && confirmedCandidate.has(currentPairingId)) {
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
  if (screen === 'pair' && currentPairingId && !confirmedCandidate.has(currentPairingId)) {
    screen = 'pairing-progress';
  }

  for (const [name, node] of Object.entries(screens)) node.hidden = name !== screen;

  if (screen === 'waiting') renderWaitingScreen();
  if (screen === 'settings') renderSettingsScreen();
  if (screen === 'confirm-delete') renderConfirmDeleteScreen();
  if (screen === 'pairing-progress') renderPairingProgressScreen();
  if (screen === 'pair') renderPairScreen();
  if (screen === 'calling') renderCallingScreen();
  if (screen === 'incoming-call') renderIncomingCallScreen();
  if (screen === 'call-outcome') renderCallOutcomeScreen();
}

// --- Name entry (mirrors NameEntryScreen) -----------------------------

// sanitizeName + slice here too, not just on receipt: this name becomes a
// *peer's* self-reported name the moment it heartbeats out — capping/
// cleaning it at the source means every other device that ever sees it
// sees the same sanitized value this device shows.
el('nameContinue').addEventListener('click', async () => {
  const name = callCore.sanitizeName(el('nameInput').value.trim().slice(0, PROTOCOL_CONSTANTS.max_name_length)) || 'Porchlight (web)';
  deviceName = name;
  localStorage.setItem(DEVICE_NAME_STORAGE_KEY, deviceName);
  await startApp();
});
// A plain <input> outside a <form> does nothing on Enter by default —
// forward to the button's own handler.
el('nameInput').addEventListener('keydown', (e) => { if (e.key === 'Enter') el('nameContinue').click(); });

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
el('renameInput').addEventListener('keydown', (e) => { if (e.key === 'Enter') el('renameSave').click(); });
el('renameCancel').addEventListener('click', () => { screen = 'settings'; render(); });

// --- Waiting screen (mirrors WaitingScreen + ContactRow) ----------------

el('settingsBtn').addEventListener('click', () => { screen = 'settings'; render(); });

/**
 * Mirrors WaitingScreen's own contact list (HomeScreens.kt): per-contact
 * Delete lives right on each row (no auto-answer toggle here — that's
 * Android/TV-only), and the list ends with a plain "Add contact" row.
 */
function renderWaitingScreen() {
  setCheck(el('chkInternet'), navigator.onLine);
  setCheck(el('chkPermissions'), permissionsOk);
  setCheck(el('chkSignaling'), signalingOk);

  const listEl = el('contactList');
  // Rebuilding from scratch (innerHTML = '') on every call resets scrollTop
  // to 0 the moment real layout happens — confirmed live once a presence
  // update or the signaling-status poll fires while the user is mid-scroll:
  // the list silently snaps back to the top. Save/restore across the
  // rebuild rather than avoiding the rebuild itself.
  const savedScrollTop = listEl.scrollTop;
  listEl.innerHTML = '';
  // The first of .contact-list's two fade spacers — see its own doc
  // (styles.css) for why an empty row here, not scroll-position tracking,
  // keeps the permanent top fade from showing when there's nothing above to
  // scroll to.
  const topFadeSpacer = document.createElement('div');
  topFadeSpacer.className = 'contact-list-fade-spacer top';
  topFadeSpacer.setAttribute('aria-hidden', 'true');
  listEl.appendChild(topFadeSpacer);
  for (const pairing of pairings) {
    const state = uiState(pairing.id);
    // No wrapping row div — nameGroup/Call/Delete are appended straight
    // into #contactList (a CSS grid) as three direct children per contact,
    // so grid-template-columns sizes each column once across every row.
    const nameGroup = document.createElement('span');
    nameGroup.className = 'contact-name-group';
    // Busy/Online/Offline are purely informational — Call stays available
    // regardless (see requestCall's own doc): tapping Call on an offline
    // contact just defers, and on a busy one resolves via the real
    // busy-signal exchange. A plain filled circle, not a text word — the
    // color alone is still the whole signal; role="img" + aria-label carry
    // the same meaning for a screen reader.
    const dot = document.createElement('span');
    dot.setAttribute('role', 'img');
    switch (state.status) {
      case 'busy':
        dot.className = 'contact-status-dot busy';
        dot.setAttribute('aria-label', 'Busy');
        break;
      case 'online':
        dot.className = 'contact-status-dot ok';
        dot.setAttribute('aria-label', 'Online');
        break;
      default:
        dot.className = 'contact-status-dot danger';
        dot.setAttribute('aria-label', 'Offline');
    }
    nameGroup.appendChild(dot);
    const name = document.createElement('span');
    name.className = 'contact-name';
    name.textContent = pairing.peerName || 'Unnamed contact';
    nameGroup.appendChild(name);
    listEl.appendChild(nameGroup);
    // Always created, even when this contact isn't callable right now —
    // hidden via .tv-button.call:disabled { visibility: hidden }, not left
    // out of the grid, so column 2 keeps the same width on every row.
    // `disabled` also takes it out of tab order.
    //
    // !callCore.isCallActive() is belt-and-suspenders: tapping Call on a
    // different contact while already on/dialing a call is a silent no-op
    // at the call_arbitration layer, currently unreachable only because
    // this screen doesn't render while a call is active, not because
    // anything enforces it.
    const canCall = isConfirmed(pairing) && !state.connected && !callCore.isCallActive();
    const btn = document.createElement('button');
    btn.className = 'tv-button call';
    // A phone-handset icon, not the word "Call" — same Material glyph path
    // as Android's Icons.Filled.Call.
    btn.innerHTML = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6.62 10.79c1.44 2.83 3.76 5.14 6.59 6.59l2.2-2.2c.27-.27.67-.36 1.02-.24 1.12.37 2.33.57 3.57.57.55 0 1 .45 1 1V20c0 .55-.45 1-1 1-9.39 0-17-7.61-17-17 0-.55.45-1 1-1h3.5c.55 0 1 .45 1 1 0 1.25.2 2.45.57 3.57.11.35.03.74-.25 1.02l-2.2 2.2z"/></svg>';
    btn.setAttribute('aria-label', `Call ${pairing.peerName || 'this contact'}`);
    btn.disabled = !canCall;
    btn.addEventListener('click', () => requestCall(pairing.id));
    listEl.appendChild(btn);
    // Destructive and irreversible (the pairing's own key is gone, not
    // just unlinked) — confirms via #screenConfirmDelete before actually
    // removing it. A real trash-can silhouette (tapered body, lid, handle,
    // three slats), same path data as Android's DeleteBinIcon.
    const del = document.createElement('button');
    del.className = 'delete-icon-btn';
    del.innerHTML = '<svg viewBox="0 0 1024 1024" aria-hidden="true"><path d="M266.2 256l47.2 581.4c0 32.4 26.2 58.6 58.6 58.6h282c32.4 0 58.6-26.2 58.6-58.6L759.2 256H266.2z m123.2 530L376 320h37l13.8 466h-37.4z m140.6 0h-36V320h36v466z m104.6 0h-37.2l13.6-466H648l-13.4 466zM728 184h-72l-52.6-46c-7.4-6.4-16.8-10-26.4-10h-129.6c-9.8 0-19.4 3.6-26.8 10L368 184h-72c-35.2 0-60 16.8-60 52h552c0-35.2-24.8-52-60-52z"/></svg>';
    del.setAttribute('aria-label', `Delete ${pairing.peerName || 'this contact'}`);
    del.addEventListener('click', () => {
      pendingDeletePairingId = pairing.id;
      screen = 'confirm-delete';
      render();
    });
    listEl.appendChild(del);
  }
  // A blank row, not just a bigger gap — sets "Add contact" apart as its
  // own action rather than one more entry in the contact list.
  const spacer = document.createElement('div');
  spacer.className = 'contact-list-spacer';
  listEl.appendChild(spacer);
  // Trailing row, not a separate Settings destination. .add-contact spans
  // every grid column and centers itself within that span — a standalone
  // action, not one more row sized to a single contact-list column.
  const addBtn = document.createElement('button');
  addBtn.className = 'tv-button add-contact';
  // A person-with-"+" icon — same Material "person_add" glyph path as
  // Android's PersonAddIcon.
  addBtn.innerHTML = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M15 12c2.21 0 4-1.79 4-4s-1.79-4-4-4-4 1.79-4 4 1.79 4 4 4zm-9-2V7H4v3H1v2h3v3h2v-3h3v-2H6zm9 4c-2.67 0-8 1.34-8 4v2h16v-2c0-2.66-5.33-4-8-4z"/></svg>';
  addBtn.setAttribute('aria-label', 'Add contact');
  addBtn.addEventListener('click', () => openEnterPhrase());
  listEl.appendChild(addBtn);
  // The second of .contact-list's two fade spacers — see the top one's own
  // doc, and .contact-list's (styles.css).
  const bottomFadeSpacer = document.createElement('div');
  bottomFadeSpacer.className = 'contact-list-fade-spacer bottom';
  bottomFadeSpacer.setAttribute('aria-hidden', 'true');
  listEl.appendChild(bottomFadeSpacer);
  // See savedScrollTop's own doc above — an out-of-range scrollTop clamps
  // to the real max on its own.
  listEl.scrollTop = savedScrollTop;
}

function setCheck(node, ok) {
  node.textContent = ok ? '✓' : '✗';
}

// --- Device settings (mirrors AdminChoiceScreen) ------------------------

function renderSettingsScreen() { el('renameInput').value = deviceName; }
el('renameBtn').addEventListener('click', () => { screen = 'rename'; render(); });
el('settingsCancel').addEventListener('click', () => { screen = 'waiting'; render(); });

// --- Delete confirmation (mirrors WaitingScreen's own OutcomeScreen use) --

function renderConfirmDeleteScreen() {
  const pairing = findPairing(pendingDeletePairingId);
  el('confirmDeleteTitle').textContent = `Delete ${(pairing && pairing.peerName) || 'this contact'}?`;
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
// click — startPairing mints a fresh crypto.randomUUID() every time, so two
// rapid submits for the same passphrase created two independent pending
// pairings at the same rendezvous tag, with only the first ever resolving.
let phraseSubmitInFlight = false;
el('phraseSubmit').addEventListener('click', async () => {
  if (phraseSubmitInFlight) return;
  const phrase = el('phraseInput').value.trim();
  if (!phrase) return;
  phraseSubmitInFlight = true;
  try {
    const pairing = await startPairing(phrase);
    currentPairingId = pairing.id;
    screen = 'pairing-progress';
    render();
  } finally {
    phraseSubmitInFlight = false;
  }
});
el('phraseInput').addEventListener('keydown', (e) => { if (e.key === 'Enter') el('phraseSubmit').click(); });
// Falls straight back to the waiting screen (where this was always reached
// from), not into Settings.
el('phraseCancel').addEventListener('click', () => { screen = 'waiting'; render(); });

// --- Pairing progress (mirrors PairingProgressScreen) --------------------

function renderPairingProgressScreen() {
  const collided = pairingCollision.has(currentPairingId);
  const timedOut = pairingTimedOut.has(currentPairingId);
  el('progressWaiting').hidden = collided || timedOut;
  el('progressCollision').hidden = !collided;
  el('progressTimeout').hidden = !timedOut;
}
function retryPairing() {
  // The attempt so far (including its unconfirmed stub) is forgotten
  // entirely, same as an explicit Cancel — startPairing mints an unrelated
  // fresh id below, so there's nothing to reuse it for.
  removePairing(currentPairingId);
  openEnterPhrase();
}
el('progressRetryCollision').addEventListener('click', retryPairing);
el('progressRetryTimeout').addEventListener('click', retryPairing);
el('progressCancel').addEventListener('click', () => { removePairing(currentPairingId); screen = 'waiting'; render(); });

// --- Name confirm (mirrors NameConfirmScreen) ----------------------------
//
// SPAKE2 already cryptographically proved both sides typed the same phrase
// by the time this shows — this tap is a cheap final sanity check, not a
// fingerprint comparison.

function renderPairScreen() {
  const candidate = confirmedCandidate.get(currentPairingId);
  el('pairTitle').textContent = `Pair with ${(candidate && candidate.name) || 'this device'}?`;
}
el('pairConfirm').addEventListener('click', () => {
  const candidate = confirmedCandidate.get(currentPairingId);
  if (candidate) confirmPeer(currentPairingId, candidate.pubkeyHex);
});
el('pairCancel').addEventListener('click', () => { removePairing(currentPairingId); screen = 'waiting'; render(); });

// --- Calling (mirrors CallingScreen) -------------------------------------

function startCallingUi(pairingId) {
  currentPairingId = pairingId;
  screen = 'calling';
  render();
}
function renderCallingScreen() {
  const pairing = findPairing(currentPairingId);
  el('callingName').textContent = (pairing && pairing.peerName) || 'Unnamed contact';
  callingLocalVideoEl.srcObject = localStream;
}
el('callingCancel').addEventListener('click', () => { hangUp(); });

// --- Incoming call (mirrors IncomingCallScreen) ---------------------------

function renderIncomingCallScreen() {
  const pairing = findPairing(currentPairingId);
  const name = (pairing && pairing.peerName) || 'Unnamed contact';
  el('incomingCallerName').textContent = name;
  el('incomingLocalVideo').srcObject = localStream;
  // See incomingCallAwaitingMedia's own doc — guards against a browser
  // permission-prompt tap landing on either button instead of the prompt
  // itself.
  el('incomingAccept').disabled = incomingCallAwaitingMedia;
  el('incomingDecline').disabled = incomingCallAwaitingMedia;
}
el('incomingAccept').addEventListener('click', () => { acceptIncomingCall(); });
el('incomingDecline').addEventListener('click', () => { hangUp(); });

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

const CALL_OUTCOME_COPY = {
  peer_ended: (name) => ['Call ended', `${name} ended the call.`],
  never_connected: (name) => ['Couldn’t connect',
    `The call with ${name} never connected. Check that both devices have a working internet connection, then try again.`],
  dropped: (name) => ['Call dropped',
    `The call with ${name} disconnected unexpectedly — usually just a brief network issue.`],
};

function renderCallOutcomeScreen() {
  const pairing = findPairing(pendingCallOutcome && pendingCallOutcome.pairingId);
  const name = (pairing && pairing.peerName) || 'Unnamed contact';
  const [title, message] = CALL_OUTCOME_COPY[pendingCallOutcome.reason](name);
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
  // Deliberately NOT awaited before connecting to relays: on a browser
  // without the Permissions API, checkPermissions() falls back to a real
  // getUserMedia() call that blocks on a human answering an actual
  // permission popup — awaiting it here would leave relay connection (and
  // the "Connected" status check) hanging behind that unrelated dialog. The
  // two checks are logically independent and must run concurrently.
  checkPermissions();
  screen = 'waiting';
  render();
  connectRelayClient();
}

window.addEventListener('online', render);
window.addEventListener('offline', render);

// Classic iOS Safari trick: body::after in styles.css makes the document
// exactly 1px taller than the viewport, and scrolling by that 1px is
// enough to collapse Safari's own address bar/toolbar in a plain browser
// tab — the page never asked to be added to the home screen, so this is
// the only lever available there. Standalone (home-screen) launches
// already have no browser chrome to collapse, so this is skipped there.
if (!window.navigator.standalone && !window.matchMedia('(display-mode: standalone)').matches) {
  window.addEventListener('load', () => {
    setTimeout(() => window.scrollTo(0, 1), 0);
  });
}

if (deviceName) {
  screen = 'waiting';
  startApp();
} else {
  screen = 'name';
  render();
}
