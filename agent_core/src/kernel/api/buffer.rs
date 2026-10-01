/// 生成注入微内核环境的全局 `Buffer` Polyfill 脚本
pub fn get_buffer_polyfill_script() -> &'static str {
    r#"
    (function() {
        if (typeof globalThis.Buffer !== 'undefined') return;

        const B64_CHARS = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';

        function strToUtf8(str) {
            const out = [];
            let p = 0;
            for (let i = 0; i < str.length; i++) {
                let c = str.charCodeAt(i);
                if (c < 128) {
                    out[p++] = c;
                } else if (c < 2048) {
                    out[p++] = (c >> 6) | 192;
                    out[p++] = (c & 63) | 128;
                } else if (((c & 0xFC00) === 0xD800) && (i + 1) < str.length && ((str.charCodeAt(i + 1) & 0xFC00) === 0xDC00)) {
                    c = 0x10000 + ((c & 0x03FF) << 10) + (str.charCodeAt(++i) & 0x03FF);
                    out[p++] = (c >> 18) | 240;
                    out[p++] = ((c >> 12) & 63) | 128;
                    out[p++] = ((c >> 6) & 63) | 128;
                    out[p++] = (c & 63) | 128;
                } else {
                    out[p++] = (c >> 12) | 224;
                    out[p++] = ((c >> 6) & 63) | 128;
                    out[p++] = (c & 63) | 128;
                }
            }
            return new Uint8Array(out);
        }

        function utf8ToStr(bytes) {
            let out = '';
            let i = 0;
            while (i < bytes.length) {
                const c = bytes[i++];
                if (c > 127) {
                    if (c > 191 && c < 224) {
                        const c2 = bytes[i++];
                        out += String.fromCharCode(((c & 31) << 6) | (c2 & 63));
                    } else if (c > 223 && c < 240) {
                        const c2 = bytes[i++];
                        const c3 = bytes[i++];
                        out += String.fromCharCode(((c & 15) << 12) | ((c2 & 63) << 6) | (c3 & 63));
                    } else if (c > 239 && c < 248) {
                        const c2 = bytes[i++];
                        const c3 = bytes[i++];
                        const c4 = bytes[i++];
                        let u = (((c & 7) << 18) | ((c2 & 63) << 12) | ((c3 & 63) << 6) | (c4 & 63)) - 0x10000;
                        out += String.fromCharCode((u >> 10) + 0xD800, (u & 0x3FF) + 0xDC00);
                    }
                } else {
                    out += String.fromCharCode(c);
                }
            }
            return out;
        }

        function toBase64(bytes) {
            let out = '';
            for (let i = 0; i < bytes.length; i += 3) {
                const b0 = bytes[i];
                const b1 = i + 1 < bytes.length ? bytes[i + 1] : undefined;
                const b2 = i + 2 < bytes.length ? bytes[i + 2] : undefined;
                out += B64_CHARS[b0 >> 2];
                out += B64_CHARS[((b0 & 3) << 4) | (b1 !== undefined ? (b1 >> 4) : 0)];
                out += b1 !== undefined ? B64_CHARS[((b1 & 15) << 2) | (b2 !== undefined ? (b2 >> 6) : 0)] : '=';
                out += b2 !== undefined ? B64_CHARS[b2 & 63] : '=';
            }
            return out;
        }

        function fromBase64(str) {
            const clean = str.replace(/[^A-Za-z0-9+/]/g, '');
            const out = [];
            let p = 0;
            for (let i = 0; i < clean.length; i += 4) {
                const c0 = B64_CHARS.indexOf(clean[i]);
                const c1 = B64_CHARS.indexOf(clean[i + 1]);
                const c2 = i + 2 < clean.length ? B64_CHARS.indexOf(clean[i + 2]) : -1;
                const c3 = i + 3 < clean.length ? B64_CHARS.indexOf(clean[i + 3]) : -1;
                out[p++] = (c0 << 2) | (c1 >> 4);
                if (c2 !== -1) out[p++] = ((c1 & 15) << 4) | (c2 >> 2);
                if (c3 !== -1) out[p++] = ((c2 & 3) << 6) | c3;
            }
            return new Uint8Array(out);
        }

        class Buffer extends Uint8Array {
            static from(value, encoding = 'utf-8') {
                if (typeof value === 'string') {
                    const enc = String(encoding).toLowerCase().replace('-', '');
                    if (enc === 'hex') {
                        const cleanHex = value.replace(/[^0-9a-fA-F]/g, '');
                        const len = Math.floor(cleanHex.length / 2);
                        const u8 = new Uint8Array(len);
                        for (let i = 0; i < len; i++) {
                            u8[i] = parseInt(cleanHex.substr(i * 2, 2), 16);
                        }
                        return new Buffer(u8.buffer);
                    } else if (enc === 'base64') {
                        return new Buffer(fromBase64(value).buffer);
                    } else {
                        return new Buffer(strToUtf8(value).buffer);
                    }
                } else if (value instanceof Uint8Array || Array.isArray(value)) {
                    return new Buffer(value);
                } else if (value instanceof ArrayBuffer) {
                    return new Buffer(value);
                }
                return new Buffer(0);
            }

            static alloc(size, fill = 0) {
                const buf = new Buffer(Number(size) || 0);
                if (fill !== 0) {
                    buf.fill(fill);
                }
                return buf;
            }

            static isBuffer(obj) {
                return obj instanceof Buffer;
            }

            toString(encoding = 'utf-8') {
                const enc = String(encoding).toLowerCase().replace('-', '');
                if (enc === 'hex') {
                    let hex = '';
                    for (let i = 0; i < this.length; i++) {
                        const byte = this[i].toString(16);
                        hex += (byte.length === 1 ? '0' : '') + byte;
                    }
                    return hex;
                } else if (enc === 'base64') {
                    return toBase64(this);
                } else {
                    return utf8ToStr(this);
                }
            }
        }

        globalThis.Buffer = Buffer;
    })();
    "#
}
