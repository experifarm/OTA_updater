// Browser smoke test of the built single-file app (viz/dist/ota-explained.html).
// Usage: PLAYWRIGHT_CORE=/path/to/playwright-core node viz/web/scripts/browser-smoke.mjs [shots-dir]
// For every preset at both scales: loads, plays to the end, checks final receiver statuses and that no
// console errors occurred; saves screenshots (mid-run and final) for visual review.
import { mkdirSync } from 'node:fs';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { PRESETS } from '../src/presets.ts';

const pw = await import(process.env.PLAYWRIGHT_CORE ?? 'playwright-core');
const chromium = pw.chromium ?? pw.default.chromium;
const html = fileURLToPath(new URL('../../dist/ota-explained.html', import.meta.url));
const shots = process.argv[2] ?? fileURLToPath(new URL('../../../target/viz-shots', import.meta.url));
mkdirSync(shots, { recursive: true });

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1600, height: 1250 }, deviceScaleFactor: 1 });
const errors = [];
page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`));
page.on('console', (m) => m.type() === 'error' && errors.push(`console: ${m.text()}`));
await page.goto(`${pathToFileURL(html).href}#sandbox`);

const ready = () => page.waitForFunction(() => window.__ota?.ready(), null, { timeout: 30_000 });
let failures = 0;
for (const scale of ['toy', 'real']) {
  await page.selectOption('#scale', scale);
  for (const [i, preset] of PRESETS.entries()) {
    await page.locator('#presets button').nth(i).click();
    await page.waitForTimeout(80);
    await ready();
    const { end } = await page.evaluate(() => window.__ota.time());
    await page.evaluate((T) => window.__ota.seek(T), end * 0.35);
    await page.waitForTimeout(250);
    await page.screenshot({ path: `${shots}/${scale}-${preset.id}-mid.png` });
    await page.evaluate((T) => window.__ota.seek(T), end);
    await page.waitForTimeout(250);
    const statuses = await page.evaluate(() => window.__ota.statuses());
    await page.screenshot({ path: `${shots}/${scale}-${preset.id}-end.png` });
    const ok = JSON.stringify(statuses) === JSON.stringify(preset.expect);
    if (!ok) failures++;
    console.log(`${ok ? 'ok  ' : 'FAIL'} ${scale} ${preset.id.padEnd(8)} [${statuses.join(', ')}]`);
  }
}
// Language toggle renders Chinese copy.
await page.click('[data-lang="zh"]');
const title = await page.textContent('h1');
console.log(`zh title: ${title}`);
if (title !== 'OTA 升级，图解') failures++;
await page.screenshot({ path: `${shots}/zh.png` });
await page.click('[data-lang="en"]');

// Guided chapters: each renders a figure without errors; key interactions give the right outcome.
const check = async (label, ok) => {
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${label}`);
  if (!ok) failures++;
};
for (let n = 1; n <= 9; n++) {
  await page.evaluate((n) => (location.hash = `#learn/${n}`), n);
  await page.waitForFunction((n) => window.__ota.view().chapter === n && document.querySelector('.chapter .figure')?.children.length > 0, n);
  const inlineErrors = await page.$$eval('.error-inline', (els) => els.map((e) => e.textContent).filter(Boolean));
  await check(`chapter ${n} renders${inlineErrors.length ? `: ${inlineErrors.join('; ')}` : ''}`, inlineErrors.length === 0);
  await page.screenshot({ path: `${shots}/chapter-${n}.png`, fullPage: true });
}
const verdict = () => page.$eval('.figure .verdict', (e) => e.className);
// 5: losing m shards is fine, one more is fatal.
await page.evaluate(() => (location.hash = '#learn/5'));
await page.waitForSelector('.rs-shard');
await check('ch5 one lost shard rebuilt', (await verdict()).includes('good'));
await page.locator('.rs-shard').nth(0).click();
await check('ch5 two lost shards rebuilt', (await verdict()).includes('good'));
await page.locator('.rs-shard').nth(2).click();
await check('ch5 three lost shards → cannot rebuild', (await verdict()).includes('bad'));
await page.screenshot({ path: `${shots}/chapter-5-fail.png`, fullPage: true });
// 6: tampering breaks the signature.
await page.evaluate(() => (location.hash = '#learn/6'));
await page.waitForSelector('.manifest-card');
const cardState = () => page.$eval('.manifest-card', (e) => (e.classList.contains('valid') ? 'valid' : 'invalid'));
await check('ch6 genuine manifest verifies', (await cardState()) === 'valid');
await page.locator('.actions button').nth(1).click();
await check('ch6 changed version fails', (await cardState()) === 'invalid');
// 8: the stepper reaches "installed", and a flipped bit fails the CRC.
await page.evaluate(() => (location.hash = '#learn/8'));
await page.waitForSelector('.packet-bytes .chip');
await page.locator('.packet-bytes .chip').nth(30).click();
await check('ch8 flipped bit → CRC mismatch', (await verdict()).includes('bad'));
const stepButton = page.locator('.row-actions button.primary');
for (let i = 0; i < 1000; i++) {
  await stepButton.click();
  const [pos, total] = (await page.textContent('.row-actions .mono')).split('/');
  if (pos === total) break;
}
const lastEvents = await page.$$eval('.step-events li', (els) => els.map((e) => e.className));
const allText = await page.textContent('.figure');
await check('ch8 stepper ends installed', allText.includes('installed') || lastEvents.includes('ev-installed'));
await page.screenshot({ path: `${shots}/chapter-8-end.png`, fullPage: true });
// 9: a corrupted byte keeps slot A.
await page.evaluate(() => (location.hash = '#learn/9'));
await page.waitForSelector('.slots');
await check('ch9 clean slot B verifies', (await verdict()).includes('good'));
await page.locator('.figure .row-actions button').first().click();
await check('ch9 corrupted slot B rejected', (await verdict()).includes('bad'));
// Chinese chapters render too.
await page.click('[data-lang="zh"]');
for (const n of [5, 8]) {
  await page.evaluate((n) => (location.hash = `#learn/${n}`), n);
  await page.waitForFunction((n) => window.__ota.view().chapter === n, n);
  await page.waitForTimeout(150);
  await page.screenshot({ path: `${shots}/chapter-${n}-zh.png`, fullPage: true });
}
await check('zh chapter title', (await page.textContent('.chapter h2')) === '接收与合并');
await browser.close();

if (errors.length) console.error(errors.join('\n'));
if (failures || errors.length) {
  console.error(`${failures} failure(s), ${errors.length} console error(s)`);
  process.exit(1);
}
console.log(`screenshots in ${shots}`);
