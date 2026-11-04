import test from 'node:test';
import assert from 'node:assert/strict';
import { alphaToR8, TextMaskCapture } from '../dist/text-mask.js';

test('R8 stores alpha coverage, independent of RGB color', () => {
  const rgba = new Uint8ClampedArray([
    255, 255, 255, 0,
    0, 0, 0, 255,
    230, 15, 90, 64,
    15, 230, 90, 64,
  ]);
  const mask = alphaToR8(rgba, 2, 2);
  assert.ok(mask instanceof Uint8Array);
  assert.deepEqual([...mask], [0, 255, 64, 64]);
  assert.equal(mask.byteLength, 4);
  assert.throws(() => alphaToR8(new Uint8Array(3), 1, 1));
});

function harness() {
  let nextId = 0, captures = 0, now = 0;
  const frames = new Map(), timers = new Map(), listeners = new Map();
  const win = {
    requestAnimationFrame(fn) { const id = ++nextId; frames.set(id, fn); return id; },
    cancelAnimationFrame(id) { frames.delete(id); },
    setInterval(fn, delay) { const id = ++nextId; timers.set(id, { fn, delay, at: now + delay }); return id; },
    clearInterval(id) { timers.delete(id); },
  };
  const doc = {
    hidden: false, defaultView: win,
    addEventListener(name, fn) { listeners.set(name, fn); },
    removeEventListener(name) { listeners.delete(name); },
  };
  const editor = { value: 'A', ownerDocument: doc };
  const images = [];
  const capture = new TextMaskCapture({ editor, stage: {}, onCapture: (image) => images.push(image) });
  // Isolate scheduling from rasterization; alpha rasterization is tested above
  // and separately exercised with a real Canvas2D/font during validation.
  capture._capture = (reason) => ({ width: 1, height: 1, data: new Uint8Array([editor.value.charCodeAt(0) || 0]), revision: ++captures, reason, capturedAt: now });
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
  return { capture, editor, doc, images, frames, timers, listeners, flushFrame, advance };
}

test('each-key input is queued and coalesces to the newest text without inline capture', () => {
  const h = harness();
  h.capture.request('edit');
  assert.equal(h.images.length, 0, 'constructor and requests wait for initial font-ready capture');
  h.capture.captureNow('initial');
  h.editor.value = 'B'; h.capture.request('edit');
  h.editor.value = 'C'; h.capture.request('edit');
  assert.equal(h.images.length, 1, 'input callback has not rasterized');
  assert.equal(h.frames.size, 1);
  h.flushFrame();
  assert.equal(h.images.length, 2);
  assert.equal(h.images.at(-1).data[0], 'C'.charCodeAt(0));
  h.capture.destroy();
});

test('three-second mode holds edits, then emits periodic snapshots even without new text', () => {
  const h = harness();
  h.capture.setMode('interval'); h.capture.captureNow('initial');
  h.editor.value = 'B'; h.capture.request('edit');
  h.advance(2999); h.flushFrame();
  assert.equal(h.images.length, 1);
  h.advance(1);
  assert.equal(h.images.length, 2);
  assert.equal(h.images.at(-1).data[0], 'B'.charCodeAt(0));
  h.advance(3000);
  assert.equal(h.images.length, 3, 'unchanged text still supplies a fresh revision');
  assert.equal(h.images.at(-1).revision, 3);
  h.capture.destroy();
  assert.equal(h.timers.size, 0);
});

test('manual input holds through edits and layout invalidations until explicit capture', () => {
  const h = harness();
  h.capture.captureNow('initial'); h.capture.setMode('manual');
  h.editor.value = 'Z'; h.capture.request('edit'); h.capture.request('resize', true);
  h.flushFrame(); h.advance(12000);
  assert.equal(h.images.length, 1);
  assert.equal(h.capture.getLatest().data[0], 'A'.charCodeAt(0));
  h.capture.captureNow();
  assert.equal(h.images.length, 2);
  assert.equal(h.capture.getLatest().data[0], 'Z'.charCodeAt(0));
  h.capture.destroy();
});

test('hidden pages stop periodic captures and resume a new interval when visible', () => {
  const h = harness();
  h.capture.setMode('interval'); h.capture.captureNow('initial');
  h.doc.hidden = true; h.listeners.get('visibilitychange')(); h.advance(6000);
  assert.equal(h.images.length, 1);
  h.doc.hidden = false; h.listeners.get('visibilitychange')(); h.advance(3000);
  assert.equal(h.images.length, 2);
  h.capture.destroy();
  assert.equal(h.listeners.size, 0);
});
