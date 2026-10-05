#!/usr/bin/env node
// The weekly relay survey, end to end: discover candidates, probe them, soak the best (and always today's list) for a
// few hours, rank, and decide whether the relay list should change. See README.md and lib/select.mjs for the policy.
//
//   node weekly.mjs [--dry-run] [--soak-hours 3] [--soak-top 40] [--size 5] [--urls a,b] [--max 400] [--min-probed 50]
// (--urls restricts the candidates, for a targeted run; --min-probed lowers the sanity check's floor to match.)
// Environment, for tests: RELAYS_FILE and RELAY_HISTORY_FILE point at copies of web-app/relays.json and history.json.
//
// Writes out/weekly-report.md (what a pull request shows) and out/proposal.json. Unless --dry-run (or --urls), a changed list is
// written to web-app/relays.json (version + 1) and the run is added to history (tools/relay-survey/history.json).
// Exits 0 whatever the outcome; `changed`, `aborted` and `proposal` are also written to $GITHUB_OUTPUT when set.
import { readFileSync, writeFileSync, existsSync, rmSync, appendFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { discover } from './discover.mjs';
import { probe } from './probe.mjs';
import { soak } from './soak.mjs';
import { report } from './report.mjs';
import { outFile, readJsonl, ensureOut, log } from './lib/util.mjs';
import { DEFAULTS, summarize, addRun, sanity, selectList } from './lib/select.mjs';

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.join(here, '..', '..');
const relaysFile = process.env.RELAYS_FILE || path.join(repoRoot, 'web-app', 'relays.json');
const historyFile = process.env.RELAY_HISTORY_FILE || path.join(here, 'history.json');

const args = process.argv.slice(2);
const flag = (name) => args.includes(`--${name}`);
const opt = (name, dflt) => { const i = args.indexOf(`--${name}`); return i >= 0 && args[i + 1] && !args[i + 1].startsWith('--') ? args[i + 1] : dflt; };
// A targeted run (--urls) only sees the relays it was given: its results are not a survey, so it never writes.
const targeted = !!opt('urls', '');
const dryRun = flag('dry-run') || targeted;
const soakHours = Number(opt('soak-hours', 3));
const soakTop = Number(opt('soak-top', 40));
const size = Number(opt('size', DEFAULTS.size));
const minProbed = Number(opt('min-probed', DEFAULTS.minProbed));
const urlsOverride = opt('urls', '') ? opt('urls').split(',').map((s) => s.trim()).filter(Boolean) : null;

const readJson = (file, dflt) => { try { return JSON.parse(readFileSync(file, 'utf8')); } catch { return dflt; } };
const setOutput = (k, v) => { if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, `${k}=${String(v).replace(/\n/g, ' ')}\n`); };

const current = readJson(relaysFile, { version: 0, relays: [] });
if (!current.relays.length) throw new Error(`${relaysFile} has no relays`);

// A fresh run: the survey's own files accumulate otherwise.
ensureOut();
for (const f of ['probe.jsonl', 'soak.jsonl', 'survivors.json', 'candidates.json', 'ranked.json', 'report.md']) rmSync(outFile(f), { force: true });

log(`weekly survey: list v${current.version} (${current.relays.length} relays), soak ${soakHours} h on the best ${soakTop}, size ${size}${dryRun ? (targeted ? ', TARGETED RUN (nothing is written)' : ', DRY RUN') : ''}`);

// 1. candidates: what discovery finds, plus today's list so it is always measured
if (urlsOverride) writeFileSync(outFile('candidates.json'), JSON.stringify(urlsOverride));
else await discover({ max: Number(opt('max', 400)) });
const candidates = [...new Set([...current.relays, ...readJson(outFile('candidates.json'), [])])];
writeFileSync(outFile('candidates.json'), JSON.stringify(candidates, null, 2));

// 2. probe, then soak the best survivors and, whatever their probe result, today's relays
await probe({ concurrency: 10 });
const probes = readJsonl(outFile('probe.jsonl'));
const survivors = readJson(outFile('survivors.json'), []);
const soakList = [...new Set([...current.relays, ...survivors])].slice(0, Math.max(soakTop, current.relays.length));
await soak({ hours: soakHours, top: soakList.length, urls: soakList });

// 3. rank
await report({ pool: 20 });
const ranked = readJson(outFile('ranked.json'), { rows: [] });
const rows = ranked.rows;

// 4. decide
const history = readJson(historyFile, { runs: [] });
const probed = probes.length;
const probePassed = probes.filter((p) => p.ok).length;
const check = sanity({ probed, probePassed, passingRows: rows.filter((r) => r.passes).length }, { ...DEFAULTS, minProbed });
const date = new Date().toISOString();
let proposal;
let nextHistory = history;
if (!check.ok) {
  proposal = { aborted: true, reason: check.reason, changed: false, relays: current.relays, changes: [], warnings: [] };
  log(`ABORTED: ${check.reason}`);
} else {
  nextHistory = addRun(history, date, summarize(rows), DEFAULTS.keepRuns);
  const sel = selectList({ current: current.relays, rows, history: nextHistory, size });
  proposal = { aborted: false, ...sel };
}

// 5. report
const row = (u) => rows.find((r) => r.url === u);
const fmt = (u) => { const r = row(u); return r ? `| ${u.replace('wss://', '')} | ${r.software ?? ''} | ${r.uptimePct ?? '–'}% | ${r.deliveredPct ?? '–'}% | ${r.p50 ?? '–'} / ${r.p95 ?? '–'} ms | ${r.passes ? 'pass' : 'FAIL: ' + r.failedGates.join('; ')} |` : `| ${u.replace('wss://', '')} | | | | | not measured |`; };
const md = [];
md.push(`# Relay survey, ${date.slice(0, 10)}`, '');
if (proposal.aborted) {
  md.push(`**The survey was not trusted and the list is unchanged:** ${proposal.reason}.`, '');
} else if (proposal.changed) {
  md.push(`## Proposed change: list v${current.version} → v${current.version + 1}`, '');
  for (const c of proposal.changes) md.push(`- ${c.removed ? `replace \`${c.removed}\` with` : 'add'} \`${c.added}\` — ${c.why}`);
  md.push('');
} else {
  md.push(`The list stays as it is (v${current.version}): every relay in it passes, or none that fails has failed twice in a row yet.`, '');
}
if (proposal.warnings.length) md.push('## Warnings', '', ...proposal.warnings.map((w) => `- ${w}`), '');
md.push('## The list', '', '| relay | software | uptime | delivered | heartbeat p50 / p95 | result |', '|---|---|---|---|---|---|', ...proposal.relays.map(fmt), '');
md.push(`## This run`, '', `- ${probed} candidates probed, ${probePassed} passed; ${rows.length} soaked for ${soakHours} h, ${rows.filter((r) => r.passes).length} passed the gates.`, `- Gates: uptime ≥ 95%, accepted ≥ 99%, delivered ≥ 99%, keep-alive ≥ 98%, no rejected offer-sized events.`, `- Measured from the runner's network, one vantage point; history so far: ${nextHistory.runs.length} run(s).`, '');
const top = rows.filter((r) => r.passes && !proposal.relays.includes(r.url)).slice(0, 8);
if (top.length) md.push('## Best of the rest', '', '| relay | software | uptime | delivered | heartbeat p50 / p95 | result |', '|---|---|---|---|---|---|', ...top.map((r) => fmt(r.url)), '');
writeFileSync(outFile('weekly-report.md'), md.join('\n') + '\n');
writeFileSync(outFile('proposal.json'), JSON.stringify({ date, from: current, ...proposal }, null, 2));
log(`report -> out/weekly-report.md; ${proposal.aborted ? 'aborted' : proposal.changed ? `${proposal.changes.length} change(s) proposed` : 'no change'}`);

// 6. apply (never on a dry run)
if (!dryRun) {
  if (!proposal.aborted) writeFileSync(historyFile, JSON.stringify(nextHistory, null, 1) + '\n');
  if (proposal.changed) {
    writeFileSync(relaysFile, JSON.stringify({ version: current.version + 1, relays: proposal.relays }, null, 2) + '\n');
    log(`web-app/relays.json -> v${current.version + 1}`);
  }
}
setOutput('changed', proposal.changed && !dryRun ? 'true' : 'false');
setOutput('aborted', proposal.aborted && !dryRun ? 'true' : 'false');
setOutput('new_version', current.version + 1);
process.exit(0);
