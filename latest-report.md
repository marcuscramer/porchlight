# Relay survey, 2026-11-02

## Proposed change: list v3 → v4

- replace `wss://relay.pyramid.li` with `wss://nostr.computingcache.com` — failed the gates (uptime 0.0%; accepted 0.0%; delivered 0.0%; keep-alive 0.0%), and again the run before

## The list

| relay | software | uptime | delivered | heartbeat p50 / p95 | result |
|---|---|---|---|---|---|
| relay.chorus.community | nostr-rs-relay | 100% | 100% | 113 / 115 ms | pass |
| relay.cyberguy.fyi | fastr | 100% | 100% | 109 / 112 ms | pass |
| nostr-01.uid.ovh | nostr-relay | 100% | 100% | 101 / 103 ms | pass |
| relay.bullishbounty.com | nostream.git | 100% | 100% | 142 / 161 ms | pass |
| nostr.computingcache.com | magnitude-relay | 100% | 100% | 82 / 85 ms | pass |

## This run

- 400 candidates probed, 199 passed; 40 soaked for 3 h, 30 passed the gates.
- Gates: uptime ≥ 95%, accepted ≥ 99%, delivered ≥ 99%, keep-alive ≥ 98%, no rejected offer-sized events.
- Measured from the runner's network, one vantage point; history so far: 6 run(s).

## Best of the rest

| relay | software | uptime | delivered | heartbeat p50 / p95 | result |
|---|---|---|---|---|---|
| nostr-rs-relay.dev.fedibtc.com | nostr-rs-relay | 100% | 100% | 18 / 19 ms | pass |
| bridge.tagomago.me | nostr-rs-relay | 100% | 100% | 24 / 26 ms | pass |
| relay.tapestry.ninja | nostr-rs-relay | 100% | 100% | 25 / 26 ms | pass |
| relay.bowlafterbowl.com | nostr-rs-relay | 100% | 100% | 26 / 27 ms | pass |
| relay-us.zombi.cloudrodion.com | nostr-rs-relay | 100% | 100% | 24 / 28 ms | pass |
| relay.nostrian-conquest.com | nostr-rs-relay | 100% | 100% | 35 / 36 ms | pass |
| nostr.janx.com | nostr-rs-relay | 100% | 100% | 36 / 43 ms | pass |
| nostrcity-club.fly.dev | nostr-rs-relay | 100% | 100% | 45 / 46 ms | pass |

