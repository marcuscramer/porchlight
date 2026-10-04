#!/usr/bin/env node
// Relay survey for Porchlight — see README.md.
import { discover } from './discover.mjs';
import { probe } from './probe.mjs';
import { soak } from './soak.mjs';
import { report } from './report.mjs';
import { log } from './lib/util.mjs';

const [cmd = 'help', ...rest] = process.argv.slice(2);
const flags = { listUrl: [], file: [] };
for (let i = 0; i < rest.length; i++) {
  const a = rest[i];
  if (!a.startsWith('--')) continue;
  const key = a.slice(2).replace(/-([a-z])/g, (_, c) => c.toUpperCase());
  const val = rest[i + 1] && !rest[i + 1].startsWith('--') ? rest[++i] : true;
  if (Array.isArray(flags[key])) flags[key].push(val); else flags[key] = val;
}
const num = (v, d) => (v === undefined || v === true ? d : Number(v));
const urls = typeof flags.urls === 'string' ? flags.urls.split(',').map((s) => s.trim()).filter(Boolean) : undefined;

switch (cmd) {
  case 'discover':
    await discover({ listUrls: flags.listUrl.length ? flags.listUrl : undefined, files: flags.file, max: num(flags.max, 300) });
    break;
  case 'probe':
    await probe({ concurrency: num(flags.concurrency, 6), limit: num(flags.limit, Infinity), urls });
    break;
  case 'soak':
    await soak({ hours: num(flags.hours, 8), top: num(flags.top, 40), urls });
    break;
  case 'report':
    await report({ pool: num(flags.pool, 20) });
    break;
  case 'all': {
    await discover({ listUrls: flags.listUrl.length ? flags.listUrl : undefined, files: flags.file, max: num(flags.max, 300) });
    await probe({ concurrency: num(flags.concurrency, 6) });
    await soak({ hours: num(flags.hours, 8), top: num(flags.top, 40) });
    await report({ pool: num(flags.pool, 20) });
    break;
  }
  default:
    console.log(`usage: node survey.mjs <command> [options]
  discover [--list-url URL]... [--file PATH]... [--max N]   build out/candidates.json
  probe    [--concurrency 6] [--limit N] [--urls a,b]        quick check of every candidate -> out/survivors.json
  soak     [--hours 8] [--top 40] [--urls a,b]                hold connections and send heartbeat-like load
  report   [--pool 20]                                        rank everything -> out/report.md
  all      [--hours 8]                                        discover, probe, soak, report`);
}
log('done');
process.exit(0);
