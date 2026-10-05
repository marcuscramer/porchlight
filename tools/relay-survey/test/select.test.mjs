import test from 'node:test';
import assert from 'node:assert/strict';
import { summarize, addRun, tier, sanity, selectList, DEFAULTS } from '../lib/select.mjs';

const row = (name, extra = {}) => ({ url: `wss://${name}.example.com`, passes: true, score: 90, failedGates: [], net: `10.0.${name.length}.${name.charCodeAt(0)}`, domain: `${name}.example.com`, software: 'strfry', ...extra });
const url = (n) => `wss://${n}.example.com`;
const hist = (...runs) => ({ runs: runs.map((rows, i) => ({ date: `2026-01-0${i + 1}`, relays: summarize(rows) })) });

test('a healthy list is left alone', () => {
  const rows = ['a', 'b', 'c', 'd', 'e'].map((n, i) => row(n, { software: `s${i}` }));
  const r = selectList({ current: rows.map((x) => x.url), rows, history: hist(rows) });
  assert.equal(r.changed, false);
  assert.deepEqual(r.relays, rows.map((x) => x.url));
});

test('one failing run only warns; the second one in a row replaces the relay', () => {
  const good = ['a', 'b', 'c', 'd'].map((n, i) => row(n, { software: `s${i}` }));
  const cand = row('z', { software: 'sz' });
  const bad = row('e', { passes: false, failedGates: ['uptime 80%'], software: 's9' });
  const week1 = [...good, bad, cand];
  const week0 = [...good, row('e', { software: 's9' }), cand];
  const current = [...good, bad].map((x) => x.url);
  // first failure
  let r = selectList({ current, rows: week1, history: hist(week0, week1) });
  assert.equal(r.changed, false);
  assert.match(r.warnings[0], /kept for now/);
  // second failure in a row, candidate is proven (passed both runs)
  const week2 = week1;
  r = selectList({ current, rows: week2, history: hist(week0, week1, week2) });
  assert.equal(r.changed, true);
  assert.deepEqual(r.changes.map((c) => [c.removed, c.added]), [[url('e'), url('z')]]);
  assert.ok(r.relays.includes(url('z')) && !r.relays.includes(url('e')));
});

test('an unreachable relay (no row this run) is replaced once it also failed the run before', () => {
  const good = ['a', 'b', 'c', 'd'].map((n, i) => row(n, { software: `s${i}` }));
  const cand = row('z', { software: 'sz' });
  const before = [...good, cand, row('gone', { passes: false, failedGates: ['uptime 0%'] })];
  const now = [...good, cand];
  const current = [...good.map((x) => x.url), url('gone')];
  const r = selectList({ current, rows: now, history: hist(before, now) });
  assert.equal(r.changed, true);
  assert.equal(r.changes[0].removed, url('gone'));
  assert.match(r.changes[0].why, /unreachable/);
});

test('a relay with no earlier record (just added, or the history was reset) is not replaced after one bad run', () => {
  const good = ['a', 'b', 'c', 'd'].map((n, i) => row(n, { software: `s${i}` }));
  const cand = row('z', { software: 'sz' });
  const bad = row('new', { passes: false, failedGates: ['uptime 0%'], software: 's9' });
  const before = [...good, cand];
  const now = [...good, cand, bad];
  const r = selectList({ current: [...good, bad].map((x) => x.url), rows: now, history: hist(before, now) });
  assert.equal(r.changed, false);
  assert.match(r.warnings[0], /kept for now/);
});

test('replacements keep the list diverse: not the same network, domain or too much of one software', () => {
  const keepRows = [row('a', { software: 'x' }), row('b', { software: 'x' }), row('c', { software: 'y' }), row('d', { software: 'z' })];
  const dup = row('twin', { net: keepRows[0].net, software: 'w', score: 99 });
  const sameDomain = row('dom', { domain: keepRows[1].domain, software: 'w', score: 98 });
  const tooManyX = row('x3', { software: 'x', score: 97 });
  const ok = row('ok', { software: 'v', score: 70 });
  const bad = row('e', { passes: false, software: 'q' });
  const all = [...keepRows, dup, sameDomain, tooManyX, ok, bad];
  const h = hist(all, all);
  const r = selectList({ current: [...keepRows, bad].map((x) => x.url), rows: all, history: h });
  assert.deepEqual(r.changes.map((c) => c.added), [url('ok')]);
});

test('a relay with a clean record beats a higher-scoring one measured only once', () => {
  const keepRows = ['a', 'b', 'c', 'd'].map((n, i) => row(n, { software: `s${i}` }));
  const bad = row('e', { passes: false, software: 'q' });
  const proven = row('proven', { score: 80, software: 'p1' });
  const fresh = row('fresh', { score: 99, software: 'p2', net: '9.9.9.9', domain: 'fresh.example.net' });
  const old = [...keepRows, bad, proven];
  const now = [...keepRows, bad, proven, fresh];
  const r = selectList({ current: [...keepRows, bad].map((x) => x.url), rows: now, history: hist(old, old, now) });
  assert.equal(r.changes[0].added, url('proven'));
});

test('with no proven candidate an unproven one may fill the slot, and says so', () => {
  const keepRows = ['a', 'b', 'c', 'd'].map((n, i) => row(n, { software: `s${i}` }));
  const bad = row('e', { passes: false, software: 'q' });
  const fresh = row('fresh', { software: 'p2' });
  const old = [...keepRows, bad];
  const now = [...keepRows, bad, fresh];
  const r = selectList({ current: [...keepRows, bad].map((x) => x.url), rows: now, history: hist(old, now) });
  assert.equal(r.changes[0].added, url('fresh'));
  assert.ok(r.warnings.some((w) => /measured only once/.test(w)));
});

test('no replacement available: the failing relay stays', () => {
  const keepRows = ['a', 'b', 'c', 'd'].map((n, i) => row(n, { software: `s${i}` }));
  const bad = row('e', { passes: false });
  const old = [...keepRows, bad];
  const r = selectList({ current: [...keepRows, bad].map((x) => x.url), rows: old, history: hist(old, old) });
  assert.equal(r.changed, false);
  assert.ok(r.relays.includes(url('e')));
  assert.ok(r.warnings.some((w) => /no acceptable replacement/.test(w)));
});

test('many replacements at once are flagged', () => {
  const bads = ['a', 'b', 'c'].map((n) => row(n, { passes: false, software: `old${n}` }));
  const keepRows = ['d', 'e'].map((n, i) => row(n, { software: `s${i}` }));
  const cands = ['p', 'q', 'r'].map((n, i) => row(n, { software: `n${i}` }));
  const all = [...bads, ...keepRows, ...cands];
  const r = selectList({ current: [...bads, ...keepRows].map((x) => x.url), rows: all, history: hist(all, all) });
  assert.equal(r.changes.length, 3);
  assert.ok(r.warnings.some((w) => /more than the usual/.test(w)));
});

test('history keeps the last runs only and the tiers read it', () => {
  let h = { runs: [] };
  for (let i = 0; i < 10; i++) h = addRun(h, `d${i}`, summarize([row('a')]), 4);
  assert.equal(h.runs.length, 4);
  assert.equal(tier(h, url('a')), 'proven');
  assert.equal(tier(h, url('nope')), null);
  assert.equal(tier(hist([row('a')]), url('a')), 'unproven');
  assert.equal(tier(hist([row('a')], [row('a', { passes: false })]), url('a')), null);
});

test('the sanity check refuses to act on a broken survey', () => {
  assert.equal(sanity({ probed: 400, probePassed: 190, passingRows: 80 }).ok, true);
  assert.equal(sanity({ probed: 10, probePassed: 9, passingRows: 9 }).ok, false);
  assert.equal(sanity({ probed: 400, probePassed: 30, passingRows: 20 }).ok, false);
  assert.equal(sanity({ probed: 400, probePassed: 190, passingRows: 1 }).ok, false);
  assert.ok(DEFAULTS.size >= 5);
});
