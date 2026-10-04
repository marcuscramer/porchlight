//! Decision logic for getting the incoming-call screen in front of the
//! Portal's screensaver ("call wake-up"). Pure: it looks at what the shell
//! observed and says what to do next; every real action (pressing Home
//! through the accessibility service, bringing the app to the front, the
//! cover panel, the timers) stays in the Android shell.
//!
//! Why Home: after a wake from deep sleep the screensaver comes up on its own
//! and beats any activity the app starts for the whole ring. A Home press ends
//! it, and the system's handling of a short Home press also makes the Portal
//! send the HDMI-CEC messages that switch the TV to its input. Home does
//! nothing while the display is still off, so the first step is to wait for
//! the device to be interactive (give up on that after [`SCREEN_ON_TIMEOUT_MS`]
//! and fall back to a plain bring-to-front).
//!
//! After the first press the shell brings the app back to the front (Home
//! takes it off the front when it was already there), then asks again. A
//! second Home press on the system's own home screen starts the screensaver,
//! so Home is pressed again only when the screensaver is really back, at most
//! [`MAX_PRESSES`] times; otherwise the shell just asks to come to the front
//! again until [`GIVE_UP_MS`] has passed.
//!
//! The history of races in the Android code for this feature (a stale ring
//! showing a cover panel nothing removed, a cancel racing a step) was all in
//! this decision logic tangled with Android calls; keeping the decisions here
//! means they run under `cargo test` instead of only on a Portal.

use serde::Serialize;

/// How often to look again while the display is still off.
pub const POLL_MS: i64 = 100;
/// Longest to wait for the display to come on before giving up on Home.
pub const SCREEN_ON_TIMEOUT_MS: i64 = 3_000;
/// After a Home press, how long until the app asks to come back to the front.
pub const BRING_BACK_DELAY_MS: i64 = 150;
/// After a Home press, how long until the result is checked.
pub const VERIFY_DELAY_MS: i64 = 700;
/// Total time before giving up on getting the call screen in front.
pub const GIVE_UP_MS: i64 = 8_000;
/// How long after the app's own Home press a Home event is not "the person
/// left the app" (see the shell's `onUserLeaveHint`).
pub const OWN_PRESS_WINDOW_MS: i64 = 2_500;
/// Most Home presses for one ring.
pub const MAX_PRESSES: u32 = 3;
/// Hard stop for the cover panel regardless of anything else, so a bug here
/// can never leave the screen covered. Covers [`GIVE_UP_MS`] with margin.
pub const MASK_TIMEOUT_MS: i64 = 10_000;

/// The constants a shell needs besides what each step tells it.
#[derive(Debug, Serialize, PartialEq)]
pub struct WakeUpConstants {
    pub own_press_window_ms: i64,
    pub mask_timeout_ms: i64,
}

pub fn constants() -> WakeUpConstants {
    WakeUpConstants { own_press_window_ms: OWN_PRESS_WINDOW_MS, mask_timeout_ms: MASK_TIMEOUT_MS }
}

/// What to do when a call starts ringing.
#[derive(Debug, Serialize, PartialEq)]
#[serde(tag = "kind")]
pub enum StartDecision {
    /// Run the Home-press steps below.
    Escalate,
    /// Just ask for the app to come to the front.
    PlainBringToFront,
}

/// Call wake-up needs the accessibility service to be enabled *and* the
/// Settings switch to be on.
pub fn start(service_enabled: bool, switch_on: bool) -> StartDecision {
    if service_enabled && switch_on { StartDecision::Escalate } else { StartDecision::PlainBringToFront }
}

/// What the shell saw at this moment.
#[derive(Debug)]
pub struct Observation {
    /// Milliseconds since the ring started.
    pub elapsed_ms: i64,
    /// Home presses made so far for this ring.
    pub presses: u32,
    /// The display is on and the device is interactive.
    pub interactive: bool,
    /// The app's main screen is resumed (in front).
    pub ui_resumed: bool,
    /// The screensaver is running.
    pub dreaming: bool,
}

/// The next thing for the shell to do.
#[derive(Debug, Serialize, PartialEq)]
#[serde(tag = "kind")]
pub enum Step {
    /// The display is still off: look again after `recheck_after_ms`.
    WaitForScreen { recheck_after_ms: i64 },
    /// The display never came on: do a plain bring-to-front and stop.
    PlainBringToFront,
    /// Press Home (this is press number `press_number`). Then, after
    /// `bring_back_after_ms`, bring the app back to the front if this ring is
    /// still current, and after `verify_after_ms` ask for the next step with
    /// `presses = press_number`.
    PressHome { press_number: u32, bring_back_after_ms: i64, verify_after_ms: i64 },
    /// Not settled yet but no reason to press Home: bring the app to the
    /// front again, and ask for the next step after `recheck_after_ms`.
    BringBack { recheck_after_ms: i64 },
    /// The call screen is in front and the screensaver is gone: remove the
    /// cover panel and stop.
    Settled,
    /// Out of time: remove the cover panel and stop.
    GiveUp,
}

pub fn next_step(o: &Observation) -> Step {
    if !o.interactive {
        return if o.elapsed_ms >= SCREEN_ON_TIMEOUT_MS {
            Step::PlainBringToFront
        } else {
            Step::WaitForScreen { recheck_after_ms: POLL_MS }
        };
    }
    // Home goes first, once; then the app puts itself back in front.
    if o.presses == 0 {
        return press_home(1);
    }
    if o.ui_resumed && !o.dreaming {
        Step::Settled
    } else if o.dreaming && o.presses < MAX_PRESSES {
        press_home(o.presses + 1)
    } else if o.elapsed_ms < GIVE_UP_MS {
        Step::BringBack { recheck_after_ms: VERIFY_DELAY_MS }
    } else {
        Step::GiveUp
    }
}

fn press_home(press_number: u32) -> Step {
    Step::PressHome { press_number, bring_back_after_ms: BRING_BACK_DELAY_MS, verify_after_ms: VERIFY_DELAY_MS }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(elapsed_ms: i64, presses: u32, interactive: bool, ui_resumed: bool, dreaming: bool) -> Observation {
        Observation { elapsed_ms, presses, interactive, ui_resumed, dreaming }
    }

    #[test]
    fn needs_both_the_service_and_the_switch() {
        assert_eq!(start(true, true), StartDecision::Escalate);
        assert_eq!(start(true, false), StartDecision::PlainBringToFront);
        assert_eq!(start(false, true), StartDecision::PlainBringToFront);
        assert_eq!(start(false, false), StartDecision::PlainBringToFront);
    }

    #[test]
    fn waits_while_the_display_is_off() {
        assert_eq!(next_step(&obs(0, 0, false, false, false)), Step::WaitForScreen { recheck_after_ms: POLL_MS });
        assert_eq!(
            next_step(&obs(SCREEN_ON_TIMEOUT_MS - 1, 0, false, false, true)),
            Step::WaitForScreen { recheck_after_ms: POLL_MS }
        );
    }

    #[test]
    fn falls_back_when_the_display_never_comes_on() {
        assert_eq!(next_step(&obs(SCREEN_ON_TIMEOUT_MS, 0, false, false, false)), Step::PlainBringToFront);
        assert_eq!(next_step(&obs(SCREEN_ON_TIMEOUT_MS + 5_000, 0, false, false, true)), Step::PlainBringToFront);
    }

    #[test]
    fn presses_home_first_even_if_the_ui_already_looks_resumed() {
        // Home also switches the TV input, so the first press happens
        // whatever the app's own state looks like.
        let expected = Step::PressHome { press_number: 1, bring_back_after_ms: BRING_BACK_DELAY_MS, verify_after_ms: VERIFY_DELAY_MS };
        assert_eq!(next_step(&obs(0, 0, true, false, true)), expected);
        assert_eq!(next_step(&obs(0, 0, true, true, false)), expected);
    }

    #[test]
    fn settles_once_in_front_without_the_screensaver() {
        assert_eq!(next_step(&obs(900, 1, true, true, false)), Step::Settled);
        assert_eq!(next_step(&obs(5_000, 2, true, true, false)), Step::Settled);
    }

    #[test]
    fn presses_again_only_when_the_screensaver_is_back_and_up_to_the_limit() {
        for presses in 1..MAX_PRESSES {
            assert_eq!(
                next_step(&obs(1_000, presses, true, false, true)),
                Step::PressHome { press_number: presses + 1, bring_back_after_ms: BRING_BACK_DELAY_MS, verify_after_ms: VERIFY_DELAY_MS }
            );
        }
        // At the limit it stops pressing and just keeps asking to come back.
        assert_eq!(next_step(&obs(1_000, MAX_PRESSES, true, false, true)), Step::BringBack { recheck_after_ms: VERIFY_DELAY_MS });
    }

    #[test]
    fn a_resumed_ui_with_the_screensaver_still_running_is_not_settled() {
        // uiResumed alone isn't enough: the dream can run over a resumed UI.
        assert_eq!(
            next_step(&obs(1_000, 1, true, true, true)),
            Step::PressHome { press_number: 2, bring_back_after_ms: BRING_BACK_DELAY_MS, verify_after_ms: VERIFY_DELAY_MS }
        );
    }

    #[test]
    fn keeps_bringing_the_app_back_then_gives_up() {
        assert_eq!(next_step(&obs(1_000, 1, true, false, false)), Step::BringBack { recheck_after_ms: VERIFY_DELAY_MS });
        assert_eq!(next_step(&obs(GIVE_UP_MS - 1, 2, true, false, false)), Step::BringBack { recheck_after_ms: VERIFY_DELAY_MS });
        assert_eq!(next_step(&obs(GIVE_UP_MS, 2, true, false, false)), Step::GiveUp);
        assert_eq!(next_step(&obs(GIVE_UP_MS + 1, MAX_PRESSES, true, false, true)), Step::GiveUp);
    }

    #[test]
    fn settling_wins_over_the_deadline() {
        assert_eq!(next_step(&obs(GIVE_UP_MS + 1_000, 2, true, true, false)), Step::Settled);
    }

    #[test]
    fn the_cover_timeout_outlasts_the_give_up_window() {
        assert!(MASK_TIMEOUT_MS > GIVE_UP_MS);
    }

    #[test]
    fn steps_serialize_with_a_kind_tag() {
        assert_eq!(serde_json::to_string(&Step::Settled).unwrap(), r#"{"kind":"Settled"}"#);
        assert_eq!(
            serde_json::to_string(&Step::PressHome { press_number: 1, bring_back_after_ms: 150, verify_after_ms: 700 }).unwrap(),
            r#"{"kind":"PressHome","press_number":1,"bring_back_after_ms":150,"verify_after_ms":700}"#
        );
        assert_eq!(serde_json::to_string(&StartDecision::Escalate).unwrap(), r#"{"kind":"Escalate"}"#);
    }
}
