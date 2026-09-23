# Security model

Porchlight has no server of its own and no account system — this doc states
what that buys you, what a handful of small third parties still see, and the
residual risks.

## Assets & actors

- **You** — each device's own user. There's no separate "admin" role and
  nothing to log into.
- **A contact** — the person on the other end of a pairing. Identified only
  by that pairing's own keypair, never by a name, phone number, or account.
- **Nostr relays** (`relay.damus.io`, `nos.lol`, `relay.primal.net`) —
  independent, free, third-party infrastructure this project doesn't run or
  control. Carries pairing/call-setup messages only.
- **STUN server** (Google's public STUN) — sees network information during
  WebRTC's NAT-traversal (ICE) step only.
- **Sensitive assets** — the live audio/video of a call, and the passphrase
  typed during pairing.

## What protects you

- **Call media is always end-to-end encrypted.** WebRTC's mandatory
  DTLS-SRTP means audio/video travels directly between the two devices,
  never through a relay or any server, encrypted or not.
- **A real password-authenticated key exchange for pairing (SPAKE2), not a
  shared-secret lookup.** Each side combines fresh, never-transmitted
  ephemeral randomness with the passphrase, so an eavesdropper watching the
  entire exchange — even knowing the correct phrase — can't derive the
  resulting session key from what it saw. Verifying a guess requires
  completing a live exchange with a real device, not a cheap offline check.
  Same lineage of algorithm Magic Wormhole has run in production for years.
- **A fresh keypair per pairing, not one device-wide identity.** Compromising
  one contact's key never exposes any other contact.
- **Signaling messages are signed and encrypted (NIP-44), and gift-wrapped.**
  Every offer/answer/ICE message is signed by its sender and encrypted so a
  relay only sees opaque ciphertext. Gift-wrapping (a fresh one-time outer
  key per message) additionally hides *which* contact you're talking to from
  relay observation — a relay sees that some message passed through, not
  who it's between.
- **No account, no signup, with anyone.** Identity is a keypair generated
  on-device. There's no company operating a login system to breach, get
  bought, change its terms, or be pressured into blocking someone.

## Threats & status

| Threat | Mitigation | Residual risk |
|---|---|---|
| Device compromise (physical access, or ADB/root reading local storage) | None — accepted trade-off | Sideloading requires USB debugging in the first place. Pairing keys sit in plain Android `SharedPreferences` (web: plain `localStorage`) — readable by anything with access to that app's storage. Post-compromise recovery is Delete + re-pair that one contact. |
| A relay refuses to deliver, or goes offline | App talks to 3 independent relays at once, not one | A relay can't forge a valid signed/encrypted message without either device's private key, but could drop traffic. Relay reliability is volunteer-grade, no SLA. |
| A relay (or anyone watching it) observes traffic | NIP-44 encryption + gift-wrapping | Relays still see call-setup *metadata* — that a message passed through, roughly when — never content, and never media (which never transits a relay at all). |
| Someone guesses a pairing passphrase | SPAKE2 requires a live, interactive exchange per guess — not crackable offline | A guess that lands mid-exchange shows up as a second candidate and gets rejected outright (see "collision" case below), not silently paired. |
| Two unrelated pairs reuse the same phrase at once, or someone deliberately collides an attempt | Pairing is refused outright for everyone at that rendezvous point, not left to a choice | Both sides have to retry with a fresh phrase. Indistinguishable from a deliberate collision by design — treated the same either way. |
| Google's public STUN server | Same category of trust already extended to it for any WebRTC ICE negotiation | Sees network information needed for NAT traversal. No TURN fallback configured today (see `WebRtcEngine.defaultIceServers` for where to add one, if a network pairing ever needs it). |
| `spake2` (the underlying crate) has no independent security audit | Wide use, active maintenance, same lineage Magic Wormhole has run in production for years | Accepted given the above. |

## Known gaps

- **No TURN fallback.** A call between two devices that are both behind
  symmetric/hard NAT at the same moment can fail to connect at all. In
  practice this needs *both* sides simultaneously affected — one side with
  ordinary NAT is enough for the call to succeed.
- **No 2FA / recovery concept.** There's nothing to recover — a device that
  loses its own storage (factory reset, app data cleared) just re-pairs each
  contact from scratch. Nothing else — no server, no sync — ever holds a
  copy of the keys to restore from.
- **Sideloading is inherently a device-integrity trade-off.** Developer
  Options/USB debugging has to stay enabled for self-update to keep working,
  which is a strictly weaker device posture than a locked-down consumer
  device. This is a property of sideloading generally, not specific to this
  app.

See [README.md](README.md) for what this app is and how to use it.
