// Shared runner of the step 4 and step 5 checks: reads a .feature file, runs one check per
// scenario, prints "action | equation | known | equation value | measured | result".
// Scenario notes use the form "# Equation: ... | Known: ...", as in step2d.feature.
// A check returns { value, measured, pass }; pass is true, false or PENDING ("pending: full
// run"), which is neither a pass nor a miss. The exit code is 1 when any check misses.
import fs from 'node:fs';

export const PENDING = 'PENDING';

/** The scenarios of a feature file with their equation and known result. */
export function readFeature(file) {
  const text = fs.readFileSync(file, 'utf8');
  return text.split(/^\s*Scenario:\s*/m).slice(1).map((block) => {
    const title = block.split(/\r?\n/)[0].trim();
    const note = block.match(/#\s*Equation:\s*(.*?)\s*\|\s*Known:\s*(.*)/);
    return { title, equation: note ? note[1] : '-', known: note ? note[2].trim() : '-' };
  });
}

/** Runs every scenario of `file` against `table` (title -> check); returns the exit code. */
export function runFeature(step, file, table) {
  console.log('action | equation | known | equation value | measured | result');
  let failed = 0;
  let pending = 0;
  for (const scenario of readFeature(file)) {
    const check = table[scenario.title];
    let outcome;
    try {
      outcome = check ? check() : { value: '-', measured: 'no check for this scenario', pass: false };
    } catch (err) {
      outcome = { value: '-', measured: `error: ${err.message}`, pass: false };
    }
    const result = outcome.pass === PENDING ? PENDING : outcome.pass === true ? 'PASS' : 'FAIL';
    if (result === 'FAIL') failed += 1;
    if (result === PENDING) pending += 1;
    console.log(`${scenario.title} | ${scenario.equation} | ${scenario.known} | ${outcome.value} | ${outcome.measured} | ${result}`);
  }
  console.log(failed ? `${step}: ${failed} failed, ${pending} pending` : `${step}: all passed${pending ? `, ${pending} pending: full run` : ''}`);
  return failed ? 1 : 0;
}

/** Relative error |a - b| / |b| (absolute when b is 0). */
export const relErr = (a, b) => (b === 0 ? Math.abs(a) : Math.abs(a - b) / Math.abs(b));
