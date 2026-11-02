#!/usr/bin/env node
// Development-only oracle for portable ajax.js helpers. No request, transport
// installation, URL, browser, fetch, socket or filesystem API reaches a hook.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
globalThis.window = { document: {} };
const { jQuery: $ } = await import(pathToFileURL(path.join(root, 'jquery-main/src/core.js')));
const original = fs.readFileSync(path.join(root, 'jquery-main/src/ajax.js'), 'utf8');
const start = original.indexOf('function addToPrefiltersOrTransports( structure )');
const end = original.indexOf('\njQuery.extend( {', start);
if (start < 0 || end < 0) throw Error('Upstream Ajax helper extraction markers changed');
// This is the unchanged contiguous original function block. The browser module
// initialization preceding it is not executed. A fresh transports lexical is
// supplied for every registry case because the helper compares its identity.
const helpers = transports => new Function('jQuery', 'rnothtmlwhite', 'transports',
  original.slice(start, end) + '\nreturn {addToPrefiltersOrTransports,inspectPrefiltersOrTransports,ajaxExtend,ajaxHandleResponses,ajaxConvert};')
  ($, /[^\x20\t\r\n\f]+/g, transports);

function encode(value) {
  if (value === undefined) return { $undefined: true };
  if (Array.isArray(value)) return value.map(encode);
  if (value && typeof value === 'object') return Object.fromEntries(Object.entries(value).map(([key, child]) => [key, encode(child)]));
  return value;
}
function decode(value) {
  if (value && typeof value === 'object' && value.$undefined === true) return undefined;
  if (Array.isArray(value)) return value.map(decode);
  if (value && typeof value === 'object') return Object.fromEntries(Object.entries(value).map(([key, child]) => [key, decode(child)]));
  return value;
}

function transform(descriptor, response) {
  switch (descriptor.kind) {
    case 'append': return String(response) + descriptor.value;
    case 'add': return Number(response) + descriptor.value;
    case 'wrap': return { value: response, tag: descriptor.value };
    case 'constant': return decode(descriptor.value);
    case 'throw': throw decode(descriptor.value);
    default: throw Error(`Unknown fixture transform ${descriptor.kind}`);
  }
}

function run(op, input) {
  const transports = {};
  const api = helpers(transports);
  if (op === 'settings') {
    $.ajaxSettings = { flatOptions: Object.fromEntries(input.flat.map(key => [key, true])) };
    return encode(api.ajaxExtend(decode(input.target), decode(input.source)));
  }
  if (op === 'conversion') {
    const trace = [], jqXHR = {};
    const options = { dataTypes: input.dataTypes.slice(), dataType: input.dataType || '',
      responseFields: Object.fromEntries(input.responseFields || []), throws: !!input.throws, converters: {} };
    for (const descriptor of input.converters || []) {
      options.converters[descriptor.key] = descriptor.kind === 'identity' ? true : response => {
        trace.push({ stage: descriptor.key, response: encode(response) });
        return transform(descriptor, response);
      };
    }
    if (input.filter) options.dataFilter = (response, type) => {
      trace.push({ stage: 'filter', response: encode(response), dataType: type });
      return transform(input.filter, response);
    };
    let result;
    try { result = api.ajaxConvert(options, decode(input.response), jqXHR, input.isSuccess !== false); }
    catch (error) { result = { thrown: error }; }
    return { result: encode(result), fields: encode(jqXHR), trace };
  }
  if (op === 'selection') {
    const options = { dataTypes: input.dataTypes.slice(), mimeType: input.mimeOverride,
      contents: Object.fromEntries((input.contents || []).map(item => [item.type, new RegExp(item.source)])),
      converters: Object.fromEntries((input.converterKeys || []).map(key => [key, true])) };
    const jqXHR = { getResponseHeader(name) { if (name !== 'Content-Type') throw Error(name); return input.contentType; } };
    const response = api.ajaxHandleResponses(options, jqXHR, Object.fromEntries(input.responses));
    return { response: encode(response), dataTypes: options.dataTypes };
  }
  if (op === 'registry') {
    const structure = input.mode === 'transport' ? transports : {};
    const register = api.addToPrefiltersOrTransports(structure);
    const trace = [], options = { dataTypes: input.dataTypes.slice() };
    for (const hook of input.hooks) {
      const factory = settings => {
        trace.push({ id: hook.id, dataTypes: settings.dataTypes.slice() });
        if (hook.append) settings.dataTypes.push(hook.append);
        return input.mode === 'transport' ? hook.transport && { transport: hook.id } : hook.redirect;
      };
      if (hook.expression == null) register(factory); else register(hook.expression, factory);
    }
    const selected = api.inspectPrefiltersOrTransports(structure, options, {}, {});
    // The typed native transport API uses None for either falsy source result.
    return { trace, dataTypes: options.dataTypes, ...(input.mode === 'transport' ? { selected: selected || null } : {}) };
  }
  throw Error(`Unknown Ajax fixture operation ${op}`);
}

const cases = [];
const add = (name, op, input) => {
  if (op === 'selection') {
    // Native callers supply MIME matches; the fixture records these using the
    // same real RegExp objects/strings that the unchanged source evaluates.
    const mime = input.mimeOverride || input.contentType || '';
    input.contents = (input.contents || []).map(item => ({ ...item, matched: new RegExp(item.source).test(mime) }));
  }
  cases.push({ name, op, input, result: run(op, input) });
};
const settings = (name, target, source, flat = ['url', 'context']) => add(name, 'settings', { target, source, flat });
settings('settings-empty', {}, {});
settings('settings-context-flat', { context: { a: 1 }, headers: { a: 1 } }, { context: { b: 2 }, headers: { b: 2 } });
settings('settings-url-flat', { url: { a: 1 }, headers: { nested: { a: 1 } } }, { url: { b: 2 }, headers: { nested: { b: 2 } } });
settings('settings-custom-flat', { headers: { a: 1 }, context: { a: 1 } }, { headers: { b: 2 }, context: { b: 2 } }, ['headers']);
settings('settings-array-merge', { a: [1, 2, 3] }, { a: [9] });
settings('settings-nested-arrays', { a: [{ a: 1 }, { b: 2 }] }, { a: [{ b: 3 }] });
settings('settings-null-replaces', { a: { nested: true }, context: { a: 1 } }, { a: null, context: null });
settings('settings-object-over-scalar', { a: false, b: 'text' }, { a: { nested: 1 }, b: [1, 2] });
settings('settings-scalar-over-object', { a: { nested: 1 }, b: [1, 2] }, { a: false, b: 'text' });
settings('settings-undefined-skipped', { a: 1, context: 2 }, { a: { $undefined: true }, b: { $undefined: true }, context: { $undefined: true } });
settings('settings-integer-property-order', { '2': { a: 1 }, b: 1 }, { '10': 3, '2': { b: 2 }, b: 2 });
settings('settings-no-flat-options', { context: { a: 1 }, url: { a: 1 } }, { context: { b: 2 }, url: { b: 2 } }, []);

const fields = [['text', 'responseText'], ['mid', 'responseMid'], ['json', 'responseJSON']];
const append = (key, value) => ({ key, kind: 'append', value });
const identity = key => ({ key, kind: 'identity' });
const conversion = (name, config) => add(name, 'conversion', { dataTypes: ['text', 'json'], response: 'x', isSuccess: true, responseFields: fields, ...config });
conversion('convert-no-stages', { dataTypes: [] });
conversion('convert-one-stage', { dataTypes: ['text'] });
conversion('convert-empty-next-stops', { dataTypes: ['text', ''] });
conversion('convert-empty-first-stops', { dataTypes: ['', 'json'] });
conversion('convert-direct', { converters: [append('text json', 'A')] });
conversion('convert-case-insensitive', { converters: [append('TEXT JSON', 'A')] });
conversion('convert-wildcard-input', { converters: [append('* json', 'A')] });
conversion('convert-direct-before-wildcard', { converters: [append('* json', 'B'), append('text json', 'A')] });
conversion('convert-auto-output', { dataTypes: ['text', '*', 'json'], converters: [append('text json', 'A')] });
conversion('convert-auto-input', { dataTypes: ['*', 'json'], converters: [append('* json', 'A')] });
conversion('convert-repeated-stage', { dataTypes: ['text', 'text', 'json'], converters: [append('text json', 'A')] });
conversion('convert-direct-identity', { converters: [identity('text json')] });
conversion('convert-intermediate-functions', { converters: [append('text mid', 'A'), append('mid json', 'B')] });
conversion('convert-intermediate-first-identity', { converters: [identity('text mid'), append('mid json', 'B')] });
conversion('convert-intermediate-second-identity', { converters: [append('text mid', 'A'), identity('mid json')] });
conversion('convert-intermediate-both-identity', { converters: [identity('text mid'), identity('mid json')] });
conversion('convert-intermediate-wildcard', { converters: [append('* mid', 'A'), append('mid json', 'B')] });
conversion('convert-intermediate-first-in-order', { converters: [append('text a', 'A'), append('a json', 'B'), append('text b', 'C'), append('b json', 'D')] });
conversion('convert-normalized-key-overwrite', { converters: [append('Text Json', 'A'), append('text json', 'B')] });
conversion('convert-missing', {});
conversion('convert-missing-throws-still-parsererror', { throws: true });
for (const throws of [false, true]) {
  for (const value of ['bad conversion', { reason: 7 }]) {
    conversion(`convert-error-${throws}-${typeof value}`, { throws, converters: [{ key: 'text json', kind: 'throw', value }] });
  }
}
conversion('convert-filter-before-stages', { dataTypes: ['text', 'mid', 'json'], dataType: 'requested-type', filter: { kind: 'append', value: 'F' }, converters: [append('text mid', 'A'), append('mid json', 'B')] });
conversion('convert-filter-failure-skipped', { isSuccess: false, filter: { kind: 'throw', value: 'filter error' }, converters: [identity('text json')] });
conversion('convert-filter-error-propagates', { filter: { kind: 'throw', value: 'filter error' }, converters: [identity('text json')] });
conversion('convert-response-field-overwrite', { responseFields: [['text', 'shared'], ['json', 'shared']], converters: [append('text json', 'A')] });
conversion('convert-empty-response-field-skipped', { responseFields: [['text', ''], ['json', 'responseJSON']], converters: [append('text json', 'A')] });
conversion('convert-numeric-then-object', { response: '12', converters: [{ key: 'text mid', kind: 'add', value: 3 }, { key: 'mid json', kind: 'wrap', value: 'converted' }] });

const contents = [{ type: 'xml', source: '\\bxml\\b' }, { type: 'html', source: '\\bhtml' }, { type: 'json', source: '\\bjson\\b' }];
const selection = (name, config) => add(name, 'selection', { dataTypes: ['json'], responses: [['text', 'raw'], ['json', { parsed: true }]], contents, ...config });
selection('select-exact', {});
selection('select-convertible', { responses: [['binary', 'bits'], ['text', 'raw']], converterKeys: ['text json'] });
selection('select-fallback', { responses: [['binary', 'bits'], ['text', 'raw']] });
selection('select-no-desired', { dataTypes: [] });
selection('select-empty-desired', { dataTypes: [''], converterKeys: ['json '] });
selection('select-auto-json', { dataTypes: ['*'], contentType: 'application/json', responses: [['text', 'raw']], converterKeys: ['text json'] });
selection('select-auto-repeated', { dataTypes: ['*', '*'], contentType: 'text/html', responses: [['text', 'raw']] });
selection('select-mime-override', { dataTypes: ['*'], contentType: 'application/json', mimeOverride: 'text/html', responses: [['text', 'raw']] });
selection('select-empty-override-falls-back', { dataTypes: ['*'], contentType: 'application/json', mimeOverride: '', responses: [['text', 'raw']] });
selection('select-mime-ignored-without-auto', { dataTypes: ['json'], contentType: 'text/html' });
selection('select-no-responses', { responses: [] });
selection('select-empty-response-key', { dataTypes: [], responses: [['', 'empty'], ['text', 'raw']] });
selection('select-empty-key-fallback-skipped', { responses: [['', 'empty'], ['text', 'raw']] });
selection('select-converter-keys-not-normalized', { responses: [['binary', 'bits'], ['text', 'raw']], converterKeys: ['TEXT JSON'] });

const registry = (name, mode, hooks, dataTypes = ['text']) => add(name, 'registry', { mode, hooks, dataTypes });
registry('prefilter-order', 'prefilter', [{ id: 'a', expression: 'text' }, { id: 'b', expression: '+TEXT' }, { id: 'star' }]);
registry('prefilter-redirect-chain', 'prefilter', [{ id: 'a', expression: 'text', redirect: 'json' }, { id: 'ignored', expression: 'text' }, { id: 'b', expression: 'json', redirect: 'xml' }, { id: 'c', expression: 'xml' }, { id: 'star' }]);
registry('prefilter-redirect-cycle-continues', 'prefilter', [{ id: 'a', expression: 'text', redirect: 'json' }, { id: 'b', expression: 'json', redirect: 'text' }, { id: 'c', expression: 'json' }, { id: 'star' }]);
registry('prefilter-redirect-star-only-once', 'prefilter', [{ id: 'a', expression: 'text', redirect: '*' }, { id: 'star', expression: '*', redirect: 'text' }]);
registry('prefilter-multiple-types-and-append', 'prefilter', [{ id: 'a', expression: 'TEXT\tjson', append: 'tail' }, { id: 'b', expression: '+', redirect: 'json' }]);
registry('prefilter-empty-expression-registers-none', 'prefilter', [{ id: 'ignored', expression: '' }, { id: 'ignored-space', expression: ' \t' }, { id: 'star' }]);
registry('transport-fallback', 'transport', [{ id: 'a', expression: 'text' }, { id: 'star', transport: true }]);
registry('transport-first-truthy', 'transport', [{ id: 'a', expression: 'text', transport: true }, { id: 'b', expression: 'text', transport: true }, { id: 'star', transport: true }]);
registry('transport-prepend', 'transport', [{ id: 'a', expression: 'text', transport: true }, { id: 'b', expression: '+TEXT', transport: true }]);
registry('transport-star-not-retried', 'transport', [{ id: 'a', expression: '*' }], ['*']);

const output = JSON.stringify({ description: 'Expected results from unchanged vendored Ajax helpers; see tools/ajax_oracle.mjs', cases }, null, 2) + '\n';
const destination = path.join(root, 'tests/fixtures/ajax.json');
if (process.argv.includes('--write')) {
  fs.mkdirSync(path.dirname(destination), { recursive: true });
  fs.writeFileSync(destination, output);
  console.log(`Recorded ${cases.length} upstream Ajax cases`);
} else {
  if (fs.readFileSync(destination, 'utf8') !== output) throw Error('Ajax oracle differs: inspect changes, then use --write');
  console.log(`Verified ${cases.length} upstream Ajax cases`);
}
