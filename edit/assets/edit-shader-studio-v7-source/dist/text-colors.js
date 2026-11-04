// Read colors from the existing highlighted DOM, without retokenizing its text.
// Run offsets use UTF-16 code units, matching textarea.value and String.slice.
export function collectTextColors(highlightElement, text, { getStyle } = {}) {
  const source = typeof text === 'string' ? text : String(text ?? '');
  const view = highlightElement?.ownerDocument?.defaultView;
  const readStyle = getStyle || (typeof view?.getComputedStyle === 'function'
    ? view.getComputedStyle.bind(view)
    : typeof globalThis.getComputedStyle === 'function'
      ? globalThis.getComputedStyle.bind(globalThis) : null);
  const colors = new Map();
  const fallbackColor = 'rgb(220, 226, 232)';
  let baseColor = fallbackColor;
  function colorOf(element) {
    if (!element) return baseColor;
    if (colors.has(element)) return colors.get(element);
    let color = baseColor;
    try {
      const computed = readStyle?.(element)?.color;
      if (typeof computed === 'string' && computed.trim()) color = computed.trim();
    } catch { /* A detached/unsupported style reader must not fail capture. */ }
    colors.set(element, color);
    return color;
  }
  baseColor = colorOf(highlightElement);
  const fallback = { baseColor, runs: [] };
  if (!highlightElement) return fallback;

  const chunks = [], pieces = [];
  let offset = 0;
  function visit(node, owner) {
    if (node.nodeType === 3 || node.nodeType === 4) {
      const value = node.nodeValue ?? node.data ?? '';
      pieces.push(value);
      const end = offset + value.length;
      if (offset < source.length && end > offset) {
        chunks.push({ start: offset, end: Math.min(end, source.length), owner });
      }
      offset = end;
      return;
    }
    if (![1, 9, 11].includes(node.nodeType)) return;
    const nextOwner = node.nodeType === 1 ? node : owner;
    for (const child of node.childNodes || []) visit(child, nextOwner);
  }
  visit(highlightElement, highlightElement);
  const rendered = pieces.join('');
  // The editor appends one layout newline to its <pre>. Anything else signals
  // stale or unrelated markup; using its token positions would color wrong text.
  if (rendered !== source && rendered !== source + '\n') return fallback;

  const runs = [];
  for (const chunk of chunks) {
    const color = colorOf(chunk.owner);
    const previous = runs[runs.length - 1];
    if (previous && previous.end === chunk.start && previous.color === color) {
      previous.end = chunk.end;
    } else runs.push({ start: chunk.start, end: chunk.end, color });
  }
  return { baseColor, runs };
}
