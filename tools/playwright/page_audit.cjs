#!/usr/bin/env node
/* eslint-disable no-console */

const { chromium } = require('playwright');
const crypto = require('node:crypto');
const fs = require('node:fs/promises');
const path = require('node:path');
const { pathToFileURL } = require('node:url');

const chunks = [];
process.stdin.on('data', (chunk) => chunks.push(chunk));
process.stdin.on('end', async () => {
  try {
    const input = JSON.parse(Buffer.concat(chunks).toString('utf8') || '{}');
    const output = await run(input);
    process.stdout.write(`${JSON.stringify(output)}\n`);
  } catch (error) {
    console.error(error && error.stack ? error.stack : String(error));
    process.exit(1);
  }
});

async function run(input) {
  const target = resolveTarget(input);
  const viewports = parseViewports(input.viewports);
  const screenshotDir = path.resolve(String(input.screenshot_dir || 'target/generated/page-audit'));
  const navigationTimeoutMs = positiveInt(input.navigation_timeout_ms, 10000);
  const maxTextChars = positiveInt(input.max_text_chars, 2000);
  const requiredText = stringArray(input.required_text);
  const forbiddenText = stringArray(input.forbidden_text);
  const requireCanvas = input.require_canvas === true;
  await fs.mkdir(screenshotDir, { recursive: true });

  const browser = await chromium.launch({ headless: true });
  const startedAt = Date.now();
  const viewportResults = [];
  const artifacts = [];
  try {
    const context = await browser.newContext({ locale: 'en-US' });
    for (const viewport of viewports) {
      const page = await context.newPage();
      const consoleErrors = [];
      const pageErrors = [];
      page.setDefaultNavigationTimeout(navigationTimeoutMs);
      page.setDefaultTimeout(navigationTimeoutMs);
      page.on('console', (message) => {
        if (message.type() === 'error') consoleErrors.push(message.text());
      });
      page.on('pageerror', (error) => pageErrors.push(error.message));
      await page.setViewportSize({ width: viewport.width, height: viewport.height });
      const auditStartedAt = Date.now();
      await page.goto(target.url, { waitUntil: 'networkidle', timeout: navigationTimeoutMs });
      const metrics = await page.evaluate((limit) => {
        const doc = document.documentElement;
        const body = document.body;
        const text = (body?.innerText || '').replace(/\s+/g, ' ').trim();
        const selectedElements = Array.from(
          document.querySelectorAll('a, button, [role="button"], h1, h2, h3, h4, p, li, label, input, textarea, select')
        );
        const visibleElements = selectedElements
          .map((element, index) => {
            const rect = element.getBoundingClientRect();
            const style = window.getComputedStyle(element);
            const rawText = (element.innerText || element.textContent || '').replace(/\s+/g, ' ').trim();
            const hasMeaningfulText = rawText.length > 0 && rawText.length <= 160;
            const clickable = ['A', 'BUTTON'].includes(element.tagName) || element.getAttribute('role') === 'button';
            const visible =
              rect.width >= 4 &&
              rect.height >= 4 &&
              style.visibility !== 'hidden' &&
              style.display !== 'none' &&
              Number(style.opacity || '1') > 0.01;
            if (!visible || (!hasMeaningfulText && !clickable)) return null;
            return {
              index,
              tag: element.tagName.toLowerCase(),
              text: rawText.slice(0, 80),
              clickable,
              rect: {
                x: Math.round(rect.x),
                y: Math.round(rect.y),
                width: Math.round(rect.width),
                height: Math.round(rect.height),
                right: Math.round(rect.right),
                bottom: Math.round(rect.bottom),
              },
            };
          })
          .filter(Boolean)
          .slice(0, 90);

        const overlaps = [];
        for (let i = 0; i < visibleElements.length; i += 1) {
          for (let j = i + 1; j < visibleElements.length; j += 1) {
            const a = visibleElements[i];
            const b = visibleElements[j];
            const nodeA = selectedElements[a.index];
            const nodeB = selectedElements[b.index];
            if (!nodeA || !nodeB || nodeA.contains(nodeB) || nodeB.contains(nodeA)) continue;
            const x = Math.max(0, Math.min(a.rect.right, b.rect.right) - Math.max(a.rect.x, b.rect.x));
            const y = Math.max(0, Math.min(a.rect.bottom, b.rect.bottom) - Math.max(a.rect.y, b.rect.y));
            const area = x * y;
            const minArea = Math.min(a.rect.width * a.rect.height, b.rect.width * b.rect.height);
            if (area >= 80 && area / Math.max(minArea, 1) > 0.18) {
              overlaps.push({ a, b, overlap_area: Math.round(area) });
              if (overlaps.length >= 12) break;
            }
          }
          if (overlaps.length >= 12) break;
        }

        const clientWidth = doc.clientWidth || window.innerWidth;
        const scrollWidth = Math.max(doc.scrollWidth || 0, body?.scrollWidth || 0);
        const canvases = Array.from(document.querySelectorAll('canvas')).map((canvas, index) => {
          const rect = canvas.getBoundingClientRect();
          const width = canvas.width || Math.round(rect.width);
          const height = canvas.height || Math.round(rect.height);
          let nonblank = false;
          let error = '';
          try {
            const context = canvas.getContext('2d', { willReadFrequently: true });
            if (!context || width <= 0 || height <= 0) {
              error = context ? 'empty_canvas_dimensions' : 'unsupported_canvas_context';
            } else {
              const xStep = Math.max(1, Math.floor(width / 16));
              const yStep = Math.max(1, Math.floor(height / 16));
              for (let y = 0; y < height && !nonblank; y += yStep) {
                for (let x = 0; x < width; x += xStep) {
                  const data = context.getImageData(x, y, 1, 1).data;
                  if (data[3] !== 0 || data[0] !== 0 || data[1] !== 0 || data[2] !== 0) {
                    nonblank = true;
                    break;
                  }
                }
              }
            }
          } catch (err) {
            error = err && err.message ? err.message : String(err);
          }
          return {
            index,
            width,
            height,
            rect: {
              x: Math.round(rect.x),
              y: Math.round(rect.y),
              width: Math.round(rect.width),
              height: Math.round(rect.height),
            },
            nonblank,
            error,
          };
        });
        return {
          title: document.title || '',
          h1: Array.from(document.querySelectorAll('h1')).map((node) => node.innerText.trim()).filter(Boolean),
          buttons: Array.from(document.querySelectorAll('button, a[role="button"], .button, .btn'))
            .map((node) => node.innerText.trim())
            .filter(Boolean)
            .slice(0, 12),
          text_preview: text.slice(0, limit),
          text_chars: text.length,
          body_height: Math.round(body?.scrollHeight || doc.scrollHeight || 0),
          client_width: clientWidth,
          scroll_width: scrollWidth,
          horizontal_overflow: scrollWidth > clientWidth + 2,
          overlap_count: overlaps.length,
          overlaps,
          canvas_count: canvases.length,
          nonblank_canvas_count: canvases.filter((canvas) => canvas.nonblank).length,
          blank_canvas_count: canvases.filter((canvas) => !canvas.nonblank && !canvas.error).length,
          canvas_error_count: canvases.filter((canvas) => canvas.error).length,
          canvases,
        };
      }, maxTextChars);

      const screenshotName = `${safeName(target.label)}-${viewport.width}x${viewport.height}-${stableId(target.url)}.png`;
      const screenshotPath = path.join(screenshotDir, screenshotName);
      await page.screenshot({ path: screenshotPath, fullPage: true });
      const screenshotStat = await fs.stat(screenshotPath);
      const result = {
        ...viewport,
        ...metrics,
        console_errors: consoleErrors.slice(0, 20),
        page_errors: pageErrors.slice(0, 20),
        screenshot_path: screenshotPath,
        screenshot_bytes: screenshotStat.size,
        elapsed_ms: Date.now() - auditStartedAt,
      };
      viewportResults.push(result);
      artifacts.push({
        id: `browser-screenshot:${viewport.width}x${viewport.height}:${stableId(target.url)}`,
        kind: 'browser_screenshot',
        title: `${target.label} ${viewport.width}x${viewport.height}`,
        uri: screenshotPath,
        content: '',
        metadata: {
          provider: 'playwright_page_audit',
          target: target.url,
          width: viewport.width,
          height: viewport.height,
          bytes: screenshotStat.size,
          horizontal_overflow: result.horizontal_overflow,
          overlap_count: result.overlap_count,
          canvas_count: result.canvas_count,
          nonblank_canvas_count: result.nonblank_canvas_count,
          console_error_count: result.console_errors.length,
          page_error_count: result.page_errors.length,
        },
      });
      await page.close();
    }
  } finally {
    await browser.close();
  }

  const combinedText = viewportResults
    .map((viewport) =>
      [
        viewport.title,
        ...(Array.isArray(viewport.h1) ? viewport.h1 : []),
        ...(Array.isArray(viewport.buttons) ? viewport.buttons : []),
        viewport.text_preview,
      ].join('\n')
    )
    .join('\n')
    .toLowerCase();
  const missingRequiredText = requiredText.filter((term) => !combinedText.includes(term.toLowerCase()));
  const presentForbiddenText = forbiddenText.filter((term) => combinedText.includes(term.toLowerCase()));
  const hasRequiredCanvas = !requireCanvas || viewportResults.some((viewport) => viewport.nonblank_canvas_count > 0);
  const diagnostics = viewportResults.map((viewport) => ({
    width: viewport.width,
    height: viewport.height,
    horizontal_overflow: viewport.horizontal_overflow,
    overlap_count: viewport.overlap_count,
    console_error_count: viewport.console_errors.length,
    page_error_count: viewport.page_errors.length,
    missing_required_text: missingRequiredText,
    present_forbidden_text: presentForbiddenText,
    require_canvas: requireCanvas,
    canvas_count: viewport.canvas_count,
    nonblank_canvas_count: viewport.nonblank_canvas_count,
    blank_canvas_count: viewport.blank_canvas_count,
    canvas_error_count: viewport.canvas_error_count,
    screenshot_path: viewport.screenshot_path,
  }));
  const success = viewportResults.every(
    (viewport) =>
      !viewport.horizontal_overflow &&
      viewport.overlap_count === 0 &&
      viewport.console_errors.length === 0 &&
      viewport.page_errors.length === 0
  ) && missingRequiredText.length === 0 && presentForbiddenText.length === 0 && hasRequiredCanvas;

  return {
    target: target.input,
    url: target.url,
    success,
    viewport_count: viewportResults.length,
    screenshot_paths: viewportResults.map((viewport) => viewport.screenshot_path),
    missing_required_text: missingRequiredText,
    present_forbidden_text: presentForbiddenText,
    require_canvas: requireCanvas,
    has_required_canvas: hasRequiredCanvas,
    viewports: viewportResults,
    diagnostics,
    elapsed_ms: Date.now() - startedAt,
    artifacts,
  };
}

function resolveTarget(input) {
  if (input.url) {
    const url = String(input.url).trim();
    if (!/^https?:\/\//.test(url) && !/^file:\/\//.test(url)) {
      throw new Error('input.url must start with http://, https://, or file://');
    }
    return { input: url, url, label: new URL(url).hostname || 'file' };
  }
  const rawPath = String(input.path || '').trim();
  if (!rawPath) throw new Error('input.path or input.url is required');
  const resolved = path.resolve(rawPath);
  return { input: rawPath, url: pathToFileURL(resolved).toString(), label: path.basename(resolved) };
}

function parseViewports(value) {
  const raw = Array.isArray(value) && value.length > 0 ? value : [{ width: 1440, height: 1000 }, { width: 390, height: 844 }];
  return raw.slice(0, 4).map((viewport, index) => ({
    label: String(viewport.label || `viewport_${index + 1}`),
    width: positiveInt(viewport.width, index === 0 ? 1440 : 390),
    height: positiveInt(viewport.height, index === 0 ? 1000 : 844),
  }));
}

function stringArray(value) {
  return Array.isArray(value)
    ? value.map((item) => String(item || '').trim()).filter(Boolean).slice(0, 20)
    : [];
}

function positiveInt(value, fallback) {
  const parsed = Number(value);
  if (!Number.isFinite(parsed) || parsed <= 0) return fallback;
  return Math.floor(parsed);
}

function stableId(value) {
  return crypto.createHash('sha1').update(value).digest('hex').slice(0, 12);
}

function safeName(value) {
  return String(value || 'page')
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 64) || 'page';
}
