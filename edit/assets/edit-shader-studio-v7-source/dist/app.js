import { ShaderEngine, PRESETS } from './shader-engine.js';
import { TextMaskCapture } from './text-mask.js';
import { collectTextColors } from './text-colors.js';

const $ = (id) => document.getElementById(id);
const clamp = (value, min, max) => Math.min(max, Math.max(min, Number(value)));
const STORAGE_KEY = 'edit-shader-studio:v2';
const LEGACY_STORAGE_KEY = 'edit-shader-studio:v1';
const FONT_SIZE_MIN = 10, FONT_SIZE_MAX = 48, FONT_SIZE_STEP = 2;
const initialText = [
  '# Your words are the source.',
  '',
  'Every letter leaves a little light behind.',
  '',
  '## Light from language',
  '',
  'The shape of this text feeds the atmosphere.',
  'Its alpha becomes a field. The field keeps moving.',
  '',
  '```javascript',
  '// The studio pipeline',
  'glyphs → alpha → R8 texture → feedback → light',
  '```',
  '',
  '## Change a word. Watch it travel.',
  '',
  'Each key sends a fresh image of the text.',
  'Try 3 seconds to feel the snapshots arrive.',
  '',
  'Manual holds the input; the light keeps evolving.',
  'Inspect the mask to see exactly what it receives.',
  '',
  '// This time, the words really are part of it.',
  ''
].join('\n');
const defaults = { preset: 'aurora', opacity: 50, speed: 1, scale: 0.5, palette: 'violet', enabled: true, running: !matchMedia('(prefers-reduced-motion: reduce)').matches, wrap: false, highlight: true, lines: true, fontSize: 14, blockFont: false, captureMode: 'keystroke', textInfluence: 1, feedback: 0.88 };
const state = { ...defaults, document: { name: 'README.md', text: initialText }, activeTab: 'document', shaderSource: PRESETS.find((p) => p.id === defaults.preset).source, customSource: '', focus: false };
let storageAvailable = true;
try {
  const saved = JSON.parse(localStorage.getItem(STORAGE_KEY) || localStorage.getItem(LEGACY_STORAGE_KEY) || 'null');
  if (saved) {
    applySavedState(saved);
    if (saved.version !== 2) {
      state.preset = 'aurora';
      state.shaderSource = PRESETS.find((p) => p.id === 'aurora').source;
    }
  }
} catch { storageAvailable = false; }

function applySavedState(saved) {
  if (!saved || typeof saved !== 'object') return;
  if (PRESETS.some((p) => p.id === saved.preset) || saved.preset === 'custom') state.preset = saved.preset;
  if (Number.isFinite(Number(saved.opacity))) state.opacity = clamp(saved.opacity, 0, 100);
  if (Number.isFinite(Number(saved.speed))) state.speed = clamp(saved.speed, 0, 2);
  if ([0.25, 0.5, 1].includes(Number(saved.scale))) state.scale = Number(saved.scale);
  if (['violet', 'mint', 'amber', 'mono'].includes(saved.palette)) state.palette = saved.palette;
  if (['keystroke', 'interval', 'manual'].includes(saved.captureMode)) state.captureMode = saved.captureMode;
  if (Number.isFinite(Number(saved.textInfluence))) state.textInfluence = clamp(saved.textInfluence, 0, 1);
  if (Number.isFinite(Number(saved.feedback))) state.feedback = clamp(saved.feedback, 0, 0.98);
  if (typeof saved.fontSize === 'number' && Number.isFinite(saved.fontSize)) state.fontSize = clamp(Math.round(saved.fontSize / FONT_SIZE_STEP) * FONT_SIZE_STEP, FONT_SIZE_MIN, FONT_SIZE_MAX);
  for (const key of ['enabled', 'running', 'wrap', 'highlight', 'lines', 'blockFont']) if (typeof saved[key] === 'boolean') state[key] = saved[key];
  if (saved.document && typeof saved.document.text === 'string' && saved.document.text.length <= 1000000) {
    state.document.text = saved.document.text;
    if (typeof saved.document.name === 'string') state.document.name = saved.document.name.slice(0, 100).replace(/[\x00-\x1f]/g, '') || 'Untitled.txt';
  }
  if (typeof saved.shaderSource === 'string' && saved.shaderSource.length < 100000) state.shaderSource = saved.shaderSource;
  else state.shaderSource = PRESETS.find((p) => p.id === state.preset)?.source || PRESETS[0].source;
  if (typeof saved.customSource === 'string' && saved.customSource.length < 100000) state.customSource = saved.customSource;
}

const editor = $('editorInput');
const code = $('codeHighlight');
const histories = { document: [{ value: state.document.text, start: 0, end: 0 }], shader: [{ value: state.shaderSource, start: 0, end: 0 }] };
const historyIndex = { document: 0, shader: 0 };
const cursorMemory = { document: { start: 0, end: 0, top: 0, left: 0 }, shader: { start: 0, end: 0, top: 0, left: 0 } };
let engine;
let maskCapture;
let latestMask = null;
let lastUploadedRevision = 0;
let maskFormat = 'R8';
let maskPending = false;
let maskPreviewFrame = 0;
let maskFlashTimer;
let maskClock;
let saveTimer;
let toastTimer;
let lastTextInput = 0;
let findMatches = [];
let findIndex = -1;
let shaderAvailable = true;
let releaseTabKey = false;
let lastCompileSucceeded = true;
let thumbnailEngines = [];
let appliedTypography = '';
let typographyRevision = 0;
let typographyReady = Promise.resolve();
let measurements = { charWidth: 8.4, lineHeight: 27, paddingTop: 24, paddingLeft: 21, paddingRight: 23 };
let renderStats = { fps: 0, width: 0, height: 0, frame: 0 };

function showToast(message, duration = 3000) {
  clearTimeout(toastTimer);
  $('toast').textContent = message;
  $('toast').hidden = false;
  toastTimer = setTimeout(() => { $('toast').hidden = true; }, duration);
}
function saveSession() {
  $('saveState').textContent = 'Saving…';
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify(serializableState()));
      storageAvailable = true;
      $('saveState').textContent = 'Saved in this browser';
    } catch {
      storageAvailable = false;
      $('saveState').textContent = 'Session only · export to keep';
    }
  }, 450);
}
function serializableState() {
  return { version: 2, preset: state.preset, opacity: state.opacity, speed: state.speed, scale: state.scale, palette: state.palette, enabled: state.enabled, running: state.running, wrap: state.wrap, highlight: state.highlight, lines: state.lines, fontSize: state.fontSize, blockFont: state.blockFont, captureMode: state.captureMode, textInfluence: state.textInfluence, feedback: state.feedback, document: { ...state.document }, shaderSource: state.shaderSource, customSource: state.customSource };
}
function escapeHTML(value) {
  return value.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;').replace(/'/g, '&#39;');
}
function highlightCode(value) {
  const rx = /(\/\/[^\n]*|\/\*[\s\S]*?\*\/|"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|`(?:\\.|[^`\\])*`|\b(?:const|let|var|new|return|if|else|for|while|break|void|float|int|bool|vec[234]|mat[234]|uniform|varying|precision|highp|mediump|out|in|inout|true|false|function|class|import|from|export)\b|\b\d+(?:\.\d*)?\b|\b[a-zA-Z_]\w*(?=\s*\())/g;
  let html = '', last = 0, match;
  while ((match = rx.exec(value))) {
    html += escapeHTML(value.slice(last, match.index));
    const token = match[0];
    const cls = token.startsWith('//') || token.startsWith('/*') ? 'comment' : /^['"`]/.test(token) ? 'string' : /^\d/.test(token) ? 'number' : /^(const|let|var|new|return|if|else|for|while|break|void|float|int|bool|vec[234]|mat[234]|uniform|varying|precision|highp|mediump|out|in|inout|true|false|function|class|import|from|export)$/.test(token) ? 'keyword' : 'function';
    html += `<span class="tok-${cls}">${escapeHTML(token)}</span>`;
    last = rx.lastIndex;
  }
  return html + escapeHTML(value.slice(last));
}
function highlightMarkdown(value) {
  let fenced = false;
  return value.split('\n').map((line) => {
    if (/^\s*```/.test(line)) { fenced = !fenced; return `<span class="tok-comment">${escapeHTML(line)}</span>`; }
    if (fenced) return highlightCode(line);
    if (/^#{1,6}\s/.test(line)) return `<span class="tok-heading"><span class="tok-marker">${escapeHTML(line.match(/^#+/)[0])}</span>${escapeHTML(line.replace(/^#+/, ''))}</span>`;
    if (/^\s*\/\//.test(line)) return `<span class="tok-comment">${escapeHTML(line)}</span>`;
    return escapeHTML(line).replace(/(`[^`]+`)/g, '<span class="tok-inline">$1</span>').replace(/(\[[^\]]+\]\([^\s]+\))/g, '<span class="tok-link">$1</span>');
  }).join('\n');
}
function language() {
  if (state.activeTab === 'shader') return 'GLSL';
  const ext = state.document.name.split('.').pop().toLowerCase();
  return ({ md: 'Markdown', glsl: 'GLSL', frag: 'GLSL', js: 'JavaScript', ts: 'TypeScript', json: 'JSON', css: 'CSS', html: 'HTML', rs: 'Rust', py: 'Python' })[ext] || 'Plain text';
}
function updateMeasurements() {
  const css = getComputedStyle(editor);
  const measureCanvas = document.createElement('canvas');
  const ctx = measureCanvas.getContext('2d');
  ctx.font = css.font;
  measurements = { charWidth: ctx.measureText('M').width + (parseFloat(css.letterSpacing) || 0), lineHeight: parseFloat(css.lineHeight), paddingTop: parseFloat(css.paddingTop), paddingLeft: parseFloat(css.paddingLeft), paddingRight: parseFloat(css.paddingRight) };
}
function applyEditorTypography() {
  const key = `${state.fontSize}:${state.blockFont}`;
  if (key === appliedTypography) return typographyReady;
  appliedTypography = key;
  const revision = ++typographyRevision;
  const panel = $('editorPanel');
  panel.style.setProperty('--text-size', `${state.fontSize}px`);
  panel.style.setProperty('--line-height', `${state.fontSize + (state.blockFont ? 12 : 13)}px`);
  panel.style.setProperty('--editor-font', state.blockFont ? 'EditBlock, var(--mono)' : 'var(--mono)');
  panel.style.setProperty('--editor-spacing', state.blockFont ? '0px' : '.05px');
  // Both visual text layers use these scoped styles; the mask reads the textarea's computed font.
  updateMeasurements();
  updateTextView();
  typographyReady = document.fonts.load(`${state.fontSize}px ${state.blockFont ? 'EditBlock' : 'EditMono'}`, 'Ag09_{}[]();').then(() => {
    if (revision !== typographyRevision) return;
    updateMeasurements();
    updateTextView();
    requestTextMask('typography', true);
  }).catch(() => {
    if (revision !== typographyRevision) return;
    showToast('The selected font could not load. The editor is using its fallback font.', 4000);
    updateMeasurements(); updateTextView(); requestTextMask('typography', true);
  });
  return typographyReady;
}
function setFontSize(size) {
  state.fontSize = clamp(Math.round(size / FONT_SIZE_STEP) * FONT_SIZE_STEP, FONT_SIZE_MIN, FONT_SIZE_MAX);
  updateControls();
  saveSession();
}
function columnsPerLine() {
  return Math.max(1, Math.floor((editor.clientWidth - measurements.paddingLeft - measurements.paddingRight) / measurements.charWidth));
}
function lineRows(line) {
  return state.wrap ? Math.max(1, Math.ceil(line.replace(/\t/g, '  ').length / columnsPerLine())) : 1;
}
function updateTextView() {
  const text = editor.value;
  if (text.length > 120000 || !state.highlight) code.textContent = text + '\n';
  else code.innerHTML = (language() === 'Markdown' ? highlightMarkdown(text) : highlightCode(text)) + '\n';
  const lines = text.split('\n');
  $('lineNumbers').innerHTML = lines.map((line, index) => `<span data-line="${index + 1}" style="height:${lineRows(line) * measurements.lineHeight}px">${index + 1}</span>`).join('');
  $('languageStatus').textContent = language();
  syncScroll();
  updateCursor();
}
function getTextColorRuns(text) {
  if (!state.highlight) return { baseColor: getComputedStyle(editor).color, runs: [] };
  return collectTextColors(code, text);
}
function syncScroll() {
  code.style.transform = `translate(${-editor.scrollLeft}px, ${-editor.scrollTop}px)`;
  $('lineNumbers').style.transform = `translateY(${-editor.scrollTop}px)`;
  updateCursor();
}
function updateCursor() {
  const before = editor.value.slice(0, editor.selectionStart);
  const split = before.split('\n');
  const line = split.length;
  const col = split[split.length - 1].length + 1;
  const selected = editor.selectionEnd - editor.selectionStart;
  $('cursorStatus').textContent = `Ln ${line}, Col ${col}${selected ? ` (${selected})` : ''}`;
  const current = $('lineNumbers').querySelector('.current');
  if (current) current.classList.remove('current');
  $('lineNumbers').children[line - 1]?.classList.add('current');
  let row = line - 1;
  if (state.wrap) {
    row = split.slice(0, -1).reduce((sum, part) => sum + lineRows(part), 0) + Math.floor((col - 1) / columnsPerLine());
  }
  $('activeLine').style.top = `${measurements.paddingTop + row * measurements.lineHeight - editor.scrollTop}px`;
}
function commitInput(forceHistory = false) {
  const key = state.activeTab;
  if (key === 'document') state.document.text = editor.value;
  else state.shaderSource = editor.value;
  const history = histories[key];
  const now = performance.now();
  if (history[historyIndex[key]]?.value !== editor.value) {
    history.splice(historyIndex[key] + 1);
    const snapshot = { value: editor.value, start: editor.selectionStart, end: editor.selectionEnd };
    if (!forceHistory && now - lastTextInput < 500 && historyIndex[key] > 0) history[historyIndex[key]] = snapshot;
    else { history.push(snapshot); historyIndex[key]++; }
    if (history.length > 150) { history.shift(); historyIndex[key]--; }
    lastTextInput = now;
  }
  updateTextView();
  if (!$('findBar').hidden) refreshFind(false);
  if (key === 'shader') $('compileStatus').textContent = 'Changes waiting to run';
  requestTextMask('edit');
  saveSession();
}
function undoRedo(direction) {
  const key = state.activeTab;
  const next = historyIndex[key] + direction;
  if (next < 0 || next >= histories[key].length) { showToast(direction < 0 ? 'Nothing to undo' : 'Nothing to redo', 1600); return; }
  historyIndex[key] = next;
  const snapshot = histories[key][next];
  editor.value = snapshot.value;
  if (key === 'document') state.document.text = editor.value; else state.shaderSource = editor.value;
  editor.focus();
  editor.setSelectionRange(snapshot.start, snapshot.end);
  updateTextView();
  lastTextInput = 0;
  requestTextMask('undo-redo');
  saveSession();
}
function switchTab(tab) {
  lastTextInput = 0;
  cursorMemory[state.activeTab] = { start: editor.selectionStart, end: editor.selectionEnd, top: editor.scrollTop, left: editor.scrollLeft };
  state.activeTab = tab;
  $('documentTab').classList.toggle('active', tab === 'document');
  $('shaderTab').classList.toggle('active', tab === 'shader');
  $('documentTab').setAttribute('aria-selected', String(tab === 'document'));
  $('shaderTab').setAttribute('aria-selected', String(tab === 'shader'));
  $('editorPanel').setAttribute('aria-labelledby', tab === 'document' ? 'documentTab' : 'shaderTab');
  $('studio').classList.toggle('shader-tab', tab === 'shader');
  $('compileBar').hidden = tab !== 'shader';
  $('compileError').hidden = tab !== 'shader' || lastCompileSucceeded;
  editor.value = tab === 'document' ? state.document.text : state.shaderSource;
  editor.setAttribute('aria-label', tab === 'document' ? 'Document text' : 'Fragment shader source');
  const memory = cursorMemory[tab];
  editor.setSelectionRange(memory.start, memory.end);
  editor.scrollTop = memory.top;
  editor.scrollLeft = memory.left;
  updateTextView();
  if (!$('findBar').hidden) refreshFind(false);
  requestTextMask('tab', true);
}
function rangeProgress(input) {
  const progress = ((Number(input.value) - Number(input.min)) / (Number(input.max) - Number(input.min))) * 100;
  input.style.background = `linear-gradient(to right,#c5dba8 ${progress}%,#3b433e ${progress}%)`;
}
function updateControls() {
  $('shaderEnabled').checked = state.enabled;
  $('studio').classList.toggle('shader-off', !state.enabled);
  $('studio').classList.toggle('wrap-on', state.wrap);
  $('studio').classList.toggle('syntax-off', !state.highlight);
  $('studio').classList.toggle('no-lines', !state.lines);
  editor.wrap = state.wrap ? 'soft' : 'off';
  $('documentScrim').style.opacity = String(1 - state.opacity / 100);
  $('opacityRange').value = state.opacity;
  $('opacityValue').innerHTML = `${state.opacity}<span>%</span>`;
  $('speedRange').value = state.speed;
  $('speedValue').innerHTML = `${Number(state.speed).toFixed(1)}<span>×</span>`;
  $('influenceRange').value = state.textInfluence;
  $('influenceValue').innerHTML = `${Math.round(state.textInfluence * 100)}<span>%</span>`;
  $('feedbackRange').value = state.feedback;
  $('feedbackValue').innerHTML = `${Math.round(state.feedback * 100)}<span>%</span>`;
  rangeProgress($('opacityRange')); rangeProgress($('speedRange')); rangeProgress($('influenceRange')); rangeProgress($('feedbackRange'));
  document.querySelectorAll('[data-cadence]').forEach((b) => { const active = b.dataset.cadence === state.captureMode; b.classList.toggle('selected', active); b.setAttribute('aria-pressed', String(active)); });
  $('upscaleTag').textContent = state.scale === 1 ? 'Native resolution' : `${1 / state.scale}× upscale`;
  document.querySelectorAll('[data-scale]').forEach((b) => { const active = Number(b.dataset.scale) === state.scale; b.classList.toggle('selected', active); b.setAttribute('aria-pressed', String(active)); });
  document.querySelectorAll('[data-palette]').forEach((b) => { const active = b.dataset.palette === state.palette; b.classList.toggle('selected', active); b.setAttribute('aria-pressed', String(active)); });
  $('paletteRow').hidden = state.preset === 'life';
  $('lifeColorNote').hidden = state.preset !== 'life';
  document.querySelectorAll('[data-preset]').forEach((b) => { const active = b.dataset.preset === state.preset; b.classList.toggle('selected', active); b.setAttribute('aria-pressed', String(active)); });
  const preset = PRESETS.find((p) => p.id === state.preset);
  $('playbackTitle').textContent = preset?.name || 'Custom shader';
  $('stagePresetName').textContent = (preset?.name || 'CUSTOM').toUpperCase();
  const subtitles = { aurora: 'Light pulled from your letters', wave: 'Glyphs become a moving wavefront', cubes: 'Your text, lifted into geometry', contour: 'Contours grown from glyph coverage', dither: 'Letterforms dissolved into pixels', echo: 'The memory of words in motion', life: 'New letters join the living world', matrix: 'Your words become digital rain' };
  $('playbackSubtitle').textContent = state.enabled ? (subtitles[state.preset] || 'An atmosphere of your own') : 'Shader mode is off';
  $('presetDescription').textContent = preset?.description || 'Your fragment, your atmosphere.';
  $('lifeControls').hidden = state.preset !== 'life';
  $('clearTrailsButton').textContent = state.preset === 'life' ? 'Restart life' : 'Clear trails';
  $('clearTrailsButton').title = state.preset === 'life' ? 'Restart Conway from the current captured text' : 'Clear the evolving feedback field';
  $('playButton').setAttribute('aria-label', state.running ? 'Pause shader' : 'Play shader');
  $('playButton').title = state.running ? 'Pause shader' : 'Play shader';
  $('playIcon').innerHTML = `<use href="#i-${state.running ? 'pause' : 'play'}"/>`;
  $('playButton').disabled = !state.enabled || !shaderAvailable;
  for (const [id, active] of [['menuShader', state.enabled], ['menuWrap', state.wrap], ['menuHighlight', state.highlight], ['menuLines', state.lines], ['menuBlockFont', state.blockFont]]) {
    $(id).setAttribute('aria-checked', String(active));
    $(id).querySelector('.menu-check').textContent = active ? '✓' : '';
  }
  $('fontSizeValue').textContent = `${state.fontSize} px`;
  $('fontSizeDecrease').disabled = state.fontSize <= FONT_SIZE_MIN;
  $('fontSizeIncrease').disabled = state.fontSize >= FONT_SIZE_MAX;
  applyEditorTypography();
  $('tabFilename').textContent = state.document.name;
  document.title = `${state.document.name} — edit / shader studio`;
  updateRenderStatus();
  updateMaskStatus();
}
function updateRenderStatus() {
  const active = state.running && state.enabled && state.speed > 0 && shaderAvailable;
  $('renderStatus').textContent = !shaderAvailable ? 'WebGL unavailable' : !state.enabled ? 'Shader off' : !state.running ? 'Frame paused' : state.speed === 0 ? 'Time frozen' : 'Shader live';
  $('liveDot').style.background = active ? '#b8d59b' : '#879587';
  $('fpsValue').textContent = active ? Math.round(renderStats.fps || 0) || '—' : '0';
  $('resolutionValue').textContent = shaderAvailable ? `${$('shaderCanvas').width} × ${$('shaderCanvas').height}` : '— × —';
  if (state.preset === 'life') $('lifeGeneration').textContent = `Generation ${engine?.lifeGeneration || 0}`;
}

// The only connection from text to the renderer is a queued, immutable R8 image.
function requestTextMask(reason = 'edit', force = false) {
  if (!maskCapture) return;
  maskPending = true;
  // Interval mode holds the image for its whole tick, including edit-induced scroll.
  maskCapture.request(reason, force && state.captureMode === 'keystroke');
  updateMaskStatus();
}
function acceptTextMask(mask) {
  latestMask = mask;
  maskPending = false;
  engine?.setTextMask(mask);
  // These are real shader previews driven by the same captured text, held between captures.
  for (const preview of thumbnailEngines) preview.setTextMask(mask);
  cancelAnimationFrame(maskPreviewFrame);
  maskPreviewFrame = requestAnimationFrame(() => {
    drawMaskPreview($('maskPreview'), mask);
    if ($('maskDialog').open) updateMaskInspector();
  });
  clearTimeout(maskFlashTimer);
  $('studio').classList.add('mask-upload-flash');
  maskFlashTimer = setTimeout(() => $('studio').classList.remove('mask-upload-flash'), 300);
  updateMaskStatus();
}
function maskAge() {
  if (!latestMask) return 0;
  const clock = latestMask.capturedAt > 1000000000000 ? Date.now() : performance.now();
  return Math.max(0, (clock - latestMask.capturedAt) / 1000);
}
function updateMaskStatus() {
  const pendingLabel = state.captureMode === 'manual' ? (maskPending ? 'Edits held' : 'Input held') : state.captureMode === 'interval' ? (maskPending ? 'Next tick' : '3s cadence') : maskPending ? 'Queued' : 'Live';
  $('maskPendingState').textContent = latestMask ? pendingLabel : 'Starting';
  $('studio').classList.toggle('mask-pending', maskPending);
  $('maskFormatBadge').textContent = maskFormat.includes('LUMINANCE') ? 'L8 ALPHA' : 'R8 ALPHA';
  if (!latestMask) return;
  const mask = latestMask;
  $('maskDimensions').textContent = `${mask.width} × ${mask.height} · ${Math.round(mask.data.byteLength / 1024)} KiB`;
  $('maskRevision').textContent = `#${mask.revision}`;
  $('snapshotAge').textContent = `${maskAge().toFixed(1)}s since capture`;
  $('stageMaskVersion').textContent = `TEXT #${lastUploadedRevision || '…'}`;
  if (state.preset === 'life') $('lifeGeneration').textContent = `Generation ${engine?.lifeGeneration || 0}`;
  if ($('maskDialog').open) {
    $('maskDialogRevision').textContent = `Snapshot #${mask.revision} · ${maskAge().toFixed(1)}s old`;
    $('maskDialogFormat').textContent = `${maskFormat} · ${mask.width} × ${mask.height} · one byte per pixel`;
  }
}
function drawMaskPreview(canvas, mask) {
  if (!mask) return;
  const native = document.createElement('canvas');
  native.width = mask.width; native.height = mask.height;
  const ctx = native.getContext('2d');
  const image = ctx.createImageData(mask.width, mask.height);
  for (let p = 0, out = 0; p < mask.data.length; p++, out += 4) {
    image.data[out] = mask.data[p]; image.data[out + 1] = mask.data[p]; image.data[out + 2] = mask.data[p]; image.data[out + 3] = 255;
  }
  ctx.putImageData(image, 0, 0);
  const wide = canvas.id === 'maskInspectorCanvas';
  const width = wide ? mask.width : Math.max(220, Math.round(canvas.clientWidth * 2));
  const height = wide ? mask.height : Math.round(width * 0.38);
  canvas.width = width; canvas.height = height;
  const preview = canvas.getContext('2d');
  preview.fillStyle = '#000'; preview.fillRect(0, 0, width, height);
  const ratio = Math.min(width / mask.width, height / mask.height);
  preview.drawImage(native, (width - mask.width * ratio) / 2, (height - mask.height * ratio) / 2, mask.width * ratio, mask.height * ratio);
}
function updateMaskInspector() {
  if (!latestMask) { $('maskDialogStats').textContent = 'Waiting for the first glyph pass.'; return; }
  drawMaskPreview($('maskInspectorCanvas'), latestMask);
  const coverage = latestMask.coverage > 1 ? latestMask.coverage : latestMask.coverage * 100;
  $('maskDialogStats').replaceChildren();
  for (const text of [`${latestMask.data.byteLength.toLocaleString()} bytes`, `${Number(coverage || 0).toFixed(2)}% coverage`, `${(latestMask.glyphPixels || 0).toLocaleString()} nonzero pixels`]) {
    const span = document.createElement('span'); span.textContent = text; $('maskDialogStats').append(span);
  }
  $('maskDialogFormat').textContent = `${maskFormat} · ${latestMask.width} × ${latestMask.height} · one byte per pixel`;
  $('maskDialogRevision').textContent = `Snapshot #${latestMask.revision} · ${maskAge().toFixed(1)}s old`;
}
function downloadRawMask() {
  if (!latestMask) return;
  const { width, height, revision, data } = latestMask;
  download(data, `glyph-mask-${width}x${height}-r${revision}.r8`, 'application/octet-stream');
  showToast(`Raw R8: ${width} × ${height}, top-down rows, one alpha byte per pixel.`, 4500);
}
function applyEngineConfig() {
  engine?.setConfig({ speed: state.speed, scale: state.scale, palette: state.palette, textInfluence: state.textInfluence, feedback: state.feedback });
  engine?.setRunning(state.running);
  engine?.setEnabled(state.enabled);
}
function setPreset(id) {
  const preset = PRESETS.find((p) => p.id === id);
  if (!preset) return;
  const result = engine?.setPreset(id);
  if (result && !result.ok) { showToast(result.error || 'This shader could not compile.'); return; }
  state.preset = id;
  state.shaderSource = preset.source;
  lastCompileSucceeded = true;
  $('compileError').hidden = true;
  $('compileStatus').textContent = 'Preset loaded · ready to experiment';
  histories.shader = [{ value: state.shaderSource, start: 0, end: 0 }]; historyIndex.shader = 0;
  cursorMemory.shader = { start: 0, end: 0, top: 0, left: 0 };
  if (state.activeTab === 'shader') { editor.value = state.shaderSource; editor.scrollTop = 0; updateTextView(); requestTextMask('preset', true); }
  updateControls(); saveSession();
}
function compileShader() {
  if (!shaderAvailable) { showToast('WebGL is unavailable in this browser.'); return; }
  const result = engine.compile(state.shaderSource);
  lastCompileSucceeded = result.ok;
  $('compileError').hidden = result.ok;
  if (result.ok) {
    state.preset = 'custom';
    state.customSource = state.shaderSource;
    $('compileStatus').textContent = 'Compiled successfully · shader updated';
    updateControls(); saveSession(); showToast('Your shader is running.');
  } else {
    $('compileError').textContent = String(result.error || 'Shader compilation failed.');
    $('compileStatus').textContent = 'Compile failed · previous shader retained';
    showToast('Compile failed. Your last working shader is still here.', 4000);
  }
}
function toggleEnabled() {
  state.enabled = !state.enabled; engine?.setEnabled(state.enabled); updateControls(); saveSession();
}
function togglePlayback() {
  if (!state.enabled || !shaderAvailable) return;
  state.running = !state.running; engine?.setRunning(state.running); updateControls(); saveSession();
}
function closeMenus() {
  document.querySelectorAll('.dropdown').forEach((menu) => { menu.hidden = true; });
  document.querySelectorAll('.menu-trigger').forEach((trigger) => trigger.setAttribute('aria-expanded', 'false'));
}
function openMenu(trigger) {
  const menu = $(trigger.dataset.menu), wasOpen = !menu.hidden;
  closeMenus();
  if (!wasOpen) { menu.hidden = false; trigger.setAttribute('aria-expanded', 'true'); }
}
function toggleFocus() {
  state.focus = !state.focus;
  document.body.classList.toggle('focus-mode', state.focus);
  $('focusButton').setAttribute('aria-label', state.focus ? 'Exit focus mode' : 'Enter focus mode');
  $('focusButton').title = state.focus ? 'Exit focus mode' : 'Focus mode';
  requestAnimationFrame(() => { updateMeasurements(); updateTextView(); });
}
function openFind() {
  $('findBar').hidden = false;
  const selected = editor.value.slice(editor.selectionStart, editor.selectionEnd);
  if (selected && !selected.includes('\n')) $('findInput').value = selected;
  $('findInput').focus(); $('findInput').select(); refreshFind(false);
}
function refreshFind(selectFirst = true) {
  const needle = $('findInput').value.toLowerCase();
  findMatches = []; findIndex = -1;
  if (needle) {
    const haystack = editor.value.toLowerCase();
    let at = 0, found;
    while ((found = haystack.indexOf(needle, at)) !== -1) { findMatches.push(found); at = found + needle.length; if (findMatches.length >= 10000) break; }
  }
  $('findCount').textContent = `${findMatches.length} result${findMatches.length === 1 ? '' : 's'}`;
  if (selectFirst && findMatches.length) nextFind(1, false);
}
function nextFind(direction = 1, focus = true) {
  if (!findMatches.length) return;
  findIndex = findIndex < 0 ? (direction < 0 ? findMatches.length - 1 : 0) : (findIndex + direction + findMatches.length) % findMatches.length;
  const at = findMatches[findIndex];
  if (focus) editor.focus();
  editor.setSelectionRange(at, at + $('findInput').value.length);
  const lines = editor.value.slice(0, at).split('\n');
  const rows = state.wrap ? lines.slice(0, -1).reduce((n, line) => n + lineRows(line), 0) : lines.length - 1;
  const y = rows * measurements.lineHeight;
  if (y < editor.scrollTop || y > editor.scrollTop + editor.clientHeight - 80) editor.scrollTop = Math.max(0, y - editor.clientHeight / 3);
  $('findCount').textContent = `${findIndex + 1} / ${findMatches.length}`;
  updateCursor(); syncScroll();
}
function download(content, name, type) {
  const url = URL.createObjectURL(new Blob([content], { type }));
  const anchor = document.createElement('a'); anchor.href = url; anchor.download = name; document.body.append(anchor); anchor.click(); anchor.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
function exportSetup() {
  download(JSON.stringify(serializableState(), null, 2), 'edit-shader-studio.json', 'application/json');
  showToast('Setup exported with your document and shader source.');
}
function saveDocument() {
  download(state.activeTab === 'shader' ? state.shaderSource : state.document.text, state.activeTab === 'shader' ? 'shader.glsl' : state.document.name, 'text/plain;charset=utf-8');
  showToast(state.activeTab === 'shader' ? 'Shader source downloaded.' : 'Document downloaded.');
}
function showDialog(title, html) {
  $('dialogTitle').textContent = title; $('dialogBody').innerHTML = html; $('infoDialog').showModal();
}
function showAbout() {
  showDialog('The text is the texture.', '<p><strong>edit / shader studio</strong> sends the shapes of your letters into eight GLSL effects. Aurora, Wave, Cubes, Contours, Dither, Echo, Game of Life, and Matrix rain all use the captured glyph image.</p><p>The visible editor text is drawn onto a separate <strong>transparent text surface</strong>. Its alpha bytes become a one-channel mask: white for covered pixels, black for empty space. WebGL 2 uploads <code>R8 / RED</code>; the WebGL 1 fallback uses one-channel <code>LUMINANCE</code>. The shader samples coverage through <code>.r</code>.</p><p><strong>Each key</strong> queues a new snapshot after the text update. <strong>3 seconds</strong> captures at a fixed cadence, even when the text is unchanged. <strong>Manual</strong> holds the input while the shader continues evolving. Inspect the glyph preview to see the actual captured image.</p><p><strong>Game of Life</strong> adds new glyph pixels to a persistent grid evolving under Conway’s B3/S23 rules. Typing adds cells without clearing the world or resetting its generation counter. Spaces, tabs, newlines, and unchanged snapshots do not emit. Removing text never erases existing cells. Scrolling refreshes the world from the newly captured visible text, including when the caret scrolls the editor. Use Restart life or the restart-animation button to start again at any time. The speed control changes the generation rate; compiling a shader keeps the world. <strong>Matrix rain</strong> turns text-derived energy into green falling glyph streams.</p><p><strong>Life colors follow your text.</strong> New cells inherit the syntax color of their source glyph. Same-color parents keep that color; births from different colors and incoming ink landing on a live cell mix each RGB channel with wraparound addition. Surviving cells and their trails retain their color. These color rules never change Conway’s birth or survival rules.</p><p>The feedback field uses two GPU buffers: one is read while the other receives the next state. The last glyph mask emits continuously, and a new snapshot adds a pulse. Text editing never waits for the shader and never reads back a shader frame.</p><p>In <strong>shader.glsl</strong>, <code>iChannel0</code> is the raw text mask and <code>iChannel1</code> is the evolving field. <code>iChannel2</code> exposes the discrete Life state when that pass is active, with <code>u_lifeGeneration</code> for its generation count. <code>iChannel3</code> holds the Life cells’ RGB colors. You also have <code>iChannelResolution</code>, <code>u_maskAge</code>, <code>u_maskRevision</code>, <code>u_emissionRevision</code>, and <code>u_textInfluence</code>, alongside time, resolution, pointer, and palette uniforms.</p><p>The menu and status bars stay solid; only the document background is blended at 50% by default. This browser studio uses separate update paths on the main thread. Native TRUEOS renderer integration and worker isolation are not implemented here.</p><p><strong>Block font:</strong> Edit Pixel Mono is a fixed-width ASCII adaptation of <a href="https://fontstruct.com/fontstructions/show/908795" target="_blank" rel="noopener noreferrer">Pixel UniCode by ivancr72</a>, licensed under <a href="https://creativecommons.org/licenses/by/3.0/" target="_blank" rel="noopener noreferrer">CC BY 3.0</a>. Glyph shapes are retained, character spacing is made uniform, and the font is subsetted for the web. <a href="./fonts/edit-pixel-mono-attribution.txt" target="_blank" rel="noopener noreferrer">Font credits and changes ↗</a></p>');
}
function showShortcuts() {
  const rows = [['Save current file', 'Ctrl / ⌘ S'], ['Find in document', 'Ctrl / ⌘ F'], ['Undo / redo', 'Ctrl / ⌘ Z / ⇧ Z'], ['Run fragment shader', 'Ctrl / ⌘ Enter'], ['Toggle shader mode', 'Alt S'], ['Toggle word wrap', 'Alt Z'], ['Indent / unindent', 'Tab / Shift Tab'], ['Leave the editor with keyboard', 'Escape, then Tab'], ['Close menus or find', 'Escape']];
  showDialog('Keyboard shortcuts', '<p>Click the document and start typing. Your usual selection and clipboard shortcuts work here.</p>' + rows.map(([label, keys]) => `<div class="shortcut-row"><span>${label}</span><kbd>${keys}</kbd></div>`).join(''));
}
function newDocument() {
  if (state.document.text && state.document.text !== initialText) {
    showDialog('Create a new document?', '<p>Your current document will be replaced in this browser. Download it first if you want to keep a copy.</p><div style="display:flex;gap:10px;margin-top:20px"><button class="button" id="newSaveFirst">Save current document</button><button class="button primary" id="newConfirm">Create blank document</button></div>');
    $('newSaveFirst').onclick = () => download(state.document.text, state.document.name, 'text/plain;charset=utf-8');
    $('newConfirm').onclick = () => { $('infoDialog').close(); replaceDocument('', 'Untitled.txt'); };
  } else replaceDocument('', 'Untitled.txt');
}
function replaceDocument(text, name) {
  state.document = { text: text.replace(/\r\n?/g, '\n'), name };
  histories.document = [{ value: state.document.text, start: 0, end: 0 }]; historyIndex.document = 0;
  cursorMemory.document = { start: 0, end: 0, top: 0, left: 0 };
  switchTab('document'); editor.setSelectionRange(0, 0); editor.scrollTop = 0; updateControls(); saveSession(); editor.focus();
}
function resetLook() {
  const retained = { document: state.document, shaderSource: state.shaderSource, wrap: state.wrap, highlight: state.highlight, lines: state.lines, fontSize: state.fontSize, blockFont: state.blockFont };
  Object.assign(state, defaults, retained);
  setPreset('aurora'); maskCapture?.setMode(state.captureMode); applyEngineConfig(); updateControls(); maskCapture?.captureNow(); saveSession(); showToast('Aurora restored. Your words remain the source.');
}
const actions = {
  new: newDocument, open: () => $('openFileInput').click(), save: saveDocument, export: exportSetup, import: () => $('importFileInput').click(),
  undo: () => undoRedo(-1), redo: () => undoRedo(1), find: openFind, select: () => { editor.focus(); editor.select(); updateCursor(); }, shader: toggleEnabled,
  wrap: () => { state.wrap = !state.wrap; updateControls(); updateTextView(); requestTextMask('wrap', true); saveSession(); },
  highlight: () => { state.highlight = !state.highlight; updateControls(); updateTextView(); requestTextMask('syntax-colors', true); saveSession(); },
  lines: () => { state.lines = !state.lines; updateControls(); updateTextView(); requestTextMask('gutter', true); saveSession(); },
  'font-smaller': () => setFontSize(state.fontSize - FONT_SIZE_STEP),
  'font-larger': () => setFontSize(state.fontSize + FONT_SIZE_STEP),
  'font-reset': () => setFontSize(defaults.fontSize),
  'block-font': () => { state.blockFont = !state.blockFont; updateControls(); saveSession(); },
  focus: toggleFocus, shortcuts: showShortcuts, about: showAbout
};

// Text commits immediately. A separate glyph capture queues a texture; it never waits for a shader frame.
editor.addEventListener('input', () => commitInput());
editor.addEventListener('scroll', () => { syncScroll(); requestTextMask('scroll', true); }, { passive: true });
editor.addEventListener('click', updateCursor);
editor.addEventListener('keyup', updateCursor);
editor.addEventListener('select', updateCursor);
editor.addEventListener('keydown', (event) => {
  if (event.key === 'Escape') { releaseTabKey = true; return; }
  if (event.key === 'Tab') {
    if (releaseTabKey) { releaseTabKey = false; return; }
    event.preventDefault();
    let start = editor.selectionStart, end = editor.selectionEnd;
    const value = editor.value;
    if (start !== end || event.shiftKey) {
      const lineStart = value.lastIndexOf('\n', start - 1) + 1;
      const selection = value.slice(lineStart, end);
      const replacement = selection.split('\n').map((line) => event.shiftKey ? line.replace(/^( {1,2}|\t)/, '') : '  ' + line).join('\n');
      const firstDelta = replacement.split('\n')[0].length - selection.split('\n')[0].length;
      editor.setRangeText(replacement, lineStart, end, 'select');
      editor.setSelectionRange(Math.max(lineStart, start + firstDelta), end + replacement.length - selection.length);
    } else editor.setRangeText('  ', start, end, 'end');
    commitInput(true);
  } else releaseTabKey = false;
});
$('documentTab').onclick = () => switchTab('document');
$('shaderTab').onclick = () => switchTab('shader');
$('sourceButton').onclick = () => { switchTab('shader'); editor.focus(); };
$('compileButton').onclick = compileShader;
$('focusButton').onclick = toggleFocus;
$('playButton').onclick = togglePlayback;
$('shaderEnabled').onchange = toggleEnabled;
$('shaderIndicator').onclick = toggleEnabled;
$('resetButton').onclick = resetLook;
$('exportButton').onclick = exportSetup;
$('indentButton').onclick = () => showToast('Tab inserts two spaces. Shift + Tab removes an indent.');
$('opacityRange').oninput = (event) => { state.opacity = Number(event.target.value); updateControls(); saveSession(); };
$('speedRange').oninput = (event) => { state.speed = Number(event.target.value); engine?.setConfig({ speed: state.speed }); updateControls(); saveSession(); };
$('influenceRange').oninput = (event) => { state.textInfluence = Number(event.target.value); engine?.setConfig({ textInfluence: state.textInfluence }); updateControls(); saveSession(); };
$('feedbackRange').oninput = (event) => { state.feedback = Number(event.target.value); engine?.setConfig({ feedback: state.feedback }); updateControls(); saveSession(); };
document.querySelectorAll('[data-cadence]').forEach((button) => { button.onclick = () => { state.captureMode = button.dataset.cadence; maskCapture?.setMode(state.captureMode); if (state.captureMode === 'keystroke') maskCapture?.captureNow(); updateControls(); saveSession(); }; });
$('captureNowButton').onclick = () => maskCapture?.captureNow();
$('snapshotDialogButton').onclick = () => maskCapture?.captureNow();
$('maskInspectButton').onclick = () => { updateMaskInspector(); $('maskDialog').showModal(); };
$('closeMaskDialog').onclick = () => $('maskDialog').close();
$('clearTrailsButton').onclick = () => {
  if (state.preset === 'life') restartLife();
  else { engine?.clearFeedback(); showToast('Feedback cleared. The captured text is kept.', 2000); }
};
$('reseedLifeButton').onclick = restartLife;
$('downloadMaskButton').onclick = downloadRawMask;
document.querySelectorAll('[data-scale]').forEach((button) => { button.onclick = () => { state.scale = Number(button.dataset.scale); engine?.setConfig({ scale: state.scale }); updateControls(); saveSession(); }; });
document.querySelectorAll('[data-palette]').forEach((button) => { button.onclick = () => { state.palette = button.dataset.palette; engine?.setConfig({ palette: state.palette }); thumbnailEngines.forEach((e) => { e.setConfig({ palette: state.palette }); e.renderOnce(); }); updateControls(); saveSession(); }; });
document.querySelectorAll('.menu-trigger').forEach((button) => {
  button.onclick = (event) => { event.stopPropagation(); openMenu(button); };
  button.onkeydown = (event) => { if (event.key === 'ArrowDown') { event.preventDefault(); openMenu(button); $(button.dataset.menu).querySelector('button:not(:disabled),a')?.focus(); } };
});
document.querySelectorAll('[data-action]').forEach((button) => { button.onclick = () => { if (!button.hasAttribute('data-keep-open')) closeMenus(); actions[button.dataset.action]?.(); }; });
document.addEventListener('click', (event) => { if (!event.target.closest('.menu-wrap')) closeMenus(); });
document.querySelectorAll('.dropdown').forEach((menu) => menu.addEventListener('keydown', (event) => {
  if (!['ArrowDown', 'ArrowUp'].includes(event.key)) return;
  event.preventDefault(); const items = [...menu.querySelectorAll('button:not(:disabled),a')], index = items.indexOf(document.activeElement); items[(index + (event.key === 'ArrowDown' ? 1 : -1) + items.length) % items.length]?.focus();
}));
$('findInput').oninput = () => refreshFind(true);
$('findInput').onkeydown = (event) => { if (event.key === 'Enter') { event.preventDefault(); nextFind(event.shiftKey ? -1 : 1, false); } };
$('findNext').onclick = () => nextFind(1);
$('findPrevious').onclick = () => nextFind(-1);
$('findClose').onclick = () => { $('findBar').hidden = true; editor.focus(); };
$('closeDialog').onclick = () => $('infoDialog').close();
$('dialogDone').onclick = () => $('infoDialog').close();
$('infoDialog').addEventListener('click', (event) => { if (event.target === $('infoDialog')) { const r = $('infoDialog').getBoundingClientRect(); if (event.clientX < r.left || event.clientX > r.right || event.clientY < r.top || event.clientY > r.bottom) $('infoDialog').close(); } });

$('openFileInput').onchange = async (event) => {
  const file = event.target.files[0]; if (!file) return;
  if (file.size > 1000000) { showToast('Open a text file smaller than 1 MB for this studio.'); event.target.value = ''; return; }
  try { const text = await file.text(); if (text.includes('\0')) throw new Error('Choose a text file.'); replaceDocument(text, file.name); showToast(`Opened ${file.name}`); }
  catch (error) { showToast(error.message || 'Could not open that file.'); }
  event.target.value = '';
};
$('importFileInput').onchange = async (event) => {
  const file = event.target.files[0]; if (!file) return;
  try {
    if (file.size > 1500000) throw new Error('This setup is too large.');
    const data = JSON.parse(await file.text());
    if (![1, 2].includes(data.version) || typeof data.document?.text !== 'string' || ![...PRESETS.map((p) => p.id), 'custom'].includes(data.preset)) throw new Error('Choose an exported edit shader studio setup.');
    if (data.preset === 'custom') {
      const source = data.customSource || data.shaderSource;
      if (typeof source !== 'string') throw new Error('The setup has no shader source.');
      const result = engine?.compile(source);
      if (result && !result.ok) throw new Error('That setup has an invalid shader. Your current setup was kept.');
    }
    applySavedState(data);
    if (state.preset !== 'custom') { engine?.setPreset(state.preset); if (data.version === 1) state.shaderSource = PRESETS.find((p) => p.id === state.preset)?.source || state.shaderSource; }
    maskCapture?.setMode(state.captureMode);
    applyEngineConfig(); histories.document = [{ value: state.document.text, start: 0, end: 0 }]; histories.shader = [{ value: state.shaderSource, start: 0, end: 0 }]; historyIndex.document = 0; historyIndex.shader = 0;
    lastCompileSucceeded = true; $('compileError').hidden = true; switchTab('document'); updateControls(); await applyEditorTypography(); maskCapture?.captureNow(); saveSession(); showToast('Studio setup imported. Your text is the input.');
  } catch (error) { showToast(error.message || 'Could not import that setup.', 4000); }
  event.target.value = '';
};
document.addEventListener('keydown', (event) => {
  if ($('infoDialog').open || $('maskDialog').open) return;
  const key = event.key.toLowerCase(), mod = event.ctrlKey || event.metaKey;
  if (mod && key === 's') { event.preventDefault(); saveDocument(); }
  else if (mod && key === 'f') { event.preventDefault(); openFind(); }
  else if (mod && key === 'o') { event.preventDefault(); $('openFileInput').click(); }
  else if (mod && key === 'n') { event.preventDefault(); newDocument(); }
  else if (mod && key === 'z' && document.activeElement === editor) { event.preventDefault(); undoRedo(event.shiftKey ? 1 : -1); }
  else if (mod && key === 'y' && document.activeElement === editor) { event.preventDefault(); undoRedo(1); }
  else if (mod && event.key === 'Enter' && state.activeTab === 'shader') { event.preventDefault(); compileShader(); }
  else if (event.altKey && key === 's') { event.preventDefault(); toggleEnabled(); }
  else if (event.altKey && key === 'z') { event.preventDefault(); actions.wrap(); }
  else if (event.key === 'Escape') { closeMenus(); if (!$('findBar').hidden) { $('findBar').hidden = true; editor.focus(); } else if (state.focus) toggleFocus(); }
});

function buildPresets() {
  const labels = { aurora: 'Aurora', wave: 'Wave', cubes: 'Cubes', contour: 'Contours', dither: 'Dither', echo: 'Echo', life: 'Game of Life', matrix: 'Matrix rain' };
  $('presetGrid').innerHTML = PRESETS.map((preset) => `<button class="preset-card" data-preset="${preset.id}" aria-label="${escapeHTML(labels[preset.id] || preset.name)} shader" aria-pressed="false"><canvas aria-hidden="true" id="preview-${preset.id}"></canvas><span class="preset-caption"><span>${escapeHTML(labels[preset.id] || preset.name)}</span><span class="preset-check">✓</span></span></button>`).join('');
  document.querySelectorAll('[data-preset]').forEach((button) => { button.onclick = () => setPreset(button.dataset.preset); });
  for (const preset of PRESETS) {
    try {
      const thumb = new ShaderEngine($(`preview-${preset.id}`));
      thumb.setRunning(false); thumb.setConfig({ scale: 0.5, palette: state.palette }); thumb.setPreset(preset.id); thumb.renderOnce(); thumbnailEngines.push(thumb);
    } catch { $(`preview-${preset.id}`).style.background = 'linear-gradient(135deg,#50456f,#172d39)'; }
  }
}
function initEngine() {
  try {
    engine = new ShaderEngine($('shaderCanvas'), {
      onStats(stats) { renderStats = stats; updateRenderStatus(); },
      onMask(info) { lastUploadedRevision = info.revision; maskFormat = info.format || 'R8'; updateMaskStatus(); },
      onError(error) { if (!shaderAvailable) { const message = typeof error === 'string' ? error : error?.message; showToast(message || 'WebGL is unavailable. Text editing is ready.', 6000); } },
      onStatus(status) {
        if (['unavailable', 'context-lost'].includes(String(status))) shaderAvailable = false;
        else if (['running', 'paused', 'disabled'].includes(String(status))) shaderAvailable = true;
        updateControls();
      }
    });
    shaderAvailable = engine.available;
    if (state.preset === 'custom' && (state.customSource || state.shaderSource)) {
      const result = engine.compile(state.customSource || state.shaderSource);
      if (!result.ok) { state.preset = 'aurora'; engine.setPreset('aurora'); showToast('The saved shader could not compile. Aurora is loaded.'); }
    } else engine.setPreset(state.preset);
    applyEngineConfig();
    if (latestMask) engine.setTextMask(latestMask);
  } catch (error) {
    shaderAvailable = false; $('playbackSubtitle').textContent = 'Your editor still works'; showToast(error.message || 'WebGL is unavailable. Text editing is ready.', 6000);
  }
}
function restartLife() {
  if (!engine?.reseedLife()) return;
  updateRenderStatus();
  showToast('Life restarted from the captured text.', 2000);
}
$('restartButton').onclick = () => {
  if (!shaderAvailable) return;
  engine?.destroy(); initEngine(); updateControls(); showToast('Animation restarted.', 1700);
};
buildPresets();
initEngine();
editor.value = state.document.text;
updateControls();
updateMeasurements();
updateTextView();
if (!storageAvailable) $('saveState').textContent = 'Session only · export to keep';
document.fonts.ready.then(() => {
  updateMeasurements(); updateTextView();
  maskCapture = new TextMaskCapture({ editor, stage: $('editorPanel'), getWrap: () => state.wrap, getColorRuns: getTextColorRuns, onCapture: acceptTextMask, onError: (error) => { $('maskPendingState').textContent = 'Capture error'; showToast(error.message || String(error), 5000); } });
  maskCapture.setMode(state.captureMode);
  maskCapture.captureNow();
  maskClock = setInterval(updateMaskStatus, 400);
});
new ResizeObserver(() => { updateMeasurements(); if (state.wrap) updateTextView(); requestTextMask('resize', true); }).observe($('editingArea'));
document.addEventListener('visibilitychange', () => { if (engine) engine.setRunning(!document.hidden && state.running); });
window.addEventListener('pagehide', () => { clearTimeout(saveTimer); try { localStorage.setItem(STORAGE_KEY, JSON.stringify(serializableState())); } catch {} });
