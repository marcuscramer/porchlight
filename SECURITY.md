# Security model

Porchlight has no server of its own and no account system — this doc states
what that buys you, what a handful of third parties still see, and the
residual risks.

Porchlight is unaudited hobby software that is still being tested. It has had
no independent security review, and neither has the Meta Portal hardware it
mainly runs on, which is discontinued (Meta doesn't guarantee that any feature
or update will remain available). Read this as an honest description of the
design, not a guarantee.

## Reporting a vulnerability

Please report security problems privately, not in a public issue: use
**Report a vulnerability** on this repository's Security tab on GitHub.

## Assets & actors

- **You** — each device's own user. There's no separate "admin" role and
  nothing to log into.
- **A contact** — the person on the other end of a pairing. Identified only
  by that pairing's own keypair, never by a name, phone number, or account.
  A contact's name is whatever their device says it is, so it isn't proof of
  identity — but pairing shows it for you to check against who you expect.
  A contact also sees whether you're online or busy, and — during a call —
  your IP address, since media goes directly between the two devices.
- **Nostr relays** (`relay.damus.io`, `nos.lol`, `relay.primal.net`,
  `relay.snort.social`, `offchain.pub`) — independent, free, third-party
  infrastructure this project doesn't run or control. Carries
  pairing/call-setup messages only. Pairing messages are public: relays, and
  anyone watching them, see the pairing tag, the per-pairing public key, and
  the key-exchange messages themselves, but not the device names, which are
  sent encrypted. Once paired, call-setup messages are encrypted (see below).
- **STUN servers** (Google's public `stun` and `stun1`) — see network
  information during WebRTC's NAT-traversal (ICE) step only.
- **GitHub** — hosts the web app (GitHub Pages) and the Android releases the
  app updates itself from. Whoever controls those can ship you different
  code.
- **esm.sh and Google Fonts** — the web app loads its Nostr library from
  `esm.sh` (pinned to one version, but without an integrity hash) and its
  fonts from Google, so both see that you opened the page. A compromised
  library would run with access to the keys the web app keeps in the
  browser.
- **Sensitive assets** — the live audio/video of a call, and the passphrase
  typed during pairing.

## What protects you

- **Call media is always end-to-end encrypted.** WebRTC's mandatory
  DTLS-SRTP means audio/video travels directly between the two devices,
  never through a relay or any server.
- **A real password-authenticated key exchange for pairing (SPAKE2), not a
  shared-secret lookup.** Each side combines fresh, never-transmitted
  ephemeral randomness with the passphrase, so an eavesdropper watching the
  exchange — even knowing the correct phrase — can't derive the resulting
  session key from what it saw. Verifying a guess *through the exchange*
  requires completing a live exchange with a real device. A device also
  rejects its own message echoed back, which would otherwise let someone who
  never saw the phrase get accepted as the other party. (Guessing the
  phrase from the rendezvous tag is a separate matter — see the first row
  of "Threats & status" and the note on phrase strength below.) SPAKE2 is
  the same family of algorithm Magic Wormhole has run in production for
  years.
- **You confirm who you're pairing with.** Before a pairing completes, both
  devices show the other side's self-reported name ("Pair with …?") and
  nothing is saved until you tap Confirm. The name only arrives after the
  key exchange has been verified, so it can't be read by someone who merely
  watches the relays, and it can't be forged by someone who didn't take part
  in the exchange. Names are stripped of direction-override and zero-width
  characters so one can't be visually spoofed. It's a human check, not a
  cryptographic one: it works against someone who doesn't know what your
  contact's device is called.
- **A fresh keypair per pairing, not one device-wide identity.** Compromising
  one contact's key never exposes any other contact.
- **After pairing, signaling messages are signed and encrypted (NIP-44), and
  gift-wrapped.** Every offer/answer/ICE message is signed by its sender and
  encrypted so a relay only sees opaque ciphertext. (The pairing messages
  themselves, the SPAKE2 exchange and key confirmation, are signed but not
  encrypted: SPAKE2 is designed to be safe in the open, and there is no shared
  secret to encrypt to before it finishes. The one thing that shouldn't be
  public, each device's name, is sent only once the exchange has produced a
  shared key, encrypted under it and tied to that exchange and to its
  sender.) Gift-wrapping (a fresh one-time outer key per message) hides who
  *sent* each message. A relay still sees the
  recipient's per-pairing public key and when messages arrive, so it can
  tell that two keys are exchanging messages around the same time — it
  can't tell whose they are or what they say.
- **No account, no signup, with anyone.** Identity is a keypair generated
  on-device. There's no company operating a login system to breach, get
  bought, change its terms, or be pressured into blocking someone.
- **No cloud backup of keys on Android.** The app opts out of Android's
  automatic backup, so pairing keys aren't swept into a cloud backup.

## Choosing a passphrase

The phrase only needs to be known to the two of you, but it needs to be hard
to guess. The pairing tag that lets two devices find each other is derived
from the phrase, and that tag is visible to relays and anyone watching them,
so someone can test guesses against it offline. The tag is deliberately
expensive to compute (Argon2id, 64 MiB of memory and three passes, about
half a second on a Portal TV), which makes each guess cost real time and
memory — but a short or common phrase can still be recovered by a determined
attacker. Use something long and unpredictable — three or four unrelated
words is a sensible minimum — and never a common word, a name, or a short
number.

## Threats & status

| Threat | Mitigation | Residual risk |
|---|---|---|
| Someone watching relay traffic guesses a weak pairing passphrase from the public rendezvous tag, then joins your pairing attempt as the other party | The tag is a slow, memory-hard hash (Argon2id) of the phrase, which makes each offline guess expensive but doesn't stop guessing a weak phrase. SPAKE2 still stops anyone without the phrase. A second party showing up at the same rendezvous point is refused for everyone (see the collision row). And the "Pair with [name]?" screen is a human check: you only confirm if the name is the contact you expect. | A weak phrase can be recovered offline. The name check stops a blind attacker, who can't see your contact's name (it is sent encrypted) and has to guess it, but it is self-reported and unverified, so it won't stop one who knows what your contact's device is called. If your contact is simply late, the attacker has until they arrive. Mitigation is a long phrase and reading the name on that screen carefully; a stronger design (a rendezvous code separate from the password) is not implemented. |
| Someone guesses a pairing passphrase by trying it against a live attempt | SPAKE2 requires a live, interactive exchange per guess | A guess that lands mid-exchange shows up as a second candidate and gets rejected outright (see the collision case below), not silently paired. |
| Two unrelated pairs reuse the same phrase at once, or someone deliberately collides an attempt | Pairing is refused outright for everyone at that rendezvous point, not left to a choice | Both sides have to retry with a fresh phrase. Indistinguishable from a deliberate collision by design — treated the same either way. |
| A relay refuses to deliver, or goes offline | App talks to 5 independent relays at once, not one | A relay can't forge a valid signed/encrypted message without either device's private key, but could drop traffic. Relay reliability is volunteer-grade, no SLA. |
| A relay (or anyone watching it) observes traffic | NIP-44 encryption + gift-wrapping | Relays still see call-setup *metadata*: that messages reached a pairing's public key, and roughly when — never call content, and never media (which never transits a relay at all). During pairing they see the tag, the public key and the key exchange, but not the device names. |
| Relays keep signaling ciphertext and a key is compromised later | Media has forward secrecy (DTLS-SRTP) | Signaling is encrypted to each pairing's long-lived key, so it has no forward secrecy. Someone who later gets a pairing key and kept the relay traffic could read the old call-setup messages, which include IP addresses — never the audio/video. |
| A paired contact (or someone with their key) has auto-answer enabled on your Portal | Auto-answer is off by default and per contact, with a 5-second on-screen countdown before it picks up | A contact you've enabled it for can start your Portal's camera and microphone without anyone tapping Accept. Only enable it for contacts you'd trust that far. |
| Device compromise (physical access, or ADB/root reading local storage) | None — accepted trade-off | Sideloading requires USB debugging in the first place. Pairing keys sit in plain Android `SharedPreferences` (web: plain `localStorage`) — readable by anything with access to that app's storage. Post-compromise recovery is Delete + re-pair that one contact. |
| The web app's host or its libraries are compromised | The page restricts what it can load and connect to (Content-Security-Policy), and pins its Nostr library to one version | GitHub Pages, `esm.sh`, and Google Fonts are trusted to serve unmodified code. There's no integrity hash on the library, and code served by any of them runs with access to the web app's keys. |
| A malicious update | Android only installs an update signed with the same key as the installed app; updates come from this project's GitHub releases | Trust rests on GitHub and on the release signing key staying secret. |
| Google's public STUN servers | Same category of trust already extended to them for any WebRTC ICE negotiation | They see network information needed for NAT traversal. No TURN fallback is configured today (see `WebRtcEngine.defaultIceServers` for where to add one, if a network pairing ever needs it). |
| `spake2` (the underlying crate) has no independent security audit | Wide use, active maintenance, same family Magic Wormhole has run in production for years | Porchlight's own use of it, and the code around it, is unaudited too. Accepted for now. |

## Known gaps

- **No TURN fallback.** A call between two devices that are both behind
  symmetric/hard NAT at the same moment can fail to connect at all. In
  practice this needs *both* sides simultaneously affected — one side with
  ordinary NAT is enough for the call to succeed. The app says so when it
  can tell that's why a call failed.
- **No 2FA / recovery concept.** There's nothing to recover — a device that
  loses its own storage (factory reset, app data cleared) just re-pairs each
  contact from scratch. Nothing else — no server, no sync — ever holds a
  copy of the keys to restore from.
- **Sideloading is inherently a device-integrity trade-off.** Installing
  needs Developer Options/USB debugging, which is a strictly weaker device
  posture than a locked-down consumer device. This is a property of
  sideloading generally, not specific to this app.
- **Optional setup steps weaken the device further.** To let the app update
  itself, the README has you turn off Meta's install verifier and Android's
  package verification as a whole, so the device stops checking any
  sideloaded install. To let an incoming call show over the screensaver and
  switch the TV to the Portal, you switch on Porchlight's accessibility
  service with one `adb` command. The service is declared with no access to
  screen content and no events, and the app only uses it to press the Home
  key when a call rings. Android's accessibility mechanism is powerful in
  general, so this still means trusting the app's code and its updates, but it
  is a much narrower grant than a permission to change system settings, which
  Porchlight does not use. Both are optional, and skipping them costs only
  convenience.
- **The platform itself is unmaintained.** Meta Portal hardware is
  discontinued and Meta doesn't guarantee further updates, so any weakness in
  the underlying Android build stays unpatched, with or without Porchlight.

See [README.md](README.md) for what this app is and how to use it.
