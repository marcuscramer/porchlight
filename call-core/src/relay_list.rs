//! The relay list, as data: which relays the app talks to, as a version number and a list that can be replaced at
//! runtime without a new release.
//!
//! `web-app/relays.json` is served next to the web page; each client has a copy built in and fetches the file now
//! and then. [`apply`] takes the file's text and replaces the active list only if its version is higher, so a
//! stale CDN copy, an old file or a malformed one changes nothing. Fetching itself (and its schedule) stays the
//! shell's business. This module owns what the file may contain and what happens when it changes:
//!
//! - **Validation**: `wss://` URLs only, no whitespace, at most [`MAX_RELAYS`] after de-duplication, a positive
//!   version.
//! - **Make before break**: a relay a new list drops stays in [`current`]'s `extra` for [`GRACE_MS`], so a device
//!   that has not fetched yet can still hear (and be heard by) one that has. Senders and listeners use
//!   `relays` plus `extra`.
//! - **Heartbeats carry the list version** (see [`crate::nostr_protocol::SignalMessage::Heartbeat`]); a contact
//!   showing a higher one than ours says "fetch now" ([`note_peer_version`], rate-limited).

use serde::{Deserialize, Serialize};

pub const MAX_RELAYS: usize = 12;
/// How long a relay that a newer list dropped is still used.
pub const GRACE_MS: i64 = 48 * 60 * 60 * 1000;
/// At most one "a contact has a newer list" fetch hint per this long.
pub const PEER_FETCH_COOLDOWN_MS: i64 = 10 * 60 * 1000;

#[derive(Deserialize)]
struct ListFile {
    version: u32,
    relays: Vec<String>,
}

pub(crate) struct RelayListState {
    version: u32,
    relays: Vec<String>,
    /// Relays dropped by a newer list and the time their grace ends.
    grace: Vec<(String, i64)>,
    last_peer_fetch_hint_ms: Option<i64>,
}

impl RelayListState {
    pub(crate) fn new() -> Self {
        RelayListState { version: 0, relays: Vec::new(), grace: Vec::new(), last_peer_fetch_hint_ms: None }
    }
}

/// What a list change did, for the shell to act on.
#[derive(Serialize, Debug, PartialEq)]
#[serde(tag = "kind")]
pub enum ApplyOutcome {
    /// The list was replaced: connect to `added`, and expect `removed` to stay in use for the grace period.
    Applied { version: u32, relays: Vec<String>, added: Vec<String>, removed: Vec<String> },
    /// Nothing to do: the version is not higher than the active one.
    Unchanged { version: u32 },
    /// The text is not an acceptable list; the active list stays.
    Rejected { reason: String },
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Current {
    pub version: u32,
    /// The active list.
    pub relays: Vec<String>,
    /// Dropped by the latest list but still in their grace period.
    pub extra: Vec<String>,
}

/// One URL in its canonical form (lower-case scheme and host, no trailing slash), or `None` if it is not an
/// acceptable relay address.
pub fn normalize(url: &str) -> Option<String> {
    let rest = url.strip_prefix("wss://").or_else(|| url.strip_prefix("WSS://"))?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) => (h, Some(p)),
        _ => (authority, None),
    };
    let host_ok = !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| !label.is_empty() && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
        && host.contains('.');
    let path_ok = path.bytes().all(|b| b.is_ascii_graphic());
    if !host_ok || !path_ok {
        return None;
    }
    let mut out = format!("wss://{}", host.to_ascii_lowercase());
    if let Some(p) = port {
        out.push(':');
        out.push_str(p);
    }
    out.push_str(path.trim_end_matches('/'));
    Some(out)
}

fn normalize_all(urls: &[String]) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for url in urls {
        let n = normalize(url.trim()).ok_or_else(|| format!("not a wss:// relay address: {url:?}"))?;
        if !out.contains(&n) {
            out.push(n);
        }
    }
    if out.is_empty() {
        return Err("the list is empty".to_string());
    }
    if out.len() > MAX_RELAYS {
        return Err(format!("more than {MAX_RELAYS} relays"));
    }
    Ok(out)
}

/// Sets the list the app was built with. Does nothing if a list is already active, so a service that restarts
/// within one process cannot undo a fetched list.
pub fn init(version: u32, relays: &[String]) {
    let mut app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    if app.relay_list.version != 0 {
        return;
    }
    if let Ok(list) = normalize_all(relays) {
        app.relay_list.version = version.max(1);
        app.relay_list.relays = list;
    }
}

/// Replaces the active list with the one in `json` (`{"version":N,"relays":[...]}`) if its version is higher.
pub fn apply(json: &str, now_ms: i64) -> ApplyOutcome {
    let file: ListFile = match serde_json::from_str(json) {
        Ok(f) => f,
        Err(e) => return ApplyOutcome::Rejected { reason: format!("not a relay list: {e}") },
    };
    if file.version == 0 {
        return ApplyOutcome::Rejected { reason: "version must be positive".to_string() };
    }
    let new = match normalize_all(&file.relays) {
        Ok(l) => l,
        Err(reason) => return ApplyOutcome::Rejected { reason },
    };
    let mut app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    let state = &mut app.relay_list;
    if file.version <= state.version {
        return ApplyOutcome::Unchanged { version: state.version };
    }
    let added: Vec<String> = new.iter().filter(|r| !state.relays.contains(r)).cloned().collect();
    let removed: Vec<String> = state.relays.iter().filter(|r| !new.contains(r)).cloned().collect();
    state.grace.retain(|(r, until)| *until > now_ms && !new.contains(r));
    for r in &removed {
        if !state.grace.iter().any(|(g, _)| g == r) {
            state.grace.push((r.clone(), now_ms + GRACE_MS));
        }
    }
    state.version = file.version;
    state.relays = new.clone();
    ApplyOutcome::Applied { version: file.version, relays: new, added, removed }
}

/// The active list and the relays still in their grace period.
pub fn current(now_ms: i64) -> Current {
    let mut app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    let state = &mut app.relay_list;
    state.grace.retain(|(_, until)| *until > now_ms);
    Current { version: state.version, relays: state.relays.clone(), extra: state.grace.iter().map(|(r, _)| r.clone()).collect() }
}

/// The version of the active list, 0 before [`init`].
pub fn version() -> u32 {
    crate::STATE.lock().unwrap_or_else(|p| p.into_inner()).relay_list.version
}

/// A contact's heartbeat reported list version `peer_version`: true if it is higher than ours and no such hint was
/// given in the last [`PEER_FETCH_COOLDOWN_MS`], i.e. the shell should fetch the list now.
pub fn note_peer_version(peer_version: u32, now_ms: i64) -> bool {
    let mut app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    let state = &mut app.relay_list;
    if peer_version <= state.version {
        return false;
    }
    if state.last_peer_fetch_hint_ms.is_some_and(|t| now_ms - t < PEER_FETCH_COOLDOWN_MS) {
        return false;
    }
    state.last_peer_fetch_hint_ms = Some(now_ms);
    true
}

/// The list state is process-global, so tests that set it run one at a time.
#[cfg(test)]
pub(crate) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) fn reset_for_test() {
    crate::STATE.lock().unwrap_or_else(|p| p.into_inner()).relay_list = RelayListState::new();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(version: u32, relays: &[&str]) -> String {
        serde_json::json!({ "version": version, "relays": relays }).to_string()
    }
    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }
    fn fresh(builtin: &[&str]) -> std::sync::MutexGuard<'static, ()> {
        let guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_for_test();
        init(1, &strings(builtin));
        guard
    }

    #[test]
    fn addresses_are_normalized_and_bad_ones_refused() {
        assert_eq!(normalize("wss://Relay.Example.com/").as_deref(), Some("wss://relay.example.com"));
        assert_eq!(normalize("wss://relay.example.com:8443").as_deref(), Some("wss://relay.example.com:8443"));
        assert_eq!(normalize("wss://relay.example.com/v1/").as_deref(), Some("wss://relay.example.com/v1"));
        for bad in ["ws://relay.example.com", "https://relay.example.com", "wss://", "wss://localhost", "wss://a b.example.com", "relay.example.com", "wss://exa mple.com", "wss://.example.com", "wss://example..com"] {
            assert_eq!(normalize(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_built_in_list_is_active_until_a_higher_version_arrives() {
        let _g = fresh(&["wss://a.example.com", "wss://b.example.com"]);
        let c = current(0);
        assert_eq!((c.version, c.relays.len(), c.extra.len()), (1, 2, 0));
        // init does not undo a later state.
        init(1, &strings(&["wss://z.example.com"]));
        assert_eq!(current(0).relays, strings(&["wss://a.example.com", "wss://b.example.com"]));
    }

    #[test]
    fn a_higher_version_replaces_the_list_and_keeps_dropped_relays_for_the_grace_period() {
        let _g = fresh(&["wss://a.example.com", "wss://b.example.com", "wss://c.example.com"]);
        let out = apply(&list(2, &["wss://b.example.com", "wss://c.example.com", "wss://d.example.com"]), 1_000);
        assert_eq!(
            out,
            ApplyOutcome::Applied {
                version: 2,
                relays: strings(&["wss://b.example.com", "wss://c.example.com", "wss://d.example.com"]),
                added: strings(&["wss://d.example.com"]),
                removed: strings(&["wss://a.example.com"]),
            }
        );
        let c = current(2_000);
        assert_eq!(c.version, 2);
        assert_eq!(c.extra, strings(&["wss://a.example.com"]), "the dropped relay is still used");
        assert!(current(1_000 + GRACE_MS).extra.is_empty(), "and then it is not");
    }

    #[test]
    fn an_older_equal_or_unacceptable_list_changes_nothing() {
        let _g = fresh(&["wss://a.example.com"]);
        assert_eq!(apply(&list(1, &["wss://x.example.com"]), 0), ApplyOutcome::Unchanged { version: 1 });
        assert_eq!(apply(&list(0, &["wss://x.example.com"]), 0), ApplyOutcome::Rejected { reason: "version must be positive".into() });
        for bad in [
            "not json".to_string(),
            list(5, &[]),
            list(5, &["ws://plain.example.com"]),
            list(5, &["wss://ok.example.com", "https://nope.example.com"]),
            list(5, &(0..13).map(|i| format!("wss://r{i}.example.com")).collect::<Vec<_>>().iter().map(String::as_str).collect::<Vec<_>>()),
            r#"{"relays":["wss://x.example.com"]}"#.to_string(),
        ] {
            assert!(matches!(apply(&bad, 0), ApplyOutcome::Rejected { .. }), "{bad}");
        }
        assert_eq!(current(0).version, 1);
        assert_eq!(current(0).relays, strings(&["wss://a.example.com"]));
    }

    #[test]
    fn duplicates_collapse_and_a_relay_that_comes_back_leaves_the_grace_list() {
        let _g = fresh(&["wss://a.example.com", "wss://b.example.com"]);
        apply(&list(2, &["wss://b.example.com", "wss://B.example.com/", "wss://c.example.com"]), 0);
        assert_eq!(current(1).relays, strings(&["wss://b.example.com", "wss://c.example.com"]));
        assert_eq!(current(1).extra, strings(&["wss://a.example.com"]));
        apply(&list(3, &["wss://a.example.com", "wss://c.example.com"]), 10);
        let c = current(11);
        assert_eq!(c.relays, strings(&["wss://a.example.com", "wss://c.example.com"]));
        assert_eq!(c.extra, strings(&["wss://b.example.com"]), "a is active again, b is the one dropped now");
    }

    #[test]
    fn a_contact_with_a_newer_list_triggers_one_fetch_hint_per_cooldown() {
        let _g = fresh(&["wss://a.example.com"]);
        assert!(!note_peer_version(1, 0), "same version");
        assert!(note_peer_version(2, 1_000));
        assert!(!note_peer_version(2, 1_001), "rate-limited");
        assert!(!note_peer_version(3, 1_000 + PEER_FETCH_COOLDOWN_MS - 1));
        assert!(note_peer_version(3, 1_000 + PEER_FETCH_COOLDOWN_MS));
        apply(&list(3, &["wss://a.example.com"]), 0);
        assert!(!note_peer_version(3, 10_000_000), "no longer newer once we have it");
    }
}
