# Relay survey, 2026-11-16

## Proposed change: list v4 → v5

- replace `wss://relay.bullishbounty.com` with `wss://nostrcity-club.fly.dev` — failed the gates (uptime 0.0%; accepted 0.0%; delivered 0.0%; keep-alive 0.0%), and again the run before

## The list

| relay | software | uptime | delivered | heartbeat p50 / p95 | result |
|---|---|---|---|---|---|
| relay.chorus.community | nostr-rs-relay | 100% | 100% | 97 / 100 ms | pass |
| relay.cyberguy.fyi | fastr | 100% | 100% | 93 / 111 ms | pass |
| nostr-01.uid.ovh | nostr-relay | 100% | 100% | 91 / 93 ms | pass |
| nostr.computingcache.com | magnitude-relay | 100% | 100% | 107 / 110 ms | pass |
| nostrcity-club.fly.dev | nostr-rs-relay | 100% | 100% | 3 / 4 ms | pass |

## This run

- 400 candidates probed, 179 passed; 40 soaked for 3 h, 29 passed the gates.
- Gates: uptime ≥ 95%, accepted ≥ 99%, delivered ≥ 99%, keep-alive ≥ 98%, no rejected offer-sized events.
- Measured from the runner's network, one vantage point; history so far: 8 run(s).

## Best of the rest

| relay | software | uptime | delivered | heartbeat p50 / p95 | result |
|---|---|---|---|---|---|
| nostr-rs-relay.dev.fedibtc.com | nostr-rs-relay | 100% | 100% | 2 / 4 ms | pass |
| dm-test-nostr-rs-42-disabled.samt.st | nostr-rs-relay | 100% | 100% | 12 / 15 ms | pass |
| bridge.tagomago.me | nostr-rs-relay | 100% | 100% | 14 / 19 ms | pass |
| relay-can.zombi.cloudrodion.com | nostr-rs-relay | 100% | 100% | 16 / 18 ms | pass |
| relay.bowlafterbowl.com | nostr-rs-relay | 100% | 100% | 31 / 32 ms | pass |
| relay-us.zombi.cloudrodion.com | nostr-rs-relay | 100% | 100% | 32 / 34 ms | pass |
| nostr-relay.corb.net | nostr-rs-relay | 100% | 100% | 46 / 67 ms | pass |
| nostr-relay.zimage.com | nostr-rs-relay | 100% | 100% | 66 / 68 ms | pass |

