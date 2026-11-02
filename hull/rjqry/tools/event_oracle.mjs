#!/usr/bin/env node
// Development-only differential oracle. Execute the pinned, original event
// registration/dispatch code on plain owner objects, without npm or a browser.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const document = {};
globalThis.window = { document };
const upstream = relative => path.join(root, 'jquery-main', 'src', relative);
const load = relative => import(pathToFileURL(upstream(relative)));
const { jQuery: $ } = await load('core.js');
const { Data } = await load('data/Data.js');
const { acceptData } = await load('data/var/acceptData.js');
const { nodeName } = await load('core/nodeName.js');
const { isWindow } = await load('var/isWindow.js');
await load('callbacks.js');
const dataPriv = new Data();

function isolated(relative, names, values) {
  const original = fs.readFileSync(upstream(relative), 'utf8');
  // Strip module wiring only. The original jQuery functions perform every
  // registration, namespace match, once removal, dispatch and trigger below.
  const body = original
    .replace(/^import\s+[^;]+;\s*/gm, '')
    .replace(/^export\s*\{[^}]*\};?\s*/gm, '');
  new Function(...names, body)(...values);
}

isolated('event.js',
  ['jQuery', 'documentElement', 'rnothtmlwhite', 'rcheckableType', 'slice', 'isIE', 'acceptData', 'dataPriv', 'nodeName'],
  [$, {}, /[^\x20\t\r\n\f]+/g, /^(?:checkbox|radio)$/i, Array.prototype.slice, false, acceptData, dataPriv, nodeName]);
isolated('event/trigger.js',
  ['jQuery', 'document', 'dataPriv', 'acceptData', 'hasOwn', 'isWindow'],
  [$, document, dataPriv, acceptData, Object.prototype.hasOwnProperty, isWindow]);

// Fixture initialization supports plain owners and the empty collection used by
// one().off(event). It does not implement selectors or a browser document.
$.fn.init = function(nodes) {
  if (nodes != null) $.merge(this, Array.isArray(nodes) ? nodes : [nodes]);
};
$.fn.init.prototype = $.fn;

function encode(value) {
  if (value === undefined) return { $undefined: true };
  if (Array.isArray(value)) return value.map(encode);
  if (value && typeof value === 'object') {
    return Object.fromEntries(Object.entries(value).map(([key, child]) => [key, encode(child)]));
  }
  return value;
}

function run(input) {
  const owner = {};
  const trace = [];
  const callbacks = new Map();
  const counts = new Map();
  const remaining = () => Object.values(dataPriv.get(owner, 'events') || {})
    .reduce((total, handlers) => total + handlers.length, 0);
  const flags = event => ({
    defaultPrevented: event.isDefaultPrevented(),
    propagationStopped: event.isPropagationStopped(),
    immediatePropagationStopped: event.isImmediatePropagationStopped()
  });

  function step(action) {
    switch (action.op) {
      case 'on':
      case 'one':
        $(owner)[action.op](action.types, action.data, callbacks.get(action.handler));
        break;
      case 'off':
        $(owner).off(action.types || '', action.handler ? callbacks.get(action.handler) : undefined);
        break;
      case 'trigger': {
        const event = $.Event(action.types);
        const result = $.event.trigger(event, action.args || [], owner, action.onlyHandlers || false);
        trace.push({ kind: 'trigger', type: action.types, onlyHandlers: !!action.onlyHandlers,
          result: encode(result), ...flags(event), remaining: remaining() });
        break;
      }
      case 'snapshot':
        trace.push({ kind: 'snapshot', remaining: remaining() });
        break;
      default:
        throw Error(`Unknown oracle action ${action.op}`);
    }
  }

  for (const descriptor of input.handlers) {
    callbacks.set(descriptor.id, function(event, ...args) {
      const count = (counts.get(descriptor.id) || 0) + 1;
      counts.set(descriptor.id, count);
      trace.push({ kind: 'handler', id: descriptor.id, type: event.type,
        namespace: event.namespace || '', bindingNamespace: event.handleObj.namespace,
        data: encode(event.data), args: encode(args), isTrigger: event.isTrigger, ...flags(event) });
      if (descriptor.control === 'prevent') event.preventDefault();
      if (descriptor.control === 'propagation') event.stopPropagation();
      if (descriptor.control === 'immediate') event.stopImmediatePropagation();
      if (descriptor.offEvent) $().off(event);
      if (!descriptor.actionsFirstOnly || count === 1) {
        for (const action of descriptor.actions || []) step(action);
      }
      return descriptor.returns;
    });
  }
  for (const action of input.steps) step(action);
  $(owner).off();
  return trace;
}

const cases = [];
const add = (name, handlers, steps) => {
  const input = { handlers, steps };
  cases.push({ name, input, result: run(input) });
};
const on = (handler, types, data) => ({ op: 'on', handler, types, ...(data === undefined ? {} : { data }) });
const one = (handler, types, data) => ({ ...on(handler, types, data), op: 'one' });
const trigger = (types, onlyHandlers = false, args) => ({ op: 'trigger', types, onlyHandlers, ...(args === undefined ? {} : { args }) });
const off = (types = '', handler) => ({ op: 'off', types, ...(handler === undefined ? {} : { handler }) });
const descriptors = ['plain', 'a', 'b', 'ab', 'bc'].map(id => ({ id }));
const bindings = [on('plain', 'custom'), on('a', 'custom.a'), on('b', 'custom.b'), on('ab', 'custom.b.a'), on('bc', 'custom.b.c')];

for (const namespace of ['', 'a', 'b', 'c', 'a.b', 'b.a', 'a.c', 'a.b.c', 'missing']) {
  for (const onlyHandlers of [false, true]) {
    const type = `custom${namespace ? '.' + namespace : ''}`;
    add(`namespace-${namespace || 'none'}-${onlyHandlers ? 'handler' : 'trigger'}`, descriptors,
      [...bindings, trigger(type, onlyHandlers), trigger(type, onlyHandlers)]);
  }
}

for (const selector of ['', '.a', '.b', '.a.b', '.b.a', '.missing', 'custom.a', 'other', '.a .b']) {
  add(`remove-${selector || 'all'}`, descriptors,
    [...bindings, off(selector), { op: 'snapshot' }, trigger('custom')]);
}

for (const order of ['on-first', 'one-first']) {
  for (const sameNamespace of [false, true]) {
    const first = on('shared', 'custom.a');
    const second = one('shared', sameNamespace ? 'custom.a' : 'custom.b');
    add(`once-shared-${order}-${sameNamespace ? 'same' : 'different'}`, [{ id: 'shared' }], [
      ...(order === 'on-first' ? [first, second] : [second, first]),
      trigger('custom'), trigger('custom'), trigger('custom')
    ]);
  }
}

add('once-multiple-types', [{ id: 'a' }], [one('a', 'alpha.ns beta.ns'), trigger('alpha'), trigger('alpha'), trigger('beta'), trigger('beta')]);
add('once-namespaced-filter', [{ id: 'a' }, { id: 'b' }], [one('a', 'custom.a'), one('b', 'custom.b'), trigger('custom.a'), trigger('custom.a'), trigger('custom'), trigger('custom')]);
add('once-duplicates-queued', [{ id: 'a' }], [one('a', 'custom.a'), one('a', 'custom.a'), trigger('custom'), trigger('custom')]);
add('same-function-remove-subset', [{ id: 'shared' }], [on('shared', 'custom.a'), on('shared', 'custom.b'), on('shared', 'other.a'), off('.a', 'shared'), trigger('custom'), trigger('other')]);
add('different-function-identity', [{ id: 'a' }, { id: 'b' }], [on('a', 'custom.a'), off('custom', 'b'), trigger('custom'), off('custom', 'a'), trigger('custom')]);
add('false-keeps-same-group', [{ id: 'a', returns: false }, { id: 'b' }, { id: 'c', returns: 42 }], [on('a', 'custom'), on('b', 'custom'), on('c', 'custom'), trigger('custom')]);
add('last-defined-return-value', [{ id: 'a', returns: 42 }, { id: 'b' }, { id: 'c', returns: null }, { id: 'd', returns: 'last' }], [on('a', 'custom'), on('b', 'custom'), trigger('custom', true), on('c', 'custom'), trigger('custom', true), on('d', 'custom'), trigger('custom', true)]);
for (const control of ['prevent', 'propagation', 'immediate']) {
  add(`control-${control}`, [{ id: 'a', control }, { id: 'b', returns: 'b' }], [on('a', 'custom'), on('b', 'custom'), trigger('custom')]);
}
add('remove-queued-next', [{ id: 'a', actions: [off('custom', 'b')] }, { id: 'b' }], [on('a', 'custom'), on('b', 'custom'), trigger('custom'), trigger('custom')]);
add('add-next-dispatch', [{ id: 'a', actionsFirstOnly: true, actions: [on('c', 'custom')] }, { id: 'b' }, { id: 'c' }], [on('a', 'custom'), on('b', 'custom'), trigger('custom'), trigger('custom')]);
add('off-event-shared-identity', [{ id: 'a', offEvent: true }], [on('a', 'custom.a'), on('a', 'custom.a.b'), on('a', 'custom.c'), trigger('custom.a'), trigger('custom')]);
add('once-removes-before-nested-trigger', [{ id: 'a', actionsFirstOnly: true, actions: [trigger('custom')] }, { id: 'b' }], [one('a', 'custom'), on('b', 'custom'), trigger('custom'), trigger('custom')]);
add('nested-trigger-namespace', [{ id: 'a', actionsFirstOnly: true, actions: [trigger('custom.b', true)] }, { id: 'b' }], [on('a', 'custom.a'), on('b', 'custom.b'), trigger('custom.a')]);
add('multiple-tokens-reverse-order', [{ id: 'shared' }], [on('shared', 'custom.a\tcustom.b\nother.c'), trigger('custom'), trigger('other'), off('.a\t.c'), trigger('custom')]);
add('trailing-empty-namespace', [{ id: 'a' }], [on('a', '.namespace'), on('a', 'custom.'), trigger('custom.'), off('custom.'), trigger('custom')]);
add('event-data-and-extra-arguments', [{ id: 'a' }, { id: 'b' }], [on('a', 'custom.a', { name: 'é', nested: [2, false] }), on('b', 'custom', null), trigger('custom', false, [1, 'extra', null, { x: true }]), trigger('custom.a', true, [])]);
add('repeated-namespace-tokens', [{ id: 'a' }, { id: 'b' }], [on('a', 'custom.a'), on('b', 'custom.a.a'), trigger('custom.a.a'), off('.a.a'), trigger('custom')]);
add('colon-custom-events', [{ id: 'a' }], [on('a', 'widget:update.a'), trigger('widget:update'), off('.a'), trigger('widget:update')]);

const output = JSON.stringify({ description: 'Expected event traces from original vendored jQuery source; see tools/event_oracle.mjs', cases }, null, 2) + '\n';
const destination = path.join(root, 'tests', 'fixtures', 'events.json');
if (process.argv.includes('--write')) {
  fs.mkdirSync(path.dirname(destination), { recursive: true });
  fs.writeFileSync(destination, output);
  console.log(`Recorded ${cases.length} upstream event cases`);
} else {
  if (fs.readFileSync(destination, 'utf8') !== output) throw Error('Event oracle differs: inspect changes, then use --write');
  console.log(`Verified ${cases.length} upstream event cases`);
}
