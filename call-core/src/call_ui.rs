//! What the call screens show, decided once for both shells.
//!
//! Each shell used to work out for itself which screen a call is on (ringing
//! vs calling vs connecting vs live, and which of them an old outcome message
//! hides) and which text an ended call gets. They drifted: the web page never
//! showed "Connecting" after Accept, and the ICE refinement of the "never
//! connected" text was written twice. Shells now report what they know and
//! render what comes back.

use crate::call_arbitration::CallOutcomeReason;
use crate::ice_evidence::IceDiagnosis;
use serde::{Deserialize, Serialize};

/// What the shell knows about the call right now.
#[derive(Deserialize, Debug, Clone, Copy, Default)]
pub struct PhaseInput {
    /// A call slot is taken (ringing, calling, connecting or live).
    #[serde(default)]
    pub has_active_call: bool,
    /// A finished call's outcome message is waiting to be dismissed.
    #[serde(default)]
    pub has_outcome: bool,
    /// An incoming call is ringing (the offer has arrived, not yet accepted).
    #[serde(default)]
    pub has_ring: bool,
    /// The person (or auto-answer) accepted the incoming call.
    #[serde(default)]
    pub accepted_incoming: bool,
    /// The media connection is up.
    #[serde(default)]
    pub peer_connected: bool,
}

#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// No call: the contact list (or whatever else the shell is showing).
    Idle,
    /// A finished call's message; wins over everything, so a stale one can
    /// never hide a call that is ringing — it is cleared when a new call
    /// claims the slot, and until then it is the only thing on screen.
    Outcome,
    Ringing,
    /// Outgoing call, waiting for the other side.
    Calling,
    /// Incoming call accepted, media not up yet.
    Connecting,
    Live,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct PhaseView {
    pub phase: Phase,
    /// Accept is offered (only while ringing).
    pub show_accept: bool,
    /// The control row can't be dismissed (until the call is live).
    pub controls_pinned: bool,
    /// String key of the centred message, `None` for the live and idle
    /// phases (the outcome has its own text, see [`outcome_text`]).
    pub label_key: Option<&'static str>,
}

pub fn phase(input: &PhaseInput) -> PhaseView {
    let phase = if input.has_outcome {
        Phase::Outcome
    } else if !input.has_active_call {
        Phase::Idle
    } else if input.peer_connected {
        Phase::Live
    } else if input.has_ring && !input.accepted_incoming {
        Phase::Ringing
    } else if input.accepted_incoming {
        Phase::Connecting
    } else {
        Phase::Calling
    };
    let (label_key, pinned) = match phase {
        Phase::Ringing => (Some("call.incomingCallFrom"), true),
        Phase::Calling => (Some("call.callingLabel"), true),
        Phase::Connecting => (Some("call.connectingLabel"), true),
        Phase::Idle | Phase::Outcome | Phase::Live => (None, false),
    };
    PhaseView { phase, show_accept: phase == Phase::Ringing, controls_pinned: pinned, label_key }
}

/// Which text an ended call gets. The shells map each case to its title and
/// message strings (an exhaustive match, so a new case can't be forgotten).
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeText {
    PeerEnded,
    NeverConnected,
    /// Never connected, and the ICE evidence says UDP is blocked.
    UdpBlocked,
    /// Never connected, and both sides tried real addresses but found no
    /// route between them.
    NoDirectPath,
    Dropped,
}

pub fn outcome_text(reason: &CallOutcomeReason, diagnosis: Option<IceDiagnosis>) -> OutcomeText {
    match (reason, diagnosis) {
        (CallOutcomeReason::PeerEnded, _) => OutcomeText::PeerEnded,
        (CallOutcomeReason::Dropped, _) => OutcomeText::Dropped,
        (CallOutcomeReason::NeverConnected, Some(IceDiagnosis::UdpBlocked)) => OutcomeText::UdpBlocked,
        (CallOutcomeReason::NeverConnected, Some(IceDiagnosis::NoDirectPath)) => OutcomeText::NoDirectPath,
        (CallOutcomeReason::NeverConnected, None) => OutcomeText::NeverConnected,
    }
}

/// [`outcome_text`] across the FFI: the names come in as `snake_case`
/// strings (`diagnosis` empty for none), the answer goes out as the case's
/// `snake_case` name (bare, not JSON). `None` for an unknown name.
pub fn outcome_text_from_names(reason: &str, diagnosis: &str) -> Option<String> {
    let reason: CallOutcomeReason = serde_json::from_value(serde_json::Value::String(reason.to_string())).ok()?;
    let diagnosis: Option<IceDiagnosis> = if diagnosis.is_empty() { None } else { Some(serde_json::from_value(serde_json::Value::String(diagnosis.to_string())).ok()?) };
    serde_json::to_value(outcome_text(&reason, diagnosis)).ok()?.as_str().map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(active: bool, outcome: bool, ring: bool, accepted: bool, connected: bool) -> PhaseInput {
        PhaseInput { has_active_call: active, has_outcome: outcome, has_ring: ring, accepted_incoming: accepted, peer_connected: connected }
    }

    #[test]
    fn nothing_going_on_is_idle() {
        let v = phase(&input(false, false, false, false, false));
        assert_eq!(v, PhaseView { phase: Phase::Idle, show_accept: false, controls_pinned: false, label_key: None });
    }

    #[test]
    fn an_outcome_wins_over_everything() {
        for (active, ring, accepted, connected) in [(false, false, false, false), (true, true, false, false), (true, false, false, true)] {
            assert_eq!(phase(&input(active, true, ring, accepted, connected)).phase, Phase::Outcome);
        }
    }

    #[test]
    fn a_ringing_call_offers_accept_and_pins_the_controls() {
        let v = phase(&input(true, false, true, false, false));
        assert_eq!(v.phase, Phase::Ringing);
        assert!(v.show_accept && v.controls_pinned);
        assert_eq!(v.label_key, Some("call.incomingCallFrom"));
    }

    #[test]
    fn accepting_moves_from_ringing_to_connecting_and_drops_accept() {
        let v = phase(&input(true, false, true, true, false));
        assert_eq!(v.phase, Phase::Connecting);
        assert!(!v.show_accept && v.controls_pinned);
        assert_eq!(v.label_key, Some("call.connectingLabel"));
        // Also when the ring itself is already cleared.
        assert_eq!(phase(&input(true, false, false, true, false)).phase, Phase::Connecting);
    }

    #[test]
    fn an_outgoing_call_is_calling() {
        let v = phase(&input(true, false, false, false, false));
        assert_eq!(v.phase, Phase::Calling);
        assert_eq!(v.label_key, Some("call.callingLabel"));
    }

    #[test]
    fn connected_is_live_with_dismissable_controls_whatever_else_is_set() {
        for (ring, accepted) in [(false, false), (true, false), (true, true), (false, true)] {
            let v = phase(&input(true, false, ring, accepted, true));
            assert_eq!(v, PhaseView { phase: Phase::Live, show_accept: false, controls_pinned: false, label_key: None });
        }
    }

    #[test]
    fn only_a_call_that_never_connected_is_refined_by_the_ice_evidence() {
        use CallOutcomeReason::*;
        assert_eq!(outcome_text(&NeverConnected, None), OutcomeText::NeverConnected);
        assert_eq!(outcome_text(&NeverConnected, Some(IceDiagnosis::UdpBlocked)), OutcomeText::UdpBlocked);
        assert_eq!(outcome_text(&NeverConnected, Some(IceDiagnosis::NoDirectPath)), OutcomeText::NoDirectPath);
        assert_eq!(outcome_text(&PeerEnded, Some(IceDiagnosis::UdpBlocked)), OutcomeText::PeerEnded);
        assert_eq!(outcome_text(&Dropped, Some(IceDiagnosis::NoDirectPath)), OutcomeText::Dropped);
    }

    #[test]
    fn names_cross_the_ffi_as_plain_strings() {
        assert_eq!(outcome_text_from_names("never_connected", "udp_blocked").as_deref(), Some("udp_blocked"));
        assert_eq!(outcome_text_from_names("never_connected", "").as_deref(), Some("never_connected"));
        assert_eq!(outcome_text_from_names("peer_ended", "").as_deref(), Some("peer_ended"));
        assert_eq!(outcome_text_from_names("bogus", ""), None);
        assert_eq!(outcome_text_from_names("dropped", "bogus"), None);
    }

    #[test]
    fn serializes_to_the_names_the_shells_match_on() {
        let v = phase(&input(true, false, true, false, false));
        assert_eq!(serde_json::to_string(&v).unwrap(), r#"{"phase":"ringing","show_accept":true,"controls_pinned":true,"label_key":"call.incomingCallFrom"}"#);
        assert_eq!(serde_json::to_string(&OutcomeText::UdpBlocked).unwrap(), r#""udp_blocked""#);
    }
}
