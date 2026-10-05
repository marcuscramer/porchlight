//! The single entry point for everything that arrives from the relays.
//!
//! A shell hands in one raw event plus a snapshot of its pairings and gets
//! back one ordered result to execute: dedup, unwrap/verify, parse, presence
//! bookkeeping and the per-message handling all happen here, so the two shells
//! no longer carry their own copies of the routing `switch`. What stays in the
//! shell is only what needs the shell: persisting the new signal watermark,
//! sending, renaming a contact, and driving WebRTC.
//!
//! Two wire shapes arrive on the same subscription (see `nostr_protocol`):
//! gift-wrapped traffic of confirmed pairings, and plain bootstrap events of a
//! pairing attempt still in progress.

use crate::call_arbitration::{self, CallEffect, IceOutcome};
use crate::nostr_protocol::{self, SignalMessage, WrapEventCandidate};
use crate::presence::{self, PresenceEffect, PresenceUpdateResult};
use crate::Effect;
use nostr::event::Event;
use serde::{Deserialize, Serialize};

/// One confirmed pairing, as the shell currently has it.
#[derive(Deserialize, Debug, Clone)]
pub struct RoutePeer {
    pub pairing_id: String,
    pub own_private_key_hex: String,
    pub peer_public_key: String,
    #[serde(default)]
    pub last_signal_created_at: u64,
    #[serde(default)]
    pub last_signal_event_id: String,
    /// The pairing's auto-answer setting *now* (a snapshot is taken for a
    /// ring; see `call_arbitration::StartRinging`).
    #[serde(default)]
    pub auto_answer: bool,
}

/// One pairing attempt still in progress, found by its rendezvous tag.
#[derive(Deserialize, Debug, Clone)]
pub struct RoutePending {
    pub pairing_id: String,
    pub rendezvous_tag: String,
}

/// Everything the router needs to know about the shell's pairings.
#[derive(Deserialize, Debug, Clone, Default)]
pub struct RouteContext {
    /// This device's own name, for answering a `hello` heartbeat.
    #[serde(default)]
    pub device_name: String,
    #[serde(default)]
    pub confirmed: Vec<RoutePeer>,
    #[serde(default)]
    pub pending: Vec<RoutePending>,
}

/// The new "last processed signal" of a pairing. The shell must persist it
/// before doing anything else with the result, so a redelivery of this event
/// (or anything older) is rejected on any later attempt, restarts included.
#[derive(Serialize, Debug, PartialEq)]
pub struct ProcessedSignal {
    pub pairing_id: String,
    pub created_at: u64,
    pub event_id: String,
}

/// What the shell does with the result that is neither presence nor call
/// state.
#[derive(Serialize, Debug, PartialEq)]
#[serde(tag = "kind")]
pub enum RouteAction {
    /// Send this ready-made heartbeat to the pairing now (the peer said
    /// `hello`).
    SendHeartbeat { pairing_id: String, payload_json: String },
    /// The peer reported a name; the shell stores it if it differs.
    UpdatePeerName { pairing_id: String, name: String },
    /// Hand the peer's answer to the active call's connection.
    ApplyRemoteAnswer { pairing_id: String, sdp: String },
    /// Hand this candidate to the active call's connection.
    AddRemoteIce { pairing_id: String, sdp_mid: Option<String>, sdp_m_line_index: i32, candidate: String },
}

/// The ordered result of routing one event. Execute in this order: persist
/// `signal`, apply presence effects, then call effects, then `actions`,
/// then `bootstrap_effects` (only one of the two groups is ever non-empty
/// for a single event).
#[derive(Serialize, Debug, Default, PartialEq)]
pub struct RouteResult {
    pub signal: Option<ProcessedSignal>,
    #[serde(flatten)]
    pub update: PresenceUpdateResult,
    pub actions: Vec<RouteAction>,
    /// Pairing-attempt effects, for a bootstrap event.
    pub bootstrap_effects: Vec<Effect>,
    /// A contact's heartbeat reported a newer relay list than ours: the shell fetches it now.
    pub fetch_relay_list: bool,
}

/// The JSON an empty result serializes to — the shells' fallback when the
/// core itself fails.
pub fn empty_result_json() -> String {
    serde_json::to_string(&RouteResult::default()).unwrap_or_else(|_| "{}".to_string())
}

/// [`route_event`] across the FFI: the context arrives as JSON, and an
/// unreadable one routes nothing.
pub fn route_event_json(event_json: &str, context_json: &str, now_ms: i64) -> RouteResult {
    match serde_json::from_str::<RouteContext>(context_json) {
        Ok(ctx) => route_event(event_json, &ctx, now_ms),
        Err(_) => RouteResult::default(),
    }
}

/// Routes one raw relay event (JSON). An event that is a duplicate, not for
/// this device, malformed or unverifiable gives an empty result.
pub fn route_event(event_json: &str, ctx: &RouteContext, now_ms: i64) -> RouteResult {
    let Ok(event) = Event::from_json(event_json) else { return RouteResult::default() };
    if !nostr_protocol::mark_seen_or_is_duplicate(&event.id.to_hex()) {
        return RouteResult::default();
    }
    let kind = event.kind.as_u16();
    if kind == nostr_protocol::WRAP_KIND {
        route_wrap(event_json, ctx, now_ms)
    } else if kind == nostr_protocol::SIGNAL_KIND {
        route_bootstrap(event_json, ctx, now_ms)
    } else {
        RouteResult::default()
    }
}

/// A pairing message older than this when it arrives is a relay replaying an old event, not the other
/// device talking now (its messages are republished every 10 s). Generous, so a phone whose clock is a few
/// minutes off can still pair.
const BOOTSTRAP_MAX_AGE_SECS: u64 = 300;

fn route_bootstrap(event_json: &str, ctx: &RouteContext, now_ms: i64) -> RouteResult {
    let mut result = RouteResult::default();
    let Some(verified) = nostr_protocol::verify_bootstrap_event(event_json) else { return result };
    if (now_ms.max(0) as u64 / 1000).saturating_sub(verified.created_at) > BOOTSTRAP_MAX_AGE_SECS {
        return result;
    }
    let Some(pending) = ctx.pending.iter().find(|p| p.rendezvous_tag == verified.rendezvous_tag) else { return result };
    let Ok(payload) = serde_json::from_str::<serde_json::Value>(&verified.payload_json) else { return result };
    let Some(type_) = payload.get("type").and_then(|t| t.as_str()).filter(|t| !t.trim().is_empty()) else { return result };
    result.bootstrap_effects = crate::handle_bootstrap_message(&pending.pairing_id, &verified.sender_pubkey_hex, type_, &verified.payload_json);
    result
}

fn route_wrap(event_json: &str, ctx: &RouteContext, now_ms: i64) -> RouteResult {
    let mut result = RouteResult::default();
    let candidates: Vec<WrapEventCandidate> = ctx
        .confirmed
        .iter()
        .map(|p| WrapEventCandidate {
            pairing_id: p.pairing_id.clone(),
            own_private_key_hex: p.own_private_key_hex.clone(),
            peer_public_key: p.peer_public_key.clone(),
            last_signal_created_at: p.last_signal_created_at,
            last_signal_event_id: p.last_signal_event_id.clone(),
        })
        .collect();
    let Some(routed) = nostr_protocol::unwrap_wrapped_event_for_any(event_json, &candidates) else { return result };
    let Some(peer) = ctx.confirmed.iter().find(|p| p.pairing_id == routed.pairing_id) else { return result };
    let Some(own_pubkey) = nostr_protocol::public_key_hex(&peer.own_private_key_hex) else { return result };

    result.signal = Some(ProcessedSignal { pairing_id: routed.pairing_id.clone(), created_at: routed.signal_created_at, event_id: routed.signal_event_id });

    // A message that doesn't parse (unknown type, bad fields) is still a
    // validly signed and decrypted one from a confirmed peer, so it still
    // counts as "the peer is alive".
    let message = nostr_protocol::parse_signal_payload(&routed.payload_json);
    let (peer_busy, peer_hello) = match &message {
        Some(SignalMessage::Heartbeat { busy, hello, .. }) => (*busy, *hello),
        _ => (None, false),
    };
    if let Some(SignalMessage::Heartbeat { list_version: Some(v), .. }) = &message {
        result.fetch_relay_list = crate::relay_list::note_peer_version(*v, now_ms);
    }
    let id = peer.pairing_id.as_str();
    let mut seen = presence::mark_seen(id, &own_pubkey, &peer.peer_public_key, now_ms, peer_busy, peer_hello);
    let wants_reply = seen.presence_effects.iter().any(|e| matches!(e, PresenceEffect::ReplyHeartbeat { pairing_id } if pairing_id == id));
    seen.presence_effects.retain(|e| !matches!(e, PresenceEffect::ReplyHeartbeat { .. }));
    // Built before the message itself is handled, like the shells did: the
    // `busy` it reports is the state at the time the peer's hello arrived.
    let reply = if wants_reply { nostr_protocol::build_heartbeat_reply_payload(&ctx.device_name, call_arbitration::is_call_active()) } else { None };
    result.update.extend(seen);
    if let Some(payload_json) = reply {
        result.actions.push(RouteAction::SendHeartbeat { pairing_id: id.to_string(), payload_json });
    }

    let Some(message) = message else { return result };
    match message {
        SignalMessage::Heartbeat { name, .. } => {
            if !name.trim().is_empty() {
                result.actions.push(RouteAction::UpdatePeerName { pairing_id: id.to_string(), name });
            }
        }
        SignalMessage::Leaving => result.update.extend(presence::handle_leaving_message(id)),
        SignalMessage::Bye { call_id, reason } => {
            result.update.call_effects.extend(call_arbitration::handle_peer_hangup(id, &call_id, reason.as_deref() == Some("media")))
        }
        SignalMessage::Busy { call_id } => {
            result.update.extend(presence::handle_peer_busy_reply(id, &own_pubkey, &peer.peer_public_key, &call_id, now_ms));
        }
        SignalMessage::Call { call_id } => {
            let effects: Vec<CallEffect> = call_arbitration::handle_should_offer(id, &call_id, &own_pubkey, &peer.peer_public_key, peer.auto_answer);
            result.update.call_effects.extend(effects);
        }
        SignalMessage::Offer { sdp, call_id } => result.update.call_effects.extend(call_arbitration::handle_offer(id, &call_id, &sdp, peer.auto_answer)),
        SignalMessage::Answer { sdp, call_id } => {
            if call_arbitration::should_apply_answer(id, &call_id) {
                result.actions.push(RouteAction::ApplyRemoteAnswer { pairing_id: id.to_string(), sdp });
            }
        }
        SignalMessage::Ice { sdp_mid, sdp_m_line_index, candidate, call_id } => {
            if let IceOutcome::Apply { sdp_mid, sdp_m_line_index, candidate } =
                call_arbitration::handle_remote_ice(id, &call_id, sdp_mid.as_deref(), sdp_m_line_index, &candidate)
            {
                result.actions.push(RouteAction::AddRemoteIce { pairing_id: id.to_string(), sdp_mid, sdp_m_line_index, candidate });
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presence::PresenceStatus;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn fresh_id() -> String {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        format!("router-pairing-{}", COUNTER.fetch_add(1, Ordering::Relaxed))
    }

    /// A confirmed pairing between "me" and a "peer", plus what the peer
    /// needs to send me things.
    struct Pair {
        ctx: RouteContext,
        peer_secret: String,
        my_pubkey: String,
        id: String,
    }

    fn pair(auto_answer: bool) -> Pair {
        let me = nostr_protocol::generate_keys();
        let peer = nostr_protocol::generate_keys();
        let id = fresh_id();
        let ctx = RouteContext {
            device_name: "Living room".to_string(),
            confirmed: vec![RoutePeer {
                pairing_id: id.clone(),
                own_private_key_hex: me.secret_key().to_secret_hex(),
                peer_public_key: peer.public_key().to_hex(),
                last_signal_created_at: 0,
                last_signal_event_id: String::new(),
                auto_answer,
            }],
            pending: vec![],
        };
        Pair { ctx, peer_secret: peer.secret_key().to_secret_hex(), my_pubkey: me.public_key().to_hex(), id }
    }

    impl Pair {
        fn wrap(&self, payload: &str) -> String {
            nostr_protocol::build_wrapped_event(&self.peer_secret, &self.my_pubkey, payload).expect("wrap")
        }
        fn route(&self, payload: &str) -> RouteResult {
            route_event(&self.wrap(payload), &self.ctx, 1_000)
        }
    }

    fn statuses(r: &RouteResult) -> Vec<PresenceStatus> {
        r.update
            .presence_effects
            .iter()
            .filter_map(|e| match e {
                PresenceEffect::SetStatus { status, .. } => Some(status.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_heartbeat_marks_the_peer_online_and_reports_its_name() {
        let _g = call_arbitration::reset_state_for_test();
        let p = pair(false);
        let r = p.route(r#"{"type":"heartbeat","name":"Anna","busy":false}"#);
        assert_eq!(statuses(&r), vec![PresenceStatus::Online]);
        assert_eq!(r.actions, vec![RouteAction::UpdatePeerName { pairing_id: p.id.clone(), name: "Anna".to_string() }]);
        let signal = r.signal.expect("signal watermark");
        assert_eq!(signal.pairing_id, p.id);
        assert!(!signal.event_id.is_empty());
    }

    #[test]
    fn a_heartbeat_with_a_newer_relay_list_version_asks_for_a_fetch() {
        let _g = call_arbitration::reset_state_for_test();
        let _l = crate::relay_list::TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        crate::relay_list::reset_for_test();
        crate::relay_list::init(3, &["wss://a.example.com".to_string()]);
        let p = pair(false);
        let same = p.route(r#"{"type":"heartbeat","name":"Anna","busy":false,"listVersion":3}"#);
        assert!(!same.fetch_relay_list, "same version");
        let older = p.route(r#"{"type":"heartbeat","name":"Anna","busy":false,"listVersion":2}"#);
        assert!(!older.fetch_relay_list);
        let none = p.route(r#"{"type":"heartbeat","name":"Anna","busy":false}"#);
        assert!(!none.fetch_relay_list, "an older client sends no version");
        let newer = p.route(r#"{"type":"heartbeat","name":"Anna","busy":false,"listVersion":4}"#);
        assert!(newer.fetch_relay_list);
        crate::relay_list::reset_for_test();
    }

    #[test]
    fn a_busy_heartbeat_marks_the_peer_busy() {
        let _g = call_arbitration::reset_state_for_test();
        let p = pair(false);
        let r = p.route(r#"{"type":"heartbeat","name":"Anna","busy":true}"#);
        assert_eq!(statuses(&r), vec![PresenceStatus::Busy]);
    }

    #[test]
    fn a_hello_gets_a_ready_made_heartbeat_back_but_never_carries_hello_itself() {
        let _g = call_arbitration::reset_state_for_test();
        let p = pair(false);
        let r = p.route(r#"{"type":"heartbeat","name":"Anna","busy":false,"hello":true}"#);
        let payload = r
            .actions
            .iter()
            .find_map(|a| match a {
                RouteAction::SendHeartbeat { pairing_id, payload_json } if *pairing_id == p.id => Some(payload_json.clone()),
                _ => None,
            })
            .expect("heartbeat reply");
        let sent: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(sent["type"], "heartbeat");
        assert_eq!(sent["name"], "Living room");
        assert_eq!(sent["busy"], false);
        assert!(sent.get("hello").is_none());
        // The internal "please reply" effect never reaches the shell.
        assert!(r.update.presence_effects.iter().all(|e| !matches!(e, PresenceEffect::ReplyHeartbeat { .. })));
    }

    #[test]
    fn answering_a_hello_leaves_this_devices_own_pending_hello_alone() {
        let _g = call_arbitration::reset_state_for_test();
        let p = pair(false);
        presence::request_hello();
        p.route(r#"{"type":"heartbeat","name":"Anna","busy":false,"hello":true}"#);
        assert!(presence::take_hello());
    }

    #[test]
    fn a_redelivered_event_is_ignored() {
        let _g = call_arbitration::reset_state_for_test();
        let p = pair(false);
        let wrapped = p.wrap(r#"{"type":"heartbeat","name":"Anna","busy":false}"#);
        assert!(route_event(&wrapped, &p.ctx, 1_000).signal.is_some());
        assert_eq!(route_event(&wrapped, &p.ctx, 1_001), RouteResult::default());
    }

    #[test]
    fn an_event_not_older_than_the_stored_watermark_is_ignored() {
        let _g = call_arbitration::reset_state_for_test();
        let mut p = pair(false);
        p.ctx.confirmed[0].last_signal_created_at = u64::MAX;
        assert_eq!(p.route(r#"{"type":"heartbeat","name":"Anna","busy":false}"#), RouteResult::default());
    }

    #[test]
    fn an_event_for_nobody_here_is_ignored() {
        let _g = call_arbitration::reset_state_for_test();
        let p = pair(false);
        let stranger = nostr_protocol::generate_keys().public_key().to_hex();
        let wrapped = nostr_protocol::build_wrapped_event(&p.peer_secret, &stranger, r#"{"type":"leaving"}"#).unwrap();
        assert_eq!(route_event(&wrapped, &p.ctx, 1_000), RouteResult::default());
    }

    #[test]
    fn a_message_from_the_wrong_sender_is_ignored() {
        let _g = call_arbitration::reset_state_for_test();
        let p = pair(false);
        let impostor = nostr_protocol::generate_keys().secret_key().to_secret_hex();
        let wrapped = nostr_protocol::build_wrapped_event(&impostor, &p.my_pubkey, r#"{"type":"leaving"}"#).unwrap();
        assert_eq!(route_event(&wrapped, &p.ctx, 1_000), RouteResult::default());
    }

    #[test]
    fn garbage_is_ignored() {
        let _g = call_arbitration::reset_state_for_test();
        let ctx = RouteContext::default();
        assert_eq!(route_event("not json", &ctx, 0), RouteResult::default());
        assert_eq!(route_event("{}", &ctx, 0), RouteResult::default());
    }

    #[test]
    fn an_unknown_message_type_still_counts_as_the_peer_being_alive() {
        let _g = call_arbitration::reset_state_for_test();
        let p = pair(false);
        let r = p.route(r#"{"type":"something-new"}"#);
        assert_eq!(statuses(&r), vec![PresenceStatus::Online]);
        assert!(r.actions.is_empty());
        assert!(r.signal.is_some());
    }

    #[test]
    fn leaving_takes_the_peer_offline() {
        let _g = call_arbitration::reset_state_for_test();
        let p = pair(false);
        p.route(r#"{"type":"heartbeat","name":"Anna","busy":false}"#);
        let r = p.route(r#"{"type":"leaving"}"#);
        assert_eq!(statuses(&r), vec![PresenceStatus::Offline]);
    }

    #[test]
    fn an_offer_starts_ringing_with_the_pairings_own_auto_answer_setting() {
        let _g = call_arbitration::reset_state_for_test();
        let p = pair(true);
        let r = p.route(r#"{"type":"offer","sdp":"v=0","callId":"c1"}"#);
        assert!(
            r.update.call_effects.iter().any(|e| matches!(e, CallEffect::StartRinging { call_id, auto_answer: true, .. } if call_id == "c1")),
            "{:?}",
            r.update.call_effects
        );
    }

    #[test]
    fn a_second_call_while_busy_is_answered_with_busy() {
        let _g = call_arbitration::reset_state_for_test();
        let p = pair(false);
        // The device's one call slot is taken by a call with someone else.
        call_arbitration::request_call("another-pairing", "zzz", "aaa", true, 0);
        let r = p.route(r#"{"type":"offer","sdp":"v=0","callId":"c2"}"#);
        assert!(r.update.call_effects.iter().any(|e| matches!(e, CallEffect::SendBusy { call_id, .. } if call_id == "c2")), "{:?}", r.update.call_effects);
    }

    #[test]
    fn a_bye_ends_the_ringing_call_it_is_about_and_only_that_one() {
        let _g = call_arbitration::reset_state_for_test();
        let p = pair(false);
        p.route(r#"{"type":"offer","sdp":"v=0","callId":"c1"}"#);
        assert!(p.route(r#"{"type":"bye","callId":"other"}"#).update.call_effects.is_empty());
        assert!(!p.route(r#"{"type":"bye","callId":"c1"}"#).update.call_effects.is_empty());
    }

    #[test]
    fn a_bye_that_blames_the_camera_tells_the_caller_the_other_side_could_not_answer() {
        let _g = call_arbitration::reset_state_for_test();
        let p = pair(false);
        let call_id = call_arbitration::request_call(&p.id, "zzz", "aaa", true, 0).call_id.expect("call id");
        let r = p.route(&format!(r#"{{"type":"bye","callId":"{call_id}","reason":"media"}}"#));
        assert!(
            r.update.call_effects.iter().any(|e| matches!(e, CallEffect::ShowCallOutcome { reason: call_arbitration::CallOutcomeReason::PeerMediaFailed, .. })),
            "{:?}",
            r.update.call_effects
        );
    }

    #[test]
    fn an_answer_is_applied_once_to_the_call_it_belongs_to() {
        let _g = call_arbitration::reset_state_for_test();
        let p = pair(false);
        let started = call_arbitration::request_call(&p.id, "zzz", "aaa", true, 0);
        let call_id = started.call_id.expect("call id");
        let answer = format!(r#"{{"type":"answer","sdp":"v=0","callId":"{call_id}"}}"#);
        assert_eq!(p.route(&answer).actions, vec![RouteAction::ApplyRemoteAnswer { pairing_id: p.id.clone(), sdp: "v=0".to_string() }]);
        assert!(p.route(&answer).actions.is_empty(), "answer applied twice");
        assert!(p.route(r#"{"type":"answer","sdp":"v=0","callId":"unrelated"}"#).actions.is_empty());
    }

    #[test]
    fn ice_is_applied_for_the_active_call_and_dropped_otherwise() {
        let _g = call_arbitration::reset_state_for_test();
        let p = pair(false);
        let call_id = call_arbitration::request_call(&p.id, "zzz", "aaa", true, 0).call_id.expect("call id");
        let ice = |id: &str| format!(r#"{{"type":"ice","candidate":"candidate:1","sdpMid":"0","sdpMLineIndex":0,"callId":"{id}"}}"#);
        assert_eq!(
            p.route(&ice(&call_id)).actions,
            vec![RouteAction::AddRemoteIce { pairing_id: p.id.clone(), sdp_mid: Some("0".to_string()), sdp_m_line_index: 0, candidate: "candidate:1".to_string() }]
        );
        assert!(p.route(&ice("unrelated")).actions.is_empty());
    }

    #[test]
    fn a_busy_reply_frees_the_call_slot() {
        let _g = call_arbitration::reset_state_for_test();
        let p = pair(false);
        let call_id = call_arbitration::request_call(&p.id, "zzz", "aaa", true, 0).call_id.expect("call id");
        let r = p.route(&format!(r#"{{"type":"busy","callId":"{call_id}"}}"#));
        assert_eq!(statuses(&r).last(), Some(&PresenceStatus::Busy));
        assert!(!call_arbitration::is_call_active());
    }

    fn candidate_of(pairing_id: &str) -> Option<String> {
        let v: serde_json::Value = serde_json::from_str(&crate::build_bootstrap_payload(pairing_id)?).ok()?;
        v.get("candidate_pubkey")?.as_str().map(String::from)
    }

    #[test]
    fn a_stale_replayed_pairing_message_is_ignored() {
        let _g = call_arbitration::reset_state_for_test();
        let me = crate::start_attempt_with("stale-test-pairing", "test-private-key", "stale-test-own-pubkey", "Me", "stale test phrase");
        let peer = crate::start_attempt_with("stale-test-peer", "test-private-key", "stale-test-peer-pubkey", "Peer", "stale test phrase");
        let sender = nostr_protocol::generate_keys();
        let sender_secret = sender.secret_key().to_secret_hex();
        let event = nostr_protocol::build_bootstrap_event(&sender_secret, &me.rendezvous_tag, None, &format!(r#"{{"type":"pake1","outbound":"{}"}}"#, peer.outbound_hex)).unwrap();
        let ctx = RouteContext { pending: vec![RoutePending { pairing_id: "stale-test-pairing".into(), rendezvous_tag: me.rendezvous_tag.clone() }], ..Default::default() };
        // Seen ten minutes after it was made: a replay, nothing happens.
        let now = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64) + 10 * 60 * 1000;
        assert_eq!(route_event(&event, &ctx, now), RouteResult::default());
        assert!(candidate_of("stale-test-pairing").is_none());
        // The same event when it is fresh does register the sender.
        // (A different sender: the very same event would be dropped as a duplicate of the stale one above.)
        let sender2_secret = nostr_protocol::generate_keys().secret_key().to_secret_hex();
        let event2 = nostr_protocol::build_bootstrap_event(&sender2_secret, &me.rendezvous_tag, None, &format!(r#"{{"type":"pake1","outbound":"{}"}}"#, peer.outbound_hex)).unwrap();
        let fresh = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64) + 1000;
        route_event(&event2, &ctx, fresh);
        assert!(candidate_of("stale-test-pairing").is_some());
        crate::cancel_attempt("stale-test-pairing");
        crate::cancel_attempt("stale-test-peer");
    }

    #[test]
    fn bootstrap_events_only_route_to_a_pending_pairing_with_their_tag() {
        let _g = call_arbitration::reset_state_for_test();
        let keys = nostr_protocol::generate_keys();
        let event = nostr_protocol::build_bootstrap_event(&keys.secret_key().to_secret_hex(), "tag-a", None, r#"{"type":"pake1","msg":"00"}"#).unwrap();
        let none = RouteContext::default();
        assert_eq!(route_event(&event, &none, 0), RouteResult::default());
        // Matching tag but no live attempt in the core's registry: nothing to do,
        // and nothing breaks.
        let ctx = RouteContext { pending: vec![RoutePending { pairing_id: "no-such-attempt".into(), rendezvous_tag: "tag-a".into() }], ..Default::default() };
        let other = nostr_protocol::build_bootstrap_event(&keys.secret_key().to_secret_hex(), "tag-a", None, r#"{"type":"pake1","msg":"01"}"#).unwrap();
        assert_eq!(route_event(&other, &ctx, 0), RouteResult::default());
    }
}
