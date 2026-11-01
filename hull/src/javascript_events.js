// Native input endpoint only. No document, elements, hit testing or rendering.
(() => {
    'use strict';
    const freeze = Object.freeze, isFrozen = Object.isFrozen, values = Object.values, apply = Reflect.apply;
    const listeners = new Map();
    let lastEvent = null, received = 0n, resetCount = 0n;
    function immutable(value) {
        if (value && typeof value === 'object' && !isFrozen(value)) {
            for (const child of values(value)) immutable(child);
            freeze(value);
        }
        return value;
    }
    const endpoint = freeze({
        addEventListener(kind, callback, options = {}) {
            if (typeof callback !== 'function') throw new TypeError('Expected input callback');
            kind = String(kind);
            const list = listeners.get(kind) || [];
            if (!list.some(entry => entry.callback === callback)) list.push({callback, once:!!options?.once});
            listeners.set(kind, list);
        },
        removeEventListener(kind, callback) {
            kind = String(kind);
            listeners.set(kind, (listeners.get(kind) || []).filter(entry => entry.callback !== callback));
        },
        snapshot() { return freeze({lastEvent, received:received.toString(), resetCount:resetCount.toString()}); },
    });
    Object.defineProperty(globalThis, 'hullInput', {value:endpoint, writable:false, configurable:false});
    Object.defineProperty(globalThis, '__hullInputReceive', {
        configurable:true,
        value(record) {
            const event = immutable(record);
            lastEvent = event; received++;
            if (event.kind === 'input-reset') resetCount++;
            let firstError, failed = false;
            const schedule = [...(listeners.get(event.kind) || []).map(entry => [event.kind, entry]), ...(listeners.get('*') || []).map(entry => ['*', entry])];
            for (const [kind,entry] of schedule) {
                if (!(listeners.get(kind) || []).includes(entry)) continue;
                if (entry.once) endpoint.removeEventListener(kind, entry.callback);
                try { apply(entry.callback, endpoint, [event]); }
                catch (error) { if (!failed) { failed=true; firstError=error; } }
            }
            if (failed) throw firstError;
        },
    });
})();
