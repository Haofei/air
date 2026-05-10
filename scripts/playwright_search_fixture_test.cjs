#!/usr/bin/env node
/* eslint-disable no-console */

const fs = require('node:fs');
const http = require('node:http');
const os = require('node:os');
const path = require('node:path');
const { spawn } = require('node:child_process');
const { chromium } = require('playwright');

const browserPath = chromium.executablePath();
if (!fs.existsSync(browserPath)) {
  console.log(`[playwright-search-fixture] skipped: Chromium is not installed at ${browserPath}`);
  process.exit(0);
}

const root = path.resolve(__dirname, '..');

function listen(server) {
  return new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => resolve(server.address().port));
  });
}

function close(server) {
  return new Promise((resolve, reject) => {
    server.close((error) => (error ? reject(error) : resolve()));
  });
}

function runSearch(input) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, ['scripts/playwright_search.cjs'], {
      cwd: root,
      stdio: ['pipe', 'pipe', 'pipe'],
    });
    let stdout = '';
    let stderr = '';
    child.stdout.on('data', (chunk) => {
      stdout += chunk.toString('utf8');
    });
    child.stderr.on('data', (chunk) => {
      stderr += chunk.toString('utf8');
    });
    child.on('error', reject);
    child.on('close', (code) => {
      if (code !== 0) {
        reject(new Error(`playwright_search exited ${code}\n${stderr}`));
        return;
      }
      try {
        resolve(JSON.parse(stdout));
      } catch (error) {
        reject(new Error(`invalid JSON output: ${error.message}\nstdout:\n${stdout}\nstderr:\n${stderr}`));
      }
    });
    child.stdin.end(`${JSON.stringify(input)}\n`);
  });
}

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

async function main() {
  let port = 0;
  const server = http.createServer((request, response) => {
    const url = new URL(request.url, 'http://127.0.0.1');
    if (url.pathname === '/search') {
      response.setHeader('content-type', 'text/html; charset=utf-8');
      response.end(`<!doctype html>
<html lang="en">
  <head><title>Fixture Search</title></head>
  <body>
    <ol id="b_results">
      <li class="b_algo">
        <h2><a href="http://127.0.0.1:${port}/alpha?utm_source=test#ignored">Alpha Playwright Timeout</a></h2>
        <div class="b_caption"><p>Playwright timeout fixture result with relevant Node.js details.</p></div>
      </li>
      <li class="b_algo">
        <h2><a href="http://127.0.0.1:${port}/beta">Beta Browser Extraction</a></h2>
        <div class="b_snippet">Browser extraction fixture result for coding agents.</div>
      </li>
    </ol>
  </body>
</html>`);
      return;
    }
    if (url.pathname === '/alpha') {
      response.setHeader('content-type', 'text/html; charset=utf-8');
      response.end(`<!doctype html><html lang="en"><body><main>
        <h1>Alpha Playwright Timeout</h1>
        <p>Alpha page body about Playwright timeout handling in Node.js tools.</p>
      </main></body></html>`);
      return;
    }
    if (url.pathname === '/beta') {
      response.setHeader('content-type', 'text/html; charset=utf-8');
      response.end(`<!doctype html><html lang="en"><body><article>
        <h1>Beta Browser Extraction</h1>
        <p>Beta page body about browser-backed extraction and provenance artifacts.</p>
      </article></body></html>`);
      return;
    }
    response.statusCode = 404;
    response.end('not found');
  });

  try {
    port = await listen(server);
    const cacheDir = fs.mkdtempSync(path.join(os.tmpdir(), 'air-playwright-search-cache-'));
    const input = {
      query: 'Playwright timeout Node.js',
      search_base_url: `http://127.0.0.1:${port}/search`,
      cache_dir: cacheDir,
      cache_ttl_seconds: 3600,
      max_results: 2,
      max_results_per_query: 5,
      max_per_domain: 2,
      max_content_chars: 2000,
      navigation_timeout_ms: 5000,
      overall_timeout_ms: 30000,
      page_concurrency: 2,
      retry_count: 0,
      fetch_pages: true,
    };
    const output = await runSearch(input);

    assert(output.documents.length === 2, `expected 2 documents, got ${output.documents.length}`);
    assert(output.diagnostics.search_runs[0].result_count === 2, 'expected 2 extracted search results');
    assert(output.diagnostics.search_base_url === `http://127.0.0.1:${port}/search`, 'search_base_url diagnostic mismatch');
    assert(output.search_urls[0].startsWith(`http://127.0.0.1:${port}/search?`), 'search URL should use fixture base URL');
    assert(output.documents[0].fetch_status === 'ok', 'first document fetch should succeed');
    assert(output.documents[0].url === `http://127.0.0.1:${port}/alpha`, 'tracking params and hash should be stripped');
    assert(output.documents[0].content.includes('Alpha page body about Playwright timeout'), 'first page content missing');
    assert(output.artifacts[0].kind === 'web_page', 'expected web_page artifact');
    assert(output.artifacts[0].uri === output.documents[0].url, 'artifact URI should match normalized URL');
    assert(output.diagnostics.cache_enabled === true, 'cache should be enabled');

    const cachedOutput = await runSearch(input);
    assert(cachedOutput.documents[0].fetch_status === 'cache_hit', 'second run should use cached page content');
    assert(cachedOutput.documents[0].cache_hit === true, 'cache_hit flag should be true');
    assert(cachedOutput.documents[0].fetch_attempts === 0, 'cache hit should not count fetch attempts');
    console.log('[playwright-search-fixture] ok');
  } finally {
    await close(server);
  }
}

main().catch((error) => {
  console.error(error && error.stack ? error.stack : String(error));
  process.exit(1);
});
