//! Pure Nostr-protocol-layer operations — gift-wrap construction/
//! unwrapping, plain bootstrap-event construction/verification, event
//! dedup, and relay filter-set construction. Mirrors
//! `NostrSignalingClient.kt`'s `publish`/`handleWrapEvent`/
//! `publishBootstrap`/`handleBootstrapEvent`/`seenEventIds`/
//! `currentFilters` and `app.js`'s identically-shaped twins.
//!
//! **What this module doesn't own**: the actual relay connection (the live
//! WebSocket, its reconnect policy) — that stays platform-native, the same
//! category as WebRTC's own `PeerConnection`. This module only ever takes
//! plain strings in and hands plain strings back — the shell owns every
//! actual byte that crosses the network.
//!
//! **The signaling message schema** (what a decrypted payload's own
//! `"type"` and fields mean — see [`SignalMessage`]) is owned here too, not
//! just the envelope around it — one schema instead of the six
//! independently hand-built send-side shapes plus a hand-parsed receive
//! side each platform used to keep in implicit agreement.
//!
//! **Wire format is unchanged from before this module existed**:
//! Porchlight's own custom kinds (`SIGNAL_KIND` 20331, `WRAP_KIND` 20336)
//! and its own simplified two-layer wrap (encrypt-then-sign an inner event,
//! encrypt-then-sign that as an outer event under a fresh one-time
//! keypair) — not the `nostr` crate's own NIP-59 gift-wrap kind/shape,
//! which this module never uses. The crate is depended on purely for
//! well-vetted NIP-44 encryption and spec-correct event construction/
//! verification, exactly like `pake_bridge` is depended on for SPAKE2.

use nostr::event::{Event, EventBuilder, FinalizeUnsignedEvent, Kind, Tag, UnsignedEvent};
use nostr::key::{Keys, PublicKey, SecretKey};
use nostr::nips::nip44;
use secp256k1::Secp256k1;
use serde::{Deserialize, Serialize};
use std::collections::{HashSet, VecDeque};

/// Must match `NostrSignalingClient.kt`'s/`app.js`'s `SIGNAL_KIND` exactly
/// — used both ways: the inner, NIP-44-encrypted payload of a confirmed
/// pairing's gift-wrapped traffic (never appearing directly on the wire),
/// and the plain, unencrypted bootstrap message of an in-progress pairing
/// attempt (appearing directly). Ephemeral (20000-29999 per NIP-01): relays
/// don't persist any of this.
pub(crate) const SIGNAL_KIND: u16 = 20331;
/// The gift-wrap kind — must match `NostrSignalingClient.kt`'s/`app.js`'s
/// `WRAP_KIND` exactly. A second custom, unregistered, ephemeral kind,
/// distinct from `SIGNAL_KIND`.
pub(crate) const WRAP_KIND: u16 = 20336;
/// Must match both platforms' `MAX_SEEN_EVENT_IDS` exactly.
const MAX_SEEN_EVENT_IDS: usize = 500;

fn keys_from_hex(secret_key_hex: &str) -> Option<Keys> {
    let sk = SecretKey::from_hex(secret_key_hex).ok()?;
    Some(Keys::new(sk))
}

fn pubkey_from_hex(hex: &str) -> Option<PublicKey> {
    PublicKey::from_hex(hex).ok()
}

/// A fresh, one-time keypair — used for the outer wrap's own signing key
/// (see [`build_wrapped_event`]) and by this module's own tests.
/// Deliberately not `nostr`'s own `Keys::generate()` (gated behind its
/// `rand`/`os-rng` feature, not enabled here): sourced from `getrandom`,
/// the same RNG every other module in this crate uses.
fn generate_keys() -> Keys {
    let mut bytes = [0u8; 32];
    let secret_key = loop {
        getrandom::getrandom(&mut bytes).expect("OS RNG must be available");
        if let Ok(sk) = SecretKey::from_slice(&bytes) {
            break sk;
        }
    };
    Keys::new(secret_key)
}

/// Signs an [`EventBuilder`] with `keys`, hand-rolled because `nostr`'s own
/// convenience `SignEvent` impl is gated behind the `os-rng` feature this
/// crate doesn't enable (see [`generate_keys`]). The auxiliary randomness
/// BIP-340 Schnorr signing takes is side-channel hardening, not a
/// correctness requirement — sourced from `getrandom`. `None` if the signed
/// event fails its own internal id/signature self-verification (would
/// indicate a bug in this function, not bad input).
fn finalize_event(builder: EventBuilder, keys: &Keys) -> Option<Event> {
    let unsigned: UnsignedEvent = builder.finalize_unsigned(keys.public_key());
    let id = unsigned.compute_id();
    let mut aux = [0u8; 32];
    getrandom::getrandom(&mut aux).ok()?;
    let secp = Secp256k1::new();
    let sig = keys.sign_schnorr_with_aux_rand(&secp, id.as_bytes(), &aux);
    unsigned.add_signature_with_ctx(&secp, sig).ok()
}

/// NIP-44 encrypt with our own externally-supplied nonce (via `getrandom`)
/// rather than the `nostr` crate's own `os-rng`-feature convenience
/// wrapper — keeps randomness sourced the same way on every target this
/// crate builds for, including `wasm32`.
fn nip44_encrypt(secret_key: &SecretKey, public_key: &PublicKey, content: &str) -> Option<String> {
    let mut nonce_bytes = [0u8; 32];
    getrandom::getrandom(&mut nonce_bytes).ok()?;
    let nonce = nip44::Nonce::V2(nonce_bytes);
    nip44::encrypt_with_nonce(secret_key, public_key, content, nonce).ok()
}

fn nip44_decrypt(secret_key: &SecretKey, public_key: &PublicKey, payload: &str) -> Option<String> {
    nip44::decrypt(secret_key, public_key, payload).ok()
}

/// Mirrors `publish`'s full two-layer wrap: NIP-44-encrypt `payload_json`
/// under the pairing's own permanent key, build+sign the inner event
/// (`SIGNAL_KIND`, tagged at `target`), generate a fresh one-time keypair,
/// NIP-44-encrypt the serialized inner event under it, build+sign the
/// outer wrap event (`WRAP_KIND`, also tagged at `target`). Returns the
/// fully-signed outer event as JSON, ready for the shell to hand to its
/// own relay-pool `publish()` call — construction and signing only, no
/// I/O. `None` on any malformed input (bad hex, a public key `nostr`
/// can't parse).
pub fn build_wrapped_event(own_private_key_hex: &str, target_pubkey_hex: &str, payload_json: &str) -> Option<String> {
    let own_keys = keys_from_hex(own_private_key_hex)?;
    let target_pubkey = pubkey_from_hex(target_pubkey_hex)?;

    let inner_content = nip44_encrypt(own_keys.secret_key(), &target_pubkey, payload_json)?;
    let inner_event = finalize_event(
        EventBuilder::new(Kind::from(SIGNAL_KIND), inner_content).tag(Tag::public_key(target_pubkey)),
        &own_keys,
    )?;

    let wrap_keys = generate_keys();
    let wrap_content = nip44_encrypt(wrap_keys.secret_key(), &target_pubkey, &inner_event.as_json())?;
    let wrap_event = finalize_event(
        EventBuilder::new(Kind::from(WRAP_KIND), wrap_content).tag(Tag::public_key(target_pubkey)),
        &wrap_keys,
    )?;

    Some(wrap_event.as_json())
}

/// Mirrors `handleWrapEvent`'s full chain: verify the outer event's own
/// signature → NIP-44-decrypt its content using (own key, the outer
/// event's own ephemeral one-time `pubkey`) → parse the inner event →
/// verify *its* signature → check `inner.pubkey == expected_peer_pubkey_hex`
/// (the wrap's `p` tag only routed this event to a pairing — it's never
/// proof of who actually sent it) → NIP-44-decrypt the inner event's
/// content using (own key, the *real pinned* peer pubkey) → return the
/// final plain JSON payload. `None` on any failed step.
///
/// Returns `(payload_json, created_at, event_id)` — both are the *inner*
/// event's own (signature-covered, unlike the outer wrap event's). Used by
/// [`unwrap_wrapped_event_for_any`] to compare against a candidate's own
/// `last_signal_created_at`/`last_signal_event_id` — see that field's own
/// doc for why both are needed, not `created_at` alone: Nostr timestamps
/// are whole seconds, so two distinct, legitimately back-to-back messages
/// can share one.
pub fn unwrap_wrapped_event(wrap_event_json: &str, own_private_key_hex: &str, expected_peer_pubkey_hex: &str) -> Option<(String, u64, String)> {
    let own_keys = keys_from_hex(own_private_key_hex)?;
    let expected_peer_pubkey = pubkey_from_hex(expected_peer_pubkey_hex)?;

    let wrap_event = Event::from_json(wrap_event_json).ok()?;
    wrap_event.verify().ok()?;

    let inner_json = nip44_decrypt(own_keys.secret_key(), &wrap_event.pubkey, &wrap_event.content)?;
    let inner_event = Event::from_json(&inner_json).ok()?;
    inner_event.verify().ok()?;

    if inner_event.pubkey != expected_peer_pubkey {
        return None;
    }

    let payload = nip44_decrypt(own_keys.secret_key(), &expected_peer_pubkey, &inner_event.content)?;
    Some((payload, inner_event.created_at.as_secs(), inner_event.id.to_hex()))
}

/// One confirmed pairing's routing-and-decryption keys, as far as
/// [`unwrap_wrapped_event_for_any`] needs them — mirrors
/// `NostrSignalingClient.ConfirmedPeer`/`app.js`'s identically-shaped
/// `confirmedPeers()` entries, plus two fields:
///
/// `last_signal_created_at`/`last_signal_event_id` — the shell's own
/// persisted record (on `Pairing`, alongside `ownPrivateKeyHex`/
/// `peerPublicKey`/...) of the newest wrap event (by inner `created_at`)
/// this pairing has actually processed, and that exact event's own inner
/// id; both `0`/`""` for a pairing that's never received one yet. A relay
/// redelivering an already-processed event is rejected by comparing
/// against this watermark; a *different* event that merely shares the same
/// `created_at` second (e.g. a `"call"` immediately followed by its own
/// `"offer"`) is not — `created_at` alone can't tell those apart since
/// Nostr timestamps are whole seconds, so `last_signal_event_id` resolves
/// the tie: only an exact `(created_at, event_id)` match — a literal
/// redelivery of the one event that set the watermark — is rejected.
#[derive(Deserialize, Debug, Clone, PartialEq)]
pub struct WrapEventCandidate {
    pub pairing_id: String,
    pub own_private_key_hex: String,
    pub peer_public_key: String,
    #[serde(default)]
    pub last_signal_created_at: u64,
    #[serde(default)]
    pub last_signal_event_id: String,
}

/// What a successfully routed-and-decrypted wrap event hands back — the
/// caller must persist `signal_created_at`/`signal_event_id` as that
/// pairing's new `last_signal_created_at`/`last_signal_event_id` (see that
/// field's own doc).
#[derive(Serialize, Debug, PartialEq)]
pub struct RoutedSignalPayload {
    pub pairing_id: String,
    pub payload_json: String,
    pub signal_created_at: u64,
    pub signal_event_id: String,
}

/// Finds which of `candidates` a gift-wrapped event's own `p` tag names,
/// then decrypts it for that one — replaces a hand-rolled `p`-tag
/// extraction plus linear search each platform used to duplicate. Each
/// pairing's own per-pairing pubkey is freshly generated and never reused,
/// so at most one candidate can ever match — returns that one, or `None`
/// if none do, the event has no `p` tag, or it doesn't parse.
///
/// Delegates the actual decrypt-verify-decrypt chain to
/// [`unwrap_wrapped_event`] once routed, rather than duplicating it.
pub fn unwrap_wrapped_event_for_any(wrap_event_json: &str, candidates: &[WrapEventCandidate]) -> Option<RoutedSignalPayload> {
    let event = Event::from_json(wrap_event_json).ok()?;
    let recipient_pubkey = event.tags.public_keys().next()?;

    let candidate = candidates
        .iter()
        .find(|c| keys_from_hex(&c.own_private_key_hex).map(|k| k.public_key() == recipient_pubkey).unwrap_or(false))?;

    let (payload_json, created_at, event_id) = unwrap_wrapped_event(wrap_event_json, &candidate.own_private_key_hex, &candidate.peer_public_key)?;

    // See WrapEventCandidate::last_signal_created_at's own doc — strictly
    // older than the watermark is always stale; equal is only stale if
    // it's literally the same event that set it.
    if created_at < candidate.last_signal_created_at
        || (created_at == candidate.last_signal_created_at && event_id == candidate.last_signal_event_id)
    {
        return None;
    }

    Some(RoutedSignalPayload { pairing_id: candidate.pairing_id.clone(), payload_json, signal_created_at: created_at, signal_event_id: event_id })
}

/// A fresh, short correlation id for one call attempt — independent of
/// `call_arbitration::random_call_id`'s identical shape: this module stays
/// independent of `call_arbitration`/`presence`. Used only by
/// [`parse_signal_payload`]'s own missing-callId fallback.
fn random_call_id() -> String {
    let mut buf = [0u8; 4];
    getrandom::getrandom(&mut buf).expect("OS RNG must be available");
    crate::hex_encode(&buf)
}

/// One signaling message exchanged between confirmed peers over the
/// gift-wrapped channel.
///
/// Wire format is unchanged: `#[serde(tag = "type")]` with each variant's
/// fields named after the exact camelCase keys both platforms have always
/// sent (`callId`/`sdpMid`/`sdpMLineIndex`). `call_id` fields deserialize
/// with `#[serde(default)]` (an empty string, never absent) —
/// [`parse_signal_payload`] replaces an empty one with a fresh random id
/// afterward, so a stale message's id can never accidentally compare-equal
/// to a real active call.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type")]
pub enum SignalMessage {
    #[serde(rename = "heartbeat")]
    Heartbeat {
        name: String,
        /// `Some(_)` only for a real heartbeat — `#[serde(default)]` so a
        /// message missing the key deserializes to `None`, matching
        /// `presence::mark_seen`'s own `peer_busy: Option<bool>` parameter.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        busy: Option<bool>,
    },
    #[serde(rename = "leaving")]
    Leaving,
    #[serde(rename = "bye")]
    Bye {
        #[serde(rename = "callId", default)]
        call_id: String,
    },
    #[serde(rename = "busy")]
    Busy {
        #[serde(rename = "callId", default)]
        call_id: String,
    },
    #[serde(rename = "call")]
    Call {
        #[serde(rename = "callId", default)]
        call_id: String,
    },
    #[serde(rename = "offer")]
    Offer {
        sdp: String,
        #[serde(rename = "callId", default)]
        call_id: String,
    },
    #[serde(rename = "answer")]
    Answer {
        sdp: String,
        #[serde(rename = "callId", default)]
        call_id: String,
    },
    #[serde(rename = "ice")]
    Ice {
        #[serde(rename = "sdpMid", default, skip_serializing_if = "Option::is_none")]
        sdp_mid: Option<String>,
        #[serde(rename = "sdpMLineIndex", default)]
        sdp_m_line_index: i32,
        candidate: String,
        #[serde(rename = "callId", default)]
        call_id: String,
    },
}

fn ensure_call_id(call_id: String) -> String {
    if call_id.trim().is_empty() { random_call_id() } else { call_id }
}

/// Mirrors `heartbeatTick`'s payload construction exactly — `name` is this
/// device's own self-reported name, `busy` is [`crate::call_arbitration::is_call_active`]'s
/// value, both sent unconditionally on every heartbeat.
pub fn build_heartbeat_payload(name: &str, busy: bool) -> Option<String> {
    serde_json::to_string(&SignalMessage::Heartbeat { name: name.to_string(), busy: Some(busy) }).ok()
}

/// Mirrors `close()`'s/its web twin's `sendToConfirmedPeer(peer, "leaving")` — no fields at all.
pub fn build_leaving_payload() -> Option<String> {
    serde_json::to_string(&SignalMessage::Leaving).ok()
}

/// Mirrors `hangUp`'s payload exactly.
pub fn build_bye_payload(call_id: &str) -> Option<String> {
    serde_json::to_string(&SignalMessage::Bye { call_id: call_id.to_string() }).ok()
}

/// Mirrors `sendBusy`'s payload — `call_id` is required here, unlike the
/// old hand-built nullable version, since every real call site already
/// always supplies one.
pub fn build_busy_payload(call_id: &str) -> Option<String> {
    serde_json::to_string(&SignalMessage::Busy { call_id: call_id.to_string() }).ok()
}

/// Mirrors `sendCall`'s payload exactly.
pub fn build_call_payload(call_id: &str) -> Option<String> {
    serde_json::to_string(&SignalMessage::Call { call_id: call_id.to_string() }).ok()
}

/// Mirrors `sendOffer`'s payload exactly.
pub fn build_offer_payload(sdp: &str, call_id: &str) -> Option<String> {
    serde_json::to_string(&SignalMessage::Offer { sdp: sdp.to_string(), call_id: call_id.to_string() }).ok()
}

/// Mirrors `sendAnswer`'s payload exactly.
pub fn build_answer_payload(sdp: &str, call_id: &str) -> Option<String> {
    serde_json::to_string(&SignalMessage::Answer { sdp: sdp.to_string(), call_id: call_id.to_string() }).ok()
}

/// Mirrors `sendIce`'s payload exactly. `sdp_mid`: `None` crosses as an
/// absent key, not an empty-string sentinel — this is JSON crossing a
/// wire, not the JNI boundary.
pub fn build_ice_payload(sdp_mid: Option<&str>, sdp_m_line_index: i32, candidate: &str, call_id: &str) -> Option<String> {
    serde_json::to_string(&SignalMessage::Ice {
        sdp_mid: sdp_mid.map(String::from),
        sdp_m_line_index,
        candidate: candidate.to_string(),
        call_id: call_id.to_string(),
    })
    .ok()
}

/// Mirrors `dispatchFromConfirmedPeer`'s full per-type extraction and
/// validation — the receive-side twin of the eight `build_*_payload`
/// functions above. `None` for an unrecognized `"type"`, a malformed shape
/// (stricter than the old duck-typed `optString`/`optInt` both platforms
/// used to tolerate — a malformed message from a confirmed,
/// cryptographically-verified peer is adversarial or buggy either way), an
/// `offer`/`answer` whose `sdp` is blank or over [`crate::MAX_SDP_LENGTH`],
/// or an `ice` whose `candidate` is blank or over
/// [`crate::MAX_ICE_CANDIDATE_LENGTH`]. A `heartbeat`'s `name` comes back
/// already capped to [`crate::MAX_NAME_LENGTH`] and run through
/// [`crate::sanitize_name`]. Every `call_id` comes back through
/// [`ensure_call_id`], substituting a fresh random one for a
/// missing/blank value.
pub fn parse_signal_payload(payload_json: &str) -> Option<SignalMessage> {
    let message: SignalMessage = serde_json::from_str(payload_json).ok()?;
    Some(match message {
        SignalMessage::Heartbeat { name, busy } => {
            SignalMessage::Heartbeat { name: crate::sanitize_name(crate::truncate_chars(&name, crate::MAX_NAME_LENGTH)), busy }
        }
        SignalMessage::Leaving => SignalMessage::Leaving,
        SignalMessage::Bye { call_id } => SignalMessage::Bye { call_id: ensure_call_id(call_id) },
        SignalMessage::Busy { call_id } => SignalMessage::Busy { call_id: ensure_call_id(call_id) },
        SignalMessage::Call { call_id } => SignalMessage::Call { call_id: ensure_call_id(call_id) },
        SignalMessage::Offer { sdp, call_id } => {
            if sdp.trim().is_empty() || sdp.len() > crate::MAX_SDP_LENGTH {
                return None;
            }
            SignalMessage::Offer { sdp, call_id: ensure_call_id(call_id) }
        }
        SignalMessage::Answer { sdp, call_id } => {
            if sdp.trim().is_empty() || sdp.len() > crate::MAX_SDP_LENGTH {
                return None;
            }
            SignalMessage::Answer { sdp, call_id: ensure_call_id(call_id) }
        }
        SignalMessage::Ice { sdp_mid, sdp_m_line_index, candidate, call_id } => {
            if candidate.trim().is_empty() || candidate.len() > crate::MAX_ICE_CANDIDATE_LENGTH {
                return None;
            }
            let sdp_mid = sdp_mid.filter(|s| !s.trim().is_empty());
            SignalMessage::Ice { sdp_mid, sdp_m_line_index, candidate, call_id: ensure_call_id(call_id) }
        }
    })
}

/// Mirrors `publishBootstrap`/`sendPairingBootstrap`: a plain
/// (unencrypted) self-signed event — no encryption is possible or needed
/// at this phase, since neither side knows the other's real pubkey until
/// this exchange itself reveals it, and a SPAKE2 blinded message is safe
/// to publish in the open by construction (see `pake_bridge`'s own doc).
/// Tagged with the rendezvous `d` tag and, once known, the candidate's own
/// `p` tag. `None` on malformed input.
pub fn build_bootstrap_event(own_private_key_hex: &str, rendezvous_tag: &str, target_pubkey_hex: Option<&str>, payload_json: &str) -> Option<String> {
    let own_keys = keys_from_hex(own_private_key_hex)?;
    let mut builder = EventBuilder::new(Kind::from(SIGNAL_KIND), payload_json).tag(Tag::identifier(rendezvous_tag));
    if let Some(target) = target_pubkey_hex {
        builder = builder.tag(Tag::public_key(pubkey_from_hex(target)?));
    }
    let event = finalize_event(builder, &own_keys)?;
    Some(event.as_json())
}

/// What a verified bootstrap event hands back — mirrors `handleBootstrapEvent`'s
/// own extraction (sender pubkey, `d` tag, plain payload) exactly.
#[derive(Serialize, Debug, PartialEq)]
pub struct VerifiedBootstrapEvent {
    pub sender_pubkey_hex: String,
    pub rendezvous_tag: String,
    pub payload_json: String,
}

/// Mirrors `handleBootstrapEvent`'s verification: still calls `verify()`
/// even though bootstrap content is plaintext (signature verification is
/// required protocol-level work independent of the encryption layer).
/// `None` if the signature is invalid, the event has no `d` tag, or the
/// JSON is malformed.
pub fn verify_bootstrap_event(event_json: &str) -> Option<VerifiedBootstrapEvent> {
    let event = Event::from_json(event_json).ok()?;
    event.verify().ok()?;
    let rendezvous_tag = event.tags.identifier()?;
    Some(VerifiedBootstrapEvent {
        sender_pubkey_hex: event.pubkey.to_hex(),
        rendezvous_tag,
        payload_json: event.content.clone(),
    })
}

/// The `seenEventIds` dedup — a bounded set (500-entry cap, evict-oldest):
/// the same relay-broadcast event can arrive redundantly from more than
/// one of the 3 relays at once, and re-delivering an offer/answer/ice
/// message a second time to the WebRTC layer is unsafe. A field of
/// `crate::AppState`, not its own separately-locked static.
pub(crate) struct DedupState {
    ids: HashSet<String>,
    order: VecDeque<String>,
}

impl DedupState {
    pub(crate) fn new() -> Self {
        DedupState { ids: HashSet::new(), order: VecDeque::new() }
    }
}

/// `true` for a genuinely new `event_id` (the caller should process this
/// event); `false` if it's already been seen (the caller must drop it
/// silently). Evicts the single oldest entry once the set exceeds
/// [`MAX_SEEN_EVENT_IDS`].
pub fn mark_seen_or_is_duplicate(event_id: &str) -> bool {
    let mut app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    let state = &mut app.dedup;
    if !state.ids.insert(event_id.to_string()) {
        return false;
    }
    state.order.push_back(event_id.to_string());
    if state.order.len() > MAX_SEEN_EVENT_IDS {
        if let Some(oldest) = state.order.pop_front() {
            state.ids.remove(&oldest);
        }
    }
    true
}

/// One relay filter's worth of data — `kind` plus the single tag type that
/// filter matches on (`'p'` for the wrap filter, `'d'` for the bootstrap
/// filter). Deliberately not a richer `Filter` type: each shell has its own
/// relay-client library's own filter type and translates this plain data
/// into it.
#[derive(Serialize, Debug, PartialEq)]
pub struct FilterSpec {
    pub kind: u16,
    pub tag_name: char,
    pub tag_values: Vec<String>,
}

/// `None` for either half when there's nothing to filter for yet —
/// matches both platforms' existing "omit if empty" behavior (a fresh
/// device with no confirmed peers and no pending pairings subscribes for
/// nothing at all).
#[derive(Serialize, Debug, PartialEq)]
pub struct RelayFilters {
    pub wrap_filter: Option<FilterSpec>,
    pub bootstrap_filter: Option<FilterSpec>,
}

/// Mirrors `currentFilters`'s exact filter-set construction: a `WRAP_KIND`/
/// `#p` filter over every confirmed peer's own pubkey (so this device is
/// notified of gift-wrapped traffic addressed to any of its per-pairing
/// identities), and a `SIGNAL_KIND`/`#d` filter over every pending
/// pairing's rendezvous tag.
pub fn build_relay_filters(confirmed_own_pubkeys: &[String], pending_rendezvous_tags: &[String]) -> RelayFilters {
    let wrap_filter = if confirmed_own_pubkeys.is_empty() {
        None
    } else {
        Some(FilterSpec { kind: WRAP_KIND, tag_name: 'p', tag_values: confirmed_own_pubkeys.to_vec() })
    };
    let bootstrap_filter = if pending_rendezvous_tags.is_empty() {
        None
    } else {
        Some(FilterSpec { kind: SIGNAL_KIND, tag_name: 'd', tag_values: pending_rendezvous_tags.to_vec() })
    };
    RelayFilters { wrap_filter, bootstrap_filter }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::types::Timestamp;

    fn fresh_keys_hex() -> (String, String) {
        let keys = generate_keys();
        (keys.secret_key().to_secret_hex(), keys.public_key().to_hex())
    }

    // --- gift wrap round trip ---

    #[test]
    fn build_then_unwrap_recovers_the_original_payload() {
        let (alice_sk, alice_pk) = fresh_keys_hex();
        let (bob_sk, bob_pk) = fresh_keys_hex();

        let payload = r#"{"type":"heartbeat","name":"Alice"}"#;
        let wrap_json = build_wrapped_event(&alice_sk, &bob_pk, payload).expect("build should succeed");

        let (recovered, _created_at, _event_id) = unwrap_wrapped_event(&wrap_json, &bob_sk, &alice_pk).expect("unwrap should succeed");
        assert_eq!(recovered, payload);
    }

    #[test]
    fn wrap_event_kind_and_outer_pubkey_are_not_the_senders_real_identity() {
        // The whole point of the outer wrap: relays never see either
        // party's real per-pairing identity, only a fresh one-time key.
        let (alice_sk, alice_pk) = fresh_keys_hex();
        let (_bob_sk, bob_pk) = fresh_keys_hex();

        let wrap_json = build_wrapped_event(&alice_sk, &bob_pk, "{}").unwrap();
        let wrap_event = Event::from_json(&wrap_json).unwrap();
        assert_eq!(u16::from(wrap_event.kind), WRAP_KIND);
        assert_ne!(wrap_event.pubkey.to_hex(), alice_pk, "the outer event must be signed by a fresh one-time key, not the real sender identity");
    }

    #[test]
    fn unwrap_rejects_when_inner_event_is_not_signed_by_the_expected_pinned_peer() {
        // The wrap's `p` tag only routes to a pairing, never proves sender
        // identity. Simulate a wrap addressed to Bob whose inner event was
        // actually signed by a third party (Eve), not the pinned peer.
        let (_alice_sk, alice_pk) = fresh_keys_hex();
        let (eve_sk, _eve_pk) = fresh_keys_hex();
        let (bob_sk, bob_pk) = fresh_keys_hex();

        // Build a wrap "as Eve" but targeted at Bob -- structurally
        // identical to a genuine message, just from the wrong sender.
        let wrap_json = build_wrapped_event(&eve_sk, &bob_pk, "{}").unwrap();

        // Bob (using his own real key, so the outer layer decrypts fine)
        // expects this pairing's peer to be Alice, not Eve.
        let recovered = unwrap_wrapped_event(&wrap_json, &bob_sk, &alice_pk);
        assert!(recovered.is_none(), "must reject an inner event not signed by the expected pinned peer");
    }

    /// A relay redelivering an event this pairing has already processed —
    /// simulated by feeding the same event's own `signal_created_at`/
    /// `signal_event_id` back in as the candidate's own watermark, the way
    /// a shell restores it from its own persisted `Pairing` after a reload.
    #[test]
    fn unwrap_for_any_rejects_a_redelivery_of_the_candidates_own_last_processed_signal() {
        let (alice_sk, alice_pk) = fresh_keys_hex();
        let (bob_sk, bob_pk) = fresh_keys_hex();

        let wrap_json = build_wrapped_event(&alice_sk, &bob_pk, r#"{"type":"call","callId":"abcd1234"}"#).unwrap();

        let fresh_candidate = WrapEventCandidate {
            pairing_id: "bob-pairing".to_string(),
            own_private_key_hex: bob_sk.clone(),
            peer_public_key: alice_pk.clone(),
            last_signal_created_at: 0,
            last_signal_event_id: String::new(),
        };
        let first_pass = unwrap_wrapped_event_for_any(&wrap_json, &[fresh_candidate]).expect("a genuinely new signal must be accepted");

        // Mirrors a reload: a brand new candidate list, built from whatever
        // the shell persisted (here, exactly what the first pass just
        // returned) rather than anything held over in memory.
        let post_reload_candidate = WrapEventCandidate {
            pairing_id: "bob-pairing".to_string(),
            own_private_key_hex: bob_sk,
            peer_public_key: alice_pk,
            last_signal_created_at: first_pass.signal_created_at,
            last_signal_event_id: first_pass.signal_event_id,
        };
        assert!(
            unwrap_wrapped_event_for_any(&wrap_json, &[post_reload_candidate]).is_none(),
            "a relay redelivering the exact same already-processed event must be dropped"
        );
    }

    /// Nostr `created_at` is whole seconds, so a `"call"` immediately
    /// followed by its own `"offer"` can easily share one — two *different*
    /// events forced onto the same second must both still go through.
    #[test]
    fn unwrap_for_any_accepts_a_different_event_sharing_the_watermarks_own_second() {
        let alice_keys = generate_keys();
        let alice_pk = alice_keys.public_key().to_hex();
        let (bob_sk, bob_pk) = fresh_keys_hex();
        let bob_pubkey = pubkey_from_hex(&bob_pk).unwrap();
        let shared_second = Timestamp::now();

        let build_at = |payload: &str, at: Timestamp| -> String {
            let inner_content = nip44_encrypt(alice_keys.secret_key(), &bob_pubkey, payload).unwrap();
            let inner_event = finalize_event(
                EventBuilder::new(Kind::from(SIGNAL_KIND), inner_content).tag(Tag::public_key(bob_pubkey)).custom_created_at(at),
                &alice_keys,
            )
            .unwrap();
            let wrap_keys = generate_keys();
            let wrap_content = nip44_encrypt(wrap_keys.secret_key(), &bob_pubkey, &inner_event.as_json()).unwrap();
            finalize_event(EventBuilder::new(Kind::from(WRAP_KIND), wrap_content).tag(Tag::public_key(bob_pubkey)), &wrap_keys).unwrap().as_json()
        };

        let call_json = build_at(r#"{"type":"call","callId":"abcd1234"}"#, shared_second);
        let offer_json = build_at(r#"{"type":"offer","callId":"abcd1234","sdp":"v=0"}"#, shared_second);

        let candidate = WrapEventCandidate {
            pairing_id: "bob-pairing".to_string(),
            own_private_key_hex: bob_sk.clone(),
            peer_public_key: alice_pk.clone(),
            last_signal_created_at: 0,
            last_signal_event_id: String::new(),
        };
        let after_call = unwrap_wrapped_event_for_any(&call_json, &[candidate]).expect("the call itself must be accepted");
        assert_eq!(after_call.signal_created_at, shared_second.as_secs());

        let candidate_after_call = WrapEventCandidate {
            pairing_id: "bob-pairing".to_string(),
            own_private_key_hex: bob_sk,
            peer_public_key: alice_pk,
            last_signal_created_at: after_call.signal_created_at,
            last_signal_event_id: after_call.signal_event_id,
        };
        let after_offer = unwrap_wrapped_event_for_any(&offer_json, &[candidate_after_call]);
        assert!(after_offer.is_some(), "a different event sharing the watermark's own second must still be accepted, not treated as a duplicate");
    }

    #[test]
    fn unwrap_returns_none_for_malformed_json() {
        let (bob_sk, _) = fresh_keys_hex();
        let (_, alice_pk) = fresh_keys_hex();
        assert!(unwrap_wrapped_event("not json", &bob_sk, &alice_pk).is_none());
    }

    #[test]
    fn build_returns_none_for_a_malformed_pubkey() {
        let (alice_sk, _) = fresh_keys_hex();
        assert!(build_wrapped_event(&alice_sk, "not-a-real-pubkey", "{}").is_none());
    }

    // --- routed unwrap for a confirmed-peer candidate list ---

    #[test]
    fn unwrap_wrapped_event_for_any_routes_to_the_matching_candidate_among_several() {
        let (alice_sk, alice_pk) = fresh_keys_hex();
        let (bob_sk, bob_pk) = fresh_keys_hex();
        let (carol_sk, carol_pk) = fresh_keys_hex();
        let (dave_sk, dave_pk) = fresh_keys_hex();

        let payload = r#"{"type":"heartbeat","name":"Alice"}"#;
        // Alice's message is addressed to (tagged at) Bob, using Bob's own
        // pairing-specific pubkey as the recipient.
        let wrap_json = build_wrapped_event(&alice_sk, &bob_pk, payload).unwrap();

        // Bob's own device has three confirmed pairings, not just Alice's --
        // Carol's and Dave's own (own_private_key_hex, peer_public_key)
        // pairs are structurally identical in shape, just for unrelated
        // relationships this event was never addressed to.
        let candidates = vec![
            WrapEventCandidate { pairing_id: "carol-pairing".to_string(), own_private_key_hex: carol_sk, peer_public_key: "irrelevant-peer".to_string(), last_signal_created_at: 0, last_signal_event_id: String::new() },
            WrapEventCandidate { pairing_id: "bob-pairing".to_string(), own_private_key_hex: bob_sk, peer_public_key: alice_pk, last_signal_created_at: 0, last_signal_event_id: String::new() },
            WrapEventCandidate { pairing_id: "dave-pairing".to_string(), own_private_key_hex: dave_sk, peer_public_key: "another-irrelevant-peer".to_string(), last_signal_created_at: 0, last_signal_event_id: String::new() },
        ];
        let _ = (carol_pk, dave_pk); // only the own_private_key_hex halves matter for routing

        let routed = unwrap_wrapped_event_for_any(&wrap_json, &candidates).expect("must route to bob-pairing");
        assert_eq!(routed.pairing_id, "bob-pairing");
        assert_eq!(routed.payload_json, payload);
    }

    #[test]
    fn unwrap_wrapped_event_for_any_is_none_when_no_candidate_matches() {
        let (alice_sk, _alice_pk) = fresh_keys_hex();
        let (_bob_sk, bob_pk) = fresh_keys_hex();
        let (carol_sk, _carol_pk) = fresh_keys_hex();

        let wrap_json = build_wrapped_event(&alice_sk, &bob_pk, "{}").unwrap();
        // Bob's own pairing is absent from this candidate list entirely --
        // e.g. deleted between the event being sent and this device
        // processing it.
        let candidates = vec![WrapEventCandidate { pairing_id: "carol-pairing".to_string(), own_private_key_hex: carol_sk, peer_public_key: "irrelevant".to_string(), last_signal_created_at: 0, last_signal_event_id: String::new() }];

        assert!(unwrap_wrapped_event_for_any(&wrap_json, &candidates).is_none());
    }

    #[test]
    fn unwrap_wrapped_event_for_any_is_none_for_an_empty_candidate_list() {
        let (alice_sk, _) = fresh_keys_hex();
        let (_, bob_pk) = fresh_keys_hex();
        let wrap_json = build_wrapped_event(&alice_sk, &bob_pk, "{}").unwrap();
        assert!(unwrap_wrapped_event_for_any(&wrap_json, &[]).is_none());
    }

    #[test]
    fn unwrap_wrapped_event_for_any_is_none_for_malformed_json() {
        let (bob_sk, _) = fresh_keys_hex();
        let candidates = vec![WrapEventCandidate { pairing_id: "bob-pairing".to_string(), own_private_key_hex: bob_sk, peer_public_key: "irrelevant".to_string(), last_signal_created_at: 0, last_signal_event_id: String::new() }];
        assert!(unwrap_wrapped_event_for_any("not json", &candidates).is_none());
    }

    #[test]
    fn unwrap_wrapped_event_for_any_still_rejects_an_inner_sender_that_doesnt_match_the_pinned_peer() {
        // Routing succeeds -- addressed to Bob's pairing with "Alice" --
        // but the inner event was signed by a third party. Confirms
        // routing doesn't weaken the pinned-peer check.
        let (_alice_sk, alice_pk) = fresh_keys_hex();
        let (eve_sk, _eve_pk) = fresh_keys_hex();
        let (bob_sk, bob_pk) = fresh_keys_hex();

        let wrap_json = build_wrapped_event(&eve_sk, &bob_pk, "{}").unwrap();
        let candidates = vec![WrapEventCandidate { pairing_id: "bob-alice-pairing".to_string(), own_private_key_hex: bob_sk, peer_public_key: alice_pk, last_signal_created_at: 0, last_signal_event_id: String::new() }];

        assert!(unwrap_wrapped_event_for_any(&wrap_json, &candidates).is_none());
    }

    // --- bootstrap event round trip ---

    #[test]
    fn build_then_verify_bootstrap_event_recovers_sender_tag_and_payload() {
        let (alice_sk, alice_pk) = fresh_keys_hex();
        let payload = r#"{"type":"pake1","outbound":"aabb","name":"Alice"}"#;

        let event_json = build_bootstrap_event(&alice_sk, "rendezvous-tag-123", None, payload).unwrap();
        let verified = verify_bootstrap_event(&event_json).expect("verification should succeed");

        assert_eq!(verified.sender_pubkey_hex, alice_pk);
        assert_eq!(verified.rendezvous_tag, "rendezvous-tag-123");
        assert_eq!(verified.payload_json, payload);
    }

    #[test]
    fn build_bootstrap_event_with_a_known_target_includes_a_p_tag() {
        let (alice_sk, _) = fresh_keys_hex();
        let (_, bob_pk) = fresh_keys_hex();
        let event_json = build_bootstrap_event(&alice_sk, "tag", Some(&bob_pk), "{}").unwrap();
        let event = Event::from_json(&event_json).unwrap();
        assert!(event.tags.public_keys().any(|pk| pk.to_hex() == bob_pk));
    }

    #[test]
    fn verify_bootstrap_event_rejects_a_tampered_signature() {
        let (alice_sk, _) = fresh_keys_hex();
        let event_json = build_bootstrap_event(&alice_sk, "tag", None, "{}").unwrap();
        // Flip a character in the JSON's content field to invalidate the
        // signature without breaking JSON parsing.
        let tampered = event_json.replacen("\"content\":\"{}\"", "\"content\":\"{\\\"x\\\":1}\"", 1);
        assert!(verify_bootstrap_event(&tampered).is_none());
    }

    #[test]
    fn verify_bootstrap_event_rejects_an_event_with_no_d_tag() {
        // Not constructible via build_bootstrap_event (which always adds
        // one) -- build the event directly to simulate a malformed/hostile
        // message missing the rendezvous tag entirely.
        let keys = generate_keys();
        let event = finalize_event(EventBuilder::new(Kind::from(SIGNAL_KIND), "{}"), &keys).unwrap();
        assert!(verify_bootstrap_event(&event.as_json()).is_none());
    }

    // --- dedup ---

    #[test]
    fn mark_seen_or_is_duplicate_is_true_once_then_false() {
        let id = format!("dedup-test-{}", generate_keys().public_key().to_hex());
        assert!(mark_seen_or_is_duplicate(&id), "first sighting must be reported as new");
        assert!(!mark_seen_or_is_duplicate(&id), "second sighting of the same id must be reported as a duplicate");
    }

    #[test]
    fn mark_seen_or_is_duplicate_evicts_the_oldest_entry_past_the_cap() {
        let prefix = format!("evict-test-{}-", generate_keys().public_key().to_hex());
        let first_id = format!("{prefix}0");
        assert!(mark_seen_or_is_duplicate(&first_id));
        for i in 1..=MAX_SEEN_EVENT_IDS {
            assert!(mark_seen_or_is_duplicate(&format!("{prefix}{i}")));
        }
        // The very first id inserted should have been evicted by now --
        // seeing it again must count as "new" a second time.
        assert!(mark_seen_or_is_duplicate(&first_id), "the oldest entry should have been evicted once the cap was exceeded");
    }

    // --- relay filter construction ---

    #[test]
    fn build_relay_filters_is_none_for_both_when_nothing_is_confirmed_or_pending() {
        let filters = build_relay_filters(&[], &[]);
        assert_eq!(filters, RelayFilters { wrap_filter: None, bootstrap_filter: None });
    }

    #[test]
    fn build_relay_filters_builds_the_wrap_filter_from_confirmed_pubkeys() {
        let pubkeys = vec!["aaa".to_string(), "bbb".to_string()];
        let filters = build_relay_filters(&pubkeys, &[]);
        assert_eq!(filters.wrap_filter, Some(FilterSpec { kind: WRAP_KIND, tag_name: 'p', tag_values: pubkeys }));
        assert_eq!(filters.bootstrap_filter, None);
    }

    #[test]
    fn build_relay_filters_builds_the_bootstrap_filter_from_pending_rendezvous_tags() {
        let tags = vec!["tag-1".to_string()];
        let filters = build_relay_filters(&[], &tags);
        assert_eq!(filters.bootstrap_filter, Some(FilterSpec { kind: SIGNAL_KIND, tag_name: 'd', tag_values: tags }));
        assert_eq!(filters.wrap_filter, None);
    }

    // --- signal message schema ---

    #[test]
    fn build_heartbeat_payload_matches_the_wire_format_both_platforms_already_send() {
        let json = build_heartbeat_payload("Living Room", true).unwrap();
        assert_eq!(json, r#"{"type":"heartbeat","name":"Living Room","busy":true}"#);
    }

    #[test]
    fn build_leaving_payload_has_no_fields_at_all() {
        assert_eq!(build_leaving_payload().unwrap(), r#"{"type":"leaving"}"#);
    }

    #[test]
    fn build_bye_payload_matches_the_wire_format() {
        assert_eq!(build_bye_payload("call1").unwrap(), r#"{"type":"bye","callId":"call1"}"#);
    }

    #[test]
    fn build_offer_payload_matches_the_wire_format() {
        assert_eq!(build_offer_payload("v=0", "call1").unwrap(), r#"{"type":"offer","sdp":"v=0","callId":"call1"}"#);
    }

    #[test]
    fn build_ice_payload_omits_sdp_mid_when_none_matching_orgjsons_null_removes_key_behavior() {
        let json = build_ice_payload(None, 0, "candidate-str", "call1").unwrap();
        assert_eq!(json, r#"{"type":"ice","sdpMLineIndex":0,"candidate":"candidate-str","callId":"call1"}"#);
    }

    #[test]
    fn build_ice_payload_includes_sdp_mid_when_present() {
        let json = build_ice_payload(Some("0"), 0, "candidate-str", "call1").unwrap();
        assert_eq!(json, r#"{"type":"ice","sdpMid":"0","sdpMLineIndex":0,"candidate":"candidate-str","callId":"call1"}"#);
    }

    #[test]
    fn every_build_payload_round_trips_through_parse_signal_payload() {
        assert_eq!(
            parse_signal_payload(&build_heartbeat_payload("Alice", false).unwrap()).unwrap(),
            SignalMessage::Heartbeat { name: "Alice".to_string(), busy: Some(false) }
        );
        assert_eq!(parse_signal_payload(&build_leaving_payload().unwrap()).unwrap(), SignalMessage::Leaving);
        assert_eq!(
            parse_signal_payload(&build_bye_payload("call1").unwrap()).unwrap(),
            SignalMessage::Bye { call_id: "call1".to_string() }
        );
        assert_eq!(
            parse_signal_payload(&build_busy_payload("call1").unwrap()).unwrap(),
            SignalMessage::Busy { call_id: "call1".to_string() }
        );
        assert_eq!(
            parse_signal_payload(&build_call_payload("call1").unwrap()).unwrap(),
            SignalMessage::Call { call_id: "call1".to_string() }
        );
        assert_eq!(
            parse_signal_payload(&build_offer_payload("v=0", "call1").unwrap()).unwrap(),
            SignalMessage::Offer { sdp: "v=0".to_string(), call_id: "call1".to_string() }
        );
        assert_eq!(
            parse_signal_payload(&build_answer_payload("v=0", "call1").unwrap()).unwrap(),
            SignalMessage::Answer { sdp: "v=0".to_string(), call_id: "call1".to_string() }
        );
        assert_eq!(
            parse_signal_payload(&build_ice_payload(Some("0"), 1, "candidate-str", "call1").unwrap()).unwrap(),
            SignalMessage::Ice { sdp_mid: Some("0".to_string()), sdp_m_line_index: 1, candidate: "candidate-str".to_string(), call_id: "call1".to_string() }
        );
    }

    #[test]
    fn parse_signal_payload_is_none_for_an_unrecognized_type() {
        assert_eq!(parse_signal_payload(r#"{"type":"nonsense"}"#), None);
    }

    #[test]
    fn parse_signal_payload_is_none_for_malformed_json() {
        assert_eq!(parse_signal_payload("not json"), None);
    }

    #[test]
    fn parse_signal_payload_substitutes_a_random_call_id_when_missing() {
        let message = parse_signal_payload(r#"{"type":"bye"}"#).unwrap();
        let SignalMessage::Bye { call_id } = message else { panic!("expected Bye, got {message:?}") };
        assert!(!call_id.is_empty(), "a missing callId must never come back as empty/absent");
    }

    #[test]
    fn parse_signal_payload_substitutes_a_random_call_id_when_blank() {
        let message = parse_signal_payload(r#"{"type":"busy","callId":"   "}"#).unwrap();
        let SignalMessage::Busy { call_id } = message else { panic!("expected Busy, got {message:?}") };
        assert!(!call_id.trim().is_empty(), "a whitespace-only callId must be replaced, not passed through");
    }

    #[test]
    fn two_missing_call_ids_get_different_random_substitutes() {
        // Not a security property -- confirms this isn't accidentally
        // returning the same fixed placeholder every time.
        let a = parse_signal_payload(r#"{"type":"bye"}"#).unwrap();
        let b = parse_signal_payload(r#"{"type":"bye"}"#).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn parse_signal_payload_is_none_for_a_blank_offer_sdp() {
        assert_eq!(parse_signal_payload(r#"{"type":"offer","sdp":"","callId":"call1"}"#), None);
        assert_eq!(parse_signal_payload(r#"{"type":"offer","sdp":"   ","callId":"call1"}"#), None);
    }

    #[test]
    fn parse_signal_payload_is_none_for_an_oversized_offer_sdp() {
        let huge_sdp = "a".repeat(crate::MAX_SDP_LENGTH + 1);
        let json = build_offer_payload(&huge_sdp, "call1").unwrap();
        assert_eq!(parse_signal_payload(&json), None);
    }

    #[test]
    fn parse_signal_payload_is_none_for_a_blank_or_oversized_ice_candidate() {
        assert_eq!(parse_signal_payload(r#"{"type":"ice","candidate":"","callId":"call1"}"#), None);
        let huge_candidate = "a".repeat(crate::MAX_ICE_CANDIDATE_LENGTH + 1);
        let json = build_ice_payload(None, 0, &huge_candidate, "call1").unwrap();
        assert_eq!(parse_signal_payload(&json), None);
    }

    #[test]
    fn parse_signal_payload_treats_a_blank_sdp_mid_the_same_as_absent() {
        let message = parse_signal_payload(r#"{"type":"ice","sdpMid":"   ","sdpMLineIndex":0,"candidate":"c","callId":"call1"}"#).unwrap();
        assert_eq!(
            message,
            SignalMessage::Ice { sdp_mid: None, sdp_m_line_index: 0, candidate: "c".to_string(), call_id: "call1".to_string() }
        );
    }

    #[test]
    fn parse_signal_payload_defaults_a_missing_sdp_m_line_index_to_zero() {
        let message = parse_signal_payload(r#"{"type":"ice","candidate":"c","callId":"call1"}"#).unwrap();
        let SignalMessage::Ice { sdp_m_line_index, .. } = message else { panic!("expected Ice, got {message:?}") };
        assert_eq!(sdp_m_line_index, 0);
    }

    #[test]
    fn parse_signal_payload_caps_the_raw_heartbeat_name_before_sanitizing() {
        // Truncate-then-sanitize, in that order -- a plain name with
        // nothing to strip, so the result length is exactly the cap.
        let long_name = "b".repeat(crate::MAX_NAME_LENGTH + 50);
        let json = build_heartbeat_payload(&long_name, false).unwrap();
        let message = parse_signal_payload(&json).unwrap();
        let SignalMessage::Heartbeat { name, .. } = message else { panic!("expected Heartbeat, got {message:?}") };
        assert_eq!(name.chars().count(), crate::MAX_NAME_LENGTH);
    }

    #[test]
    fn parse_signal_payload_strips_zero_width_characters_from_the_heartbeat_name() {
        // A stripped character inside the truncation window legitimately
        // shrinks the result below MAX_NAME_LENGTH -- asserts the
        // character is gone and the result never exceeds the cap.
        let dirty_name = format!("A\u{200B}{}", "b".repeat(crate::MAX_NAME_LENGTH + 50));
        let json = build_heartbeat_payload(&dirty_name, false).unwrap();
        let message = parse_signal_payload(&json).unwrap();
        let SignalMessage::Heartbeat { name, .. } = message else { panic!("expected Heartbeat, got {message:?}") };
        assert!(!name.contains('\u{200B}'), "zero-width space must be stripped");
        assert!(name.chars().count() <= crate::MAX_NAME_LENGTH);
    }

    #[test]
    fn parse_signal_payload_leaves_a_missing_heartbeat_busy_as_none() {
        let message = parse_signal_payload(r#"{"type":"heartbeat","name":"Alice"}"#).unwrap();
        assert_eq!(message, SignalMessage::Heartbeat { name: "Alice".to_string(), busy: None });
    }
}

/// A chained test across all three `call-core` modules, covering the full
/// wire-to-decision path in one shot rather than each module in isolation:
/// unwrap a wrap event, feed the payload into `presence::mark_seen` (which
/// internally calls into `call_arbitration` when a deferred call was
/// waiting), and confirm this module's own dedup catches a redelivery —
/// all without a real relay.
#[cfg(test)]
mod full_stack_chain_tests {
    use super::*;
    use crate::{call_arbitration, presence};

    fn fresh_keys_hex() -> (String, String) {
        let keys = generate_keys();
        (keys.secret_key().to_secret_hex(), keys.public_key().to_hex())
    }

    #[test]
    fn wrapped_heartbeat_resolves_a_deferred_call_through_presence_into_call_arbitration() {
        // Serializes against every other call_arbitration/presence test in
        // this workspace.
        let _guard = call_arbitration::reset_state_for_test();

        let (alice_sk, alice_pk) = fresh_keys_hex();
        let (bob_sk, bob_pk) = fresh_keys_hex();
        let pairing_id = "full-stack-chain-alice-bob";

        // 1. Alice wants to call Bob, but he's offline right now --
        //    request_call's deferred-call branch.
        let request = call_arbitration::request_call(pairing_id, &alice_pk, &bob_pk, false);
        assert_eq!(request.effects, vec![call_arbitration::CallEffect::AcquireMedia]);

        // 2. Bob's device sends its regular gift-wrapped heartbeat,
        //    wrapped from Bob to Alice.
        let heartbeat_payload = r#"{"type":"heartbeat"}"#;
        let wrap_json = build_wrapped_event(&bob_sk, &alice_pk, heartbeat_payload).expect("build must succeed");

        // 3. Alice's relay subscription delivers that event. Unwrap it --
        //    this is nostr_protocol's own contribution to the chain.
        let (recovered_payload, _created_at, _event_id) = unwrap_wrapped_event(&wrap_json, &alice_sk, &bob_pk).expect("unwrap must succeed");
        assert_eq!(recovered_payload, heartbeat_payload);

        // 4. Dedup: confirm the guard catches a redelivery, using the wrap
        //    event's real id.
        let wrap_event_id = Event::from_json(&wrap_json).unwrap().id.to_hex();
        assert!(mark_seen_or_is_duplicate(&wrap_event_id), "first delivery must be reported as new");
        assert!(!mark_seen_or_is_duplicate(&wrap_event_id), "a relay redelivering the same event must be caught as a duplicate");

        // 5. Every dispatched message calls presence::mark_seen first,
        //    regardless of type.
        let update = presence::mark_seen(pairing_id, &alice_pk, &bob_pk, 1_000, None);

        // Bob coming online must resolve Alice's deferred call: accept
        // either legitimate resolution (CreateOffer or SendCall) since the
        // tiebreak direction depends on the random keys generated this run.
        assert_eq!(update.call_effects.len(), 1, "{:?}", update.call_effects);
        assert!(
            matches!(
                update.call_effects[0],
                call_arbitration::CallEffect::CreateOffer { .. } | call_arbitration::CallEffect::SendCall { .. }
            ),
            "expected the deferred call to resolve one way or the other, got {:?}",
            update.call_effects
        );

        // And presence itself must now consider Bob online.
        assert!(presence::is_online(pairing_id));
    }
}
