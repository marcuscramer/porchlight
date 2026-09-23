# Call state machine — invariants

This exists because three real bugs shipped in the same feature (per-contact
auto-answer) all came from the same root cause: the call state machine's
rules were only ever written down as scattered doc comments next to the
code that happens to enforce them, not as a single place a future change
could check itself against.

**This state machine has exactly one implementation**:
`call-core/src/call_arbitration.rs`, verified by `cargo test` (one or more
tests per invariant below — see each invariant's own pointer). Android
(`CallCoreBridge.kt`) and web (`app.js`, via the WASM build) are both thin
binding layers over it, not independent implementations — read this before
touching either platform's call-handling code, and update it when the
state machine itself changes (in Rust), not just when a bug in it gets
fixed.

The field names below are given as `android / web` where a platform still
keeps its own UI-facing mirror of Rust's state; the state itself lives once,
in Rust.

## The state (owned by `call_arbitration::CallState`)

`CallState` is a field of the crate-wide `AppState` behind `crate::STATE`'s
one shared `Mutex`, not this module's own separate lock — see `AppState`'s
own doc in `lib.rs` for why every module's state is merged behind one lock.

- **`state.slot: CallSlot`** — the device's one call slot, exactly one of:
  - `CallSlot::Idle` — nothing claimed. Corresponds to the old
    `activePairingId == null`.
  - `CallSlot::Ringing(PendingOffer)` (`pendingOffer` on android /
    `pendingIncomingCall` on web — the shell's own UI-facing mirror) — an
    incoming offer has arrived but hasn't been handed to the
    `PeerConnection` yet: either counting down (auto-answer) or waiting on
    a human tap (manual accept). Holds the raw SDP plus an `ice_buffer`
    for ICE candidates that arrive during that window (trickle ICE means
    the caller can be sending them before ringing even resolves; `pc`
    doesn't exist yet to hand them to — see `handle_remote_ice`'s
    `Buffered` outcome).
  - `CallSlot::Claimed(ActiveCall)` — a call is placed or being
    negotiated: `pairing_id`/`call_id` (old `activePairingId`/
    `activeCallId` — distinct because a pairing can be called again right
    after a previous attempt ends; every offer/answer/ice/bye is tagged
    with `call_id` and anything arriving with a different one for the same
    pairing is a stale/abandoned attempt, not the current call), plus
    `answer_applied`/`offer_applied`/`connected_once`.

  Only one pairing can hold the slot (any variant but `Idle`) at a time; a
  second pairing trying to call in while it's held gets a `busy` reply
  instead (`handle_should_offer`/`handle_offer`'s first guard).
- `incomingCall` (android, published in `AgentState`) — the *UI-facing*
  half of `CallSlot::Ringing`: whether this is an auto-answer countdown or a
  manual prompt, and the seconds remaining, derived from Rust's
  `StartRinging`/`TickOutcome` effects. Web has no separate field for this;
  `pendingIncomingCall` itself carries `autoAnswer`/`secondsRemaining`.
- `pc` (`WebRtcEngine` / web global) — the actual `PeerConnection`/
  `RTCPeerConnection`. Non-null only once an offer has actually been
  applied — i.e. **after** ringing resolves (`CallSlot` has moved from
  `Ringing` to `Claimed`), never during it. This is the line between "a
  call slot is reserved" and "a call is actually being negotiated." `pc`
  itself is deliberately **not** owned by Rust — `call_arbitration` never
  holds a real WebRTC object, only `has_active_peer_connection: bool`/
  `hasActivePeerConnection` where a function still takes it explicitly
  (most no longer do — see invariant #7's own "removed entirely" note),
  snapshotted by the shell and passed in.

## Invariants

Each one is enforced by a specific Rust function now — if you think you've
found a violation, the fix belongs in `call_arbitration.rs`, then a
`cargo test`, and both platforms inherit it through their bindings. It is
*not* something to hand-patch on one platform and port to the other later.

1. **The slot (`state.slot`, old `activePairingId`/`activeCallId`) is
   claimed the moment a call *could* happen, not once it's accepted.**
   `request_call`/`handle_offer`/`handle_should_offer` all move `state.slot`
   out of `Idle` immediately — before any human has decided anything — so
   a second contact calling in during that window correctly gets `busy`
   instead of silently racing the first. Don't gate this claim on
   acceptance; that's exactly the bug class in #2 below.
   Tests: `request_call_claims_slot_and_offers_when_peer_online_and_own_pubkey_wins_tiebreak`,
   `handle_should_offer_claims_slot_and_rings_respecting_auto_answer`,
   `handle_offer_fresh_call_starts_ringing_not_apply`.
2. **`CallSlot::Ringing`/`pendingIncomingCall` is the *only* thing
   acceptance gates.** `pc` must not exist until a real accept (manual tap,
   countdown reaching zero, or the tie-break bypass in #3) actually
   happens. Anything that needs "is this call really live" must check
   `pc`/`hasActivePeerConnection`, not whether the slot is merely claimed.
   Tests:
   `handle_offer_fresh_call_starts_ringing_not_apply`,
   `accept_incoming_call_hands_back_offer_and_buffered_ice_then_clears_pending`.
3. **An offer that completes a call *this side* already placed must skip
   ringing entirely.** The pubkey tie-break means the side that loses it
   sends a plain `call` message and *waits for the other side's offer* as
   the next step of the same call — that offer arrives through the exact
   same `handle_offer` path as a genuinely fresh incoming call. The check
   is `state.slot` already being `CallSlot::Claimed` with this exact
   `pairing_id`/`call_id` when the offer arrives (set by this side's own
   `request_call`, before the offer came back) — if so, apply it
   immediately (`ApplyRemoteOffer`), no ring, no countdown. Without this
   check, a caller sees its own outgoing call reflected back as a bogus
   "Incoming call from X" screen. It only reproduces in *one* of the two
   tie-break directions — both are pinned down as separate tests, not one:
   `handle_offer_applies_immediately_when_it_completes_a_call_this_side_already_placed_tiebreak_direction_a`
   and `..._direction_b`.
4. **Ending a call slot must read the pairing/call id before tearing
   anything down, never after.** `hang_up()` captures both *first* (via
   one `std::mem::replace(&mut state.slot, CallSlot::Idle)`, which both
   reads the outgoing `pairing_id`/`call_id` and clears the slot in the
   same statement — structurally atomic, not just conventionally ordered),
   *then* builds the `SendBye` effect from the captured (not re-read)
   values — all in one function with no shell-visible ordering left to get
   wrong. This exact bug class (reading state after clearing it instead of
   before) is now structurally impossible to reintroduce on either
   platform — there's only one `hang_up()`. Tests:
   `hang_up_sends_bye_using_the_captured_pairing_and_call_id`,
   `hang_up_clears_state_so_a_second_hang_up_sends_no_bye`.
5. **A media/capture failure only means "end the call" if a call is
   actually in progress.** `should_end_call_on_media_failure(has_active_peer_connection)`
   is the whole decision — trivial today (it's literally the input echoed
   back), kept as its own function because the *caller* (Android's
   `WebRtcEngine.onMediaFailure`) needs a single place to route through
   regardless of how the check might grow later. Ringing must not be as
   fragile as an active call is to this class of failure: showing a
   self-view during ringing means the camera gets acquired before a human
   has decided anything, a real exposure to camera failure a plain "end
   the call" response would handle badly. Web has no camera-failure
   trigger wired to this function yet — a known, accepted gap. Test:
   `should_end_call_on_media_failure_matches_has_active_peer_connection`.
6. **Every teardown path must clear the incoming-call state, not just the
   call slot.** `hang_up`/`handle_peer_hangup`/`handle_peer_left`/
   `forget_pairing` all set `state.slot = CallSlot::Idle` (which, being one
   enum now rather than two independent fields, clears a live ring the
   same motion as clearing an active call — there's no separate "also
   clear pending_offer" step left to forget) and emit
   `ClearIncomingCallTimer` — call from every path that can end a call
   while one might be ringing, not just the ones that were reachable
   before ringing existed. A stale countdown tick firing after the call
   already resolved some other way is exactly the kind of bug this
   guards against — `tick_incoming_call_countdown` re-checks that
   `state.slot` is still `Ringing` for this exact `pairing_id`/`call_id`
   before doing anything, but that's a second line of defense, not a
   substitute for clearing eagerly. Tests:
   `handle_peer_hangup_clears_a_pending_ring_too`,
   `forget_pairing_closes_the_active_call_without_sending_bye`,
   `tick_incoming_call_countdown_is_stale_after_accept`.
7. **Redelivery of the same offer/call while already ringing must be a
   no-op, not a reset.** `handle_offer` guards on `state.slot` already
   being `Claimed` for this pairing/call with its offer already applied
   (`ActiveCall::offer_applied`), and separately on `state.slot` already
   being `Ringing` for the same `pairing_id`/`call_id` — one `match` over
   the enum. `handle_should_offer` guards on the pairing already owning
   the slot at all — this alone is race-free (set synchronously the
   instant this side's own `request_call` claims the slot). Public relays
   redeliver ephemeral events under load — a redelivered offer must not
   restart an auto-answer countdown from 5, and must not re-ring a contact
   who's already ringing. Tests:
   `handle_offer_redelivered_while_ringing_is_a_no_op`,
   `handle_should_offer_is_a_no_op_when_redelivered_for_the_same_call`,
   `handle_should_offer_is_a_no_op_for_a_second_call_id_while_already_active`,
   `handle_offer_is_a_no_op_when_already_active_with_a_real_peer_connection`,
   `handle_offer_redelivered_after_accept_before_pc_exists_is_a_no_op`.
8. **The pubkey tie-break is the *first* thing `handle_should_offer`
   checks, before busy/redelivery — a `"call"` message that arrives from
   the side that should have lost the tie-break is a silent no-op.**
   Every other message type (`offer`/`answer`/`ice`/`busy`) is always
   forwarded into `call-core` unconditionally, Rust deciding every no-op
   case; `"call"` needs the same unconditional treatment. Test:
   `handle_should_offer_no_ops_when_this_side_would_lose_the_tiebreak`
   (the winning direction is exercised by every other `handle_should_offer`
   test, which all pass a winning pubkey pair).
9. **A `"busy"` reply must release the *caller's own* claimed slot, not
   just drive a contact-list badge.** Without this, a caller bounced by
   busy is stuck on the calling screen forever, unable to call anyone
   else, with no recovery short of manually cancelling. `handle_peer_busy`
   is gated exactly like `handle_peer_hangup` (both `pairing_id` and
   `call_id` must match the currently active attempt) and returns the same
   effects. Tests:
   `handle_peer_busy_releases_the_callers_slot_when_pairing_and_call_id_match`,
   `handle_peer_busy_requires_both_pairing_and_call_id_to_match`.

## Presence's own invariants (`call-core/src/presence.rs`)

A second, smaller state machine, worth documenting here since it calls
directly into `call_arbitration` (not through the shell) and has a few
real rules of its own. `presence`'s and `call_arbitration`'s state are
fields of one shared `AppState` behind one `Mutex` (`crate::STATE`, see
`lib.rs`), not two independently-locked statics.

- **A pairing's presence is one `PresenceStatus` (`Offline`/`Online`/`Busy`),
  not two independent booleans.** Two separate maps (`online_state`,
  `peer_busy`) would let a pairing end up `peer_busy = true` while
  `online_state` still said `false`, making the three states only
  *conventionally* exclusive. Collapsing both into one
  `HashMap<String, PresenceStatus>` makes "offline but also busy"
  unrepresentable, not just guarded against — see `set_status`/
  `ensure_status`, the single choke point every transition goes through.
- **`Busy` implies `Online`, structurally, not by convention.**
  `handle_peer_busy_reply` (a `"busy"` reply to our own outgoing call)
  performs its own `Offline` → `Busy` transition via `ensure_status` rather
  than assuming the shell already called `mark_seen` for this same message
  first. Test:
  `handle_peer_busy_reply_transitions_a_still_offline_pairing_structurally_not_by_convention`.
- **A `"leaving"` message *removes* `last_seen_at`; the timeout sweep
  merely *leaves it stale* and lets it be overwritten naturally.** Not
  interchangeable — `handle_leaving_message` deletes the entry outright,
  `check_online_timeouts` only flips `status` to `Offline`. Test:
  `handle_leaving_message_removes_last_seen_and_transitions_offline`.
- **Deleting a pairing needs *two* calls, not one**: `presence::remove_pairing`
  clears this module's own three maps, but has no visibility into (and
  correctly doesn't touch) `call_arbitration`'s own deferred-call state —
  a pairing with an in-flight deferred call needs
  `call_arbitration::forget_pairing` too, called alongside it from both
  platforms' `removePairing`. Test:
  `remove_pairing_with_an_in_flight_deferred_call_also_needs_forget_pairing`
  documents this split explicitly.

## Before changing this code

- **Change `call_arbitration.rs`/`presence.rs`, add a `cargo test`, not
  "port to both platforms."** A Rust-side fix reaches both platforms
  automatically through their bindings. It's still correct advice for
  what's genuinely platform-specific and *not* covered by either file:
  - The `answerApplied`/real-`signalingState` dual-guard in
    `WebRtcEngine.kt`'s `handleRemoteAnswer`/`app.js`'s `onAnswer` — a
    deliberate, documented *second* independent guard on real WebRTC state
    `call_arbitration` has no visibility into, not migration fallout.
  - Wake-lock/foreground-service lifecycle (Android-only).
  - The two platforms' timer-loop scheduling mechanics (intentionally
    different — fixed-delay vs. fixed-rate, `setTimeout` chains vs.
    `scheduleWithFixedDelay` — with no observable behavioral consequence;
    not something to unify).
- If you're adding anything that runs *before* a call is accepted
  (another preview, another background acquisition, anything), ask
  whether a failure in it should be allowed to kill `activePairingId`/
  `pendingOffer`. Default answer: no — see #5.
- If you're adding a new way a call can end, make sure it goes through
  the shared teardown path (#6), and reason about whether it needs to
  capture `activePairingId`/`activeCallId` before or after whatever it
  calls (#4).
- If you're touching anything in the offer/call-request/tie-break path
  (`request_call`, `handle_peer_online`, `handle_should_offer`,
  `wins_tiebreak`), add `cargo test` cases for **both** pubkey tie-break
  directions — one direction alone cannot prove the other is correct (#3),
  and neither can a single synthetic/mocked offer. A live two-peer test is
  still worth doing for anything touching the actual WebRTC negotiation
  path, though the tie-break *decision* itself is now fully covered by
  `cargo test` alone.
