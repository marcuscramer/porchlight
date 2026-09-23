# Porchlight

Two-way video calling that works between Meta Portal TVs, browsers, or any
mix of the two — no account, no subscription, no server run by anyone.
Each device generates its own private identity the moment it's set up,
and two devices pair by simply agreeing on a shared phrase.

The two ends of a call don't need matching setups: Portal to Portal,
Portal to browser, or browser to browser all work the same way.

## What you need

- **Just a browser** — the web version needs nothing installed. See
  "Using the web version" below.
- **A Meta Portal TV**, if you want it running there too:
  - Porchlight installs and shows up on the stock Portal home screen on
    its own — no third-party launcher required.
    [Immortal](https://github.com/starbrightlab/immortal) is still worth
    considering for reasons that have nothing to do with running
    Porchlight itself: its provisioning kit can freeze the device against
    further Meta OS updates, it revives a Portal whose own stock
    experience has otherwise degraded, and it gives you a proper
    on-device catalog for managing whatever else you sideload. Porchlight
    doesn't depend on it either way.
  - **Developer Options → USB debugging** enabled on the Portal, with a
    one-time USB trust prompt accepted. Porchlight is sideload-only — it
    will never be on the Google Play Store, since the Portal has no
    Google Play Services at all.
  - A computer to run `adb install` from, for the initial install (see
    below).

## Installing on a Portal TV

There's no Play Store on this device, so installing means sideloading the
APK directly from a computer with [adb](https://developer.android.com/tools/adb)
installed:

```
adb install porchlight.apk
```

The app appears on the Portal's own home screen once installed. If you use
Immortal, it also shows up in its app list/catalog the same way any other
sideloaded app does.

On first launch, grant the camera, microphone, and notification
permissions the app asks for.

## Using the web version

`web-app/` is a browser page that works exactly like the Android app —
same pairing flow, same calling. It's a real, full option for regular
calls, not a fallback or a developer testing tool.

To use it, host `web-app/` somewhere reachable (any static web host), or
open `web-app/index.html` directly for a quick local test.

The web version stores its name and contacts in that browser's own local
storage — clearing the browser's site data for that page resets it to a
fresh, unpaired profile, the same as reinstalling the Android app.

## Pairing two devices

No account, no QR code, nothing to sign up for. Pairing works exactly the
same way regardless of which combination of Portal and browser is on each
end.

1. Each device asks for a short name the first time it runs ("Living
   Room," "Mom's laptop") — shown to the other device during pairing so
   you can tell devices apart.
2. On the contacts screen, tap **Add contact**. Agree on a short phrase
   with the person on the other end (over a phone call, a text, however
   you're already in touch) and have both of you type the *exact same
   phrase* into your own device. It doesn't matter who types first. Pick
   a phrase only the two of you would think of — avoid a common word, a
   name, or a short number.
3. The two devices find each other automatically once the phrases match.
   If more than one device answers the same phrase at once, both sides
   are told and asked to try again with a fresh phrase, rather than
   guessing which one is right. Once matched, confirm the other side's
   name with a single tap — pairing is done, for good, for that contact.

## Day to day

- **Calling**: tap a contact's Call button. If they're not reachable right
  this second, the call just waits — there's no need for them to already
  be "online."
- **Incoming calls**: ring for a manual Accept/Decline. On the Portal app
  (not the web version), auto-answer can be turned on for a contact — a
  toggle right on their row in the contact list — so a call from them
  connects automatically after a short countdown instead.
- **Status dots** next to each contact are purely informational — green
  means they're currently reachable, orange means they're on another call
  right now, red means they're not currently reachable, and none of this
  is required before you can tap Call.
- **Adding another contact**: tap the "+" button below the contact list.
- **Renaming this device**: tap the small gear icon on the waiting screen
  to reach Settings.
- **Removing a contact**: tap the bin icon next to their name. This is
  permanent — to reconnect with them later, pair again with a fresh
  phrase.

## Privacy and security, in brief

Calls are always peer-to-peer and end-to-end encrypted — video and audio
never pass through any server, ours or anyone else's. Pairing itself uses
a real cryptographic key exchange (the same family of algorithm behind
Magic Wormhole): the shared phrase you and the other person type is never
transmitted anywhere in a form that could be guessed at offline, and each
contact gets its own independent key, so one contact's key doesn't say
anything about any other. Signaling (the brief "here's how to reach me"
exchange that happens before a call connects) travels over public Nostr
relays rather than a private server — nobody runs infrastructure for this
app, and nobody needs an account to use it.

If you think a specific contact's security has been compromised, delete
that contact and pair again with a fresh phrase — every contact is its
own independent identity, so this never affects any of your other
contacts. See [SECURITY.md](SECURITY.md) for the full threat model and
what this does and doesn't protect against.

## Credits

Started as a fork of [Ishtiaqhossain/portal-security-camera](https://github.com/Ishtiaqhossain/portal-security-camera)
(MIT licensed — see `LICENSE`), which proved the Portal camera + WebRTC
pipeline could work at all. It's since diverged substantially: two-way
(not just one-way) calling, symmetric peer roles, Nostr-based signaling
with no self-hosted server and no account of any kind, per-device signing
identity with explicit human-confirmed pairing, boot-launch and crash
recovery, and sideload-specific work (screensaver handling, dual stock/
Immortal launcher support, wake-lock behavior).

Implementation was done with [Claude](https://claude.com/claude-code)
(Anthropic).

---

This is an independent hobby project, not affiliated with, endorsed by,
or sponsored by Meta. "Meta" and "Portal" are trademarks of Meta
Platforms, Inc., used here only to describe the hardware this software
runs on.
