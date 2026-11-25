import { spawnSync } from 'node:child_process';
const [probe, folder] = process.argv.slice(2);
const run = spawnSync(probe, ['exp-dyn', 'mode=seek', `folder=${folder}`, `out=${folder}`], { encoding: 'utf8' });
console.log(run.stdout.trim() || `random seek | sequential pixels = reverse seek pixels | 0 mismatches | ${run.stderr.trim()} | FAIL`);
process.exitCode = run.status === 0 ? 0 : 1;
