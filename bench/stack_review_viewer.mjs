// Offline browser proof for stores checked by stack_review_bridge.py.
// Usage: node bench/stack_review_viewer.mjs CHRONOZARR_REPO RECEIPT.json [...]
import assert from 'node:assert/strict';
import { readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

const [repository, ...receipts] = process.argv.slice(2);
assert(repository && receipts.length, 'expected Chronozarr checkout and source-check receipts');
const js = path.resolve(repository, 'js');
const { chromium } = await import(pathToFileURL(path.join(js, 'node_modules/playwright/index.mjs')));
const { startStaticServer } = await import(pathToFileURL(path.join(js, 'support/static-server.js')));
const app = await startStaticServer(js);
const browser = await chromium.launch({
  headless: true,
  args: ['--use-gl=angle', '--use-angle=swiftshader', '--enable-unsafe-swiftshader', '--ignore-gpu-blocklist'],
});
const results = [];
try {
  for (const receiptFile of receipts) {
    const receipt = JSON.parse(await readFile(receiptFile, 'utf8'));
    const data = await startStaticServer(path.dirname(receipt.output));
    const context = await browser.newContext({ viewport: { width: 1280, height: 800 }, serviceWorkers: 'block' });
    await context.route((url) => !['localhost', '127.0.0.1'].includes(url.hostname), (route) => route.abort('blockedbyclient'));
    const page = await context.newPage();
    const errors = [];
    page.on('pageerror', (error) => errors.push(error.message));
    page.on('console', (message) => { if (message.type() === 'error') errors.push(message.text()); });
    try {
      const store = `${data.url}/${path.basename(receipt.output)}/`;
      await page.goto(`${app.url}/demo/index.html?store=${encodeURIComponent(store)}&t=0`);
      await page.waitForFunction(() => window.chronozarr?.ready);
      assert(await page.evaluate(() => window.chronozarr.ready), 'viewer failed to open');
      const checks = [];
      const selections = [...receipt.browser_pixel_checks,
        ...(receipt.browser_invalid_checks ?? []).map((check) => ({ ...check, valid: false }))];
      if (Number.isInteger(receipt.screenshot_t)) {
        selections.sort((a, b) => Number(a.t === receipt.screenshot_t) - Number(b.t === receipt.screenshot_t));
      }
      for (const expected of selections) {
        await page.evaluate((t) => {
          const viewer = window.chronozarr.viewer;
          viewer.lodOverride = 0;
          viewer.goToTime(t);
          viewer.endScrub();
          viewer.renderNow();
          window.stackReviewClick = null;
          viewer.hooks.click = (payload) => { window.stackReviewClick = payload; };
        }, expected.t);
        await page.waitForFunction((t) => {
          const viewer = window.chronozarr.viewer;
          return viewer.paintedT === t && viewer.renderNow().complete && viewer.shownFrame?.lod === 0;
        }, expected.t);
        const point = await page.evaluate(({ x, y }) => {
          const { camera, canvas } = window.chronozarr.viewer;
          const rect = canvas.getBoundingClientRect();
          return { x: rect.left + ((x + 0.5 - camera.cx) * camera.scale + canvas.width / 2) * rect.width / canvas.width,
            y: rect.top + ((y + 0.5 - camera.cy) * camera.scale + canvas.height / 2) * rect.height / canvas.height };
        }, expected);
        await page.mouse.click(point.x, point.y);
        await page.waitForFunction(() => window.stackReviewClick !== null);
        const actual = await page.evaluate(() => window.stackReviewClick);
        assert.equal(actual.t, expected.t);
        assert.equal(actual.lod, 0);
        assert.equal(actual.pixel.x, expected.x);
        assert.equal(actual.pixel.y, expected.y);
        assert.equal(actual.info.valid, expected.valid !== false);
        if (expected.valid !== false) {
          assert.deepEqual(actual.info.bands.map((b) => b.stored), expected.stored);
          actual.info.bands.forEach((band, i) => assert(
            Math.abs(band.value - expected.physical[i]) <= 1e-7 * Math.max(1, Math.abs(expected.physical[i])),
            `physical value mismatch at timestep ${expected.t}, band ${i}`,
          ));
        }
        checks.push({ t: actual.t, pixel: actual.pixel, valid: actual.info.valid,
          stored: expected.stored, physical: expected.physical });
      }
      assert(checks.length, 'no valid source pixel was available for a query');
      assert.deepEqual(errors, [], 'browser errors');
      const screenshot = path.join(path.dirname(receiptFile), `${path.basename(receiptFile, '.json')}.png`);
      await page.screenshot({ path: screenshot });
      results.push({ status: 'passed', store: receipt.output,
        timesteps_queried: checks.length, checks, browser_errors: errors,
        requests: data.requests.length, screenshot });
    } finally {
      await context.close();
      await data.close();
    }
  }
} finally {
  await browser.close();
  await app.close();
}
const result = { status: 'passed', scope: 'local static hosting, WebGL rendering, temporal navigation and click-to-query; external requests blocked', stores: results };
const output = path.join(path.dirname(receipts[0]), 'viewer-receipt.json');
await writeFile(output, `${JSON.stringify(result, null, 2)}\n`);
console.log(JSON.stringify(result, null, 2));
