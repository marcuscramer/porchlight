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

### Letting the app update itself

Porchlight checks for new releases and can install them for you from
Settings. For that to actually work, Meta's own on-device install
verifier needs to be turned off once — otherwise it silently rejects
*any* sideloaded app's install, including Porchlight's own updates, no
matter how the install is triggered. This is a one-time step, run from
the same computer you used for the initial `adb install`:

```
adb shell pm disable-user --user 0 com.facebook.appverifier
adb shell settings put global package_verifier_enable 0
```

The first command turns off the specific app that does the rejecting;
the second turns off Android's own install-verification system as a
whole, which is what asks that app to approve or reject installs in the
first place. Both are one-time, reversible (`adb shell pm enable
com.facebook.appverifier` and setting the value back to `1` restore
them), and only work from `adb shell` since they touch protected system
settings no ordinary app is allowed to change.

If you'd rather not touch that setting, that's fine — just skip this
step. In-app updates won't install, but you can still update the same way
you installed: download the new APK and run `adb install -r` yourself.

If you're using [Immortal](https://github.com/starbrightlab/immortal),
you don't need to do this manually — its provisioning kit disables the
same verifier automatically as part of setup, for the same reason (it
needs to install and update apps on-device too).

### Letting an incoming call beat the screensaver

On real Portal TV hardware, the stock screensaver can re-take the screen
right as an incoming call tries to show, so the call is there but never
visible. Porchlight can prevent this by turning the screensaver off for
the few seconds a call is actually ringing (and back on the moment it's
answered, declined, or times out) — but doing that needs a permission
Android only grants from `adb shell`, the same way as the self-update
step above:

```
adb shell pm grant dev.porchlight.app android.permission.WRITE_SECURE_SETTINGS
```

This is a one-time step, same computer as the initial install. Skip it
and calls still ring but may be hidden behind the screensaver.

Unlike the self-update step, **this one isn't covered by Immortal's own
provisioning kit** even if you're using Immortal — provisioning grants
that permission to Immortal's own app, not Porchlight's, since permission
grants are always per-app. Run the command above for Porchlight either
way.

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
- **If your Portal TV is plugged into a TV via HDMI and that TV is on a
  different input when a call comes in**, the call still rings and
  connects normally, but the TV itself won't switch to the Portal's input
  on its own — you'll need to switch it manually to see the call. This is
  a Portal TV OS limitation, not something Porchlight (or any sideloaded
  app) can fix: automatically switching a TV's input over HDMI-CEC
  requires a permission (`HDMI_CEC`) that's restricted to apps signed
  with Meta's own platform key, and the Android version Portal TV runs
  doesn't wake-trigger this automatically the way newer Android TV
  versions do.
- **Status dots** next to each contact are purely informational — green
  means they're currently reachable, orange means they're on another call
  right now, red means they're not currently reachable, and none of this
  is required before you can tap Call.
- **Removing a contact**: tap the bin icon next to their name. This is
  permanent — to reconnect with them later, pair again with a fresh
  phrase.

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
