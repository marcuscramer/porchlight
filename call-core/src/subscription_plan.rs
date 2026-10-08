//! Which relay subscriptions this device should hold, decided once for both
//! shells.
//!
//! A subscription is never edited in place: whenever what it listens for
//! changes (a contact was added or removed, a pairing attempt started or
//! ended), the old subscription is closed and a new one opened under a fresh
//! id. Replacing a subscription's filters under the same id depends on how the
//! relay client library handles it, and with Android's it silently failed for
//! a second pairing attempt started soon after a cancelled one: relays then
//! delivered nothing for the attempt's tag, not even this device's own
//! messages, and the attempt waited for a peer that was already publishing.
//!
//! The shells report what they want to listen for (their contacts' own public
//! keys, the live pairing attempts' rendezvous tags, the relay list) and get
//! back what to close and what to open. A call that changes nothing returns an
//! empty plan, so repeated calls cost no relay traffic.
//!
//! # Invariants
//!
//! - **A subscription's id is never reused for different filters.** Test:
//!   `a_changed_set_gets_a_new_id_and_the_old_one_is_closed`.
//! - **An unchanged wish returns an empty plan**, however the values are
//!   ordered or repeated. Tests: `an_unchanged_wish_changes_nothing`,
//!   `the_order_and_repeats_of_the_values_do_not_matter`.
//! - **A changed relay list replaces everything** (the new relays have none of
//!   the old subscriptions). Test: `a_changed_relay_list_replaces_both`.
//! - **A refresh replaces everything** (a relay ended a subscription, so it is opened again). Test:
//!   `a_refresh_replaces_both`.
//! - **A fresh client (`reset`) opens everything and closes nothing**: the old
//!   subscriptions went with the old client. Test:
//!   `a_reset_opens_everything_and_closes_nothing`.

use crate::nostr_protocol::{build_relay_filters, FilterSpec};
use serde::Serialize;

/// A field of `crate::AppState`, like the other modules' state.
pub(crate) struct PlanState {
    wrap: Option<Held>,
    pairing: Option<Held>,
    relays: Vec<String>,
    next_seq: u64,
}

struct Held {
    id: String,
    values: Vec<String>,
}

impl PlanState {
    pub(crate) fn new() -> Self {
        PlanState { wrap: None, pairing: None, relays: Vec::new(), next_seq: 0 }
    }
}

/// One subscription to open on every relay.
#[derive(Serialize, Debug, PartialEq)]
pub struct Subscription {
    pub id: String,
    pub kind: u16,
    pub tag_name: char,
    pub tag_values: Vec<String>,
}

#[derive(Serialize, Debug, PartialEq, Default)]
pub struct SubscriptionPlan {
    pub subscribe: Vec<Subscription>,
    pub close: Vec<String>,
}

fn sorted(mut values: Vec<String>) -> Vec<String> {
    values.sort();
    values.dedup();
    values
}

/// Brings the held subscriptions in line with what the shell wants and says
/// what to close and open to get there. `reset` is true when the shell just
/// built a fresh relay client (first connect, a rebuild after the watchdog,
/// resuming a paused page): nothing is held on it yet. `refresh` is true when a relay ended one of our
/// subscriptions: everything is closed and opened again under new ids.
pub fn plan(own_pubkeys: &[String], pending_tags: &[String], relays: &[String], reset: bool, refresh: bool) -> SubscriptionPlan {
    let mut app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    let state = &mut app.subscription_plan;
    let filters = build_relay_filters(&sorted(own_pubkeys.to_vec()), &sorted(pending_tags.to_vec()));
    let relays = sorted(relays.to_vec());
    let replace_all = refresh || relays != state.relays;
    state.relays = relays;
    if reset {
        state.wrap = None;
        state.pairing = None;
    }
    let mut plan = SubscriptionPlan::default();
    reconcile("wrap", filters.wrap_filter, &mut state.wrap, replace_all, &mut state.next_seq, &mut plan);
    reconcile("pair", filters.bootstrap_filter, &mut state.pairing, replace_all, &mut state.next_seq, &mut plan);
    plan
}

fn reconcile(name: &str, wanted: Option<FilterSpec>, held: &mut Option<Held>, replace: bool, next_seq: &mut u64, plan: &mut SubscriptionPlan) {
    let wanted_values = wanted.as_ref().map(|f| f.tag_values.clone());
    let held_values = held.as_ref().map(|h| h.values.clone());
    if !replace && wanted_values == held_values {
        return;
    }
    if let Some(old) = held.take() {
        plan.close.push(old.id);
    }
    if let Some(filter) = wanted {
        *next_seq += 1;
        let id = format!("porchlight-{name}-{next_seq}");
        *held = Some(Held { id: id.clone(), values: filter.tag_values.clone() });
        plan.subscribe.push(Subscription { id, kind: filter.kind, tag_name: filter.tag_name, tag_values: filter.tag_values });
    }
}

/// The plan state is process-global, so tests that use it run one at a time.
#[cfg(test)]
pub(crate) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) fn reset_for_test() {
    crate::STATE.lock().unwrap_or_else(|p| p.into_inner()).subscription_plan = PlanState::new();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn fresh() -> std::sync::MutexGuard<'static, ()> {
        let guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_for_test();
        guard
    }

    const RELAYS: &[&str] = &["wss://a", "wss://b"];

    #[test]
    fn the_first_wish_opens_both_subscriptions() {
        let _g = fresh();
        let p = plan(&strings(&["k1", "k2"]), &strings(&["t1"]), &strings(RELAYS), true, false);
        assert!(p.close.is_empty());
        assert_eq!(p.subscribe.len(), 2);
        assert_eq!(p.subscribe[0].tag_values, strings(&["k1", "k2"]));
        assert_eq!(p.subscribe[1].tag_values, strings(&["t1"]));
        assert_ne!(p.subscribe[0].id, p.subscribe[1].id);
    }

    #[test]
    fn an_empty_wish_opens_nothing() {
        let _g = fresh();
        let p = plan(&[], &[], &strings(RELAYS), true, false);
        assert_eq!(p, SubscriptionPlan::default());
    }

    #[test]
    fn an_unchanged_wish_changes_nothing() {
        let _g = fresh();
        plan(&strings(&["k1"]), &strings(&["t1"]), &strings(RELAYS), true, false);
        let p = plan(&strings(&["k1"]), &strings(&["t1"]), &strings(RELAYS), false, false);
        assert_eq!(p, SubscriptionPlan::default());
    }

    #[test]
    fn the_order_and_repeats_of_the_values_do_not_matter() {
        let _g = fresh();
        plan(&strings(&["k1", "k2"]), &strings(&["t1", "t2"]), &strings(&["wss://a", "wss://b"]), true, false);
        let p = plan(&strings(&["k2", "k1", "k2"]), &strings(&["t2", "t1"]), &strings(&["wss://b", "wss://a", "wss://a"]), false, false);
        assert_eq!(p, SubscriptionPlan::default());
    }

    #[test]
    fn a_changed_set_gets_a_new_id_and_the_old_one_is_closed() {
        let _g = fresh();
        let first = plan(&strings(&["k1"]), &strings(&["t1"]), &strings(RELAYS), true, false);
        let pairing_id = first.subscribe[1].id.clone();
        let wrap_id = first.subscribe[0].id.clone();
        // A second attempt replaces the first: only the pairing subscription changes.
        let second = plan(&strings(&["k1"]), &strings(&["t2"]), &strings(RELAYS), false, false);
        assert_eq!(second.close, vec![pairing_id.clone()]);
        assert_eq!(second.subscribe.len(), 1);
        assert_eq!(second.subscribe[0].tag_values, strings(&["t2"]));
        assert_ne!(second.subscribe[0].id, pairing_id);
        assert_ne!(second.subscribe[0].id, wrap_id);
    }

    #[test]
    fn a_new_contact_replaces_the_wrap_subscription_only() {
        let _g = fresh();
        let first = plan(&strings(&["k1"]), &strings(&["t1"]), &strings(RELAYS), true, false);
        let second = plan(&strings(&["k1", "k2"]), &strings(&["t1"]), &strings(RELAYS), false, false);
        assert_eq!(second.close, vec![first.subscribe[0].id.clone()]);
        assert_eq!(second.subscribe.len(), 1);
        assert_eq!(second.subscribe[0].tag_values, strings(&["k1", "k2"]));
    }

    #[test]
    fn the_end_of_every_attempt_closes_the_pairing_subscription() {
        let _g = fresh();
        let first = plan(&strings(&["k1"]), &strings(&["t1"]), &strings(RELAYS), true, false);
        let second = plan(&strings(&["k1"]), &[], &strings(RELAYS), false, false);
        assert_eq!(second.close, vec![first.subscribe[1].id.clone()]);
        assert!(second.subscribe.is_empty());
    }

    #[test]
    fn a_changed_relay_list_replaces_both() {
        let _g = fresh();
        let first = plan(&strings(&["k1"]), &strings(&["t1"]), &strings(RELAYS), true, false);
        let second = plan(&strings(&["k1"]), &strings(&["t1"]), &strings(&["wss://a", "wss://c"]), false, false);
        assert_eq!(second.close.len(), 2);
        assert!(second.close.contains(&first.subscribe[0].id) && second.close.contains(&first.subscribe[1].id));
        assert_eq!(second.subscribe.len(), 2);
    }

    #[test]
    fn a_refresh_replaces_both() {
        let _g = fresh();
        let first = plan(&strings(&["k1"]), &strings(&["t1"]), &strings(RELAYS), true, false);
        let again = plan(&strings(&["k1"]), &strings(&["t1"]), &strings(RELAYS), false, true);
        assert_eq!(again.close.len(), 2);
        assert!(again.close.contains(&first.subscribe[0].id) && again.close.contains(&first.subscribe[1].id));
        assert_eq!(again.subscribe.len(), 2);
    }

    #[test]
    fn a_reset_opens_everything_and_closes_nothing() {
        let _g = fresh();
        plan(&strings(&["k1"]), &strings(&["t1"]), &strings(RELAYS), true, false);
        let again = plan(&strings(&["k1"]), &strings(&["t1"]), &strings(RELAYS), true, false);
        assert!(again.close.is_empty());
        assert_eq!(again.subscribe.len(), 2);
    }

    #[test]
    fn ids_are_never_reused() {
        let _g = fresh();
        let mut seen = std::collections::HashSet::new();
        for round in 0..6 {
            let tag = format!("t{round}");
            let p = plan(&strings(&["k1"]), &[tag], &strings(RELAYS), round == 0, false);
            for s in p.subscribe {
                assert!(seen.insert(s.id), "an id was reused");
            }
        }
    }
}
