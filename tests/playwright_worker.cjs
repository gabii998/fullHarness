// Protocol tests without installing Chromium: node --test tests/playwright_worker.cjs
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawn } = require('node:child_process');
const readline = require('node:readline');
const worker = fs.readFileSync(path.join(__dirname, '../src/playwright_worker.cjs'), 'utf8');

test('bounded protocol, references, interactions and session close', async () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'ascurra-playwright-test-'));
  const moduleDir = path.join(directory, 'node_modules', 'playwright');
  fs.mkdirSync(moduleDir, { recursive: true });
  fs.writeFileSync(path.join(moduleDir, 'index.js'), `
    let url = 'about:blank';
    let value = '';
    const el = {
      isVisible: async () => true,
      evaluate: async fn => fn({ getAttribute: key => key === 'data-testid' ? 'submit' : key === 'aria-label' ? 'Label'.repeat(100) : null, innerText: 'Demo' }),
      waitFor: async () => {},
      click: async () => { url += '/clicked'; },
      fill: async text => { value = text; },
      press: async () => { url += '/pressed'; },
    };
    const page = {
      setDefaultTimeout() {}, setDefaultNavigationTimeout() {}, on() {},
      goto: async target => { url = target; }, url: () => url,
      title: async () => value || 'Demo',
      locator: selector => selector === 'body'
        ? { innerText: async () => 'x'.repeat(6000) }
        : { ...el, count: async () => 100, nth: () => el },
      screenshot: async () => {},
    };
    exports.chromium = { launch: async () => ({
      newContext: async () => ({ newPage: async () => page }), close: async () => {},
    }) };
  `);
  const child = spawn(process.execPath, ['-e', worker], { cwd: directory, stdio: ['pipe', 'pipe', 'pipe'] });
  const lines = readline.createInterface({ input: child.stdout })[Symbol.asyncIterator]();
  async function call(action, target = '', value = '') {
    child.stdin.write(JSON.stringify({ action, target, value }) + '\n');
    const response = await lines.next();
    assert.equal(response.done, false);
    return JSON.parse(response.value);
  }
  try {
    assert.equal((await call('navigate', '', 'https://example.test')).ok, true);
    const snapshot = await call('snapshot');
    assert.equal(snapshot.text.length, 3000);
    assert.equal(snapshot.truncated, true);
    assert.equal(snapshot.elements.length, 30);
    assert.equal(snapshot.elements[0].label.length, 80);
    const filled = await call('fill', '@e1', 'demo');
    assert.equal(filled.title, 'demo');
    assert.deepEqual(filled.locator, { kind: 'testid', value: 'submit' });
    assert.equal((await call('click', '@e2')).url, 'https://example.test/clicked');
    assert.equal((await call('click', '@e2')).ok, false, 'click invalidates references');
    assert.equal((await call('press', 'input', 'Enter')).ok, true);
    assert.equal((await call('assert_text', '', 'xxx')).ok, true);
    assert.equal((await call('assert_text', '', 'missing')).ok, false);
    assert.equal((await call('assert_url', '', 'https://example.test/clicked/pressed')).ok, true);
    assert.equal((await call('assert_visible', 'button')).ok, true);
    assert.equal((await call('screenshot')).ok, true);
    assert.equal((await call('navigate', '', 'file:///secret')).ok, false);
    assert.equal((await call('evaluate', '', 'process.exit()')).ok, false);
    assert.equal((await call('close')).ok, true);
  } finally {
    child.stdin.end();
    await new Promise(resolve => child.once('exit', resolve));
    fs.rmSync(directory, { recursive: true, force: true });
  }
});
