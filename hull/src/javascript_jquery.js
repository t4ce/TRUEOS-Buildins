// This trusted binding is inserted inside the DOM wrapper closure. Collection
// algorithms and mutations run in rjqry; JS owns JS callbacks and node identity.
(() => {
    const page = globalThis, oldDollar = page.$, oldJQuery = page.jQuery;
    const array = Array.isArray, keys = Object.keys, prototype = Object.getPrototypeOf;
    const plain = Object.prototype, define = Object.defineProperty, brands = new WeakSet();
    const slice = Array.prototype.slice, iterator = Array.prototype[Symbol.iterator];
    function unsupported(message) { throw new DOMException(message, 'NotSupportedError'); }
    function call(operation, ...args) { return request('jquery', operation, ...args); }
    function transport(values) { setPrototypeOf(values, null); return values; }
    function handles(value) {
        if (!brands.has(value)) throw new TypeError('Expected a native jQuery collection');
        if (!Number.isSafeInteger(value.length) || value.length < 0 || value.length > 32768)
            throw new DOMException('jQuery collection budget exceeded', 'QuotaExceededError');
        const result = [];
        for (let i = 0; i < value.length; i++) result.push(state(value[i]).id);
        return transport(result);
    }
    function Collection(ids, previous) {
        brands.add(this);
        for (let i = 0; i < ids.length; i++) this[i] = wrap(ids[i]);
        this.length = ids.length;
        if (previous !== undefined) this.prevObject = previous;
    }
    function idsFrom(value) {
        if (value == null) return transport([]);
        if (brands.has(value)) return handles(value);
        if (states.has(value)) return transport([state(value).id]);
        if (array(value) || lists.has(value)) {
            const result = [];
            if (value.length > 32768) throw new DOMException('jQuery collection budget exceeded', 'QuotaExceededError');
            for (const node of value) result.push(state(node).id);
            return transport(result);
        }
        throw new TypeError('Native jQuery collections currently require DOM nodes');
    }
    function jQuery(value, context) {
        if (!value) return new Collection([]);
        if (typeof value === 'function') unsupported('jQuery ready callbacks require a DOM-ready scheduler');
        if (typeof value === 'string') {
            if (/^\s*</.test(value)) unsupported('jQuery HTML construction is not admitted');
            if (context) return jQuery(context).find(value);
            return new Collection(call('query', transport([]), string(value)));
        }
        return new Collection(idsFrom(value));
    }
    function stack(collection, operation, ...args) {
        return new Collection(call(operation, handles(collection), ...args), collection);
    }
    function integer(value, length) {
        const number = +value;
        return Number.isNaN(number) ? 0 : Math.max(-length, Math.min(length, Math.trunc(number)));
    }
    function classes(value) {
        if (typeof value === 'string') return string(value);
        if (array(value)) return value.map(item => string(item)).join(' ');
        return null;
    }
    const fn = Collection.prototype;
    fn.constructor = jQuery;
    fn.jquery = '4.0.0-hull-native';
    fn.toArray = function() { return apply(slice, this, []); };
    fn.get = function(index) { return index == null ? this.toArray() : this[index < 0 ? index + this.length : index]; };
    fn.pushStack = function(nodes) { return new Collection(idsFrom(nodes), this); };
    fn.end = function() { return this.prevObject || jQuery(); };
    fn.eq = function(index) {
        const number = +index;
        if (Number.isFinite(number) && !Number.isInteger(number))
            unsupported('Native jQuery DOM collections require integer indexes');
        return Number.isFinite(number) && Number.isInteger(number) ? stack(this, 'eq', Math.max(-32769, Math.min(32769, number))) : new Collection([], this);
    };
    for (const operation of ['first','last','even','odd']) fn[operation] = function() { return stack(this, operation); };
    fn.slice = function(start, end) {
        return stack(this, 'slice', integer(start, this.length), end === undefined ? null : integer(end, this.length));
    };
    fn.find = function(selector) {
        if (typeof selector !== 'string') unsupported('Native jQuery find currently takes a selector string');
        if (selector === '') return new Collection([], this);
        return stack(this, 'find', string(selector));
    };
    for (const operation of ['parent','parents','children','siblings','next','prev','nextAll','prevAll']) {
        fn[operation] = function(selector) {
            if (selector !== undefined) unsupported('Native jQuery traversal filtering is not admitted');
            return stack(this, operation);
        };
    }
    fn.each = function(callback) {
        const length = this.length;
        for (let i = 0; i < length; i++) if (apply(callback, this[i], [i, this[i]]) === false) break;
        return this;
    };
    fn.text = function(value) {
        if (value === undefined) return call('text', handles(this));
        if (typeof value === 'function') return this.each(function(index) {
            const next = apply(value, this, [index, jQuery(this).text()]);
            if (next !== undefined) jQuery(this).text(next);
        });
        call('setText', handles(this), value == null ? '' : string(value)); return this;
    };
    fn.attr = function(name, value) {
        if (name !== null && typeof name === 'object') {
            for (const key in name) this.attr(key, name[key]); return this;
        }
        name = string(name);
        if (value === undefined) return call('attr', handles(this), name) ?? undefined;
        if (typeof value === 'function') return this.each(function(index) {
            const next = apply(value, this, [index, jQuery(this).attr(name)]);
            if (next !== undefined) jQuery(this).attr(name, next);
        });
        const encoded = value === null ? transport(['null']) : typeof value === 'boolean' ? transport(['bool', value]) : transport(['string', string(value)]);
        call('setAttr', handles(this), name, encoded); return this;
    };
    fn.removeAttr = function(names) {
        if (names == null || names === false || names === 0 || names === '') return this;
        if (typeof names !== 'string') throw new TypeError('Expected attribute names');
        call('removeAttr', handles(this), string(names)); return this;
    };
    for (const operation of ['addClass','removeClass','toggleClass']) fn[operation] = function(value, stateValue) {
        if (typeof value === 'function') return this.each(function(index) {
            const next = apply(value, this, [index, typeof this.getAttribute === 'function' ? this.getAttribute('class') || '' : '', stateValue]);
            jQuery(this)[operation](next, stateValue);
        });
        const names = classes(value);
        if (operation === 'removeClass' && arguments.length === 0) call(operation, handles(this), null);
        else if (names !== null) call(operation, handles(this), names, typeof stateValue === 'boolean' ? stateValue : null);
        return this;
    };
    fn.hasClass = function(name) { return call('hasClass', handles(this), string(name)); };
    fn[Symbol.iterator] = iterator;
    // Tag data rather than JSON-coercing the caller's objects: undefined and
    // nonfinite numbers survive, toJSON is never invoked, cycles/functions and
    // unsupported object types fail explicitly. All containers have null proto.
    function pack(value, seen, budget, depth) {
        if (++budget.count > 2048 || depth > 32) throw new DOMException('jQuery data conversion budget exceeded', 'QuotaExceededError');
        if (value === undefined) return transport(['undefined']);
        if (value === null) return transport(['null']);
        if (typeof value === 'boolean') return transport(['bool', value]);
        if (typeof value === 'number') return transport(['number', `${value}`]);
        if (typeof value === 'string') return transport(['string', string(value)]);
        if (typeof value !== 'object') unsupported('jQuery.param currently accepts data values without function callbacks');
        if (seen.has(value)) throw new TypeError('Cyclic jQuery.param data');
        if (!array(value) && prototype(value) !== plain && prototype(value) !== null) unsupported('jQuery.param requires arrays or plain data objects');
        seen.add(value);
        const result = [array(value) ? 'array' : 'object'];
        if (array(value)) {
            if (value.length > 32768) throw new DOMException('jQuery data conversion budget exceeded', 'QuotaExceededError');
            for (let i = 0; i < value.length; i++) result.push(pack(value[i], seen, budget, depth + 1));
        } else {
            const names = keys(value);
            if (names.length > 128) throw new DOMException('jQuery object width exceeds native work budget', 'QuotaExceededError');
            for (const key of names) result.push(transport([string(key), pack(value[key], seen, budget, depth + 1)]));
        }
        seen.delete(value); return transport(result);
    }
    jQuery.param = function(value, traditional) { return call('param', pack(value, new Set(), {count:0}, 0), !!traditional); };
    jQuery.escapeSelector = function(value) { return call('escapeSelector', string(value)); };
    jQuery.noConflict = function(deep) {
        if (page.$ === jQuery) page.$ = oldDollar;
        if (deep && page.jQuery === jQuery) page.jQuery = oldJQuery;
        return jQuery;
    };
    jQuery.fn = fn;
    define(page, 'jQuery', {value:jQuery, writable:true, configurable:true});
    define(page, '$', {value:jQuery, writable:true, configurable:true});
})();
