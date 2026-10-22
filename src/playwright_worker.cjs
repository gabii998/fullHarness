// Private JSON-lines worker. Never evaluates agent-provided JavaScript.
const { createRequire } = require('node:module');
const readline = require('node:readline');
const path = require('node:path');
const load = createRequire(path.join(process.cwd(), 'package.json'));
let browser, page;
const refs = new Map();
async function describe(el) {
  return el.evaluate(e => {
    const testid = e.getAttribute('data-testid');
    if (testid) return { kind: 'testid', value: testid };
    const label = e.getAttribute('aria-label') || (e.labels && e.labels[0] && e.labels[0].innerText);
    if (label) return { kind: 'label', value: label };
    if (e.id) return { kind: 'css', value: '#' + CSS.escape(e.id) };
    const parts = [];
    for (let node = e; node && node.nodeType === 1; node = node.parentElement) {
      const tag = node.tagName.toLowerCase();
      const siblings = node.parentElement ? Array.from(node.parentElement.children).filter(n => n.tagName === node.tagName) : [node];
      parts.unshift(tag + ':nth-of-type(' + (siblings.indexOf(node) + 1) + ')');
    }
    return { kind: 'css', value: parts.join(' > ') };
  });
}
async function perform(a) {
  if (a.action === 'close') {
    if (browser) await browser.close();
    browser = page = undefined;
    refs.clear();
    return { ok: true };
  }
  if (!browser) {
    let chromium;
    try { ({ chromium } = load('playwright')); }
    catch { throw new Error('Instala Playwright en el proyecto: npm install --save-dev playwright; npx playwright install chromium'); }
    browser = await chromium.launch({ headless: true });
    const context = await browser.newContext({ viewport: { width: 1280, height: 720 }, acceptDownloads: false });
    page = await context.newPage();
    page.setDefaultTimeout(10000);
    page.setDefaultNavigationTimeout(20000);
    // Avoid page-controlled modal dialogs blocking the protocol.
    page.on('dialog', dialog => dialog.dismiss().catch(() => {}));
  }
  const locator = () => {
    if (a.target.startsWith('@')) {
      const result = refs.get(a.target);
      if (!result) throw new Error('Referencia desconocida; solicita snapshot de nuevo.');
      return result;
    }
    return page.locator(a.target);
  };
  let exportedLocator;
  if (['click', 'fill', 'press', 'assert_visible'].includes(a.action)) {
    exportedLocator = a.target.startsWith('@') ? await describe(locator()) : { kind: 'css', value: a.target };
  }
  switch (a.action) {
    case 'navigate':
      if (!/^https?:\/\//i.test(a.value)) throw new Error('Solo se permiten URLs HTTP/HTTPS.');
      refs.clear();
      await page.goto(a.value, { waitUntil: 'domcontentloaded' });
      break;
    case 'click': await locator().click(); refs.clear(); break;
    case 'fill': await locator().fill(a.value); break;
    case 'press': await locator().press(a.value); refs.clear(); break;
    case 'assert_text':
      if (!(await page.locator('body').innerText()).includes(a.value)) throw new Error('No se encontr? el texto esperado.');
      break;
    case 'assert_url':
      if (page.url() !== a.value) throw new Error('La URL no coincide con la esperada.');
      break;
    case 'assert_visible': await locator().waitFor({ state: 'visible' }); break;
    case 'snapshot': {
      refs.clear();
      const text = await page.locator('body').innerText({ timeout: 10000 });
      const candidates = page.locator('a,button,input,textarea,select,[role="button"]');
      const count = await candidates.count();
      const elements = [];
      for (let i = 0; i < count && elements.length < 30 && i < 200; i++) {
        const el = candidates.nth(i);
        if (!await el.isVisible()) continue;
        const ref = '@e' + (elements.length + 1);
        refs.set(ref, el);
        const label = await el.evaluate(e => e.getAttribute('aria-label') || e.innerText || e.getAttribute('placeholder') || e.getAttribute('name') || e.tagName);
        elements.push({ ref, label: Array.from(label).slice(0, 80).join('') });
      }
      return { ok: true, url: Array.from(page.url()).slice(0, 512).join(''), text: Array.from(text).slice(0, 3000).join(''), truncated: Array.from(text).length > 3000, elements };
    }
    case 'screenshot': await page.screenshot({ path: a.screenshot, fullPage: false, timeout: 10000 }); break;
    default: throw new Error('Acción desconocida.');
  }
  return { ok: true, locator: exportedLocator, url: Array.from(page.url()).slice(0, 512).join(''), title: (await page.title()).slice(0, 200) };
}
const input = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
(async () => {
  for await (const line of input) {
    try { console.log(JSON.stringify(await perform(JSON.parse(line)))); }
    catch (error) { console.log(JSON.stringify({ ok: false, error: String(error.message).slice(0, 1000) })); }
  }
  if (browser) await browser.close();
})().catch(() => process.exit(1));
