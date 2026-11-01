// Small, synchronous helpers. Binary strings contain bytes, not UTF-8 text.
(() => {
    'use strict';
    const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
    const fromCharCode = String.fromCharCode;
    function invalid() {
        const error = new Error('Invalid Base64 character or byte string');
        error.name = 'InvalidCharacterError';
        throw error;
    }
    function btoa(value) {
        if (!arguments.length) throw new TypeError('btoa requires one argument');
        const text = `${value}`, output = [];
        for (let index = 0; index < text.length; index += 3) {
            const a = text.charCodeAt(index);
            const b = index + 1 < text.length ? text.charCodeAt(index + 1) : 0;
            const c = index + 2 < text.length ? text.charCodeAt(index + 2) : 0;
            if (a > 255 || b > 255 || c > 255) invalid();
            output.push(alphabet[a >> 2], alphabet[((a & 3) << 4) | (b >> 4)],
                index + 1 < text.length ? alphabet[((b & 15) << 2) | (c >> 6)] : '=',
                index + 2 < text.length ? alphabet[c & 63] : '=');
        }
        return output.join('');
    }
    function atob(value) {
        if (!arguments.length) throw new TypeError('atob requires one argument');
        let text = `${value}`.replace(/[\t\n\f\r ]/g, '');
        if (text.length % 4 === 0) text = text.replace(/={1,2}$/, '');
        if (text.length % 4 === 1 || /[^A-Za-z0-9+/]/.test(text)) invalid();
        const output = [];
        let buffer = 0, bits = 0;
        for (let index = 0; index < text.length; index++) {
            buffer = (buffer << 6) | alphabet.indexOf(text[index]);
            bits += 6;
            if (bits >= 8) {
                bits -= 8;
                output.push(fromCharCode((buffer >> bits) & 255));
            }
        }
        return output.join('');
    }
    Object.defineProperties(globalThis, {
        atob: {value:atob, writable:true, configurable:true, enumerable:true},
        btoa: {value:btoa, writable:true, configurable:true, enumerable:true},
    });
})();
