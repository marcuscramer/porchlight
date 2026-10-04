# relay-survey

Measures public Nostr relays for Porchlight's signaling so the relay pool can be chosen from data.
No dependencies (Node 22+). It builds the same events the apps send, using the project's own core
(`web-app/wasm`), with throwaway keys; nothing from a real pairing is ever used.

```
node survey.mjs discover            # candidates: online lists (if reachable) + seed-relays.txt -> out/candidates.json
node survey.mjs probe               # quick check of each -> out/probe.jsonl, out/survivors.json
node survey.mjs soak --hours 8      # overnight: heartbeat-like load on the survivors -> out/soak.jsonl
node survey.mjs report --pool 20    # ranking + proposed pool -> out/report.md, out/ranked.json
node survey.mjs all --hours 8       # all of the above
```

Overnight on a Mac: `caffeinate -i node survey.mjs all --hours 8`. Options: `--list-url URL` and `--file PATH` add
candidate sources, `--urls a,b` restricts probe/soak to given relays, `--top N` caps how many survivors are soaked.

## What it checks

- **Probe:** the relay's NIP-11 limits (auth, payment, proof-of-work, restricted writes, message size); then, with one
  publishing and one subscribed connection, whether it accepts *and delivers* a heartbeat (~1.4 KB), offers of ~17, ~33
  and ~65 KB on the wire (a real offer is ~17 KB), and a pairing bootstrap event, and the delivery latency.
- **Soak:** per relay, 3 heartbeats 400 ms apart every 25 s, an offer-sized event every 10 min, and a keep-alive every
  29 s (the REQ/EOSE round trip nostr-tools uses). Drops are reconnected with backoff. Records connection uptime, drops,
  rejections with the relay's reason, delivery success and latency (p50/p95).
- **Report:** gates (uptime, accepted, delivered, keep-alive, no rejected offers), a score, and a proposed pool that never
  has two relays on the same /24 network or registered domain.

## Caveats

- One vantage point: your machine and network. Relays that rate-limit by IP, or whose DNS is IPv6-only where you have no
  IPv6 route (relay.primal.net did for the first run), look different from a phone's view. Run it from a second network
  before trusting a single failure.
- Not a browser: relays that require a browser `Origin` won't show up as working here.
- Ephemeral event kinds (20331/20336) are what the apps use; a relay may accept them but not deliver, which the probe
  and soak do check.
