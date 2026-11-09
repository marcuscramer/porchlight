# Relay survey, 2026-11-09

The list stays as it is (v4): every relay in it passes, or none that fails has failed twice in a row yet.

## Warnings

- wss://relay.bullishbounty.com failed the gates (uptime 0.0%; accepted 0.0%; delivered 0.0%; keep-alive 0.0%) this run; kept for now, it is replaced if it fails again next run

## The list

| relay | software | uptime | delivered | heartbeat p50 / p95 | result |
|---|---|---|---|---|---|
| relay.chorus.community | nostr-rs-relay | 100% | 100% | 159 / 176 ms | pass |
| relay.cyberguy.fyi | fastr | 100% | 100% | 150 / 162 ms | pass |
| nostr-01.uid.ovh | nostr-relay | 100% | 100% | 143 / 145 ms | pass |
| relay.bullishbounty.com |  | 0% | 0% | – / – ms | FAIL: uptime 0.0%; accepted 0.0%; delivered 0.0%; keep-alive 0.0% |
| nostr.computingcache.com | magnitude-relay | 100% | 100% | 36 / 39 ms | pass |

## This run

- 400 candidates probed, 188 passed; 40 soaked for 3 h, 29 passed the gates.
- Gates: uptime ≥ 95%, accepted ≥ 99%, delivered ≥ 99%, keep-alive ≥ 98%, no rejected offer-sized events.
- Measured from the runner's network, one vantage point; history so far: 7 run(s).

## Best of the rest

| relay | software | uptime | delivered | heartbeat p50 / p95 | result |
|---|---|---|---|---|---|
| relay.aarpia.com | nostr-rs-relay | 100% | 100% | 25 / 26 ms | pass |
| relay.nostrian-conquest.com | nostr-rs-relay | 100% | 100% | 30 / 32 ms | pass |
| nostr.tac.lol | nostr-rs-relay | 100% | 100% | 23 / 31 ms | pass |
| nostr-relay.corb.net | nostr-rs-relay | 100% | 100% | 32 / 33 ms | pass |
| nostr-relay.zimage.com | nostr-rs-relay | 100% | 100% | 36 / 37 ms | pass |
| relay-us.zombi.cloudrodion.com | nostr-rs-relay | 100% | 100% | 46 / 47 ms | pass |
| relay-can.zombi.cloudrodion.com | nostr-rs-relay | 100% | 100% | 66 / 67 ms | pass |
| relay.gulugulu.moe | nostr | 100% | 99.9% | 48 / 55 ms | pass |

