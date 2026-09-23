# Contributing to Porchlight

This is the developer-facing companion to [README.md](README.md), which is
written for people installing and using the app. Read `CALL_STATE.md` too
before touching any call-handling code — it's the single write-up of the
invariants `call-core`'s `call_arbitration` module enforces.

## Layout

- `android-app/` — Android app (Kotlin/Compose) for the device itself.
- `web-app/` — a browser page standing in for a second device, or a real
  calling option for a family where only one side has a Portal, using the
  same Nostr relays and wire protocol as the real app.
- `call-core/` — shared Rust crate for the call/pairing-bootstrap state
  machine, compiled to a JNI library for `android-app` and a WASM module
  for `web-app` — one implementation instead of two hand-mirrored platform
  copies.
- `pake-bridge/` — shared Rust crate for the SPAKE2 passphrase-pairing
  crypto (trim/normalize, exchange, confirmation). A plain Rust dependency
  of `call-core`, not a separate JNI/WASM build of its own — both
  platforms reach it only through `call-core`.
- `tokens/` — shared design tokens (color/typography/spacing), built with
  Style Dictionary into both `android-app`'s generated Kotlin theme files
  and `web-app/tokens.css`, so the two clients stay visually in sync.
- `CALL_STATE.md` — the call state machine's invariants. Read it before
  changing either platform's call-handling code.

## Toolchain setup

- JDK 17 (`sourceCompatibility`/`kotlin.jvmTarget` in
  `android-app/app/build.gradle.kts`).
- Android NDK `27.3.13750724`, pinned in `build.gradle.kts`'s own
  `ndkVersion` — install via `sdkmanager --install "ndk;27.3.13750724"`.
- Rust targets for cross-compiling `call-core`/`pake-bridge`:
  ```
  rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android wasm32-unknown-unknown
  ```
- Node.js (LTS; this project doesn't pin an exact version) — needed for
  `npm`/`npx` to resolve when Style Dictionary regenerates design tokens.

With all of that in place, `./gradlew assembleDebug` from `android-app/`
builds the Rust crate, regenerates the design tokens, and compiles the app
in one step — no separate manual "build the Rust part first."

## Before you push

`git push` should run `cargo test --workspace` first and refuse to push if
it fails — one-time setup per clone, since git doesn't track `.git/hooks/`
itself:

```
git config core.hooksPath .githooks
```

See `.githooks/pre-push` — it only covers the Rust workspace (no
Android/NDK or web/wasm toolchain needed, so it works on any machine with
a plain Rust install) as a fast, local, *pre*-push layer in front of the
hosted CI (`.github/workflows/ci.yml`), which additionally does a full
Android cross-compile.

## Local test loop (web client)

Serve `web-app/` with any static file server, e.g.:

```
cd web-app && python3 -m http.server 8000
```

Open `http://localhost:8000/` — name it, then "Add contact" and enter the
exact same phrase on a device (or a second browser tab/profile, for a
same-machine two-party test) at the same time.

**After editing `app.js`, hard-refresh (not just reload).** `python3 -m
http.server` sends no cache-control headers, so the browser is free to
keep serving a stale cached copy of `app.js` from before your edit even
after restarting the server. Use a hard refresh (Cmd/Ctrl+Shift+R) or an
incognito/private window to be sure you're testing the current file.

## Rebuilding the WASM blob

`web-app/wasm/` (`call_core.js` + `call_core_bg.wasm`) is a prebuilt
artifact of the `call-core` crate, committed like any other prebuilt
dependency this project ships (e.g. Android's `.so` files arriving
prebuilt inside AARs). Only needs rebuilding after changing `call-core`'s
or `pake-bridge`'s source. One-time setup:

```
cargo install wasm-pack
rustup target add wasm32-unknown-unknown
```

Then, from `call-core/`:

```
wasm-pack build --target web --release
cp pkg/call_core.js pkg/call_core_bg.wasm ../web-app/wasm/
```

Not wired into any build step, since `web-app/` has no build step at all
otherwise and this project deliberately doesn't want to add one just for
this.

## Deployment

Sideload-only — this app is never going through the Play Store (a
security app called `dev.porchlight.app` isn't getting approved there),
so "deploying" means producing a signed APK and getting it onto each
device.

**Release signing.** `assembleRelease` falls back to the debug key if no
release keystore is configured, so it always produces *an* installable
APK — but for anything beyond your own dev device, set up a real keystore
first. In the gitignored `local.properties`, set `release.storeFile`,
`release.storePassword`, `release.keyAlias`, `release.keyPassword` (or the
matching `RELEASE_STORE_FILE`/`RELEASE_STORE_PASSWORD`/`RELEASE_KEY_ALIAS`/
`RELEASE_KEY_PASSWORD` env vars) — see `app/build.gradle.kts`. Keep that
keystore somewhere durable and back it up: losing it means every future
release is unverifiable as a continuation of prior ones, and Android
refuses to install an update signed by a different key over an existing
install.

**First install on a device** is manual either way — `adb install` from a
laptop, or transfer the APK by whatever means and tap through the
"install unknown app" prompt on-device. There's no way around a human
touching the device once, the first time.

**Routine updates after that** go through `UpdateChecker.kt`, so nobody
needs a laptop or ADB access to a device that might be in another room or
another house. It polls a GitHub repo's releases and, when a newer one is
found, downloads the APK and posts a notification — tapping it hands the
file to Android's own install-confirm prompt. To use it:

1. Set `porchlight.updateRepo=owner/repo` in `local.properties` (or pass
   `-PporchlightUpdateRepo=owner/repo`) before building — this becomes
   `BuildConfig.UPDATE_REPO`. Left unset, the whole feature is inert (no
   network calls, nothing checked) — this is the default for anyone not
   explicitly deploying to real devices.
2. Bump `versionCode` in `app/build.gradle.kts` for the new release.
3. Cut a GitHub release on that repo tagged **`v<versionCode>`** — a
   plain integer matching the bumped value exactly (e.g. `v3`), not
   semver. A tag that doesn't parse this way is silently ignored rather
   than breaking the check for everyone still on the working version.
4. Attach the signed release APK as a release asset named exactly
   **`app-release.apk`**. A release missing this exact name is likewise
   ignored.

Every device already running an update-repo-configured build checks on
launch and roughly every 12 hours after, and only ever notifies once per
version — no repeat nagging for a release nobody's acted on yet.

Self-updating this way needs the repo's `/releases/latest` API to be
reachable unauthenticated, which means the GitHub repo has to be public —
a private repo's release API returns 404 to an anonymous request, so
`UpdateChecker` would never find anything.

## Signaling design: why Nostr

This project split the problem in two from the start: **negotiation** (the
metadata needed for two devices to find each other and agree on a
connection) and **the call itself** (video/audio, always peer-to-peer).
That split is what makes the rest of this section bearable — the call is
always secure, and it's only the "how do we say hello" step that's ever
needed a compromise.

**The call itself is always secure.** WebRTC encrypts media end-to-end
between the two devices (DTLS-SRTP) — audio and video never touch a third
party, encrypted or not. The offer/answer messages that set up that
connection ride as Nostr events, signed by construction (every event
carries its sender's signature, checked before anything else touches it)
and additionally encrypted end-to-end (NIP-44) so relays only ever see
opaque ciphertext.

**Negotiation is where the compromise lives.** Before that peer-to-peer
link can even start, two devices need to (a) find each other and (b)
exchange enough network information to punch through NAT — which needs a
STUN server and only works at all if your ISP/router allows it (no TURN
fallback yet; see `WebRtcEngine.defaultIceServers` for where to add one).
Both of those need *some* third party in the loop, if only for a few
seconds per call — picking what that third party looks like has been the
real design problem, not the call itself. Three shapes were considered:

1. **One account, run by us.** The simplest experience, but it puts us
   permanently in the loop: every pairing, forever, depending on an
   account we maintain, asking users to trust us (not just the software)
   to keep it running honestly and not become the one place that can see
   when everyone's calls happen.
2. **Your own account, on a real platform (Ably).** What this project ran
   on for most of its life before this — nobody depended on our own
   infrastructure, and Ably is a real company evaluable on its own terms.
   The cost: everyone pairing still had to sign up with yet another third
   party first, and that party saw call-setup metadata for as long as the
   account existed.
3. **No account at all — Nostr.** What's shipped today. Nostr's identity
   model is just a public/private keypair, not an account with any
   provider. Every `Pairing` mints its own fresh keypair rather than one
   device-wide identity doing every job at once — compromising one
   contact's key never exposes any other. Messages get carried by any of
   several independent, free, permissionless relays instead of one
   company's servers — nobody signs up for anything, and no single
   operator sits in the loop.

**Why Nostr specifically, and not some other free serverless option:** not
because it's a more mature or purpose-built signaling transport — it
isn't; dedicated signaling servers and platforms like Matrix are more
battle-tested for calling specifically. It wins here because it bundles
everything this project's constraints require at once: self-sovereign
identity (the keypair *is* the account — no signup, unlike Matrix, which
needs one registered against a homeserver), encryption specified as part
of the protocol (NIP-44), and a set of independent public relays that are
free and redundant because they're subsidized by Nostr's social-network
use case, not by one operator's goodwill (unlike, say, PeerJS's free cloud
server, a single centralized instance explicitly not meant for production
use, with no identity or encryption layer of its own to build on). Losing
any one of those three would mean building it ourselves or reintroducing
an account somewhere.

**The resulting risk/security profile:**
- No admin burden and no third-party account for anyone to trust or
  maintain — identity is a fresh keypair generated on-device per pairing,
  never shared across contacts or transmitted anywhere except as that
  pairing's own public key.
- Relays still see call-setup metadata — who's talking to whom and roughly
  when. Never the content, which is end-to-end encrypted, and never media,
  which never transits it at all.
- Relays are open-write: anyone can address an event at any device's
  public key. This app never trusted the transport for identity anyway
  (see the pairing design below), so this mostly changes the shape of
  pairing-time noise (a stray/hostile guess at a pairing code shows up as
  a candidate to reject, not as access to anything) rather than opening a
  new hole.
- No single company to go down, get bought, change terms, or be pressured
  into blocking a specific account — relays are redundant and
  interchangeable by design, so the app talks to several at once rather
  than depending on one.
- The trade-off for that redundancy: relay reliability is volunteer-grade,
  not backed by any SLA — a relay can vanish or refuse traffic without
  notice.
- A native crypto dependency (secp256k1, via JNI, not pure Kotlin) —
  verified working on real Portal hardware, but it's still one more thing
  that could break on hardware not yet tested.

## Pairing protocol design

**A real password-authenticated key exchange (SPAKE2), not a shared-secret
lookup.** Anything derived *only* from a value a relay can see is
crackable offline by anyone who captured it, given enough guesses. SPAKE2
doesn't have that problem — each side combines fresh, *never-transmitted*
ephemeral randomness with the passphrase, so an eavesdropper watching the
entire exchange over the relay — even knowing the correct phrase — can't
derive the resulting session key from what it saw. Verifying a passphrase
guess requires actually completing a live, interactive exchange with a
real device, not a cheap offline check. This is the same lineage of
algorithm Magic Wormhole has run in production for years.

**Fully symmetric — no roles, nothing to compare.** Both devices run the
identical "Enter a phrase" screen; there's no generator/enterer
distinction to assign. Once both sides publish their SPAKE2 message at a
rendezvous point derived from the phrase, completing the exchange and
matching a key-confirmation tag *is* the proof both sides typed the same
words — there's no separate fingerprint ceremony to perform. The one
remaining human step is a lightweight "Pair with [name]?" tap, which
exists to catch the narrow case of confirming the wrong contact by
mistake, not to verify cryptographic identity.

**If more than one device answers the same phrase at once, pairing is
refused outright, not left to a choice.** A normal pairing is strictly
one-to-one, so a second distinct responder at the same rendezvous point is
already anomalous — coincidental phrase reuse by two unrelated pairs or a
deliberate collision attempt look identical from either side's
perspective, so both are handled the same way: discard the attempt
entirely and ask for a fresh phrase.

**A confirmed pairing can't be impersonated.** Since relays route messages
by public key rather than by a shared secret everyone on a channel holds,
forging traffic from a trusted peer would require breaking secp256k1
itself, not just capturing relay traffic. Every contact's traffic is also
gift-wrapped (a fresh one-time key per message) so a relay can't even see
which of your contacts you're talking to at any given moment, only that
*some* message passed through.

**What this doesn't cover, and why that's an accepted trade-off, not an
oversight:**
- **Compromise of the device itself** (physical or ADB/root access reading
  local storage directly) isn't defended against — a direct consequence
  of sideloading requiring USB debugging enabled. The web client has the
  same trade-off in a different shape: name and contacts (including each
  pairing's own private key) sit in the browser's plain `localStorage`,
  under the keys `porchlight-device-name` and `porchlight-pairings` —
  readable by anything with access to that browser profile, and gone if
  the site's storage is ever cleared. Nothing else — no server, no sync —
  ever has a copy.
- **Relays see call-setup metadata** — that a call happened and roughly
  when, and which public keys were involved — and could refuse to deliver
  messages, even though they can't forge a valid signed/encrypted
  offer/answer without either device's private key. Video and audio never
  touch them.
- **Google's public STUN servers** see enough network information for NAT
  traversal, the same category of trust already extended to them for ICE.
- **`spake2` (the underlying crate) has no independent security audit** —
  accepted given its wide use, active maintenance, and being the same
  lineage Magic Wormhole has run in production for years.

See [README.md](README.md) for project credits.
