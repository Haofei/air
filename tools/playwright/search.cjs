#!/usr/bin/env node
/* eslint-disable no-console */

const { chromium } = require('playwright');
const crypto = require('node:crypto');
const fs = require('node:fs/promises');
const path = require('node:path');

const SECOND_LEVEL_TLDS = new Set(['co.uk', 'org.uk', 'ac.uk', 'com.au', 'com.br', 'co.jp']);

const inputChunks = [];
process.stdin.on('data', (chunk) => inputChunks.push(chunk));
process.stdin.on('end', async () => {
  try {
    const input = JSON.parse(Buffer.concat(inputChunks).toString('utf8') || '{}');
    const output = await run(input);
    process.stdout.write(`${JSON.stringify(output)}\n`);
  } catch (error) {
    console.error(error && error.stack ? error.stack : String(error));
    process.exit(1);
  }
});

async function run(input) {
  const query = String(input.query || '').trim();
  if (!query) {
    throw new Error('input.query must be a non-empty string');
  }

  const maxResults = positiveInt(input.max_results, 5);
  const maxResultsPerQuery = positiveInt(input.max_results_per_query, 10);
  const maxPerDomain = positiveInt(input.max_per_domain, 2);
  const maxContentChars = positiveInt(input.max_content_chars, 12000);
  const navigationTimeoutMs = positiveInt(input.navigation_timeout_ms, 20000);
  const overallTimeoutMs = positiveInt(input.overall_timeout_ms, 120000);
  const searchDelayMs = nonNegativeInt(input.search_delay_ms, 0);
  const pageConcurrency = positiveInt(input.page_concurrency, 3);
  const retryCount = nonNegativeInt(input.retry_count, 1);
  const fetchPages = input.fetch_pages !== false;
  const searchBaseUrl = searchBaseUrlFromInput(input.search_base_url);
  const cacheDir = optionalCacheDir(input.cache_dir);
  const cacheTtlSeconds = nonNegativeInt(input.cache_ttl_seconds, 0);
  const queries = uniqueNonEmpty([query, ...stringArray(input.query_variants)]).slice(0, 8);
  const includeDomains = domainArray(input.include_domains);
  const excludeDomains = domainArray(input.exclude_domains);
  const requiredTerms = stringArray(input.required_terms).map((term) => term.toLowerCase());
  const excludeTerms = stringArray(input.exclude_terms).map((term) => term.toLowerCase());
  const deadlineAt = overallTimeoutMs > 0 ? Date.now() + overallTimeoutMs : 0;

  const browser = await chromium.launch({ headless: true });
  try {
    const contextOptions = { locale: 'en-US' };
    if (input.user_agent) {
      contextOptions.userAgent = String(input.user_agent);
    }
    const context = await browser.newContext(contextOptions);
    await context.route('**/*', (route) => {
      const type = route.request().resourceType();
      if (['font', 'image', 'media', 'stylesheet'].includes(type)) {
        return route.abort().catch(() => {});
      }
      return route.continue().catch(() => {});
    });

    const searchRuns = [];
    for (let queryIndex = 0; queryIndex < queries.length; queryIndex += 1) {
      const currentQuery = queries[queryIndex];
      const queryStartedAt = Date.now();
      if (deadlineExceeded(deadlineAt)) {
        searchRuns.push({
          query: currentQuery,
          search_url: searchUrl(currentQuery, searchBaseUrl),
          ok: false,
          error: 'overall_timeout_exceeded',
          elapsed_ms: Date.now() - queryStartedAt,
          results: [],
        });
        continue;
      }
      const currentSearchUrl = searchUrl(currentQuery, searchBaseUrl);
      const timeoutMs = remainingTimeout(deadlineAt, navigationTimeoutMs);
      if (timeoutMs < 500) {
        searchRuns.push({
          query: currentQuery,
          search_url: currentSearchUrl,
          ok: false,
          error: 'overall_timeout_exceeded',
          elapsed_ms: Date.now() - queryStartedAt,
          results: [],
        });
        continue;
      }
      const page = await context.newPage();
      page.setDefaultNavigationTimeout(timeoutMs);
      page.setDefaultTimeout(timeoutMs);
      try {
        await page.goto(currentSearchUrl, { waitUntil: 'domcontentloaded', timeout: timeoutMs });
        const searchResult = await collectSearchResults(page);
        const results = searchResult.results;
        const title = await page.title().catch(() => '');
        const body_preview =
          results.length === 0
            ? await page
                .locator('body')
                .textContent({ timeout: Math.min(timeoutMs, 1000) })
                .then((text) => cleanText(text || '').slice(0, 300))
                .catch(() => '')
            : '';
        const result_container_preview =
          results.length === 0
            ? await page
                .locator('#b_results')
                .evaluate(
                  (node) => (node.innerHTML || '').replace(/\s+/g, ' ').trim().slice(0, 500),
                  undefined,
                  { timeout: Math.min(timeoutMs, 1000) }
                )
                .catch(() => '')
            : '';
        const challengeDetected =
          /captcha|verify you are human|unusual traffic|robot check|are you a robot|checking your browser|just a moment|access denied|ray id|blocked|challenge platform|cf-browser-verification/i.test(
            `${title} ${body_preview} ${result_container_preview}`
          );
        searchRuns.push({
          query: currentQuery,
          search_url: currentSearchUrl,
          ok: true,
          elapsed_ms: Date.now() - queryStartedAt,
          page_title: title,
          body_preview,
          result_container_preview,
          attempted_selectors: searchResult.selectors.map((selector) => selector.selector),
          matched_selector: searchResult.selectors.find((selector) => selector.count > 0)?.selector,
          warning: challengeDetected
            ? 'challenge_detected'
            : results.length === 0
              ? 'selector_mismatch'
              : undefined,
          results: results.slice(0, maxResultsPerQuery),
        });
      } catch (error) {
        searchRuns.push({
          query: currentQuery,
          search_url: currentSearchUrl,
          ok: false,
          elapsed_ms: Date.now() - queryStartedAt,
          error: errorMessage(error),
          results: [],
        });
      } finally {
        await page.close();
      }
      if (searchDelayMs > 0 && queryIndex + 1 < queries.length) {
        await delay(Math.min(searchDelayMs, remainingTimeout(deadlineAt, searchDelayMs)));
      }
    }

    const candidates = selectCandidates(searchRuns, {
      maxResults,
      maxPerDomain,
      includeDomains,
      excludeDomains,
      requiredTerms,
      excludeTerms,
    });

    const fetched = fetchPages
      ? await mapConcurrent(candidates, Math.min(pageConcurrency, 8), async (candidate) => {
          const pageResult = await fetchPageText(context, candidate.url, {
            navigationTimeoutMs,
            maxChars: maxContentChars,
            retryCount,
            deadlineAt,
            cacheDir,
            cacheTtlSeconds,
          });
          return { ...candidate, ...pageResult };
        })
      : candidates.map((candidate) => ({
          ...candidate,
          content: '',
          content_chars: 0,
          truncated: false,
          fetch_status: 'skipped',
        }));

    const documents = fetched.map((result) => ({
      id: stableId(result.url),
      title: result.title,
      url: result.url,
      domain: result.domain,
      domain_group: result.domain_group,
      query: result.query,
      result_rank: result.result_rank,
      snippet: result.snippet,
      content: result.content || result.snippet || result.title,
      content_chars: result.content_chars || 0,
      truncated: Boolean(result.truncated),
      fetch_status: result.fetch_status,
      fetch_attempts: result.fetch_attempts || 0,
      status: result.status,
      content_type: result.content_type,
      language: result.language,
      fetch_error: result.fetch_error,
      cache_hit: Boolean(result.cache_hit),
    }));

    const artifacts = documents.map((doc) => ({
      id: doc.id,
      kind: 'web_page',
      title: doc.title,
      uri: doc.url,
      content: doc.content,
      metadata: {
        provider: 'playwright_search',
        query: doc.query,
        domain: doc.domain,
        domain_group: doc.domain_group,
        result_rank: doc.result_rank,
        snippet: doc.snippet,
        content_chars: doc.content_chars,
        truncated: doc.truncated,
        fetch_status: doc.fetch_status,
        fetch_attempts: doc.fetch_attempts,
        status: doc.status,
        content_type: doc.content_type,
        language: doc.language,
        cache_hit: doc.cache_hit,
      },
    }));

    return {
      query,
      queries,
      search_urls: searchRuns.map((run) => run.search_url),
      warnings: searchRuns
        .filter((run) => run.warning)
        .map((run) => ({ query: run.query, warning: run.warning })),
      documents,
      artifacts,
      diagnostics: {
        search_runs: searchRuns.map((run) => ({
          query: run.query,
          search_url: run.search_url,
          ok: run.ok,
          error: run.error,
          warning: run.warning,
          elapsed_ms: run.elapsed_ms,
          page_title: run.page_title,
          body_preview: run.body_preview,
          result_container_preview: run.result_container_preview,
          attempted_selectors: run.attempted_selectors,
          matched_selector: run.matched_selector,
          result_count: run.results.length,
        })),
        candidate_count: candidates.length,
        document_count: documents.length,
        failed_fetch_count: documents.filter((doc) => doc.fetch_status === 'failed').length,
        max_per_domain: maxPerDomain,
        include_domains: includeDomains,
        exclude_domains: excludeDomains,
        required_terms: requiredTerms,
        exclude_terms: excludeTerms,
        overall_timeout_ms: overallTimeoutMs,
        search_delay_ms: searchDelayMs,
        retry_count: retryCount,
        search_base_url: searchBaseUrl,
        cache_enabled: Boolean(cacheDir && cacheTtlSeconds > 0),
        cache_ttl_seconds: cacheTtlSeconds,
      },
    };
  } finally {
    await browser.close();
  }
}

async function fetchPageText(context, url, options) {
  if (!isHttpUrl(url)) {
    return {
      content: '',
      content_chars: 0,
      truncated: false,
      fetch_status: 'failed',
      fetch_error: 'unsupported_url_scheme',
      fetch_attempts: 0,
      status: 0,
      content_type: '',
      language: '',
    };
  }
  const cached = await readPageCache(url, options.cacheDir, options.cacheTtlSeconds);
  if (cached) {
    return {
      ...cached,
      fetch_status: cached.fetch_status === 'ok' ? 'cache_hit' : cached.fetch_status,
      fetch_error: '',
      fetch_attempts: 0,
      cache_hit: true,
    };
  }
  let lastFailure = null;
  for (let attempt = 0; attempt <= options.retryCount; attempt += 1) {
    if (deadlineExceeded(options.deadlineAt)) {
      return {
        content: '',
        content_chars: 0,
        truncated: false,
        fetch_status: 'failed',
        fetch_error: 'overall_timeout_exceeded',
        fetch_attempts: attempt,
        status: 0,
        content_type: '',
        language: '',
      };
    }
    const result = await fetchPageTextOnce(context, url, options);
    if (result.fetch_status !== 'failed') {
      const output = { ...result, fetch_attempts: attempt + 1, cache_hit: false };
      await writePageCache(url, output, options.cacheDir, options.cacheTtlSeconds);
      return output;
    }
    lastFailure = result;
    if (attempt < options.retryCount) {
      await delay(Math.min(30_000, 500 * 2 ** attempt));
    }
  }
  return { ...lastFailure, fetch_attempts: options.retryCount + 1, cache_hit: false };
}

async function fetchPageTextOnce(context, url, options) {
  const page = await context.newPage();
  try {
    const timeoutMs = remainingTimeout(options.deadlineAt, options.navigationTimeoutMs);
    if (timeoutMs < 500) {
      return {
        content: '',
        content_chars: 0,
        truncated: false,
        fetch_status: 'failed',
        fetch_error: 'overall_timeout_exceeded',
        status: 0,
        content_type: '',
        language: '',
      };
    }
    page.setDefaultNavigationTimeout(timeoutMs);
    page.setDefaultTimeout(timeoutMs);
    const response = await page.goto(url, { waitUntil: 'domcontentloaded', timeout: timeoutMs });
    const contentType = response ? response.headers()['content-type'] || '' : '';
    const status = response ? response.status() : 0;
    if (status && (status < 200 || status >= 300)) {
      return {
        content: '',
        content_chars: 0,
        truncated: false,
        fetch_status: 'non_ok_status',
        fetch_error: `HTTP ${status}`,
        status,
        content_type: contentType,
        language: '',
      };
    }
    if (contentType && !/text\/html|application\/xhtml\+xml/i.test(contentType)) {
      return {
        content: '',
        content_chars: 0,
        truncated: false,
        fetch_status: 'non_html',
        fetch_error: contentType ? `unsupported content type: ${contentType}` : 'unsupported content type',
        status,
        content_type: contentType,
        language: '',
      };
    }
    const text = await page.evaluate(() => {
      for (const selector of [
        'script',
        'style',
        'noscript',
        'svg',
        'iframe',
        'template',
        'nav',
        'header',
        'footer',
        'aside',
        '[role="dialog"]',
        '[id*="cookie" i]',
        '[class*="cookie" i]',
        '[class*="consent" i]',
        '[id*="consent" i]',
      ]) {
        for (const node of Array.from(document.querySelectorAll(selector))) {
          node.remove();
        }
      }
      const main =
        document.querySelector('main') ||
        document.querySelector('article') ||
        document.querySelector('[role="main"]') ||
        document.body;
      return main ? main.innerText || main.textContent || '' : '';
    });
    const language = await page.evaluate(() => document.documentElement.lang || '').catch(() => '');
    const cleaned = cleanText(text || '');
    return {
      content: cleaned.slice(0, options.maxChars),
      content_chars: Math.min(cleaned.length, options.maxChars),
      truncated: cleaned.length > options.maxChars,
      fetch_status: cleaned ? 'ok' : 'empty',
      status,
      content_type: contentType,
      language,
      fetch_error: '',
    };
  } catch (error) {
    return {
      content: '',
      content_chars: 0,
      truncated: false,
      fetch_status: 'failed',
      fetch_error: errorMessage(error),
      status: 0,
      content_type: '',
      language: '',
    };
  } finally {
    await page.close();
  }
}

async function collectSearchResults(page) {
  const extraction = await page.evaluate(() => {
    const resultSelectors = [
      '#b_results li.b_algo h2 a',
      '#b_results h2 a',
      'li.b_algo h2 a',
      'main h2 a',
    ];
    const selectorCounts = resultSelectors.map((selector) => ({
      selector,
      count: document.querySelectorAll(selector).length,
    }));
    const anchors = Array.from(
      document.querySelectorAll(resultSelectors.find((selector) => document.querySelector(selector)) || resultSelectors[0])
    );
    const seen = new Set();
    const results = anchors
      .map((anchor) => {
        const href = anchor.href || anchor.getAttribute('href') || '';
        const title = (anchor.textContent || '').replace(/\s+/g, ' ').trim();
        if (!href || !title || seen.has(href)) return null;
        seen.add(href);
        const container = anchor.closest('li.b_algo') || anchor.parentElement;
        const snippetNode = container
          ? container.querySelector('.b_caption p, .b_caption, .b_snippet')
          : null;
        const snippet = snippetNode
          ? (snippetNode.textContent || '').replace(/\s+/g, ' ').trim()
          : '';
        return { title, url: href, snippet };
      })
      .filter(Boolean);
    return { results, selectors: selectorCounts };
  });
  const results = extraction.results
    .map((result, index) => {
      const url = normalizeUrl(unwrapSearchRedirectUrl(result.url));
      if (!isHttpUrl(url) || isSearchEngineUrl(url)) return null;
      return {
        title: result.title,
        url,
        domain: hostname(url),
        domain_group: domainGroup(hostname(url)),
        snippet: result.snippet,
        result_rank: index + 1,
      };
    })
    .filter(Boolean);
  return {
    results,
    selectors: extraction.selectors,
  };
}

function selectCandidates(searchRuns, options) {
  const seenUrls = new Set();
  const domainCounts = new Map();
  const selected = [];
  const maxDepth = Math.max(0, ...searchRuns.map((run) => run.results.length));
  for (let depth = 0; depth < maxDepth; depth += 1) {
    for (const run of searchRuns) {
      const result = run.results[depth];
      if (!result) continue;
      if (selected.length >= options.maxResults) return selected;
      if (seenUrls.has(result.url)) continue;
      if (!domainAllowed(result.domain, options.includeDomains, options.excludeDomains)) continue;
      if (!termsAllowed(result, options.requiredTerms, options.excludeTerms)) continue;
      const domainKey = result.domain_group || result.domain;
      const domainCount = domainCounts.get(domainKey) || 0;
      if (domainCount >= options.maxPerDomain) continue;
      seenUrls.add(result.url);
      domainCounts.set(domainKey, domainCount + 1);
      selected.push({ ...result, query: run.query });
    }
  }
  return selected;
}

async function mapConcurrent(items, concurrency, fn) {
  const results = new Array(items.length);
  let index = 0;
  // JavaScript runs these workers on one event loop; the synchronous index increment assigns
  // each item exactly once before any awaited work yields.
  const workers = Array.from({ length: Math.max(1, Math.min(concurrency, items.length)) }, async () => {
    while (index < items.length) {
      const current = index;
      index += 1;
      try {
        results[current] = await fn(items[current], current);
      } catch (error) {
        results[current] = {
          ...items[current],
          content: '',
          content_chars: 0,
          truncated: false,
          fetch_status: 'failed',
          fetch_error: errorMessage(error),
          fetch_attempts: 0,
          status: 0,
          content_type: '',
          language: '',
        };
      }
    }
  });
  await Promise.all(workers);
  return results;
}

function cleanText(text) {
  return text.replace(/\s+/g, ' ').trim();
}

function searchBaseUrlFromInput(value) {
  const baseUrl = String(value || 'https://www.bing.com/search').trim();
  if (!isValidHttpUrl(baseUrl)) {
    throw new Error('input.search_base_url must be an http(s) URL when provided');
  }
  return baseUrl;
}

function optionalCacheDir(value) {
  const cacheDir = String(value || '').trim();
  if (!cacheDir) return '';
  if (path.isAbsolute(cacheDir)) return cacheDir;
  return path.resolve(process.cwd(), cacheDir);
}

function searchUrl(query, baseUrl) {
  const parsed = new URL(baseUrl);
  parsed.searchParams.set('q', query);
  return parsed.toString();
}

function unwrapSearchRedirectUrl(url) {
  try {
    const parsed = new URL(url);
    if (parsed.hostname.endsWith('bing.com') && parsed.pathname.includes('/ck/')) {
      const encoded = parsed.searchParams.get('u');
      if (encoded) {
        const decoded = decodeBingUrl(encoded);
        return isValidHttpUrl(decoded) ? decoded : url;
      }
    }
    if (parsed.hostname.endsWith('duckduckgo.com')) {
      const uddg = parsed.searchParams.get('uddg');
      return uddg ? decodeURIComponent(uddg) : url;
    }
    return url;
  } catch (_error) {
    return url;
  }
}

function normalizeUrl(url) {
  try {
    const parsed = new URL(url);
    parsed.hash = '';
    for (const param of Array.from(parsed.searchParams.keys())) {
      if (/^(utm_|fbclid$|gclid$|mc_cid$|mc_eid$)/i.test(param)) {
        parsed.searchParams.delete(param);
      }
    }
    return parsed.toString();
  } catch (_error) {
    return url;
  }
}

function isHttpUrl(url) {
  return url.startsWith('http://') || url.startsWith('https://');
}

function isValidHttpUrl(url) {
  try {
    new URL(url);
    return isHttpUrl(url);
  } catch (_error) {
    return false;
  }
}

function isSearchEngineUrl(url) {
  const host = hostname(url);
  return ['bing.com', 'www.bing.com', 'duckduckgo.com', 'www.google.com', 'google.com'].includes(host);
}

function hostname(url) {
  try {
    return new URL(url).hostname.toLowerCase().replace(/^www\./, '');
  } catch (_error) {
    return '';
  }
}

function domainGroup(domain) {
  const parts = domain.split('.').filter(Boolean);
  if (parts.length <= 2) return domain;
  const lastTwo = parts.slice(-2).join('.');
  if (SECOND_LEVEL_TLDS.has(lastTwo) && parts.length >= 3) {
    return parts.slice(-3).join('.');
  }
  return lastTwo;
}

function domainAllowed(domain, includeDomains, excludeDomains) {
  if (!domain) return false;
  if (excludeDomains.some((blocked) => domainMatches(domain, blocked))) return false;
  if (includeDomains.length === 0) return true;
  return includeDomains.some((allowed) => domainMatches(domain, allowed));
}

function domainMatches(domain, pattern) {
  const normalized = pattern.toLowerCase().replace(/^\*\./, '').replace(/^www\./, '');
  return domain === normalized || domain.endsWith(`.${normalized}`);
}

function termsAllowed(result, requiredTerms, excludeTerms) {
  const haystack = `${result.title} ${result.url} ${result.snippet}`.toLowerCase();
  if (requiredTerms.some((term) => !haystack.includes(term))) return false;
  if (excludeTerms.some((term) => haystack.includes(term))) return false;
  return true;
}

function decodeBingUrl(encoded) {
  try {
    const normalized = encoded.startsWith('a1') ? encoded.slice(2) : encoded;
    const padded = normalized + '='.repeat((4 - (normalized.length % 4)) % 4);
    return Buffer.from(padded.replace(/-/g, '+').replace(/_/g, '/'), 'base64').toString('utf8');
  } catch (_error) {
    return encoded;
  }
}

function stableId(url) {
  try {
    const parsed = new URL(url);
    const path = parsed.pathname.replace(/\/$/, '');
    const raw = `${parsed.hostname}${path}`.toLowerCase();
    const slug = raw.replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '').slice(0, 96);
    const querySuffix = parsed.search ? `-${hashId(parsed.search).slice(0, 8)}` : '';
    return `web:${slug || hashId(url)}${querySuffix}`;
  } catch (_error) {
    return `web:${hashId(url)}`;
  }
}

async function readPageCache(url, cacheDir, ttlSeconds) {
  if (!cacheDir || ttlSeconds <= 0) return null;
  try {
    const filePath = cachePath(cacheDir, url);
    const body = await fs.readFile(filePath, 'utf8');
    const entry = JSON.parse(body);
    if (entry.url !== url || !entry.cached_at) return null;
    const ageMs = Date.now() - Date.parse(entry.cached_at);
    if (!Number.isFinite(ageMs) || ageMs < 0 || ageMs > ttlSeconds * 1000) return null;
    return entry.result || null;
  } catch (_error) {
    return null;
  }
}

async function writePageCache(url, result, cacheDir, ttlSeconds) {
  if (!cacheDir || ttlSeconds <= 0 || result.fetch_status === 'failed') return;
  try {
    await fs.mkdir(cacheDir, { recursive: true });
    const entry = {
      url,
      cached_at: new Date().toISOString(),
      result: {
        content: result.content || '',
        content_chars: result.content_chars || 0,
        truncated: Boolean(result.truncated),
        fetch_status: result.fetch_status,
        status: result.status || 0,
        content_type: result.content_type || '',
        language: result.language || '',
      },
    };
    await fs.writeFile(cachePath(cacheDir, url), JSON.stringify(entry), 'utf8');
  } catch (_error) {
    // Cache is an optimization; search results should not fail because cache writes fail.
  }
}

function cachePath(cacheDir, url) {
  return path.join(cacheDir, `${hashId(url)}.json`);
}

function positiveInt(value, fallback) {
  const parsed = Number.parseInt(value, 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
}

function nonNegativeInt(value, fallback) {
  const parsed = Number.parseInt(value, 10);
  return Number.isFinite(parsed) && parsed >= 0 ? parsed : fallback;
}

function remainingTimeout(deadlineAt, fallbackMs) {
  if (!deadlineAt) return fallbackMs;
  return Math.max(1, Math.min(fallbackMs, deadlineAt - Date.now()));
}

function deadlineExceeded(deadlineAt) {
  return Boolean(deadlineAt && Date.now() >= deadlineAt);
}

function hashId(value) {
  return crypto.createHash('sha256').update(String(value)).digest('hex').slice(0, 24);
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function stringArray(value) {
  if (!Array.isArray(value)) return [];
  return value.map((item) => String(item || '').trim()).filter(Boolean);
}

function domainArray(value) {
  return stringArray(value)
    .map((domain) => domain.toLowerCase().replace(/^\*\./, '').replace(/^www\./, ''))
    .filter((domain) => !domain.includes('/') && !domain.includes(':'));
}

function uniqueNonEmpty(values) {
  const seen = new Set();
  const output = [];
  for (const value of values) {
    const normalized = String(value || '').replace(/\s+/g, ' ').trim();
    const key = normalized.toLowerCase();
    if (!normalized || seen.has(key)) continue;
    seen.add(key);
    output.push(normalized);
  }
  return output;
}

function errorMessage(error) {
  return String(error && error.message ? error.message : error).slice(0, 500);
}
