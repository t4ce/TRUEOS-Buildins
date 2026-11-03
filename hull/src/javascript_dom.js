// Lazy, identity-preserving wrappers for the Rust-owned rp5 tree. This exposes
// only the implemented standard DOM members, without browser/network services.
(() => {
    'use strict';
    const native = globalThis.__hullDom;
    delete globalThis.__hullDom;
    const states = new WeakMap(), lists = new WeakMap(), wrappers = new Map();
    const token = {}, apply = Reflect.apply, wellFormed = String.prototype.isWellFormed;
    const setPrototypeOf = Object.setPrototypeOf, own = Object.hasOwn, toWellFormed = String.prototype.toWellFormed;
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
            if (response.name === 'RangeError') throw new RangeError(response.message);
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
    /* HULL_DOM_EVENTS */
    class Node extends EventTarget {
        constructor(key, id, description) {
            super();
            if (key !== token) throw new TypeError('Illegal constructor');
            states.set(this, {id, ...description});
            eventTarget(this).parent=()=>description.type===9 ? eventWindow : wrap(get(this,'parentNode'));
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
        get baseURI() { return get(this, 'baseURI'); }
        appendChild(child) { required(arguments,1); return wrap(request('appendChild',state(this).id,state(child).id)); }
        insertBefore(child, before) { required(arguments,2); return wrap(request('insertBefore',state(this).id,state(child).id,before==null?null:state(before).id)); }
        removeChild(child) { required(arguments,1); return wrap(request('removeChild',state(this).id,state(child).id)); }
        cloneNode(deep=false) { return wrap(request('clone',state(this).id,!!deep)); }
    }
    class CharacterData extends Node {
        get data() {
            if(![3,8].includes(state(this).type))throw new TypeError('Expected CharacterData');
            return get(this,'nodeValue');
        }
        set data(value) {
            const s=state(this);if(![3,8].includes(s.type))throw new TypeError('Expected CharacterData');
            request('set',s.id,'nodeValue',string(value));
        }
        get length() { return this.data.length; }
    }
    class Text extends CharacterData {
        constructor(key='',id,description) {
            if(key!==token)return wrap(request('create',state(document).id,'text',string(key)));
            super(key,id,description);
        }
    }
    class Comment extends CharacterData {
        constructor(key='',id,description) {
            if(key!==token)return wrap(request('create',state(document).id,'comment',string(key)));
            super(key,id,description);
        }
    }
    /* HULL_STYLE */
    class Element extends Node {
        get style(){return inlineStyle(this);}
        set style(value){inlineStyle(this).cssText=value;}
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
        getElementsByTagName(name) { const s=state(this); if(s.type!==1)throw new TypeError('Illegal invocation'); required(arguments, 1); return new HTMLCollection(token, s.id, string(name)); }
        insertAdjacentElement(position,element) {
            if(state(this).type!==1)throw new TypeError('Expected an Element');
            required(arguments,2);position=string(position).replace(/[A-Z]/g,c=>c.toLowerCase());
            if(state(element).type!==1)throw new TypeError('Expected an Element');
            switch(position) {
                case 'beforebegin': if(!this.parentNode)return null;this.parentNode.insertBefore(element,this);break;
                case 'afterbegin': this.insertBefore(element,this.firstChild);break;
                case 'beforeend': this.appendChild(element);break;
                case 'afterend': if(!this.parentNode)return null;this.parentNode.insertBefore(element,this.nextSibling);break;
                default: throw new DOMException('Invalid insertion position','SyntaxError');
            }
            return element;
        }
    }
    class Document extends Node {
        get URL() { return get(this,'URL'); }
        get documentURI() { return get(this,'documentURI'); }
        createElement(name,options) {
            const s=state(this);if(s.type!==9)throw new TypeError('Expected a Document');required(arguments,1);
            if(options!=null&&typeof options!=='string'&&(options.is!==undefined||options.customElementRegistry!==undefined))
                throw new DOMException('Custom element creation is not implemented','NotSupportedError');
            return wrap(request('create',s.id,'element',string(name)));
        }
        createTextNode(value) { const s=state(this);if(s.type!==9)throw new TypeError('Expected a Document');required(arguments,1);return wrap(request('create',s.id,'text',string(value))); }
        createComment(value) { const s=state(this);if(s.type!==9)throw new TypeError('Expected a Document');required(arguments,1);return wrap(request('create',s.id,'comment',string(value))); }
        createDocumentFragment() { const s=state(this);if(s.type!==9)throw new TypeError('Expected a Document');return wrap(request('create',s.id,'fragment')); }
        get readyState() {
            if(state(this).type!==9)throw new TypeError('Expected a Document');
            return documentReadyState;
        }
        get currentScript() { return wrap(get(this, 'currentScript')); }
        get documentElement() { return wrap(get(this, 'documentElement')); }
        get head() { return wrap(get(this, 'head')); }
        get body() { return wrap(get(this, 'body')); }
        getElementById(id) { required(arguments, 1); return wrap(request('id', state(this).id, string(id))); }
        querySelector(selector) { required(arguments, 1); return wrap(request('query', state(this).id, string(selector))[0] ?? null); }
        querySelectorAll(selector) { required(arguments, 1); return new NodeList(token, request('query', state(this).id, string(selector))); }
        getElementsByTagName(name) { const s=state(this); if(s.type!==9)throw new TypeError('Illegal invocation'); required(arguments, 1); return new HTMLCollection(token, s.id, string(name)); }
    }
    class HTMLElement extends Element {}
    function linkAttribute(value,action,name,content){
        const s=state(value);
        if(s.type!==1||s.name!=='LINK'||s.namespace!=='http://www.w3.org/1999/xhtml')throw new TypeError('Expected an HTMLLinkElement');
        return attribute(value,action,name,content);
    }
    class HTMLLinkElement extends HTMLElement {
        get href() { return get(this,'linkHref'); }
        set href(value) { linkAttribute(this,'set','href',apply(toWellFormed,`${value}`,[])); }
        get rel() { return linkAttribute(this,'get','rel')??''; }
        set rel(value) { linkAttribute(this,'set','rel',string(value)); }
        get as() {
            const value=(linkAttribute(this,'get','as')??'').toLowerCase();
            return ['fetch','font','image','script','style','track','json','text','audioworklet','paintworklet','serviceworker','sharedworker','worker'].includes(value)?value:'';
        }
        set as(value) { linkAttribute(this,'set','as',string(value)); }
        get crossOrigin() {
            const value=linkAttribute(this,'get','crossorigin');
            return value===null?null:value.toLowerCase()==='use-credentials'?'use-credentials':'anonymous';
        }
        set crossOrigin(value) { if(value==null)linkAttribute(this,'remove','crossorigin');else linkAttribute(this,'set','crossorigin',string(value)); }
    }
    class HTMLTemplateElement extends HTMLElement {get content(){return wrap(get(this,'templateContent'));}}
    /* HULL_MEDIA */
    class DocumentFragment extends Node {
        querySelector(selector){required(arguments,1);return wrap(request('query',state(this).id,string(selector))[0]??null);}
        querySelectorAll(selector){required(arguments,1);return new NodeList(token,request('query',state(this).id,string(selector)));}
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
    const collections = new WeakMap();
    function collectionIds(value) {
        const c = collections.get(value);
        if (!c) throw new TypeError('Illegal invocation');
        return request('tags', c.root, c.name);
    }
    function collectionNames(value, ids = collectionIds(value)) {
        const names = [];
        for (const id of ids) {
            const node = wrap(id);
            const idName = attribute(node, 'get', 'id') ?? '';
            const nameName = get(node, 'namespaceURI') === 'http://www.w3.org/1999/xhtml' ? attribute(node, 'get', 'name') : null;
            for (const name of [idName, nameName]) {
                if (name && !names.includes(name) && !Object.prototype.hasOwnProperty.call(value, name) && !hasCollectionPrototypeProperty(value, name)) names.push(name);
            }
        }
        return names;
    }
    function hasCollectionPrototypeProperty(value, name) {
        for (let prototype = Reflect.getPrototypeOf(value); prototype !== null; prototype = Reflect.getPrototypeOf(prototype)) {
            if (Object.prototype.hasOwnProperty.call(prototype, name)) return true;
        }
        return false;
    }
    function collectionNamedItem(value, name) {
        if (name === '') return null;
        for (const id of collectionIds(value)) {
            const node = wrap(id);
            if (attribute(node, 'get', 'id') === name || get(node, 'namespaceURI') === 'http://www.w3.org/1999/xhtml' && attribute(node, 'get', 'name') === name) return node;
        }
        return null;
    }
    function arrayIndex(key) {
        if (typeof key !== 'string' || !/^(0|[1-9][0-9]*)$/.test(key)) return null;
        const value = Number(key);
        return Number.isInteger(value) && value < 4294967295 ? value : null;
    }
    class HTMLCollection {
        constructor(key, root, name) {
            if (key !== token) throw new TypeError('Illegal constructor');
            const proxy = new Proxy(this, {
                get(target, property, receiver) {
                    const index = arrayIndex(property);
                    if (index !== null) {
                        const id = collectionIds(target)[index];
                        if (id !== undefined) return wrap(id);
                        return Reflect.get(target, property, receiver);
                    }
                    if (typeof property === 'string' && !Reflect.has(target, property) && collectionNames(target).includes(property)) return collectionNamedItem(target, property);
                    return Reflect.get(target, property, receiver);
                },
                has(target, property) {
                    const index = arrayIndex(property);
                    if (index !== null) return index < collectionIds(target).length || Reflect.has(target, property);
                    return Reflect.has(target, property) || typeof property === 'string' && collectionNames(target).includes(property);
                },
                set(target, property, value, receiver) {
                    const index = arrayIndex(property);
                    if (index !== null) return index < collectionIds(target).length ? false : Reflect.set(target, property, value, receiver);
                    if (typeof property === 'string' && collectionNames(target).includes(property)) return false;
                    return Reflect.set(target, property, value, receiver);
                },
                defineProperty(target, property, descriptor) {
                    const index = arrayIndex(property);
                    if (index !== null) return false;
                    if (typeof property === 'string' && collectionNames(target).includes(property)) return false;
                    return Reflect.defineProperty(target, property, descriptor);
                },
                deleteProperty(target, property) {
                    const index = arrayIndex(property);
                    if (index !== null) return index >= collectionIds(target).length;
                    if (typeof property === 'string' && collectionNames(target).includes(property)) return false;
                    return Reflect.deleteProperty(target, property);
                },
                ownKeys(target) {
                    const ids = collectionIds(target);
                    return [...new Set([...Reflect.ownKeys(target), ...ids.map((_, index) => `${index}`), ...collectionNames(target, ids)])];
                },
                getOwnPropertyDescriptor(target, property) {
                    const index = arrayIndex(property);
                    if (index !== null) {
                        const id = collectionIds(target)[index];
                        if (id !== undefined) return {value:wrap(id), writable:false, enumerable:true, configurable:true};
                        return Reflect.getOwnPropertyDescriptor(target, property);
                    }
                    if (typeof property === 'string' && collectionNames(target).includes(property)) return {value:collectionNamedItem(target, property), writable:false, enumerable:false, configurable:true};
                    return Reflect.getOwnPropertyDescriptor(target, property);
                },
            });
            collections.set(this, {root, name}); collections.set(proxy, {root, name});
            return proxy;
        }
        get length() { return collectionIds(this).length; }
        item(index) { required(arguments, 1); const n = +index; const ix = Number.isFinite(n) ? ((Math.trunc(n) % 4294967296) + 4294967296) % 4294967296 : 0; return wrap(collectionIds(this)[ix] ?? null); }
        namedItem(name) {
            required(arguments, 1); return collectionNamedItem(this, string(name));
        }
        *[Symbol.iterator]() { for (let i = 0; i < this.length; i++) yield this.item(i); }
    }
    function wrap(id) {
        if (id === null) return null;
        if (wrappers.has(id)) return wrappers.get(id);
        const description = request('describe', id);
        let Constructor=description.type===9?Document:description.type===11?DocumentFragment:description.type===3?Text:description.type===8?Comment:description.type===1?Element:Node;
        if(description.type===1&&description.namespace==='http://www.w3.org/1999/xhtml')
            Constructor=description.name==='LINK'?HTMLLinkElement:description.name==='TEMPLATE'?HTMLTemplateElement:description.name==='VIDEO'?HTMLVideoElement:HTMLElement;
        const node = new Constructor(token, id, description);
        wrappers.set(id, node);
        return node;
    }
    for (const Constructor of [EventTarget, Event, CustomEvent, Node, CharacterData, Text, Comment, Element, HTMLElement, HTMLMediaElement, HTMLVideoElement, HTMLLinkElement, HTMLTemplateElement, DocumentFragment, Document, NodeList, HTMLCollection, DOMException]) {
        Object.defineProperty(Constructor.prototype, Symbol.toStringTag, {value:Constructor.name, configurable:true});
        Object.defineProperty(globalThis, Constructor.name, {value:Constructor, writable:true, configurable:true});
    }
    for (const [name, value] of Object.entries({ELEMENT_NODE:1, ATTRIBUTE_NODE:2, TEXT_NODE:3,
        CDATA_SECTION_NODE:4, ENTITY_REFERENCE_NODE:5, ENTITY_NODE:6, PROCESSING_INSTRUCTION_NODE:7,
        COMMENT_NODE:8, DOCUMENT_NODE:9, DOCUMENT_TYPE_NODE:10, DOCUMENT_FRAGMENT_NODE:11, NOTATION_NODE:12})) {
        Object.defineProperty(Node, name, {value, enumerable:true});
        Object.defineProperty(Node.prototype, name, {value, enumerable:true});
    }
    /* HULL_COOKIES */
    Object.defineProperty(globalThis, 'document', {value:wrap(request('root')), writable:false, configurable:false});
    if (request('window') !== null) {
        const pageGlobal = globalThis, define = Object.defineProperty;
        eventWindow=pageGlobal;
        registerEventTarget(pageGlobal);
        for(const name of ['addEventListener','removeEventListener','dispatchEvent']) {
            const method=EventTarget.prototype[name];
            define(pageGlobal,name,{value:function(...args){return eventApply(method,this ?? pageGlobal,args)},writable:true,configurable:true});
        }
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
    /* HULL_NATIVE_URL */
    /* HULL_NAVIGATION */
    /* HULL_NAVIGATOR */
    if(request('timers'))Object.defineProperty(globalThis,'__hullTimerSchedule',{value:args=>request('timer',args[0],args[1]),configurable:true});
    /* HULL_INPUT_EVENTS */
})();
