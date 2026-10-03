# Porchlight

### Two-way video calling for the discontinued Meta Portal TV

**Still testing** — expect crashes and rough edges.

No account.  
No subscription.  
No servers run.

Portal TV ↔ Browser   
Portal TV ↔ Portal TV  
Browser ↔ Browser  

Each pairing gets its own private key, generated on the device,
and two devices pair by simply agreeing on a shared phrase.

Calls connect directly between the two devices whenever a direct
connection is possible. No fallback yet for networks
where it isn't (for example, both people on separate, more restrictive
office or mobile networks) — in that case the call may simply fail to
connect.

### Privacy model

🔒 Audio/video: direct P2P  
🔑 Keys: generated locally, one per contact  
🚫 Account: none  
🚫 Subscription: none  
🚫 Video relay: none  
📡 Signaling: public Nostr relays

See [SECURITY.md](SECURITY.md) for the full threat model and
what this does and doesn't protect against.

## Try it

Open Porchlight in your browser:
[marcuscramer.github.io/porchlight](https://marcuscramer.github.io/porchlight/)

Have someone else open it too. Agree on a phrase. Pair. Call. No signup required.

## What you need
### Just a browser
The web version needs nothing installed. See
  "Using the web version" below.

### A Meta Portal TV
If you want it running there too:  
  - USB debugging enabled on the Portal — Porchlight is sideload-only.
  - A USB-connected computer to run `adb install` from, for the initial install (see
    below).

Porchlight installs and shows up on the stock Portal home screen on its own — no third-party launcher required.
[Immortal](https://github.com/starbrightlab/immortal) is still worth
considering for reasons that have nothing to do with running
Porchlight itself: its provisioning kit can freeze the device against further Meta OS updates and it gives you a proper on-device catalog for managing whatever else you sideload.

## Installing on a Portal TV

Sideload the
APK directly from a computer with [adb](https://developer.android.com/tools/adb)
installed:

```
adb install app-release.apk
```

The app appears on the Portal's own home screen once installed. If you use
Immortal, it also shows up in its app list/catalog the same way any other
sideloaded app does.

On first launch, grant the camera, microphone, and notification
permissions the app asks for.

### Convenience (highly recommended)

Two one-time setups make a Portal much nicer to live with. Both are `adb`
commands run from the same computer you used for the install, because they
touch protected system settings that no ordinary app is allowed to change.
Neither is needed for calls to work, but you'll want both on any Portal that
isn't sitting next to you.

#### 1. Updating from inside the app

Porchlight checks for new releases and can install them for you from
Settings. For that to actually work, Meta's own on-device install
verifier needs to be turned off once — otherwise it silently rejects
*any* sideloaded app's install, including Porchlight's own updates, no
matter how the install is triggered:

```
adb shell pm disable-user --user 0 com.facebook.appverifier
adb shell settings put global package_verifier_enable 0
```

The first command turns off the specific app that does the rejecting;
the second turns off Android's own install-verification system as a
whole, which is what asks that app to approve or reject installs in the
first place. Both are reversible (`adb shell pm enable
com.facebook.appverifier` and setting the value back to `1` restore
them).

If you'd rather not touch that setting, skip this step. In-app updates won't
install, but you can still update the way you installed: download the new APK
and run `adb install -r` yourself.

If you're using [Immortal](https://github.com/starbrightlab/immortal),
you don't need to do this manually — its provisioning kit disables the
same verifier automatically as part of setup, for the same reason (it
needs to install and update apps on-device too).

#### 2. Waking the Portal and switching the TV for incoming calls

This one is optional as a whole: skip it and calls still ring, but if the
Portal is asleep when one arrives its screensaver can take over the screen so
the call is there but never visible, and the TV stays on whatever HDMI input
it was showing. With it, when a call rings Porchlight presses the Portal's
Home key, which ends the screensaver and makes the Portal switch the TV to its
input. It's two commands, and you want both.

First, Porchlight needs its accessibility service switched on. The Portal's
own Settings has no screen for this:

```
adb shell 'cur=$(settings get secure enabled_accessibility_services); [ "$cur" = null ] && cur=""; settings put secure enabled_accessibility_services "${cur:+$cur:}dev.porchlight.app/dev.porchlight.app.CallWakeUpAccessibilityService"; settings put secure accessibility_enabled 1'
```

It adds Porchlight to the Portal's existing accessibility services and doesn't
replace them (the Portal already runs two of its own), so run it only once.
The service can't read anything on the screen; all Porchlight does with it is
press Home when a call rings.

Second, let Porchlight cover the screen for the moment it's pressing Home.
Without this you'd see the Portal's own home screen flash for well under a
second as the call comes in; with it, nothing but the call screen itself ever
shows. It needs Android's "Allow display over other apps" access:

```
adb shell appops set dev.porchlight.app SYSTEM_ALERT_WINDOW allow
```

That's a normal, narrow permission. The cover is just a plain color, drawn
while Home is being pressed and removed the moment the call screen is
confirmed up, with a safety timer so it can never stay covering the screen.

Settings in Porchlight shows whether the wake-up service is on. To turn the
whole thing off again, read the service list with
`adb shell settings get secure enabled_accessibility_services`, remove the
`dev.porchlight.app/…` entry and put the rest back with
`adb shell settings put secure enabled_accessibility_services "<the rest>"`;
revoke the cover with
`adb shell appops set dev.porchlight.app SYSTEM_ALERT_WINDOW deny`.

## Using the web version

`web-app/` is a browser page that works exactly like the Android app —
same pairing flow, same calling. It's a real, full option for regular
calls, not a fallback or a developer testing tool.

The easiest way to use it: **[marcuscramer.github.io/porchlight](https://marcuscramer.github.io/porchlight/)**
— nothing to install, just open it. If you'd rather run your own copy,
host `web-app/` somewhere reachable (any static web host), or open
`web-app/index.html` directly for a quick local test.

The web version stores its name and contacts in that browser's own local
storage — clearing the browser's site data for that page resets it to a
fresh, unpaired profile, the same as reinstalling the Android app.

## Pairing two devices

Pairing works exactly the
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
   Once matched, confirm the other side's
   name with a single tap — pairing is done, for good, for that contact.

## Day to day

- **Incoming calls**: ring for a manual Accept/Decline. On the Portal app
  (not the web version), auto-answer can be turned on for a contact — a
  toggle right on their row in the contact list — so a call from them
  connects automatically after a short countdown instead.
- **TV on a different HDMI input**: if your Portal TV is plugged into a TV
  that's showing something else when a call comes in, the call still rings
  and connects, but whether the TV switches to the Portal depends on the
  call wake-up setup above. With it, Porchlight presses the Portal's Home
  key as the call rings, and the Portal then asks the TV (over HDMI-CEC) to
  switch to its input — so the call just appears. Without it, you'll need to
  switch the input yourself to see the call. (A sideloaded app can't send
  that HDMI-CEC request itself — the permission is restricted to apps signed
  with Meta's own key — which is why this goes through the Home key.) It
  needs a TV with HDMI-CEC turned on.
- **Status dots** next to each contact are purely informational — green
  means they're currently reachable, orange means they're on another call
  right now, red means they're not currently reachable, and none of this
  is required before you can tap Call.
- **Removing a contact**: tap the bin icon next to their name. This is
  permanent — to reconnect with them later, pair again with a fresh
  phrase.

## Setting one up for the grandparents

The goal is a Portal that someone who never wants to touch it can leave
plugged in, and you can call whenever you like. Do this once, ideally on a
phone call with them so you can agree the pairing phrase.

1. Install Porchlight on their Portal and do the
   [convenience setup](#convenience-highly-recommended) — both parts. Updating
   from inside the app means you can push fixes later without a visit, and
   call wake-up means a call appears even if the Portal is asleep and the TV
   is on another input.
2. Pair it with your own phone or browser (see [Pairing two devices](#pairing-two-devices)).
3. In Porchlight's Settings on their Portal, turn on **Kiosk mode** so Porchlight
   comes back up by itself after a restart and when the screensaver ends. Set
   **Ringtone volume** to something they'll hear from across the room.
4. On their contact list, switch **Auto-answer** on for your contact — and for
   anyone else they'd want connected without a tap.

From then on, when you call, their Portal wakes, the TV switches over and a
short countdown ("… will automatically connect in 5 seconds") runs before the
call connects on its own. Pressing Back during the countdown declines the call,
so nobody is ever surprised without a way out. The green dot next to their name
on your device tells you the Portal is reachable before you call.

Worth knowing:

- **Auto-answer means the camera and microphone go live on that contact's
  call** without anyone pressing Accept. Only turn it on for people you'd
  trust with that, and tell the person at the other end. It's per contact,
  off by default.
- The Portal's physical **camera cover** has to be open for video. If it's
  closed the call can't use the camera, so ask them to leave it open — or
  close it deliberately when they want privacy.
- Leave the Portal powered and on Wi-Fi. Porchlight keeps itself running in
  the background to receive calls, and with Kiosk mode on it also starts
  again by itself after a restart.

## Privacy and security, in brief

Calls are always peer-to-peer and end-to-end encrypted — video and audio
never pass through any server, ours or anyone else's. Pairing itself uses
a real cryptographic key exchange (the same family of algorithm behind
Magic Wormhole): the phrase you and the other person type is never sent
as-is, and watching the exchange doesn't reveal the key it produces. The
public pairing tag derived from the phrase can still be used to guess a
short or common phrase, so pick a long, unpredictable one. The device name you
see while pairing is sent encrypted, and only after the exchange succeeds.
Each contact gets its own independent key, so one contact's key doesn't say
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
with no self-hosted server and no account of any kind, per-contact signing
keys with explicit human-confirmed pairing, boot-launch and crash
recovery, and sideload-specific work (screensaver handling, dual stock/
Immortal launcher support, wake-lock behavior).

Implementation was done with [Claude](https://claude.com/claude-code)
(Anthropic).

---

This is an independent hobby project, not affiliated with, endorsed by,
or sponsored by Meta. "Meta" and "Portal" are trademarks of Meta
Platforms, Inc., used here only to describe the hardware this software
runs on.

Meta Portal hardware is discontinued, and Meta doesn't guarantee that any feature or update will remain available.
Porchlight is third-party, unofficial software, sideloaded outside
Meta's own app ecosystem. You're installing it at your own risk, the same as any
other sideloaded app.
