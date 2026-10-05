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

Overnight on a Mac: `caffeinate -i node survey.mjs all --hours 8`. Candidate sources: a JSON list (`--list-url`, default nostr.watch, often down), a CSV (`--csv-url`, default the bitchat project's
relay list), NIP-66 monitor events from a few relays (`--nip66 wss://…`, default damus/oxtr/snort), `seed-relays.txt` and
`--file PATH`. Other options, `--urls a,b` restricts probe/soak to given relays, `--top N` (default 120) caps how many survivors are soaked, dropping the weakest by probe quality.

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


## The weekly job

`weekly.mjs` runs the whole survey and decides whether `web-app/relays.json` should change; the workflow
`.github/workflows/relay-survey.yml` runs it on GitHub and opens a pull request. Run it by hand from the Actions tab
("Run workflow"); the default is a dry run that only produces the report (run summary and a downloadable artifact).

```
node weekly.mjs --dry-run                       # locally: discover, probe, soak 3 h, report, decide
node weekly.mjs --dry-run --soak-hours 0.05 --urls a,b,... --min-probed 5    # a quick targeted run
npm test                                        # the selection policy's unit tests
```

Each run probes the candidates (the current list is always among them), soaks the best of them and today's relays,
ranks them with the same gates as before, and applies the policy in `lib/select.mjs`:

- A relay in the list stays while it passes. One bad run is a warning; it is replaced when it fails, or cannot be
  reached at all, in two runs in a row.
- A replacement has a clean record in the history (measured in at least two runs, passing the last ones); a relay
  measured only once may fill a slot if nothing better fits, and the report says so.
- Replacements never share a /24 network or registered domain with another relay in the list, and at most two relays
  run the same software.
- The list is only changed when something in it needs replacing. More than two replacements at once is allowed but
  flagged in the report.
- If the survey itself looks broken (few candidates probed or passing: usually the runner's network), the list is
  left alone and a GitHub issue is opened instead.

`history.json` is the results of earlier runs (the last eight); the file committed here is the starting point, and
the workflow keeps the running copy on the `relay-survey-data` branch so a week without a change leaves `main`
alone. A changed list becomes a pull request: `web-app/relays.json` with the version raised by one, plus the files
`npm run build:shared` generates from it. A person reads the report and merges it; Pages then serves the new list.

One setting is needed once: Settings > Actions > General > "Allow GitHub Actions to create and approve pull requests".
Pull requests opened with the default token do not start the CI workflow, which is why the job runs the policy's
unit tests itself. Measurements come from the runner's network (a datacenter), not from a home connection; compare
with a local run when a relay looks different.
