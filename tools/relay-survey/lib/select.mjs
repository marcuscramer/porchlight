// Chooses the relay list from this week's survey rows and the history of earlier ones. Pure functions, no I/O,
// so the policy is testable (test/select.test.mjs).
//
// The policy, in one place:
// - A relay in the current list stays while it passes. One failing week is a warning; it is only replaced when it
//   fails (or cannot be reached at all) in two runs in a row, so a bad night does not swap relays.
// - A replacement must pass this week and have a clean record: measured in at least two runs (three when there are
//   that many), passing all of the last few. If no such relay exists, a relay measured once may fill the slot, and
//   the report says so.
// - Replacements never share a /24 network or registered domain with another relay in the list, and at most
//   `maxPerSoftware` relays run the same software (a bug or policy change in one software must not take out the list).
// - The list is not shrunk, and not changed at all if nothing in it needs replacing.
// - More than `maxChanges` replacements is allowed but flagged; a person reads the PR.

export const DEFAULTS = {
  size: 5,
  maxChanges: 2,
  window: 4,
  removeAfter: 2,
  maxPerSoftware: 2,
  keepRuns: 8,
  minProbeShare: 0.25,
  minProbed: 50,
  minPassing: 3,
};

/** The rows of one survey as a url -> summary map (only relays that were measured). */
export function summarize(rows) {
  const out = {};
  for (const r of rows) {
    out[r.url] = {
      passes: !!r.passes, score: r.score ?? 0, failedGates: r.failedGates ?? [], uptimePct: r.uptimePct ?? null,
      deliveredPct: r.deliveredPct ?? null, acceptPct: r.acceptPct ?? null, p50: r.p50 ?? null, p95: r.p95 ?? null,
      software: r.software ?? null, net: r.net ?? null, domain: r.domain ?? null, hours: r.hours ?? null,
    };
  }
  return out;
}

/** history is { runs: [{ date, relays: { url: summary } }] }, oldest first. */
export function addRun(history, date, relays, keepRuns = DEFAULTS.keepRuns) {
  const runs = [...(history?.runs ?? []), { date, relays }];
  return { runs: runs.slice(-keepRuns) };
}

/** A relay's results over the last `window` runs, oldest first; `null` where it was not measured. */
function record(history, url, window) {
  return history.runs.slice(-window).map((run) => run.relays[url] ?? null);
}

/** Is this relay a safe pick? tier 'proven', 'unproven' (measured once, passed) or null. */
export function tier(history, url, opts = DEFAULTS) {
  const rec = record(history, url, opts.window).filter(Boolean);
  if (!rec.length || !rec[rec.length - 1].passes) return null;
  if (rec.length >= 2) {
    const need = Math.min(3, rec.length);
    return rec.filter((r) => r.passes).length >= need && rec.slice(-need).every((r) => r.passes) ? 'proven' : null;
  }
  return 'unproven';
}

/**
 * Has this relay failed in each of the last `n` runs? The newest run counts a relay that was not measured at all as
 * failing (unreachable); an earlier run only counts an explicit failing entry. A relay with no entry in an earlier run
 * was simply not in the list then (it was just added, or the history was reset), which is not a failure.
 */
function failedLast(history, url, n) {
  const runs = history.runs.slice(-n);
  if (runs.length < n) return false;
  const newestFailed = !runs[runs.length - 1].relays[url]?.passes;
  const earlierFailed = runs.slice(0, -1).every((run) => run.relays[url] && !run.relays[url].passes);
  return newestFailed && earlierFailed;
}

/**
 * Decides whether the survey itself looks trustworthy. A broken network on the runner makes every relay look bad;
 * changing the list then would be the wrong response.
 */
export function sanity({ probed, probePassed, passingRows }, opts = DEFAULTS) {
  if (probed < opts.minProbed) return { ok: false, reason: `only ${probed} candidates were probed (discovery probably failed)` };
  if (probePassed / probed < opts.minProbeShare) return { ok: false, reason: `only ${probePassed} of ${probed} candidates passed the probe (the runner's network is probably the problem)` };
  if (passingRows < opts.minPassing) return { ok: false, reason: `only ${passingRows} relays passed the soak gates` };
  return { ok: true };
}

/**
 * @param current  the relays now in relays.json
 * @param rows     this week's ranked rows (already added to `history` as its newest run)
 * @returns { relays, changed, changes: [{ removed, added, why }], warnings }
 */
export function selectList({ current, rows, history, size = DEFAULTS.size, maxChanges = DEFAULTS.maxChanges }, opts = DEFAULTS) {
  const byUrl = new Map(rows.map((r) => [r.url, r]));
  const warnings = [];
  const keep = [];
  const removed = [];
  for (const url of current) {
    const row = byUrl.get(url);
    if (row?.passes) { keep.push(url); continue; }
    const why = row ? `failed the gates (${(row.failedGates || []).join('; ') || 'unknown'})` : 'could not be measured (unreachable)';
    if (failedLast(history, url, opts.removeAfter)) removed.push({ url, why: `${why}, and again the run before` });
    else { keep.push(url); warnings.push(`${url} ${why} this run; kept for now, it is replaced if it fails again next run`); }
  }

  const chosenRows = () => keep.map((u) => byUrl.get(u)).filter(Boolean);
  const nets = () => new Set(chosenRows().map((r) => r.net).filter(Boolean));
  const domains = () => new Set(chosenRows().map((r) => r.domain).filter(Boolean));
  const softwareCount = (sw) => chosenRows().filter((r) => r.software && r.software === sw).length;
  const softwareInList = () => new Set(chosenRows().map((r) => r.software).filter(Boolean));

  const wanted = removed.length + Math.max(0, size - (keep.length + removed.length));
  const changes = [];
  const candidates = rows.filter((r) => r.passes && !current.includes(r.url));
  for (let i = 0; i < wanted; i++) {
    const usable = candidates
      .filter((r) => !keep.includes(r.url))
      .filter((r) => !(r.net && nets().has(r.net)) && !(r.domain && domains().has(r.domain)))
      .filter((r) => !(r.software && softwareCount(r.software) >= opts.maxPerSoftware))
      .map((r) => ({ r, t: tier(history, r.url, opts) }))
      .filter((x) => x.t);
    // proven relays first; among them, a software not yet in the list gets a small bonus on top of the score
    usable.sort((a, b) => (a.t === b.t ? 0 : a.t === 'proven' ? -1 : 1) || (b.r.score + (softwareInList().has(b.r.software) ? 0 : 3)) - (a.r.score + (softwareInList().has(a.r.software) ? 0 : 3)));
    const pick = usable[0];
    const gone = removed[i];
    if (!pick) {
      if (gone) { keep.push(gone.url); warnings.push(`${gone.url} ${gone.why}, but no acceptable replacement was found; it stays`); }
      continue;
    }
    if (pick.t === 'unproven') warnings.push(`${pick.r.url} was measured only once (no history yet); chosen because no relay with a clean record fit`);
    keep.push(pick.r.url);
    changes.push({ removed: gone?.url ?? null, added: pick.r.url, why: gone?.why ?? 'the list was below its target size' });
  }
  if (changes.length > maxChanges) warnings.push(`${changes.length} relays are replaced in one update (more than the usual ${maxChanges}); read the report carefully`);
  const relays = keep;
  const changed = changes.length > 0;
  return { relays, changed, changes, warnings };
}
