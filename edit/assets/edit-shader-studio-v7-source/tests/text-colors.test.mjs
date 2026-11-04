import test from 'node:test';
import assert from 'node:assert/strict';
import { collectTextColors } from '../dist/text-colors.js';

const BASE = 'rgb(220, 226, 232)';
const KEYWORD = 'rgb(192, 173, 235)';
const NUMBER = 'rgb(227, 188, 147)';
const STRING = 'rgb(197, 216, 167)';
const textNode = (value) => ({ nodeType: 3, nodeValue: value });
function element(color, ...children) {
  const node = { nodeType: 1, color, childNodes: children.map((child) => typeof child === 'string' ? textNode(child) : child) };
  for (const child of node.childNodes) child.parentElement = node;
  return node;
}
function styles() {
  const reads = new Map();
  return {
    reads,
    getStyle(node) {
      reads.set(node, (reads.get(node) || 0) + 1);
      let current = node;
      while (current && !current.color) current = current.parentElement;
      return { color: current?.color || BASE };
    },
  };
}

test('nested syntax spans supply their actual computed colors and source offsets', () => {
  const keyword = element(KEYWORD, 'const');
  const number = element(NUMBER, '42');
  const pre = element(BASE, keyword, ' n = ', number, ';\n');
  const css = styles();
  const result = collectTextColors(pre, 'const n = 42;', css);
  assert.deepEqual(result, { baseColor: BASE, runs: [
    { start: 0, end: 5, color: KEYWORD },
    { start: 5, end: 10, color: BASE },
    { start: 10, end: 12, color: NUMBER },
    { start: 12, end: 13, color: BASE },
  ] });
  assert.equal(css.reads.get(pre), 1);
  assert.equal(css.reads.get(keyword), 1);
  assert.equal(css.reads.get(number), 1);
});

test('literal DOM text keeps entities, quotes, Unicode, tabs, and newline offsets intact', () => {
  const literal = '"<>&\'😀e\u0301"';
  const source = 'x\t= ' + literal + ';\nnext';
  const pre = element(BASE, 'x\t= ', element(STRING, literal), ';\nnext\n');
  const result = collectTextColors(pre, source, styles());
  const colored = result.runs.find((run) => run.color === STRING);
  assert.deepEqual(colored, { start: 4, end: 4 + literal.length, color: STRING });
  assert.equal(source.slice(colored.start, colored.end), literal);
  assert.equal(result.runs.at(-1).end, source.length);
  assert.equal(result.runs.map((run) => source.slice(run.start, run.end)).join(''), source);
});

test('inherited and adjacent equal colors merge while explicit black remains a color', () => {
  const nested = element(KEYWORD, 'A', element(undefined, 'B'), 'C');
  const pre = element(BASE, nested, element(KEYWORD, 'D'), element('rgb(0, 0, 0)', 'E'), '\n');
  const css = styles();
  assert.deepEqual(collectTextColors(pre, 'ABCDE', css).runs, [
    { start: 0, end: 4, color: KEYWORD },
    { start: 4, end: 5, color: 'rgb(0, 0, 0)' },
  ]);
  assert.equal(css.reads.get(nested), 1, 'Repeated text siblings share one style read');
});

test('the layout newline is excluded, including when the source itself ends in a newline', () => {
  const pre = element(BASE, 'line\n', element(KEYWORD, '\n'));
  const css = styles();
  assert.deepEqual(collectTextColors(pre, 'line\n', css).runs, [{ start: 0, end: 5, color: BASE }]);
  assert.equal(css.reads.size, 1, 'A layout-only node does not need a style lookup');
  assert.deepEqual(collectTextColors(element(BASE, '\n'), '', styles()), { baseColor: BASE, runs: [] });
});

test('stale or mismatched markup falls back without using incorrect token positions', () => {
  const pre = element(BASE, element(KEYWORD, 'old'), '\n');
  const css = styles();
  assert.deepEqual(collectTextColors(pre, 'new', css), { baseColor: BASE, runs: [] });
  assert.equal(css.reads.size, 1, 'Only the fallback base style is read for stale markup');
  assert.deepEqual(collectTextColors(element(BASE, 'constellation\n'), 'const', styles()), { baseColor: BASE, runs: [] });
  assert.deepEqual(collectTextColors(element(BASE, 'short'), 'longer text', styles()), { baseColor: BASE, runs: [] });
});

test('default style access uses the owning window and tolerates unavailable style readers', () => {
  const pre = element(BASE, 'A\n');
  const view = { getComputedStyle(node) { assert.equal(this, view); assert.equal(node, pre); return { color: BASE }; } };
  pre.ownerDocument = { defaultView: view };
  assert.deepEqual(collectTextColors(pre, 'A'), { baseColor: BASE, runs: [{ start: 0, end: 1, color: BASE }] });
  const unsupported = element(BASE, 'B\n');
  assert.deepEqual(collectTextColors(unsupported, 'B', { getStyle() { throw new Error('Unavailable'); } }), { baseColor: BASE, runs: [{ start: 0, end: 1, color: BASE }] });
  assert.deepEqual(collectTextColors(null, 'A'), { baseColor: BASE, runs: [] });
});
