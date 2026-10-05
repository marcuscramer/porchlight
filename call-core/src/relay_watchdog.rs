//! When to give up on the relay connections and rebuild them.
//!
//! The Android shell used to force a full reconnect whenever fewer than *all* relays were connected for 45 s.
//! One relay that is simply down for days (a 502, a ban) made that fire every 50 s forever; each forced
//! reconnect disconnects the healthy relays too, and on a Portal that sat idle overnight the repeated
//! reconnects ended with the connection library wedged: the connect call became a silent no-op and the device
//! was deaf for hours (1 of 5 relays, 64 open sockets) until the app was restarted.
//!
//! Now the shell reports how many relays are connected and this decides:
//!
//! - enough relays connected (a majority) is healthy, however long one relay is down;
//! - otherwise, after [`DEGRADED_FOR_MS`], a soft reconnect, at most once per [`DEGRADED_FOR_MS`];
//! - if [`SOFT_ATTEMPTS`] soft reconnects did not bring the count back, tear everything down and build the
//!   connections from scratch.

use serde::Serialize;

/// How long the relays must stay under-connected before anything is done, and the pause between two actions.
pub const DEGRADED_FOR_MS: i64 = 45_000;
/// Soft reconnects tried before the connections are rebuilt from scratch.
pub const SOFT_ATTEMPTS: u32 = 2;

#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Nothing,
    /// Disconnect and connect again with the same connection objects.
    SoftReconnect,
    /// Throw the connection objects (and their sockets) away and build new ones.
    Rebuild,
}

#[derive(Debug, Default)]
pub struct WatchdogState {
    degraded_since_ms: Option<i64>,
    last_action_ms: Option<i64>,
    soft_attempts: u32,
}

impl WatchdogState {
    pub fn new() -> Self {
        Self::default()
    }

    /// A majority of the relays is enough for the signaling to work.
    fn healthy(connected: u32, total: u32) -> bool {
        total == 0 || connected * 2 > total
    }

    pub fn check(&mut self, connected: u32, total: u32, now_ms: i64) -> Action {
        if Self::healthy(connected, total) {
            *self = Self::default();
            return Action::Nothing;
        }
        let since = *self.degraded_since_ms.get_or_insert(now_ms);
        let waited_enough = now_ms - since >= DEGRADED_FOR_MS && self.last_action_ms.is_none_or(|t| now_ms - t >= DEGRADED_FOR_MS);
        if !waited_enough {
            return Action::Nothing;
        }
        self.last_action_ms = Some(now_ms);
        if self.soft_attempts < SOFT_ATTEMPTS {
            self.soft_attempts += 1;
            Action::SoftReconnect
        } else {
            self.soft_attempts = 0;
            Action::Rebuild
        }
    }
}

/// [`WatchdogState::check`] on the shared state.
pub fn check(connected: u32, total: u32, now_ms: i64) -> Action {
    let mut app = crate::STATE.lock().unwrap_or_else(|p| p.into_inner());
    app.relay_watchdog.check(connected, total, now_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_majority_of_relays_is_healthy_whatever_one_dead_relay_does() {
        let mut w = WatchdogState::new();
        for minute in 0..600 {
            assert_eq!(w.check(4, 5, minute * 60_000), Action::Nothing, "one relay down for hours must never trigger anything");
        }
        assert_eq!(w.check(3, 5, 0), Action::Nothing);
    }

    #[test]
    fn under_connected_for_long_enough_reconnects_softly_then_rebuilds() {
        let mut w = WatchdogState::new();
        assert_eq!(w.check(1, 5, 0), Action::Nothing, "the clock starts now");
        assert_eq!(w.check(1, 5, 44_000), Action::Nothing);
        assert_eq!(w.check(1, 5, 45_000), Action::SoftReconnect);
        assert_eq!(w.check(1, 5, 60_000), Action::Nothing, "one action per window");
        assert_eq!(w.check(1, 5, 90_000), Action::SoftReconnect);
        assert_eq!(w.check(1, 5, 135_000), Action::Rebuild, "two soft reconnects did not help");
        assert_eq!(w.check(1, 5, 180_000), Action::SoftReconnect, "and the cycle starts over");
    }

    #[test]
    fn recovering_clears_everything() {
        let mut w = WatchdogState::new();
        w.check(1, 5, 0);
        assert_eq!(w.check(1, 5, 50_000), Action::SoftReconnect);
        assert_eq!(w.check(5, 5, 60_000), Action::Nothing);
        // A fresh outage starts from scratch: the clock and the attempt count.
        assert_eq!(w.check(0, 5, 100_000), Action::Nothing);
        assert_eq!(w.check(0, 5, 145_000), Action::SoftReconnect);
        assert_eq!(w.check(0, 5, 190_000), Action::SoftReconnect);
        assert_eq!(w.check(0, 5, 235_000), Action::Rebuild);
    }

    #[test]
    fn no_relays_configured_is_never_a_problem() {
        let mut w = WatchdogState::new();
        assert_eq!(w.check(0, 0, 0), Action::Nothing);
        assert_eq!(w.check(0, 0, 10_000_000), Action::Nothing);
    }
}
