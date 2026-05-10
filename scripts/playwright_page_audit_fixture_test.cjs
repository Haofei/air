#!/usr/bin/env node
/* eslint-disable no-console */

const assert = require('node:assert/strict');
const childProcess = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');

const root = path.resolve(__dirname, '..');
const fixtureDir = path.join(root, 'target', 'generated', 'page-audit-fixture');
const screenshotDir = path.join(root, 'target', 'generated', 'page-audit-fixture-shots');
fs.mkdirSync(fixtureDir, { recursive: true });
fs.mkdirSync(screenshotDir, { recursive: true });

const pagePath = path.join(fixtureDir, 'index.html');
fs.writeFileSync(
  pagePath,
  `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>AIR Page Audit Fixture</title>
  <style>
    body { margin: 0; font-family: system-ui, sans-serif; color: #111; background: #f7f7f8; }
    main { min-height: 100vh; display: grid; place-items: center; padding: 48px 24px; box-sizing: border-box; }
    section { width: min(760px, 100%); display: grid; gap: 20px; }
    h1 { font-size: clamp(34px, 6vw, 72px); margin: 0; line-height: 1; }
    p { font-size: 18px; line-height: 1.6; margin: 0; max-width: 62ch; }
    a { display: inline-flex; width: max-content; padding: 12px 18px; border-radius: 999px; background: #111; color: white; text-decoration: none; }
  </style>
</head>
<body>
  <main>
    <section>
      <h1>Rendered layout</h1>
      <p>This page is intentionally simple so the audit fixture can verify screenshots, text extraction, overflow checks, and overlap diagnostics.</p>
      <a href="#buy">Inspect</a>
    </section>
  </main>
</body>
</html>`,
  'utf8'
);

const child = childProcess.spawnSync(
  'node',
  [path.join(root, 'scripts', 'playwright_page_audit.cjs')],
  {
    input: JSON.stringify({
      path: pagePath,
      screenshot_dir: screenshotDir,
      viewports: [
        { label: 'desktop', width: 1024, height: 768 },
        { label: 'mobile', width: 390, height: 844 },
      ],
      required_text: ['Rendered layout'],
      forbidden_text: ['markdown fence'],
      navigation_timeout_ms: 10000,
    }),
    encoding: 'utf8',
    maxBuffer: 1024 * 1024,
  }
);

if (child.status !== 0) {
  process.stderr.write(child.stderr);
  process.exit(child.status || 1);
}

const output = JSON.parse(child.stdout);
assert.equal(output.success, true);
assert.equal(output.viewport_count, 2);
assert.deepEqual(output.missing_required_text, []);
assert.deepEqual(output.present_forbidden_text, []);
assert.equal(output.screenshot_paths.length, 2);
for (const screenshotPath of output.screenshot_paths) {
  assert.equal(fs.existsSync(screenshotPath), true, `missing screenshot ${screenshotPath}`);
  assert(fs.statSync(screenshotPath).size > 1000, `screenshot too small ${screenshotPath}`);
}
assert.equal(output.viewports[0].horizontal_overflow, false);
assert.equal(output.viewports[0].overlap_count, 0);
assert(output.viewports[0].text_preview.includes('Rendered layout'));

const failingChild = childProcess.spawnSync(
  'node',
  [path.join(root, 'scripts', 'playwright_page_audit.cjs')],
  {
    input: JSON.stringify({
      path: pagePath,
      screenshot_dir: screenshotDir,
      viewports: [{ label: 'desktop', width: 1024, height: 768 }],
      required_text: ['Missing product name'],
      forbidden_text: ['Rendered layout'],
      navigation_timeout_ms: 10000,
    }),
    encoding: 'utf8',
    maxBuffer: 1024 * 1024,
  }
);

if (failingChild.status !== 0) {
  process.stderr.write(failingChild.stderr);
  process.exit(failingChild.status || 1);
}

const failingOutput = JSON.parse(failingChild.stdout);
assert.equal(failingOutput.success, false);
assert.deepEqual(failingOutput.missing_required_text, ['Missing product name']);
assert.deepEqual(failingOutput.present_forbidden_text, ['Rendered layout']);
assert.deepEqual(failingOutput.diagnostics[0].missing_required_text, ['Missing product name']);
assert.deepEqual(failingOutput.diagnostics[0].present_forbidden_text, ['Rendered layout']);
console.log('[playwright-page-audit-fixture] ok');
