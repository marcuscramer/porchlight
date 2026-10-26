# Relay survey, 2026-10-26

The list stays as it is (v3): every relay in it passes, or none that fails has failed twice in a row yet.

## Warnings

- wss://relay.pyramid.li failed the gates (uptime 0.0%; accepted 0.0%; delivered 0.0%; keep-alive 0.0%) this run; kept for now, it is replaced if it fails again next run

## The list

| relay | software | uptime | delivered | heartbeat p50 / p95 | result |
|---|---|---|---|---|---|
| relay.chorus.community | nostr-rs-relay | 100% | 100% | 146 / 147 ms | pass |
| relay.cyberguy.fyi | fastr | 100% | 100% | 142 / 143 ms | pass |
| relay.pyramid.li |  | 0% | 0% | – / – ms | FAIL: uptime 0.0%; accepted 0.0%; delivered 0.0%; keep-alive 0.0% |
| nostr-01.uid.ovh | nostr-relay | 100% | 100% | 148 / 149 ms | pass |
| relay.bullishbounty.com | nostream.git | 100% | 100% | 210 / 214 ms | pass |

## This run

- 400 candidates probed, 185 passed; 40 soaked for 3 h, 32 passed the gates.
- Gates: uptime ≥ 95%, accepted ≥ 99%, delivered ≥ 99%, keep-alive ≥ 98%, no rejected offer-sized events.
- Measured from the runner's network, one vantage point; history so far: 5 run(s).

## Best of the rest

| relay | software | uptime | delivered | heartbeat p50 / p95 | result |
|---|---|---|---|---|---|
| nostr.tac.lol | nostr-rs-relay | 100% | 100% | 20 / 29 ms | pass |
| relay.aarpia.com | nostr-rs-relay | 100% | 100% | 25 / 28 ms | pass |
| nostrride.io |  | 100% | 100% | 28 / 30 ms | pass |
| nostr-relay.zimage.com | nostr-rs-relay | 100% | 100% | 34 / 35 ms | pass |
| nostr-relay.corb.net | nostr-rs-relay | 100% | 100% | 31 / 33 ms | pass |
| nostr.computingcache.com | magnitude-relay | 100% | 100% | 40 / 43 ms | pass |
| relay.bowlafterbowl.com | nostr-rs-relay | 100% | 100% | 43 / 44 ms | pass |
| relay.illuminodes.com | nostr-rs-relay | 100% | 100% | 10 / 49 ms | pass |

