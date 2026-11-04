import test from 'node:test';
import assert from 'node:assert/strict';
import { TextMaskCapture, layoutVisibleLines } from '../dist/text-mask.js';

const segmenter = new Intl.Segmenter(undefined, { granularity: 'grapheme' });
const glyphs = (text) => [...segmenter.segment(text)].map((part) => part.segment);

function harness(text, getColorRuns) {
  let nextId = 0;
  const css = { fontFamily: 'monospace', fontSize: '12px', fontWeight: '400', fontStyle: 'normal',
    lineHeight: '16px', letterSpacing: '0px', paddingTop: '2px', paddingLeft: '2px',
    paddingRight: '2px', direction: 'ltr', tabSize: '2' };
  let fillStyle = '#000000';
  const context = {
    letterSpacing: '0px', runs: [], reads: 0,
    get fillStyle() { return fillStyle; },
    set fillStyle(value) {
      const named = { red: '#ff0000', black: '#000000', blue: '#0000ff' };
      if (named[value]) fillStyle = named[value];
      else if (/^#(?:[\da-f]{3}|[\da-f]{6})$/i.test(value) || /^rgb\(\d+,\d+,\d+\)$/.test(value)) fillStyle = value;
    },
    save() {}, restore() {}, setTransform() {}, beginPath() {}, rect() {}, clip() {},
    clearRect() { this.runs = []; },
    measureText(value) { return { width: glyphs(value).length * 6, fontBoundingBoxAscent: 8, fontBoundingBoxDescent: 2 }; },
    fillText(value, x, y) { this.runs.push({ value, x, y, color: fillStyle }); },
    getImageData(x, y, width, height) {
      this.reads++;
      const data = new Uint8ClampedArray(width * height * 4);
      for (const run of this.runs) {
        const color = run.color.startsWith('rgb') ? run.color.match(/\d+/g).map(Number)
          : run.color.slice(1).match(/.{2}/g).map((channel) => Number.parseInt(channel, 16));
        glyphs(run.value).forEach((glyph, i) => {
          const px = Math.floor(run.x + i * 6), py = Math.floor(run.y);
          if (/\s/u.test(glyph) || px < 0 || px >= width || py < 0 || py >= height) return;
          const index = (py * width + px) * 4;
          const alpha = this.glyphAlpha ?? 64 + glyph.codePointAt(0) % 192;
          data.set([...(this.roundTripColor ? this.roundTripColor(color, alpha) : color), alpha], index);
        });
      }
      return { data };
    },
  };
  const win = { getComputedStyle: () => css, requestAnimationFrame: () => ++nextId,
    cancelAnimationFrame() {}, setInterval: () => ++nextId, clearInterval() {} };
  const doc = { hidden: false, defaultView: win, fonts: { check: () => true },
    addEventListener() {}, removeEventListener() {},
    createElement() { return { width: 0, height: 0, getContext: () => context }; } };
  const editor = { ownerDocument: doc, value: text, wrap: 'off', clientLeft: 0, clientTop: 0,
    clientWidth: 172, clientHeight: 108, scrollLeft: 0, scrollTop: 0,
    getBoundingClientRect: () => ({ left: 8, top: 0, width: 172, height: 108 }) };
  const stage = { getBoundingClientRect: () => ({ left: 0, top: 0, width: 180, height: 108 }) };
  const capture = new TextMaskCapture({ editor, stage, getColorRuns, onError(error) { throw error; } });
  return { capture, editor, css, context };
}

function paintedPixels(mask) {
  const pixels = [];
  for (let pixel = 0; pixel < mask.data.length; pixel++) if (mask.data[pixel]) pixels.push({
    x: pixel % mask.width, y: Math.floor(pixel / mask.width), alpha: mask.data[pixel],
    rgba: [...mask.colorData.subarray(pixel * 4, pixel * 4 + 4)],
  });
  return pixels;
}
function assertAlphaMatches(mask) {
  assert.equal(mask.colorData.length, mask.data.length * 4);
  for (let pixel = 0; pixel < mask.data.length; pixel++) {
    assert.equal(mask.colorData[pixel * 4 + 3], mask.data[pixel]);
    if (!mask.data[pixel]) assert.ok(mask.colorData.subarray(pixel * 4, pixel * 4 + 4).every((byte) => byte === 0));
  }
}

test('source offsets preserve original UTF-16 indices through CRLF, tabs, graphemes and wrapping', () => {
  const text = 'A\tB\r\n😀e\u0301\tZ\rQ';
  const options = { text, width: 18, lineHeight: 10, viewportHeight: 200, measure: (value) => glyphs(value).length * 6 };
  const rows = layoutVisibleLines({ ...options, includeSourceOffsets: true });
  assert.deepEqual(rows.flatMap((row) => row.runs.map((run) => [run.text, run.sourceStart])),
    [['A', 0], ['B', 2], ['😀e\u0301', 5], ['Z', 10], ['Q', 12]]);
  for (const wrap of [false, true]) {
    const enriched = layoutVisibleLines({ ...options, wrap, includeSourceOffsets: true });
    const legacy = layoutVisibleLines({ ...options, wrap });
    assert.deepEqual(enriched.map((row) => ({ ...row, runs: row.runs.map(({ sourceStart, ...run }) => run) })), legacy,
      'Color metadata does not change positioned text');
    for (const row of enriched) for (const run of row.runs) {
      assert.equal(text.slice(run.sourceStart, run.sourceStart + run.text.length), run.text);
    }
  }
});

test('optional RGBA uses actual token RGB while black and CSS alpha preserve independent R8 coverage', () => {
  const text = 'R G B K';
  const plain = harness(text);
  const color = harness(text, (snapshotText) => {
    assert.equal(snapshotText, text);
    return { baseColor: '#dce2e8', runs: [
      { start: 0, end: 1, color: '#f00' }, { start: 2, end: 3, color: 'rgb(0 100% 0 / .2)' },
      { start: 4, end: 5, color: '#0000ff00' }, { start: 6, end: 7, color: 'black' },
    ] };
  });
  const before = plain.capture.captureNow(), image = color.capture.captureNow();
  assert.equal(before.colorData, undefined, 'Legacy users do not allocate or read a second image');
  assert.equal(plain.context.reads, 1);
  assert.equal(color.context.reads, 2);
  assert.deepEqual(image.data, before.data);
  assertAlphaMatches(image);
  assert.deepEqual(paintedPixels(image).map((pixel) => pixel.rgba.slice(0, 3)),
    [[255, 0, 0], [0, 255, 0], [0, 0, 255], [0, 0, 0]]);
  assert.ok(paintedPixels(image).every((pixel) => pixel.alpha > 0));
  plain.capture.destroy(); color.capture.destroy();
});

test('token boundaries inside surrogate pairs or combining clusters cannot split a glyph', () => {
  const h = harness('😀e\u0301Z', () => ({ baseColor: '#0000ff', runs: [
    { start: 0, end: 1, color: '#ff0000' }, { start: 1, end: 3, color: '#00ff00' },
  ] }));
  const mask = h.capture.captureNow();
  assert.deepEqual(paintedPixels(mask).map((pixel) => pixel.rgba.slice(0, 3)),
    [[255, 0, 0], [0, 255, 0], [0, 0, 255]]);
  assertAlphaMatches(mask);
  h.capture.destroy();
});

test('low-alpha palette ambiguity preserves source token identity rather than a neighboring pastel', () => {
  const h = harness('A', () => ({ baseColor: '#b3d8da', runs: [{ start: 0, end: 1, color: '#b4dcd6' }] }));
  h.context.glyphAlpha = 34;
  h.context.roundTripColor = (rgb, alpha) => rgb.map((channel) => Math.round(Math.round(channel * alpha / 255) * 255 / alpha));
  // The link color [180,220,214] round-trips to [180,218,218]. A global
  // nearest palette match incorrectly calls it function color [179,216,218].
  const mask = h.capture.captureNow('antialiased-token');
  assert.deepEqual(paintedPixels(mask)[0].rgba, [180, 220, 214, 34]);
  assertAlphaMatches(mask);
  h.capture.destroy();
});

test('syntax color changes refresh RGBA without restarting emission identity or changing R8', () => {
  let color = '#ff0000';
  const h = harness('A', () => ({ baseColor: color, runs: [] }));
  const first = h.capture.captureNow('initial');
  color = '#0000ff';
  const next = h.capture.captureNow('highlight');
  assert.deepEqual(next.data, first.data);
  assert.notDeepEqual(next.colorData, first.colorData);
  assert.equal(next.emissionRevision, first.emissionRevision);
  assert.equal(next.scrollRevision, first.scrollRevision);
  assert.equal(next.revision, first.revision + 1);
  h.editor.value = ' A\n';
  assert.equal(h.capture.captureNow('whitespace').emissionRevision, first.emissionRevision);
  h.editor.scrollTop = 1;
  assert.equal(h.capture.captureNow('scroll').scrollRevision, first.scrollRevision + 1);
  h.capture.destroy();
});

test('invalid optional highlighting falls back safely without losing the glyph capture', () => {
  for (const getColorRuns of [
    () => { throw new Error('Highlight DOM unavailable'); },
    () => null,
    () => ({ baseColor: 'invalid-color', runs: [{ start: NaN, end: 5, color: '#ff0000' }] }),
    () => ({ baseColor: '#dce2e8', runs: [{ start: 0, end: 1, color: 'invalid-color' }] }),
  ]) {
    const h = harness('A', getColorRuns);
    const mask = h.capture.captureNow();
    assert.equal(mask.revision, 1);
    assert.equal(mask.emissionRevision, 1);
    assert.deepEqual(paintedPixels(mask)[0].rgba.slice(0, 3), [220, 226, 232]);
    assertAlphaMatches(mask);
    h.capture.destroy();
  }
});
