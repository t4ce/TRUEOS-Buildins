// A text console only: bounded native records, no terminal or document access.
(() => {
    'use strict';
    const write = globalThis.__hullConsoleWrite;
    delete globalThis.__hullConsoleWrite;
    const string = String, number = Number, integer = parseInt, decimal = parseFloat;
    const stringify = JSON.stringify;
    const LIMIT = 4096, ARGS = 32;

    function inspect(value) {
        if (value !== null && typeof value === 'object') {
            try {
                const json = stringify(value);
                if (json !== undefined) return json;
            } catch (_) { /* Cycles and unsupported values fall back to text. */ }
        }
        return string(value);
    }

    function format(args) {
        if (!args.length) return '';
        let used = 0, text = '';
        if (typeof args[0] === 'string') {
            used = 1;
            const first = args[0];
            for (let i = 0; i < first.length && text.length < LIMIT; i++) {
                if (first[i] === '%' && i + 1 < first.length) {
                    const token = first[i + 1];
                    if (token === '%') { text += '%'; i++; continue; }
                    if (used < args.length && used < ARGS && 'sdifoO'.includes(token)) {
                        const value = args[used++];
                        text += (token === 's' ? string(value)
                            : token === 'd' ? string(number(value))
                            : token === 'i' ? string(integer(value, 10))
                            : token === 'f' ? string(decimal(value)) : inspect(value)).slice(0, LIMIT - text.length);
                        i++;
                        continue;
                    }
                }
                text += first[i];
            }
        }
        for (; used < args.length && used < ARGS && text.length < LIMIT; used++) {
            if (used) text += ' ';
            text += inspect(args[used]).slice(0, LIMIT - text.length);
        }
        return text;
    }

    const console = {};
    for (const [name, level] of [['log', 0], ['debug', 1], ['info', 2], ['warn', 3], ['error', 4]]) {
        console[name] = (...args) => write(level, format(args));
    }
    console.assert = (condition, ...args) => {
        if (!condition) write(4, 'Assertion failed' + (args.length ? ': ' + format(args) : ''));
    };
    Object.defineProperty(globalThis, 'console', {value:console, writable:true, configurable:true});
})();
