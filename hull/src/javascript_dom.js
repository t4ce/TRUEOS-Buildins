// Lazy, identity-preserving wrappers for the Rust-owned rp5 tree. This exposes
// only the implemented standard DOM members, without browser/network services.
(() => {
    'use strict';
    const native = globalThis.__hullDom;
    delete globalThis.__hullDom;
    const states = new WeakMap(), lists = new WeakMap(), wrappers = new Map();
    const token = {}, apply = Reflect.apply, wellFormed = String.prototype.isWellFormed;
    const setPrototypeOf = Object.setPrototypeOf, own = Object.hasOwn;
    const exceptionCodes = {IndexSizeError:1, DOMStringSizeError:2, HierarchyRequestError:3,
        WrongDocumentError:4, InvalidCharacterError:5, NoDataAllowedError:6,
        NoModificationAllowedError:7, NotFoundError:8, NotSupportedError:9,
        InUseAttributeError:10, InvalidStateError:11, SyntaxError:12,
        InvalidModificationError:13, NamespaceError:14, InvalidAccessError:15,
        ValidationError:16, TypeMismatchError:17, SecurityError:18, NetworkError:19,
        AbortError:20, URLMismatchError:21, QuotaExceededError:22, TimeoutError:23,
        InvalidNodeTypeError:24, DataCloneError:25};
    class DOMException extends Error {
        constructor(message = '', name = 'Error') {
            super(`${message}`);
            this.name = `${name}`;
        }
        get code() { return own(exceptionCodes, this.name) ? exceptionCodes[this.name] : 0; }
    }
    function string(value) {
        const text = `${value}`;
        // rp5 stores Unicode scalar strings. Reject the unrepresentable case
        // explicitly rather than silently changing DOMString code units.
        if (!apply(wellFormed, text, [])) throw new DOMException('Unpaired UTF-16 surrogates are not supported by this DOM gateway', 'NotSupportedError');
        return text;
    }
    function required(args, count) {
        if (args.length < count) throw new TypeError(`Expected at least ${count} argument(s)`);
    }
    function request(...args) {
        setPrototypeOf(args, null); // Do not invoke page-defined inherited toJSON.
        const response = native(args);
        if (!response.ok) {
            if (response.name === 'TypeError') throw new TypeError(response.message);
            throw new DOMException(response.message, response.name);
        }
        return response.value;
    }
    function state(value) {
        const result = states.get(value);
        if (!result) throw new TypeError('Illegal invocation');
        return result;
    }
    function get(value, name) { return request('get', state(value).id, name); }
    function attribute(value, action, name, content) {
        return request('attribute', state(value).id, action, string(name), content);
    }
    class Node {
        constructor(key, id, description) {
            if (key !== token) throw new TypeError('Illegal constructor');
            states.set(this, {id, ...description});
        }
        get nodeType() { return state(this).type; }
        get nodeName() { return state(this).name; }
        get nodeValue() { return get(this, 'nodeValue'); }
        set nodeValue(value) { request('set', state(this).id, 'nodeValue', value == null ? '' : string(value)); }
        get textContent() { return get(this, 'textContent'); }
        set textContent(value) { request('set', state(this).id, 'textContent', value == null ? '' : string(value)); }
        get parentNode() { return wrap(get(this, 'parentNode')); }
        get parentElement() { return wrap(get(this, 'parentElement')); }
        get ownerDocument() { return wrap(get(this, 'ownerDocument')); }
        get firstChild() { return wrap(get(this, 'firstChild')); }
        get lastChild() { return wrap(get(this, 'lastChild')); }
        get nextSibling() { return wrap(get(this, 'nextSibling')); }
        get previousSibling() { return wrap(get(this, 'previousSibling')); }
        get isConnected() { return get(this, 'isConnected'); }
    }
    class Element extends Node {
        get tagName() { return state(this).name; }
        get localName() { return get(this, 'localName'); }
        get namespaceURI() { return get(this, 'namespaceURI'); }
        get id() { return attribute(this, 'get', 'id') ?? ''; }
        set id(value) { attribute(this, 'set', 'id', string(value)); }
        get className() {
            if (get(this, 'namespaceURI') !== 'http://www.w3.org/1999/xhtml') throw new DOMException('SVGAnimatedString is not implemented', 'NotSupportedError');
            return attribute(this, 'get', 'class') ?? '';
        }
        set className(value) {
            if (get(this, 'namespaceURI') !== 'http://www.w3.org/1999/xhtml') throw new DOMException('SVGAnimatedString is not implemented', 'NotSupportedError');
            attribute(this, 'set', 'class', string(value));
        }
        getAttribute(name) { required(arguments, 1); return attribute(this, 'get', name); }
        hasAttribute(name) { required(arguments, 1); return attribute(this, 'has', name); }
        setAttribute(name, value) { required(arguments, 2); attribute(this, 'set', name, string(value)); }
        removeAttribute(name) { required(arguments, 1); attribute(this, 'remove', name); }
        querySelector(selector) { required(arguments, 1); return wrap(request('query', state(this).id, string(selector))[0] ?? null); }
        querySelectorAll(selector) { required(arguments, 1); return new NodeList(token, request('query', state(this).id, string(selector))); }
    }
    class Document extends Node {
        get documentElement() { return wrap(get(this, 'documentElement')); }
        get head() { return wrap(get(this, 'head')); }
        get body() { return wrap(get(this, 'body')); }
        getElementById(id) { required(arguments, 1); return wrap(request('id', state(this).id, string(id))); }
        querySelector(selector) { required(arguments, 1); return wrap(request('query', state(this).id, string(selector))[0] ?? null); }
        querySelectorAll(selector) { required(arguments, 1); return new NodeList(token, request('query', state(this).id, string(selector))); }
    }
    function list(value) {
        const ids = lists.get(value);
        if (!ids) throw new TypeError('Illegal invocation');
        return ids;
    }
    class NodeList {
        constructor(key, ids) {
            if (key !== token) throw new TypeError('Illegal constructor');
            lists.set(this, ids);
            for (let i = 0; i < ids.length; i++) {
                Object.defineProperty(this, i, {get:() => wrap(ids[i]), enumerable:true});
            }
        }
        get length() { return list(this).length; }
        item(index) { required(arguments, 1); return wrap(list(this)[(+index) >>> 0] ?? null); }
        forEach(callback, thisArg) {
            if (typeof callback !== 'function') throw new TypeError('Expected a callback');
            const ids = list(this);
            for (let i = 0; i < ids.length; i++) apply(callback, thisArg, [wrap(ids[i]), i, this]);
        }
        *keys() { for (let i = 0; i < list(this).length; i++) yield i; }
        *values() { for (const id of list(this)) yield wrap(id); }
        *entries() { const ids = list(this); for (let i = 0; i < ids.length; i++) yield [i, wrap(ids[i])]; }
        [Symbol.iterator]() { return this.values(); }
    }
    function wrap(id) {
        if (id === null) return null;
        if (wrappers.has(id)) return wrappers.get(id);
        const description = request('describe', id);
        const Constructor = description.type === 9 ? Document : description.type === 1 ? Element : Node;
        const node = new Constructor(token, id, description);
        wrappers.set(id, node);
        return node;
    }
    for (const Constructor of [Node, Element, Document, NodeList, DOMException]) {
        Object.defineProperty(Constructor.prototype, Symbol.toStringTag, {value:Constructor.name, configurable:true});
        Object.defineProperty(globalThis, Constructor.name, {value:Constructor, writable:true, configurable:true});
    }
    for (const [name, value] of Object.entries({ELEMENT_NODE:1, ATTRIBUTE_NODE:2, TEXT_NODE:3,
        CDATA_SECTION_NODE:4, ENTITY_REFERENCE_NODE:5, ENTITY_NODE:6, PROCESSING_INSTRUCTION_NODE:7,
        COMMENT_NODE:8, DOCUMENT_NODE:9, DOCUMENT_TYPE_NODE:10, DOCUMENT_FRAGMENT_NODE:11, NOTATION_NODE:12})) {
        Object.defineProperty(Node, name, {value, enumerable:true});
        Object.defineProperty(Node.prototype, name, {value, enumerable:true});
    }
    Object.defineProperty(globalThis, 'document', {value:wrap(request('root')), writable:false, configurable:false});
    if (request('window') !== null) {
        const pageGlobal = globalThis, define = Object.defineProperty;
        // The single page realm owns its global identity; replacing the ordinary
        // globalThis property must not change the window getter's target.
        define(pageGlobal, 'window', {get:() => pageGlobal, enumerable:true, configurable:false});
        function replaceable(name, getter) {
            define(pageGlobal, name, {
                get:getter,
                set(value) { define(pageGlobal, name, {value, writable:true, enumerable:true, configurable:true}); },
                enumerable:true, configurable:true,
            });
        }
        replaceable('self', () => pageGlobal);
        replaceable('innerWidth', () => request('window').innerWidth);
        replaceable('innerHeight', () => request('window').innerHeight);
    }
    /* HULL_NATIVE_JQUERY */
})();
