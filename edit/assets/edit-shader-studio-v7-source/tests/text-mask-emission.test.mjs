import test from 'node:test';
import assert from 'node:assert/strict';
import { TextMaskCapture } from '../dist/text-mask.js';

function harness(text = 'const n = 42;') {
  let nextId = 0, now = 0, wrap = false;
  const frames = new Map(), timers = new Map(), listeners = new Map();
  const css = {
    fontFamily: 'EditMono', fontSize: '14px', fontWeight: '400', fontStyle: 'normal',
    lineHeight: '27px', letterSpacing: '.05px', paddingTop: '4px', paddingLeft: '4px',
    paddingRight: '4px', direction: 'ltr', tabSize: '2',
  };
  // This fixture exercises the production capture path. It supplies only the
  // Canvas2D platform boundary; these tests concern revision and cadence, not
  // font rasterization (which is validated separately with a real Canvas2D).
  const ctx = {
    letterSpacing: '0px', runs: [], failNextRead: false,
    setTransform() {}, save() {}, restore() {}, beginPath() {}, rect() {}, clip() {},
    clearRect() { this.runs = []; },
    measureText(value) { return { width: Array.from(value).length * 6, fontBoundingBoxAscent: 8, fontBoundingBoxDescent: 2 }; },
    fillText(value, x, y) { this.runs.push({ value, x, y }); },
    getImageData(x, y, width, height) {
      if (this.failNextRead) { this.failNextRead = false; throw new Error('Readback fixture failure'); }
      const data = new Uint8ClampedArray(width * height * 4);
      for (const run of this.runs) {
        let column = 0;
        for (const glyph of run.value) {
          const px = Math.floor(run.x + column * 6), py = Math.floor(run.y);
          if (!/\s/u.test(glyph) && px >= 0 && px < width && py >= 0 && py < height) data[(py * width + px) * 4 + 3] = 64 + glyph.codePointAt(0) % 192;
          column++;
        }
      }
      return { data };
    },
  };
  const canvas = { width: 0, height: 0, getContext: () => ctx };
  const win = {
    getComputedStyle: () => css,
    requestAnimationFrame(fn) { const id = ++nextId; frames.set(id, fn); return id; },
    cancelAnimationFrame(id) { frames.delete(id); },
    setInterval(fn, delay) { const id = ++nextId; timers.set(id, { fn, delay, at: now + delay }); return id; },
    clearInterval(id) { timers.delete(id); },
  };
  const doc = {
    hidden: false, defaultView: win,
    addEventListener(name, fn) { listeners.set(name, fn); },
    removeEventListener(name) { listeners.delete(name); },
    createElement(tag) { assert.equal(tag, 'canvas'); return canvas; },
  };
  const stageRect = { left: 0, top: 0, width: 180, height: 108 };
  const editor = {
    value: text, ownerDocument: doc, clientLeft: 0, clientTop: 0,
    clientWidth: 164, clientHeight: 108, scrollTop: 0, scrollLeft: 0,
    getBoundingClientRect: () => ({ left: 8, top: 0, width: editor.clientWidth, height: editor.clientHeight }),
  };
  const images = [], errors = [];
  const capture = new TextMaskCapture({
    editor, stage: { getBoundingClientRect: () => stageRect }, getWrap: () => wrap,
    onCapture: (image) => images.push(image), onError: (error) => errors.push(error),
  });
  function flushFrame() { const work = [...frames.values()]; frames.clear(); work.forEach((fn) => fn(now)); }
  function advance(ms) {
    const end = now + ms;
    while (true) {
      let next = Infinity;
      for (const timer of timers.values()) next = Math.min(next, timer.at);
      if (next > end) break;
      now = next;
      for (const timer of [...timers.values()]) if (timer.at <= now) { timer.at += timer.delay; timer.fn(); }
    }
    now = end;
  }
  return { capture, editor, css, ctx, doc, stageRect, images, errors, frames, timers, listeners, flushFrame, advance, setWrap(value) { wrap = value; } };
}

test('first successful capture starts emission revision at one; identical snapshots keep it', () => {
  for (const text of ['', 'A']) {
    const h = harness(text);
    const first = h.capture.captureNow('initial'), second = h.capture.captureNow('again');
    assert.equal(first.revision, 1);
    assert.equal(first.emissionRevision, 1);
    assert.equal(second.revision, 2);
    assert.equal(second.emissionRevision, 1);
    assert.deepEqual(second.data, first.data);
    assert.equal(second.emissionText, undefined, 'Normalized source text stays private');
    assert.equal(second.text, undefined, 'Snapshots contain pixels and metadata, not document text');
    h.capture.destroy();
  }
});

test('whitespace-only edits and their layout changes never advance emission revision', () => {
  const h = harness('const n = 42;');
  const initial = h.capture.captureNow('initial');
  for (const text of ['constn=42;', '  const n = 42;\t', '\tconst\n n \r\n=\v4\f2;\n', 'const\u00a0n\u2003=\u202842;']) {
    h.editor.value = text;
    h.editor.scrollTop += 5;
    h.editor.scrollLeft += 1;
    h.editor.clientWidth -= 7;
    h.stageRect.width += 3;
    h.setWrap(true);
    const image = h.capture.captureNow('whitespace-layout');
    assert.equal(image.emissionRevision, initial.emissionRevision);
    assert.ok(image.revision > initial.revision, 'The new raw mask is still captured');
  }
  h.capture.destroy();
});

test('ink edits, same-length replacements, repeated characters, and deletions advance emission revision', () => {
  const h = harness('A');
  h.capture.captureNow('initial');
  let expected = 1;
  for (const text of ['AA', 'AB', 'BA', 'B', '']) {
    h.editor.value = text;
    assert.equal(h.capture.captureNow('edit').emissionRevision, ++expected);
  }
  h.editor.value = ' \t\n';
  const blank = h.capture.captureNow('whitespace');
  assert.equal(blank.emissionRevision, expected);
  assert.equal(blank.glyphPixels, 0);
  assert.ok(blank.data.every((alpha) => alpha === 0));
  h.capture.destroy();
});

test('selected typography changes advance emission revision without a text edit', () => {
  const h = harness();
  h.capture.captureNow('initial');
  let expected = 1;
  for (const [key, value] of [
    ['fontFamily', 'EditBlock, EditMono'], ['fontSize', '32px'], ['fontWeight', '700'],
    ['fontStyle', 'italic'], ['letterSpacing', '0px'], ['lineHeight', '44px'],
  ]) {
    h.css[key] = value;
    assert.equal(h.capture.captureNow('typography').emissionRevision, ++expected, key);
    assert.equal(h.capture.captureNow('same-typography').emissionRevision, expected);
  }
  h.css.fontSize = '32.0px'; h.css.letterSpacing = '0.00px'; h.css.lineHeight = '44.0px';
  assert.equal(h.capture.captureNow('same-numeric-style').emissionRevision, expected);
  h.capture.destroy();
});

test('a web font becoming ready emits once even when its CSS and text are unchanged', () => {
  const h = harness('Ag09_{}');
  let ready = false;
  const queries = [];
  h.doc.fonts = { check(query, probe) { queries.push({ query, probe }); return ready; } };
  const fallback = h.capture.captureNow('initial');
  assert.equal(fallback.emissionRevision, 1);
  h.editor.value = ' Ag09_{}\n';
  assert.equal(h.capture.captureNow('still-loading').emissionRevision, 1);
  ready = true;
  const loaded = h.capture.captureNow('font-ready');
  assert.equal(loaded.emissionRevision, 2, 'The loaded face replaces the fallback input');
  assert.equal(h.capture.captureNow('same-loaded-font').emissionRevision, 2);
  assert.equal(queries.length, 4);
  assert.ok(queries.every(({ query, probe }) => query === 'normal 400 14px EditMono' && probe === 'Ag09_{}'));
  h.capture.destroy();
});

test('unavailable or failing optional font readiness checks do not block capture', () => {
  const h = harness('A');
  const first = h.capture.captureNow('no-font-api');
  h.doc.fonts = { check() { throw new Error('Readiness fixture failure'); } };
  const next = h.capture.captureNow('failing-font-api');
  assert.equal(next.revision, first.revision + 1);
  assert.equal(next.emissionRevision, first.emissionRevision);
  assert.deepEqual(next.data, first.data);
  assert.equal(h.errors.length, 0);
  h.capture.destroy();
});

test('each-key coalescing compares captured content, including edits undone before capture', () => {
  const h = harness('A');
  h.capture.captureNow('initial');
  h.editor.value = 'B'; h.capture.request('edit');
  h.editor.value = 'C'; h.capture.request('edit');
  assert.equal(h.images.length, 1);
  h.flushFrame();
  assert.equal(h.images.at(-1).revision, 2);
  assert.equal(h.images.at(-1).emissionRevision, 2);
  h.editor.value = 'D'; h.capture.request('edit');
  h.editor.value = 'C'; h.capture.request('undo');
  h.flushFrame();
  assert.equal(h.images.at(-1).revision, 3);
  assert.equal(h.images.at(-1).emissionRevision, 2, 'An uncaptured round trip does not create input');
  h.capture.destroy();
});

test('manual mode keeps emission identity until the final content is explicitly captured', () => {
  const h = harness('A');
  h.capture.captureNow('initial'); h.capture.setMode('manual');
  h.editor.value = 'B'; h.capture.request('edit');
  h.editor.value = '  C\n'; h.capture.request('edit');
  h.capture.request('resize', true); h.flushFrame(); h.advance(12000);
  assert.equal(h.images.length, 1);
  assert.equal(h.capture.getLatest().emissionRevision, 1);
  const next = h.capture.captureNow();
  assert.equal(next.revision, 2);
  assert.equal(next.emissionRevision, 2);
  h.editor.value = 'D'; h.capture.request('edit');
  h.editor.value = 'C'; h.capture.request('undo');
  assert.equal(h.capture.captureNow().emissionRevision, 2);
  h.capture.destroy();
});

test('three-second snapshots retain their raw cadence while only new ink advances emission identity', () => {
  const h = harness('A');
  h.capture.setMode('interval'); h.capture.captureNow('initial');
  h.advance(3000);
  assert.equal(h.images.at(-1).revision, 2);
  assert.equal(h.images.at(-1).emissionRevision, 1);
  h.editor.value = '\n\tA '; h.capture.request('edit'); h.advance(3000);
  assert.equal(h.images.at(-1).revision, 3);
  assert.equal(h.images.at(-1).emissionRevision, 1);
  h.editor.value = '\n\tAB '; h.capture.request('edit'); h.advance(2999);
  assert.equal(h.images.at(-1).emissionRevision, 1);
  h.advance(1);
  assert.equal(h.images.at(-1).revision, 4);
  assert.equal(h.images.at(-1).emissionRevision, 2);
  h.capture.destroy();
});

test('failed capture does not consume an emission revision or the pending text change', () => {
  const h = harness('A');
  const first = h.capture.captureNow('initial');
  h.editor.value = 'B'; h.ctx.failNextRead = true;
  assert.equal(h.capture.captureNow('failed'), first);
  assert.equal(h.errors.length, 1);
  assert.equal(h.capture.getLatest().emissionRevision, 1);
  const recovered = h.capture.captureNow('retry');
  assert.equal(recovered.revision, 2);
  assert.equal(recovered.emissionRevision, 2);
  h.capture.destroy();
});

test('captured vertical and horizontal scrolling each advance the refresh revision once', () => {
  const h = harness('A');
  assert.equal(h.capture.captureNow().scrollRevision, 0);
  h.editor.scrollTop = 27;
  let image = h.capture.captureNow('scroll');
  assert.equal(image.scrollRevision, 1);
  assert.equal(image.emissionRevision, 1);
  assert.equal(h.capture.captureNow('same-view').scrollRevision, 1);
  h.editor.scrollLeft = 12;
  assert.equal(h.capture.captureNow('scroll').scrollRevision, 2);
  h.editor.value = ' A\n';
  image = h.capture.captureNow('whitespace');
  assert.equal(image.scrollRevision, 2);
  assert.equal(image.emissionRevision, 1);
  h.editor.scrollTop = 54;
  assert.equal(h.capture.captureNow('caret-scroll').scrollRevision, 3, 'Any actual viewport move may refresh');
  h.capture.destroy();
});

test('scroll refresh follows Manual and interval capture timing and survives failed capture', () => {
  const h = harness('A');
  h.capture.captureNow(); h.capture.setMode('manual');
  h.editor.scrollTop = 20; h.capture.request('scroll'); h.flushFrame(); h.advance(6000);
  assert.equal(h.capture.getLatest().scrollRevision, 0);
  assert.equal(h.capture.captureNow().scrollRevision, 1);
  h.capture.setMode('interval'); h.editor.scrollLeft = 10; h.capture.request('scroll');
  h.advance(2999); assert.equal(h.capture.getLatest().scrollRevision, 1);
  h.advance(1); assert.equal(h.capture.getLatest().scrollRevision, 2);
  h.editor.scrollTop = 40; h.ctx.failNextRead = true;
  assert.equal(h.capture.captureNow('failed-scroll').scrollRevision, 2);
  assert.equal(h.capture.captureNow('retry').scrollRevision, 3);
  h.capture.destroy();
});
