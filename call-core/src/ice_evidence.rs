//! "Why did this call never connect?" — the ICE-candidate evidence collected
//! while a `PeerConnection` negotiates, and the diagnosis drawn from it
//! (`WebRtcEngine.kt`'s `candidateType`/`diagnose`/`iceDiagnosis` and
//! `app.js`'s `iceCandidateType`/`diagnoseIce`/`iceEvidence`, which were two
//! independently hand-written copies of the same decision).
//!
//! **What this module doesn't own**: the `PeerConnection` itself and its
//! connection state — the same category [`crate::nostr_protocol`]'s own doc
//! draws around the live relay connection. The shell reports each candidate
//! line it sees, tells this module when a connection was ever established,
//! and passes the current ICE connection state's name when it asks. Only
//! candidate *types* (`host`/`srflx`/`relay`/`prflx`) are kept, never an
//! address.
//!
//! The question being answered is a best-effort "would a TURN relay have
//! saved this call?". It only means something once both sides exchanged
//! candidates and ICE still never connected: no server-reflexive (or relay)
//! candidate of our own means this network blocks UDP/STUN
//! ([`IceDiagnosis::UdpBlocked`]); otherwise both sides tried real addresses
//! and no direct path worked — the case TURN exists for
//! ([`IceDiagnosis::NoDirectPath`]). `None` means "can't tell" (never got
//! that far, so not a NAT problem as far as anything here can see).
//!
//! One call at a time device-wide (see [`crate::call_arbitration`]), so one
//! evidence set; [`reset`] starts a fresh one with each new `PeerConnection`.

use serde::Serialize;
use std::collections::HashSet;

/// Why a call that never connected most likely failed at the network level.
/// Serialized as its `snake_case` name, which both shells compare/map from.
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IceDiagnosis {
    NoDirectPath,
    UdpBlocked,
}

pub(crate) struct IceState {
    local_types: HashSet<String>,
    remote_types: HashSet<String>,
    ever_connected: bool,
    /// What [`remember_diagnosis`] last concluded, kept so the outcome screen
    /// can still ask after the `PeerConnection` itself is gone.
    last: Option<IceDiagnosis>,
}

impl IceState {
    pub(crate) fn new() -> Self {
        IceState { local_types: HashSet::new(), remote_types: HashSet::new(), ever_connected: false, last: None }
    }
}

/// The candidate type of an ICE candidate line (`... typ srflx raddr ...`) —
/// `declared_type` (what the platform's own candidate object says, when it
/// says anything) wins over parsing the line.
fn candidate_type(candidate_line: &str, declared_type: Option<&str>) -> Option<String> {
    if let Some(declared) = declared_type.filter(|t| !t.is_empty()) {
        return Some(declared.to_string());
    }
    let mut words = candidate_line.split_whitespace();
    while let Some(word) = words.next() {
        if word == "typ" {
            return words.next().filter(|t| t.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')).map(str::to_string);
        }
    }
    None
}

/// Starts a fresh evidence set (and forgets the last diagnosis) — call when a
/// new `PeerConnection` is created.
pub fn reset() {
    crate::STATE.lock().unwrap_or_else(|p| p.into_inner()).ice = IceState::new();
}

/// One of this device's own candidates was gathered.
pub fn note_local_candidate(candidate_line: &str, declared_type: Option<&str>) {
    if let Some(t) = candidate_type(candidate_line, declared_type) {
        crate::STATE.lock().unwrap_or_else(|p| p.into_inner()).ice.local_types.insert(t);
    }
}

/// A candidate arrived from the peer. Only the line is available here, never
/// a platform-declared type.
pub fn note_remote_candidate(candidate_line: &str) {
    if let Some(t) = candidate_type(candidate_line, None) {
        crate::STATE.lock().unwrap_or_else(|p| p.into_inner()).ice.remote_types.insert(t);
    }
}

/// The connection reached `connected` at least once — from then on there is
/// no "never connected" to explain.
pub fn note_connected() {
    crate::STATE.lock().unwrap_or_else(|p| p.into_inner()).ice.ever_connected = true;
}

/// The diagnosis right now, given the connection's current ICE state
/// (`checking`/`failed`/`disconnected`, case-insensitive, are the ones that
/// can still be "never connected"; anything else — including `connected` —
/// yields `None`) and remembers it as the last one. Call as the
/// `PeerConnection` is torn down, before it goes away.
pub fn remember_diagnosis(ice_connection_state: &str) {
    let mut app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    let state = &mut app.ice;
    state.last = diagnose(state, ice_connection_state);
}

/// What [`remember_diagnosis`] last concluded for the current evidence set.
pub fn last_diagnosis() -> Option<IceDiagnosis> {
    crate::STATE.lock().unwrap_or_else(|p| p.into_inner()).ice.last
}

fn diagnose(state: &IceState, ice_connection_state: &str) -> Option<IceDiagnosis> {
    if state.ever_connected || state.remote_types.is_empty() {
        return None;
    }
    if !matches!(ice_connection_state.to_ascii_lowercase().as_str(), "checking" | "failed" | "disconnected") {
        return None;
    }
    if !state.local_types.contains("srflx") && !state.local_types.contains("relay") {
        return Some(IceDiagnosis::UdpBlocked);
    }
    Some(IceDiagnosis::NoDirectPath)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Same serialization pattern as `signal_retry`'s tests: every test
    /// touches the one shared `crate::STATE.ice`.
    static TEST_SERIAL: Mutex<()> = Mutex::new(());

    fn fresh() -> std::sync::MutexGuard<'static, ()> {
        let guard = TEST_SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        reset();
        guard
    }

    const HOST: &str = "candidate:1 1 udp 2122260223 192.168.1.5 50000 typ host generation 0";
    const SRFLX: &str = "candidate:2 1 udp 1686052607 203.0.113.9 50000 typ srflx raddr 192.168.1.5 rport 50000";

    #[test]
    fn candidate_type_prefers_the_declared_type_then_parses_the_line() {
        assert_eq!(candidate_type(HOST, Some("srflx")).as_deref(), Some("srflx"));
        assert_eq!(candidate_type(HOST, Some("")).as_deref(), Some("host"));
        assert_eq!(candidate_type(SRFLX, None).as_deref(), Some("srflx"));
        assert_eq!(candidate_type("candidate:3 1 udp 1 10.0.0.1 9 typ relay raddr 0.0.0.0 rport 0", None).as_deref(), Some("relay"));
        assert_eq!(candidate_type("no type here", None), None);
        assert_eq!(candidate_type("trailing typ", None), None);
        assert_eq!(candidate_type("x typ ../etc", None), None, "not a plausible type word");
    }

    #[test]
    fn nothing_to_diagnose_without_remote_candidates() {
        let _g = fresh();
        note_local_candidate(HOST, None);
        remember_diagnosis("failed");
        assert_eq!(last_diagnosis(), None);
    }

    #[test]
    fn no_server_reflexive_candidate_of_our_own_means_udp_blocked() {
        let _g = fresh();
        note_local_candidate(HOST, None);
        note_remote_candidate(SRFLX);
        remember_diagnosis("failed");
        assert_eq!(last_diagnosis(), Some(IceDiagnosis::UdpBlocked));
    }

    #[test]
    fn a_server_reflexive_or_relay_candidate_of_our_own_means_no_direct_path() {
        let _g = fresh();
        note_local_candidate(HOST, None);
        note_local_candidate(SRFLX, None);
        note_remote_candidate(HOST);
        remember_diagnosis("checking");
        assert_eq!(last_diagnosis(), Some(IceDiagnosis::NoDirectPath));

        reset();
        note_local_candidate("x", Some("relay"));
        note_remote_candidate(HOST);
        remember_diagnosis("disconnected");
        assert_eq!(last_diagnosis(), Some(IceDiagnosis::NoDirectPath));
    }

    #[test]
    fn a_connection_that_ever_connected_is_never_blamed_on_the_network() {
        let _g = fresh();
        note_remote_candidate(SRFLX);
        note_connected();
        remember_diagnosis("failed");
        assert_eq!(last_diagnosis(), None);
    }

    #[test]
    fn only_checking_failed_or_disconnected_can_be_never_connected() {
        let _g = fresh();
        note_remote_candidate(SRFLX);
        for yes in ["checking", "FAILED", "Disconnected"] {
            remember_diagnosis(yes);
            assert_eq!(last_diagnosis(), Some(IceDiagnosis::UdpBlocked), "{yes}");
        }
        for no in ["new", "connected", "completed", "closed", ""] {
            remember_diagnosis(no);
            assert_eq!(last_diagnosis(), None, "{no}");
        }
    }

    #[test]
    fn reset_forgets_evidence_and_the_last_diagnosis() {
        let _g = fresh();
        note_remote_candidate(SRFLX);
        remember_diagnosis("failed");
        assert!(last_diagnosis().is_some());
        reset();
        assert_eq!(last_diagnosis(), None);
        remember_diagnosis("failed");
        assert_eq!(last_diagnosis(), None, "the remote evidence is gone too");
    }
}
