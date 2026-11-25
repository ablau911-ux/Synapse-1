// Builds r-texts.html: one screen text shown at R = 1, R = 0.9996 and R = 0.989 (simulation).
// R = 1 − edit distance / source length (exp_d.rs share_read). Error count E = round((1 − R) · N), verified below.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const SOURCE = `Order #A-48213   Status: Shipped   Placed: 2026-11-19 14:32
Customer: Dana Levi   Account ID: 7730-1295   Tier: Business
Shipping address: 18 Herzl Street, Apt 4, Haifa 3303118, Israel

Item                                Qty   Unit price      Total
Wireless keyboard (Hebrew/English)    2       $74.90    $149.80
USB-C docking station, 11-in-1        1      $189.00    $189.00
27" monitor, 2560 x 1440, 75 Hz       2      $312.50    $625.00
Monitor arm, dual, clamp mount        1       $68.40     $68.40
HDMI cable 2 m (pack of 3)            1       $21.99     $21.99

Subtotal: $1,054.19   VAT 18%: $189.75   Shipping: $0.00
Total charged: $1,243.94   Card ending 4417   Auth code 0B7Q2

Notes from support (ticket 55102):
The customer called on 20 Nov at 09:15 and asked to split the delivery.
Monitors ship from the central warehouse in Modiin; the rest ships
from Haifa. Tracking numbers: IL-883 120 457 and IL-883 120 961.
Expected delivery: 24 Nov between 08:00 and 13:00. If nobody is home,
the courier leaves a note and tries again the next working day.
Return policy: 14 days from delivery, unused items in original box.
Refunds go back to the same card within 5-7 business days.

Delivery history
19 Nov 14:40  Label created for part 1 (monitors), 2 boxes, 21.6 kg
19 Nov 18:05  Label created for part 2 (accessories), 1 box, 3.9 kg
20 Nov 07:30  Part 1 left the Modiin warehouse, truck 41-227-63
20 Nov 11:52  Part 2 scanned at the Haifa sorting center, belt 7
21 Nov 09:10  Part 1 arrived at the Haifa depot, shelf R-12
21 Nov 16:45  Customer chose a new window: 24 Nov, 08:00-13:00
22 Nov 08:20  Both parts loaded on route H-05, driver ID 3391
22 Nov 08:21  SMS sent to the customer with the driver's arrival link
22 Nov 12:47  Driver reported a closed gate; customer asked for 24 Nov
23 Nov 06:58  Route H-05 planned again, 31 stops, this order is stop 9

Internal log
14:32:07  payment.authorize   ok   amount=1243.94 currency=USD
14:32:09  stock.reserve       ok   sku=KB-HE-201 qty=2 bin=C14
14:32:09  stock.reserve       ok   sku=MN-27Q-75 qty=2 bin=A03
14:32:11  shipment.split      ok   parts=2 reason=customer_request
14:35:40  email.send          ok   template=order_confirmed
09:15:22  ticket.update       ok   id=55102 by=agent_noa
09:15:23  delivery.reschedule ok   window=24-11 08:00-13:00`;

// Lookalike confusions of blurred screen text (what a decoder's softened edges turn one glyph into).
const LOOK = { l: '1', 1: 'l', O: '0', 0: 'O', S: '5', 5: 'S', 8: '3', 3: '8', B: '8', e: 'c', c: 'e', a: 'o', n: 'h', h: 'b',
  i: 'l', I: 'l', '.': ',', ',': '.', ':': ';', u: 'v', m: 'n', t: 'f', g: 'q', 6: 'b', 9: 'g', 7: '1', 2: 'Z', E: 'F', D: '0', r: 'n', s: 'z' };

function editDistance(a, b) {
  let row = Array.from({ length: b.length + 1 }, (_, j) => j);
  for (let i = 0; i < a.length; i += 1) {
    let prev = row[0];
    row[0] = i + 1;
    for (let j = 0; j < b.length; j += 1) {
      const sub = prev + (a[i] === b[j] ? 0 : 1);
      prev = row[j + 1];
      row[j + 1] = Math.min(sub, row[j] + 1, prev + 1);
    }
  }
  return row[b.length];
}

// E errors spread evenly over the text, each on the nearest glyph that has a lookalike (deterministic).
function corrupt(text, count, fixed) {
  const chars = [...text];
  const marks = [];
  if (fixed) {
    for (const [at, to] of fixed) { marks.push({ at, from: chars[at], to }); chars[at] = to; }
  } else {
    let seed = 20261123;
    const rand = () => { seed = (seed * 1103515245 + 12345) % 2147483648; return seed / 2147483648; };
    const step = chars.length / count;
    for (let k = 0; k < count; k += 1) {
      let at = Math.floor(k * step + rand() * step * 0.8);
      while (at < chars.length && (!(chars[at] in LOOK) || marks.some((m) => Math.abs(m.at - at) < 3))) at += 1;
      marks.push({ at, from: chars[at], to: LOOK[chars[at]] });
      chars[at] = LOOK[chars[at]];
    }
  }
  return { text: chars.join(''), marks };
}

const N = [...SOURCE].length;
const oneAt = SOURCE.indexOf('$1,243.94') + 5; // the "3" of 1,243.94
const versions = [
  { key: 'r1', r: 1, ...corrupt(SOURCE, 0, []) },
  { key: 'r9996', r: 0.9996, ...corrupt(SOURCE, 1, [[oneAt, '8']]) },
  { key: 'r989', r: 0.989, ...corrupt(SOURCE, Math.round((1 - 0.989) * N)) },
];
for (const v of versions) {
  v.errors = editDistance(SOURCE, v.text);
  v.rMeasured = 1 - v.errors / N;
  if (v.errors !== v.marks.length) throw new Error(`${v.key}: edit distance ${v.errors} != ${v.marks.length} marks`);
  console.log(v.key, 'N', N, 'E', v.errors, 'R', v.rMeasured.toFixed(5));
}

const esc = (s) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
function render(v) {
  const byAt = new Map(v.marks.map((m) => [m.at, m]));
  return [...v.text].map((ch, i) => {
    const m = byAt.get(i);
    return m ? `<mark title="במקור: ${esc(m.from)}" data-from="${esc(m.from)}">${esc(ch)}</mark>` : esc(ch);
  }).join('');
}

const template = fs.readFileSync(path.join(HERE, 'template.html'), 'utf8');
const html = template
  .replaceAll('{{N}}', N.toLocaleString('en-US'))
  .replace('{{T_r1}}', render(versions[0]))
  .replace('{{T_r9996}}', render(versions[1]))
  .replace('{{T_r989}}', render(versions[2]))
  .replace('{{E_r989}}', String(versions[2].errors))
  .replace('{{R_r989}}', versions[2].rMeasured.toFixed(4))
  .replace('{{R_r9996}}', versions[1].rMeasured.toFixed(4))
  .replace('{{PER_LINE}}', Math.round(N / versions[2].errors).toString());
fs.writeFileSync(path.join(HERE, 'r-texts.html'), html);
console.log('wrote r-texts.html', html.length, 'bytes');
