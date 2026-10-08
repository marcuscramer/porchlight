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

Porchlight is a good fit for a Portal at a senior's home, with a few
optional features for it. When you call, the Portal wakes up, the TV turns on
and switches to the right HDMI input, and, with Auto-answer switched on for you
as a trusted contact, the call connects by itself after a short countdown, so
the person at home never has to press anything. [Kiosk mode](#3-kiosk-mode)
brings Porchlight back up after a restart, and updates install from inside the
app, so you can fix things without a visit (see the
[convenience setup](#convenience-optional-but-recommended)). Because
Auto-answer turns the camera and microphone on without anyone pressing Accept,
it is off by default and set per contact.

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

## Installing on a Portal TV

Porchlight is sideloaded from a computer, so you need three things:

1. **USB debugging allowed on the Portal**, in its Settings (**Settings >
   Debug > ADB Enabled**).
2. **The Portal connected to the computer by USB.** The first time, the
   Portal asks "Allow USB debugging?" on its screen: choose Allow, and tick
   "Always allow".
3. **adb and the Porchlight APK on the computer:** download Google's
   [platform-tools](https://developer.android.com/tools/releases/platform-tools)
   (which contain [adb](https://developer.android.com/tools/adb)) and
   `app-release.apk` from the
   [latest release](https://github.com/marcuscramer/porchlight/releases/latest).

Then, from the folder with the APK, install it:

```
adb install app-release.apk
```

The app appears on the Portal's own home screen once installed. You may have to
grant camera and microphone permissions.

### Convenience (optional but recommended)

A few one-time setups make a Portal much nicer to live with. Most are done
with `adb` commands run from the same computer you used for the install,
because they touch protected system settings that no ordinary app is allowed
to change. None of them is needed for calls to
work.

#### 1. Updating from inside the app

Porchlight checks for new releases and can install them for you from
Settings. For that to work, run these three commands once:

```
adb shell appops set dev.porchlight.app REQUEST_INSTALL_PACKAGES allow
adb shell pm disable-user --user 0 com.facebook.appverifier
adb shell settings put global package_verifier_enable 0
```

**The first command allows Porchlight to install apps.** Android only lets an
app install packages once you've allowed it. Without this command, the first
time you press Install the Portal opens its "Install unknown apps" page for
Porchlight, and you can switch it on there instead; until you do, Install
just keeps opening that page. The permission is also reset whenever the app is
uninstalled and installed afresh, so it's worth repeating after a full
reinstall. To take it back:
`adb shell appops set dev.porchlight.app REQUEST_INSTALL_PACKAGES deny`.

**The other two turn off Meta's install verifier.** Meta's own on-device
install verifier silently rejects *any* sideloaded app's install, including
Porchlight's own updates, no matter how the install is triggered. The first
of the two turns off the specific app that does the rejecting; the second
turns off Android's own install-verification system as a whole, which is what
asks that app to approve or reject installs in the first place. Both are
reversible (`adb shell pm enable com.facebook.appverifier` and setting the
value back to `1` restore them).

If you'd rather not touch these settings, skip this step. In-app updates won't
install, but you can still update the way you installed: download the new APK
and run `adb install -r` yourself.

#### 2. Waking the Portal and turning on the TV for incoming calls

Skip this and calls still ring, but if the
Portal is asleep when one arrives its screensaver can take over the screen so
the call is there but never visible, and the TV stays off, or on whatever HDMI
input it was showing. With it, when a call rings Porchlight ends the screensaver
and makes the Portal turn the TV on if it's off and switch it to its input.

It takes two steps. The first one runs inside the Portal's own shell. Open that shell first:

```
adb shell
```

Wait for the Portal's prompt to appear, then paste these lines (the last one
leaves the shell again):

```
S=dev.porchlight.app/dev.porchlight.app.CallWakeUpAccessibilityService
settings put secure enabled_accessibility_services "$(settings get secure enabled_accessibility_services):$S"
settings put secure accessibility_enabled 1
exit
```

Then, back in your own terminal:

```
adb shell appops set dev.porchlight.app SYSTEM_ALERT_WINDOW allow
```

**The first step switches on Porchlight's accessibility service.** It adds
Porchlight to the Portal's existing accessibility services without replacing
them. The service can't read anything on the screen; all Porchlight does with
it is press Home when a call rings. Once it is on, Settings in Porchlight has a
**Call wake-up** switch to turn the feature off and on without touching `adb`
again (until the service is enabled, the switch is off and can't be reached).

**The second step is purely cosmetic.** It lets Porchlight cover the
screen for the moment it's pressing Home. Without it everything still works,
but you'd see the Portal's own home screen flash briefly as the call comes in.
It needs Android's "Allow display over other apps" access, which is a normal,
narrow permission.

To undo this completely, read the service list with
`adb shell settings get secure enabled_accessibility_services`, remove the
`dev.porchlight.app/…` entry and put the rest back with
`adb shell settings put secure enabled_accessibility_services "<the rest>"`;
revoke the cover with
`adb shell appops set dev.porchlight.app SYSTEM_ALERT_WINDOW deny`.

#### 3. Kiosk mode

Settings in Porchlight has a **Kiosk mode** switch, off by default. Calls don't
need it: Porchlight's background service starts by itself when the Portal
boots and rings either way. What the switch adds is the screen. With Kiosk mode
on, Porchlight's own screen comes up by itself after the Portal restarts and
whenever the screensaver ends, so the Portal always lands on Porchlight
instead of on Meta's home screen. That's what you want for a Portal that only
makes calls, such as one you set up for someone else. Leave it off if the
Portal is also used for other things, since Porchlight would otherwise pull
itself back to the front each time the screensaver ends. Unlike the other
steps this needs no `adb`: it is just a switch in Porchlight's Settings, and
you can turn it off again at any time. (An app update doesn't bring the screen
up by itself, even with Kiosk mode on.)

#### 4. Granting camera and microphone up front

Porchlight needs the camera and the microphone, and Android normally asks for
each the first time they're used, with a pop-up on the screen. If you'd rather
not deal with those pop-ups with the remote, or you're setting the Portal up
for someone else and want it to just work, grant them from the computer:

```
adb shell pm grant dev.porchlight.app android.permission.CAMERA
adb shell pm grant dev.porchlight.app android.permission.RECORD_AUDIO
```

Like the install permission in step 1, these are forgotten when the app is
uninstalled, so repeat them after a full reinstall. Together with steps 1 and
2 that is every permission Porchlight asks for that you'd otherwise have to
grant by hand: install apps, display over other apps, the accessibility
service, the camera and the microphone. (Everything else it uses, such as the
network, is granted automatically.) To take them back:
`adb shell pm revoke dev.porchlight.app android.permission.CAMERA`, and the
same for the microphone.

#### 5. Preventing updates from Meta

The setup above is made of settings on the Portal, and a system update from
Meta could in principle put some of it back: turn the install verifier on again,
for instance, which would stop in-app updates without any warning. Worse, an
update could also switch USB debugging off, and without it you can no longer
reach the Portal from a computer, which makes it a lot less useful as a
device. If you'd rather that couldn't happen, you can switch off the part of
the Portal that installs Meta's system updates:

```
adb shell pm disable-user --user 0 com.facebook.aloha.otaui
adb shell pm disable-user --user 0 com.facebook.aloha.alohaotasetup
```

Some Portal builds don't have the second one; if adb complains about it,
that's fine.

The price is that your Portal then gets no more operating-system or security
updates from Meta at all. Meta has discontinued the Portal, so there aren't
many to miss, but it is your call, and it's a good reason to skip this step
on a Portal that does more than make calls. To get updates back:

```
adb shell pm enable com.facebook.aloha.otaui
adb shell pm enable com.facebook.aloha.alohaotasetup
```

## Using the web version

`web-app/` is a browser page that works exactly like the Android app —
same pairing flow, same calling. It's a real, full option for regular
calls, not a fallback or a developer testing tool.

The easiest way to use it: **[marcuscramer.github.io/porchlight](https://marcuscramer.github.io/porchlight/)**
— nothing to install, just open it. If you'd rather run your own copy,
host `web-app/` somewhere reachable (any static web host), or open
`web-app/index.html` directly for a quick local test.

A web page that has been hidden for about 30 seconds with nothing going on
(no call, no pairing) disconnects from the relays to save battery, and its
contacts see it as offline until you come back to it; it reconnects as soon as
you do. While a call is ringing or running it stays connected. Because of
that, a hidden or closed page can't ring: calls only reach a web page that
is open and visible.

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
   Once matched, each of you confirms the other side's name with a single
   tap. Pairing is done, for good, once *both* of you have confirmed; if
   one of you cancels (or doesn't confirm in time), nothing is saved on
   either device.

## Relays

Signaling goes through public Nostr relays (see [SECURITY.md](SECURITY.md) for what they can see). The relay list is a
file, [`web-app/relays.json`](web-app/relays.json), served next to the web page. The Portal app fetches it on start
and every six hours (and sooner when a contact's heartbeat shows a newer version), the web page checks the same way
and reloads itself when a newer list exists. **Settings > Status** shows the list version and has an "Update list" button at the bottom.

## Day to day

- **Back**: on the Portal, the remote's **Back** key goes back or cancels on
  every screen (Settings, adding a contact, pairing, a dialog); no screen has
  an on-screen Back or Cancel button. The web version has those buttons
  instead.
- **Incoming calls**: ring with a green Accept button (the same one as
  Call) in the controls at the bottom of the screen; the red button next to
  it, or Back on the Portal, declines. On the Portal app (not the web
  version), auto-answer can be turned on for a contact — a toggle right on
  their row in the contact list — so a call from them connects
  automatically after a short countdown instead (Accept skips the wait).
- **During a call**: the controls (self-view position, Audio, Video, hang
  up) are shown while a call is placed or rings and stay on screen once it
  connects. To get them off the video, press Back once on the Portal (in the
  web version: click the video, or press Esc, Backspace, Enter or Space);
  OK on the Portal (a click, Enter or Space on the web) brings them back.
  Pressing Back again, with the controls hidden, hangs up. While a call is still ringing or connecting,
  Back cancels or declines it.
- **TV off, or on a different HDMI input**: if your Portal TV is plugged into
  a TV that's off or showing something else when a call comes in, the call
  still rings and connects, but whether the TV follows depends on the call
  wake-up setup above. With it, as the call rings, the Portal asks the TV
  (over HDMI-CEC) to turn on
  if it's off and to switch to its input — so the call just appears (a TV
  that was off can take several seconds to come up). Without it, you'll need
  to turn the TV on and switch the input yourself to see the call. (A
  sideloaded app can't send that HDMI-CEC request itself — the permission is
  restricted to apps signed with Meta's own key — which is why this goes
  through a "hacky" Home-key press.) It needs a TV with HDMI-CEC turned on.
- **When a call doesn't go through**, the screen says why instead of a
  generic failure: *Call declined* (they pressed decline), *No answer* (it
  rang for a minute and nobody picked up), *Not reachable* (they appear to be
  offline, or may have removed you from their contacts; while it waits, the
  calling screen says they look offline), *Busy* (they're on another call),
  *Camera or microphone problem* (this device couldn't use its camera or
  microphone), *Couldn't be answered* (the other device couldn't answer
  because of a camera or microphone problem on its side), and *Couldn't connect* (both sides were there but the
  video never came up; if it can tell, the message says whether the network
  seems to block calls, as on two phones on mobile data). The device that was
  being called sees *Missed call* if the caller hung up before it was answered,
  and *Call ended* when a call that was already underway is ended from the
  other side. These screens close by themselves after a while, or with Back
  on the Portal (the OK button on the web). If the Portal's camera stops
  working during a call (for example the camera cover is closed), the call
  carries on with sound only; switching Video on again tries the camera again.
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
recovery, and sideload-specific work (screensaver handling, wake-lock behavior).

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
