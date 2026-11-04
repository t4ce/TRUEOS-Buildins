// A transparent text-only render pass. No page screenshot, background color,
// caret, selection, menu, or previous shader frame enters this image.

const MODES = new Set(['keystroke', 'interval', 'manual']);
const SNAPSHOT_INTERVAL = 3000;
const MAX_MASK_DIMENSION = 1536;

/** Extract coverage, not luminance: opaque black is 255; transparent white is 0. */
export function alphaToR8(rgba, width, height) {
  if (!Number.isSafeInteger(width) || !Number.isSafeInteger(height) || width < 1 || height < 1) {
    throw new RangeError('A text mask needs positive integer dimensions.');
  }
  const pixels = width * height;
  if (!Number.isSafeInteger(pixels) || !Number.isSafeInteger(pixels * 4)) {
    throw new RangeError('Text mask dimensions are too large.');
  }
  if (!(rgba instanceof Uint8Array || rgba instanceof Uint8ClampedArray) || rgba.length !== pixels * 4) {
    throw new TypeError('Expected exactly width × height × 4 RGBA bytes.');
  }
  const coverage = new Uint8Array(pixels);
  for (let pixel = 0, alpha = 3; pixel < pixels; pixel++, alpha += 4) coverage[pixel] = rgba[alpha];
  return coverage;
}

let graphemeSegmenter;
function graphemes(text) {
  if (typeof Intl !== 'undefined' && typeof Intl.Segmenter === 'function') {
    graphemeSegmenter ||= new Intl.Segmenter(undefined, { granularity: 'grapheme' });
    return Array.from(graphemeSegmenter.segment(text), (part) => part.segment);
  }
  // This still preserves surrogate pairs on older browsers.
  return Array.from(text);
}

function nextTabStop(x, tabWidth) {
  return (Math.floor((x + 0.001) / tabWidth) + 1) * tabWidth;
}

/**
 * Layout pre/pre-wrap editor text into positioned runs. Only visible rows are
 * returned. `measure` includes the current font's letter spacing. Wrapping is
 * by grapheme, matching this editor's word-break: break-all policy.
 *
 * Positions are in CSS pixels, before the viewport's gutter/padding transform.
 */
export function layoutVisibleLines({
  text,
  wrap = false,
  width,
  lineHeight,
  scrollTop = 0,
  viewportHeight,
  paddingTop = 0,
  measure,
  tabSize = 2,
  includeSourceOffsets = false,
}) {
  if (typeof measure !== 'function') throw new TypeError('Text layout needs a measure function.');
  if (!(lineHeight > 0) || !(width > 0) || !(viewportHeight > 0)) return [];
  const source = String(text);
  const lines = source.replace(/\r\n?/g, '\n').split('\n');
  const lineStarts = [0];
  if (includeSourceOffsets) {
    const breaks = /\r\n?|\n/g;
    let match;
    while ((match = breaks.exec(source))) lineStarts.push(match.index + match[0].length);
  }
  const spaceWidth = Math.max(0.1, measure(' '));
  const tabWidth = spaceWidth * Math.max(1, Number(tabSize) || 2);
  const firstRow = Math.max(0, Math.floor((scrollTop - paddingTop) / lineHeight) - 1);
  const lastRow = Math.max(firstRow, Math.ceil((scrollTop - paddingTop + viewportHeight) / lineHeight) + 1);
  const visible = [];

  if (!wrap) {
    for (let lineIndex = firstRow; lineIndex < lines.length && lineIndex <= lastRow; lineIndex++) {
      let x = 0, sourceOffset = 0;
      const runs = [];
      const parts = lines[lineIndex].split('\t');
      for (let i = 0; i < parts.length; i++) {
        if (parts[i]) {
          runs.push({ text: parts[i], x, ...(includeSourceOffsets ? { sourceStart: lineStarts[lineIndex] + sourceOffset } : {}) });
          x += measure(parts[i]);
        }
        sourceOffset += parts[i].length;
        if (i < parts.length - 1) { x = nextTabStop(x, tabWidth); sourceOffset++; }
      }
      visible.push({ row: lineIndex, line: lineIndex + 1, y: lineIndex * lineHeight, runs });
    }
    return visible;
  }

  let row = 0;
  const advances = new Map();
  for (let lineIndex = 0; lineIndex < lines.length && row <= lastRow; lineIndex++) {
    let x = 0, chunk = '', chunkX = 0, chunkSource = 0, sourceOffset = 0, runs = [];
    const flush = () => {
      if (chunk) runs.push({ text: chunk, x: chunkX, ...(includeSourceOffsets ? { sourceStart: chunkSource } : {}) });
      chunk = '';
    };
    const finishRow = () => {
      flush();
      if (row >= firstRow && row <= lastRow) visible.push({ row, line: lineIndex + 1, y: row * lineHeight, runs });
      row++; x = 0; chunkX = 0; runs = [];
    };
    for (const glyph of graphemes(lines[lineIndex])) {
      if (glyph === '\t') {
        flush();
        let target = nextTabStop(x, tabWidth);
        if (x > 0 && target > width + 0.01) { finishRow(); target = tabWidth; }
        x = target; chunkX = x; sourceOffset += glyph.length;
        continue;
      }
      let advance = advances.get(glyph);
      if (advance === undefined) { advance = measure(glyph); advances.set(glyph, advance); }
      if (x > 0 && x + advance > width + 0.01) finishRow();
      if (!chunk) { chunkX = x; if (includeSourceOffsets) chunkSource = lineStarts[lineIndex] + sourceOffset; }
      chunk += glyph;
      x += advance; sourceOffset += glyph.length;
    }
    finishRow();
  }
  return visible;
}

function numberStyle(value, fallback = 0) {
  const parsed = Number.parseFloat(value);
  return Number.isFinite(parsed) ? parsed : fallback;
}

// Coverage always comes from the white glyph pass. CSS alpha is deliberately
// ignored here: even an opaque black glyph must be a colored Life seed.
function parseColorBytes(value) {
  if (typeof value !== 'string') return null;
  const color = value.trim().toLowerCase();
  const hex = /^#([\da-f]{3,4}|[\da-f]{6}|[\da-f]{8})$/.exec(color);
  if (hex) {
    const rgb = hex[1].length < 5 ? hex[1].slice(0, 3).split('').map((x) => x + x).join('') : hex[1].slice(0, 6);
    return [0, 2, 4].map((offset) => Number.parseInt(rgb.slice(offset, offset + 2), 16));
  }
  const rgb = /^rgba?\((.*)\)$/.exec(color);
  if (!rgb) return null;
  const channels = rgb[1].split('/')[0].trim().split(/[\s,]+/).slice(0, 3);
  if (channels.length !== 3) return null;
  const bytes = channels.map((channel) => {
    if (!/^[+-]?(?:\d+(?:\.\d*)?|\.\d+)%?$/.test(channel)) return NaN;
    const n = Number.parseFloat(channel) * (channel.endsWith('%') ? 2.55 : 1);
    return Math.round(Math.max(0, Math.min(255, n)));
  });
  return bytes.every(Number.isFinite) ? bytes : null;
}

function colorInput(ctx, description, length) {
  const palette = new Map(), resolved = new Map();
  const intern = (rgb) => {
    const key = (rgb[0] << 16) | (rgb[1] << 8) | rgb[2];
    if (!palette.has(key)) palette.set(key, { key, rgb, css: `rgb(${rgb.join(',')})` });
    return palette.get(key);
  };
  const resolve = (value, fallback) => {
    if (typeof value !== 'string') return fallback;
    if (resolved.has(value)) return resolved.get(value);
    let rgb = parseColorBytes(value);
    if (!rgb) {
      // Canvas normalizes named CSS colors to hex/rgb. Invalid assignments
      // retain the explicit fallback; no DOM read is needed per token/pixel.
      const previous = ctx.fillStyle;
      try {
        ctx.fillStyle = fallback.css;
        ctx.fillStyle = value;
        rgb = parseColorBytes(ctx.fillStyle);
      } catch { /* Unsupported CSS colors fall back to the editor text. */ }
      ctx.fillStyle = previous;
    }
    const result = rgb ? intern(rgb) : fallback;
    resolved.set(value, result);
    return result;
  };
  const defaultColor = intern([220, 226, 232]);
  const base = resolve(description?.baseColor, defaultColor);
  const candidates = Array.isArray(description?.runs) ? description.runs : [];
  const runs = [];
  for (const run of candidates) {
    if (!run || !Number.isFinite(run.start) || !Number.isFinite(run.end)) continue;
    const start = Math.max(0, Math.min(length, Math.floor(run.start)));
    const end = Math.max(0, Math.min(length, Math.floor(run.end)));
    if (end > start) runs.push({ start, end, color: resolve(run.color, base) });
  }
  runs.sort((a, b) => a.start - b.start || a.end - b.end);
  let edge = 0;
  const disjoint = [];
  for (const run of runs) {
    // Computed DOM spans are disjoint. Clip malformed overlaps defensively.
    run.start = Math.max(edge, run.start);
    if (run.end <= run.start) continue;
    disjoint.push(run);
    edge = run.end;
  }
  // Include only source colors which can actually be painted. In particular,
  // an unused default must never win the antialiasing round-trip correction.
  const used = new Map([[base.key, base]]);
  for (const run of disjoint) used.set(run.color.key, run.color);
  return { base, runs: disjoint, palette: [...used.values()] };
}

function coloredChunks(run, input) {
  const start = run.sourceStart, end = start + run.text.length;
  let lo = 0, hi = input.runs.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if (input.runs[mid].end <= start) lo = mid + 1;
    else hi = mid;
  }
  let index = lo;
  const first = input.runs[index];
  if (!first || first.start >= end) return [{ text: run.text, color: input.base }];
  if (first.start <= start && first.end >= end) return [{ text: run.text, color: first.color }];
  const chunks = [];
  let offset = start;
  for (const glyph of graphemes(run.text)) {
    while (index < input.runs.length && input.runs[index].end <= offset) index++;
    const style = input.runs[index];
    // A token edge inside a surrogate pair or combining cluster cannot split
    // a rendered glyph. Its first UTF-16 code unit supplies the glyph color.
    const color = style && style.start <= offset ? style.color : input.base;
    const previous = chunks[chunks.length - 1];
    if (previous?.color === color) previous.text += glyph;
    else chunks.push({ text: glyph, color });
    offset += glyph.length;
  }
  return chunks;
}

export class TextMaskCapture {
  constructor({ editor, stage, getWrap = () => editor.wrap !== 'off', getColorRuns, onCapture, onError, onDirty } = {}) {
    if (!editor || !stage) throw new TypeError('TextMaskCapture needs the text editor and document stage.');
    this.editor = editor;
    this.stage = stage;
    this.getWrap = getWrap;
    this.getColorRuns = typeof getColorRuns === 'function' ? getColorRuns : null;
    this.onCapture = onCapture;
    this.onError = onError;
    this.onDirty = onDirty;
    this.mode = 'keystroke';
    this.dirty = true;
    this._document = editor.ownerDocument || document;
    this._window = this._document.defaultView || window;
    this._revision = 0;
    this._emissionRevision = 0;
    this._emissionText = null;
    this._emissionTypography = null;
    this._scrollRevision = 0;
    this._scrollPosition = null;
    this._latest = null;
    this._started = false;
    this._destroyed = false;
    this._frame = 0;
    this._timer = 0;
    this._reason = 'initial';
    this._canvas = null;
    this._context = null;
    this._onVisibility = () => {
      if (this._document.hidden) {
        this._cancelScheduled();
        this._stopTimer();
      } else if (this._started) {
        this._startTimer();
        if (this.mode === 'keystroke' && this.dirty) this._schedule();
      }
    };
    this._document.addEventListener('visibilitychange', this._onVisibility);
    // The owner calls captureNow() after document.fonts.ready. Construction or
    // changing mode alone never races the editor's webfont load.
  }

  setMode(mode) {
    if (!MODES.has(mode)) throw new RangeError('Mask mode must be keystroke, interval, or manual.');
    if (this._destroyed) return;
    this.mode = mode;
    this._cancelScheduled();
    this._stopTimer();
    this._startTimer();
    if (mode === 'keystroke' && this.dirty && this._started) this._schedule();
  }

  /**
   * Mark the current text/layout as changed. This never rasterizes inline in a
   * text input callback. Multiple requests in one frame use the newest state.
   * Force bypasses the interval cadence for a viewport change; manual holds.
   */
  request(reason = 'edit', force = false) {
    if (this._destroyed) return;
    this.dirty = true;
    this._reason = String(reason);
    this.onDirty?.({ reason: this._reason, mode: this.mode });
    if (!this._started || this.mode === 'manual') return;
    if (this.mode === 'keystroke' || force) this._schedule();
  }

  /** Explicit capture overrides cadence and manual hold. Returns the new mask. */
  captureNow(reason = 'manual') {
    if (this._destroyed) return this._latest;
    this._started = true;
    this._cancelScheduled();
    this._startTimer();
    if (this._document.hidden) { this.dirty = true; return this._latest; }
    try {
      const snapshot = this._capture(String(reason));
      if (!snapshot) return this._latest;
      this._latest = snapshot;
      this.dirty = false;
      this.onCapture?.(snapshot);
      return snapshot;
    } catch (error) {
      this.dirty = true;
      this.onError?.(error);
      return this._latest;
    }
  }

  getLatest() { return this._latest; }

  _schedule() {
    if (this._frame || this._destroyed || this._document.hidden || !this._started) return;
    this._frame = this._window.requestAnimationFrame(() => {
      this._frame = 0;
      if (this.mode !== 'manual') this.captureNow(this._reason);
    });
  }

  _cancelScheduled() {
    if (this._frame) this._window.cancelAnimationFrame(this._frame);
    this._frame = 0;
  }

  _startTimer() {
    if (this._timer || !this._started || this._destroyed || this.mode !== 'interval' || this._document.hidden) return;
    this._timer = this._window.setInterval(() => {
      if (!this._document.hidden) this.captureNow('interval');
    }, SNAPSHOT_INTERVAL);
    // Feedback shaders receive every snapshot revision. Life uses the separate
    // emissionRevision, so repeated captures do not re-emit unchanged text.
  }

  _stopTimer() {
    if (this._timer) this._window.clearInterval(this._timer);
    this._timer = 0;
  }

  _getContext(width, height) {
    if (!this._canvas) {
      // This is a 2D CPU readback target, entirely separate from the WebGL frame.
      this._canvas = typeof this._window.OffscreenCanvas === 'function'
        ? new this._window.OffscreenCanvas(width, height)
        : this._document.createElement('canvas');
      this._context = this._canvas.getContext('2d', { alpha: true, willReadFrequently: true });
      if (!this._context) throw new Error('A text-mask Canvas2D context could not be created.');
    }
    if (this._canvas.width !== width) this._canvas.width = width;
    if (this._canvas.height !== height) this._canvas.height = height;
    return this._context;
  }

  _capture(reason) {
    const stageRect = this.stage.getBoundingClientRect();
    const editorRect = this.editor.getBoundingClientRect();
    if (stageRect.width <= 0 || stageRect.height <= 0 || editorRect.width <= 0 || editorRect.height <= 0) return null;
    const scale = Math.min(1, MAX_MASK_DIMENSION / Math.max(stageRect.width, stageRect.height));
    const width = Math.max(1, Math.round(stageRect.width * scale));
    const height = Math.max(1, Math.round(stageRect.height * scale));
    const ctx = this._getContext(width, height);
    const css = this._window.getComputedStyle(this.editor);
    const fontSize = numberStyle(css.fontSize, 14);
    const lineHeight = numberStyle(css.lineHeight, fontSize * 1.2);
    const letterSpacing = numberStyle(css.letterSpacing);
    const paddingTop = numberStyle(css.paddingTop);
    const paddingLeft = numberStyle(css.paddingLeft);
    const paddingRight = numberStyle(css.paddingRight);
    const clientX = editorRect.left - stageRect.left + this.editor.clientLeft;
    const clientY = editorRect.top - stageRect.top + this.editor.clientTop;
    const clientWidth = this.editor.clientWidth;
    const clientHeight = this.editor.clientHeight;
    const contentWidth = Math.max(1, clientWidth - paddingLeft - paddingRight);

    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.clearRect(0, 0, width, height);
    ctx.globalAlpha = 1;
    ctx.globalCompositeOperation = 'source-over';
    ctx.font = `${css.fontStyle || 'normal'} ${css.fontWeight || '400'} ${fontSize}px ${css.fontFamily || 'monospace'}`;
    if ('fontKerning' in ctx) ctx.fontKerning = 'none';
    const nativeSpacing = 'letterSpacing' in ctx;
    if (nativeSpacing) ctx.letterSpacing = `${letterSpacing}px`;
    ctx.textAlign = 'left';
    ctx.textBaseline = 'alphabetic';
    ctx.direction = css.direction || 'ltr';
    ctx.fillStyle = '#ffffff';

    const metrics = ctx.measureText('Mg');
    const ascent = Number.isFinite(metrics.fontBoundingBoxAscent) ? metrics.fontBoundingBoxAscent : metrics.actualBoundingBoxAscent;
    const descent = Number.isFinite(metrics.fontBoundingBoxDescent) ? metrics.fontBoundingBoxDescent : metrics.actualBoundingBoxDescent;
    // Center the font's own ascent/descent inside the CSS line box. This avoids
    // guessing an arbitrary percentage of font size for the glyph baseline.
    const baseline = (lineHeight - ascent - descent) * 0.5 + ascent;
    const measure = nativeSpacing
      ? (text) => ctx.measureText(text).width
      : (text) => ctx.measureText(text).width + graphemes(text).length * letterSpacing;
    const text = this.editor.value;
    const rows = layoutVisibleLines({
      text,
      wrap: Boolean(this.getWrap()),
      width: contentWidth,
      lineHeight,
      scrollTop: this.editor.scrollTop,
      viewportHeight: clientHeight,
      paddingTop,
      measure,
      tabSize: numberStyle(css.tabSize || css.MozTabSize, 2),
      includeSourceOffsets: Boolean(this.getColorRuns),
    });
    const x = clientX + paddingLeft - this.editor.scrollLeft;
    const y = clientY + paddingTop - this.editor.scrollTop + baseline;
    ctx.save();
    ctx.setTransform(width / stageRect.width, 0, 0, height / stageRect.height, 0, 0);
    // Match the textarea's clipping rectangle, including the gutter offset and
    // excluding scrollbar pixels. No UI text is ever drawn into this pass.
    ctx.beginPath();
    ctx.rect(clientX, clientY, clientWidth, clientHeight);
    ctx.clip();
    for (const row of rows) {
      for (const run of row.runs) {
        if (nativeSpacing || letterSpacing === 0) ctx.fillText(run.text, x + run.x, y + row.y);
        else {
          let cursorX = x + run.x;
          for (const glyph of graphemes(run.text)) {
            ctx.fillText(glyph, cursorX, y + row.y);
            cursorX += ctx.measureText(glyph).width + letterSpacing;
          }
        }
      }
    }
    ctx.restore();

    // getImageData reads this transparent glyph pass, never the compositor or
    // previous shader frame. The channel drop is explicit and lossless for A8.
    const rgba = ctx.getImageData(0, 0, width, height).data;
    const data = alphaToR8(rgba, width, height);
    let glyphPixels = 0, alphaSum = 0;
    for (const alpha of data) { if (alpha > 0) glyphPixels++; alphaSum += alpha; }
    // Color is an optional companion input. The original R8 raster above stays
    // independent of syntax color, including black or translucent font colors.
    const colorData = this.getColorRuns ? this._captureColors({
      ctx, text, rows, data, width, height, stageRect, clientX, clientY, clientWidth, clientHeight,
      x, y, baseline, lineHeight, measure, nativeSpacing, letterSpacing,
    }) : null;
    // Text and typography decide whether this snapshot carries new Life input.
    // Whitespace may move pixels, wrap lines, or scroll the viewport without
    // adding ink. Compare exact captured content, not key events or a hash.
    const emissionText = text.replace(/\s+/gu, '');
    // The same CSS family can first render fallback glyphs, then its web font.
    // Readiness distinguishes those captures without delaying or changing them.
    let fontReady = null;
    try {
      const fonts = this._document.fonts;
      if (typeof fonts?.check === 'function') fontReady = Boolean(fonts.check(ctx.font, 'Ag09_{}'));
    } catch { /* Optional readiness information must not fail a glyph capture. */ }
    const emissionTypography = JSON.stringify([
      css.fontFamily || 'monospace', fontSize, css.fontWeight || '400',
      css.fontStyle || 'normal', letterSpacing, lineHeight, fontReady,
    ]);
    if (emissionText !== this._emissionText || emissionTypography !== this._emissionTypography) {
      this._emissionRevision++;
      this._emissionText = emissionText;
      this._emissionTypography = emissionTypography;
    }
    // A captured viewport move explicitly refreshes Life from the visible text.
    // Record only successful captures; ordinary edits keep their additive path.
    const scrollPosition = [this.editor.scrollLeft, this.editor.scrollTop];
    if (this._scrollPosition && scrollPosition.some((value, index) => value !== this._scrollPosition[index])) this._scrollRevision++;
    this._scrollPosition = scrollPosition;
    return {
      width,
      height,
      data,
      ...(colorData ? { colorData } : {}),
      revision: ++this._revision,
      emissionRevision: this._emissionRevision,
      scrollRevision: this._scrollRevision,
      capturedAt: performance.now(),
      reason,
      glyphPixels,
      coverage: alphaSum / (255 * width * height),
      sourceChars: Array.from(text).length,
    };
  }

  _captureColors({ ctx, text, rows, data, width, height, stageRect,
    clientX, clientY, clientWidth, clientHeight, x, y, baseline, lineHeight, measure, nativeSpacing, letterSpacing }) {
    let description;
    try { description = this.getColorRuns(text); }
    catch { /* A stale highlighting view must not discard a valid text mask. */ }
    const input = colorInput(ctx, description, text.length);
    // Reuse the transparent surface only after the independent R8 readback.
    // No shader frame, editor background, selection, or gutter enters either
    // render pass. All coordinates, font metrics and clipping stay identical.
    ctx.save();
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.clearRect(0, 0, width, height);
    ctx.setTransform(width / stageRect.width, 0, 0, height / stageRect.height, 0, 0);
    ctx.beginPath();
    ctx.rect(clientX, clientY, clientWidth, clientHeight);
    ctx.clip();
    const bands = [];
    for (const row of rows) {
      const spans = [];
      for (const run of row.runs) {
        let cursorX = x + run.x;
        for (const chunk of coloredChunks(run, input)) {
          const left = cursorX;
          ctx.fillStyle = chunk.color.css;
          if (nativeSpacing || letterSpacing === 0) {
            ctx.fillText(chunk.text, cursorX, y + row.y);
            cursorX += measure(chunk.text);
          } else {
            for (const glyph of graphemes(chunk.text)) {
              ctx.fillText(glyph, cursorX, y + row.y);
              cursorX += ctx.measureText(glyph).width + letterSpacing;
            }
          }
          spans.push({ left, right: cursorX, color: chunk.color.rgb });
        }
      }
      bands.push({ top: y + row.y - baseline, bottom: y + row.y - baseline + lineHeight, spans });
    }
    ctx.restore();
    const painted = ctx.getImageData(0, 0, width, height).data;
    const colors = new Uint8Array(data.length * 4);
    const nearest = new Map(input.palette.map((color) => [color.key, color.rgb]));
    const sourceColor = (offset) => {
      const key = (painted[offset] << 16) | (painted[offset + 1] << 8) | painted[offset + 2];
      if (!nearest.has(key)) {
        // Canvas internally premultiplies antialiased pixels. Restore the
        // exact source RGB instead of propagating its byte rounding as new
        // colors into the cellular simulation. The palette is usually tiny.
        let best = input.base.rgb, distance = Infinity;
        for (const color of input.palette) {
          const delta = color.rgb.reduce((sum, channel, i) => sum + (channel - painted[offset + i]) ** 2, 0);
          if (delta < distance) { best = color.rgb; distance = delta; }
        }
        nearest.set(key, best);
      }
      return nearest.get(key);
    };
    let bandIndex = 0, spanIndex = 0, pixelRow = -1;
    const scaleX = stageRect.width / width, scaleY = stageRect.height / height;
    for (let pixel = 0; pixel < data.length; pixel++) {
      const alpha = data[pixel];
      if (alpha === 0) continue;
      const offset = pixel * 4;
      const px = pixel % width, py = Math.floor(pixel / width);
      const centerX = (px + 0.5) * scaleX, centerY = (py + 0.5) * scaleY;
      if (pixelRow !== py) {
        pixelRow = py; spanIndex = 0;
        while (bandIndex < bands.length && bands[bandIndex].bottom <= centerY) bandIndex++;
      }
      const band = bands[bandIndex];
      let owner;
      if (band && band.top <= centerY) {
        while (spanIndex < band.spans.length && band.spans[spanIndex].right <= centerX) spanIndex++;
        const span = band.spans[spanIndex];
        if (span && span.left <= centerX) owner = span.color;
      }
      let color = input.base.rgb;
      // In the editor's monospace line boxes the source span owns its advance
      // interval. Keep that identity explicitly: at alpha 1, for example,
      // Canvas readback cannot distinguish two close pastel syntax colors.
      // Never infer a glyph's source color from those lossy RGB bytes.
      if (owner) color = owner;
      else if (painted[offset + 3]) color = sourceColor(offset);
      else {
        // A shaping/rounding edge can differ where a syntax span splits a
        // run. Keep the R8 outline and borrow a nearby painted glyph color.
        let found = false;
        for (let radius = 1; radius <= 2 && !found; radius++) {
          for (let dy = -radius; dy <= radius && !found; dy++) {
            for (let dx = -radius; dx <= radius; dx++) {
              if (px + dx < 0 || px + dx >= width || py + dy < 0 || py + dy >= height) continue;
              const near = ((py + dy) * width + px + dx) * 4;
              if (painted[near + 3]) { color = sourceColor(near); found = true; break; }
            }
          }
        }
      }
      colors[offset] = color[0];
      colors[offset + 1] = color[1];
      colors[offset + 2] = color[2];
      colors[offset + 3] = alpha;
    }
    return colors;
  }

  destroy() {
    this._destroyed = true;
    this._cancelScheduled();
    this._stopTimer();
    this._document.removeEventListener('visibilitychange', this._onVisibility);
    this._canvas = null;
    this._context = null;
    this._emissionText = null;
    this._emissionTypography = null;
    this._scrollPosition = null;
  }
}
