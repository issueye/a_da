/// 生成注入微内核环境的 `node:path` / `path` 模块 Polyfill 脚本
pub fn get_path_polyfill_script() -> &'static str {
    r#"
    (function() {
        const isWindows = typeof process !== 'undefined' && process.platform === 'win32';
        const sep = isWindows ? '\\' : '/';
        const delimiter = isWindows ? ';' : ':';

        function normalizeArray(parts, allowAboveRoot) {
            const res = [];
            for (let i = 0; i < parts.length; i++) {
                const p = parts[i];
                if (!p || p === '.') continue;
                if (p === '..') {
                    if (res.length && res[res.length - 1] !== '..') {
                        res.pop();
                    } else if (allowAboveRoot) {
                        res.push('..');
                    }
                } else {
                    res.push(p);
                }
            }
            return res;
        }

        function isAbsolute(path) {
            if (typeof path !== 'string' || path.length === 0) return false;
            if (path.charCodeAt(0) === 47 /* / */ || path.charCodeAt(0) === 92 /* \ */) return true;
            // 支持 Windows 盘符如 C:\ 或 D:/
            if (path.length >= 3 && path.charCodeAt(1) === 58 /* : */ &&
                (path.charCodeAt(2) === 47 || path.charCodeAt(2) === 92)) {
                const code = path.charCodeAt(0);
                if ((code >= 65 && code <= 90) || (code >= 97 && code <= 122)) return true;
            }
            return false;
        }

        function normalize(path) {
            if (typeof path !== 'string' || path.length === 0) return '.';
            const isAbs = isAbsolute(path);
            const trailingSlash = path.endsWith('/') || path.endsWith('\\');
            
            let device = '';
            let rest = path;
            if (path.length >= 2 && path.charCodeAt(1) === 58) {
                device = path.slice(0, 2);
                rest = path.slice(2);
            }

            const segments = rest.split(/[\/\\]+/);
            const filtered = normalizeArray(segments, !isAbs);
            let result = filtered.join(sep);

            if (!result && !isAbs) {
                result = '.';
            }
            if (isAbs) {
                result = sep + result;
            }
            if (device) {
                result = device + (result.startsWith(sep) ? result : sep + result);
            }
            if (trailingSlash && !result.endsWith(sep)) {
                result += sep;
            }
            return result;
        }

        function join(...paths) {
            if (paths.length === 0) return '.';
            let joined = '';
            for (let i = 0; i < paths.length; i++) {
                const segment = paths[i];
                if (typeof segment !== 'string') {
                    throw new TypeError('Path segments must be strings');
                }
                if (!segment) continue;
                if (!joined) {
                    joined = segment;
                } else {
                    joined += '/' + segment;
                }
            }
            return normalize(joined);
        }

        function resolve(...paths) {
            let resolvedPath = '';
            let resolvedAbsolute = false;
            const cwd = typeof process !== 'undefined' && process.cwd ? process.cwd() : '.';

            for (let i = paths.length - 1; i >= -1 && !resolvedAbsolute; i--) {
                const path = i >= 0 ? paths[i] : cwd;
                if (!path || typeof path !== 'string') continue;

                resolvedPath = path + '/' + resolvedPath;
                resolvedAbsolute = isAbsolute(path);
            }

            return normalize(resolvedPath);
        }

        function dirname(path) {
            if (typeof path !== 'string' || path.length === 0) return '.';
            let end = -1;
            let matchedSlash = true;
            for (let i = path.length - 1; i >= 0; --i) {
                const code = path.charCodeAt(i);
                if (code === 47 /* / */ || code === 92 /* \ */) {
                    if (!matchedSlash) {
                        end = i;
                        break;
                    }
                } else {
                    matchedSlash = false;
                }
            }
            if (end === -1) return isAbsolute(path) ? path[0] : '.';
            if (end === 2 && path.charCodeAt(1) === 58) return path.slice(0, 3);
            return path.slice(0, end);
        }

        function basename(path, ext) {
            if (typeof path !== 'string') return '';
            let start = 0;
            let end = -1;
            let matchedSlash = true;
            for (let i = path.length - 1; i >= 0; --i) {
                const code = path.charCodeAt(i);
                if (code === 47 /* / */ || code === 92 /* \ */) {
                    if (!matchedSlash) {
                        start = i + 1;
                        break;
                    }
                } else {
                    if (end === -1) {
                        end = i + 1;
                    }
                    matchedSlash = false;
                }
            }
            if (end === -1) return '';
            let base = path.slice(start, end);
            if (ext && typeof ext === 'string' && base.endsWith(ext) && base.length > ext.length) {
                base = base.slice(0, base.length - ext.length);
            }
            return base;
        }

        function extname(path) {
            if (typeof path !== 'string') return '';
            let startDot = -1;
            let startPart = 0;
            let end = -1;
            let matchedSlash = true;
            let preDotState = 0;
            for (let i = path.length - 1; i >= 0; --i) {
                const code = path.charCodeAt(i);
                if (code === 47 /* / */ || code === 92 /* \ */) {
                    if (!matchedSlash) {
                        startPart = i + 1;
                        break;
                    }
                    continue;
                }
                if (end === -1) {
                    end = i + 1;
                }
                if (code === 46 /* . */) {
                    if (startDot === -1) startDot = i;
                    else if (preDotState !== 1) preDotState = 1;
                } else if (startDot !== -1) {
                    preDotState = -1;
                }
                matchedSlash = false;
            }

            if (startDot === -1 || end === -1 || startDot === end - 1 ||
                (startDot === startPart && preDotState === 0)) {
                return '';
            }
            return path.slice(startDot, end);
        }

        function relative(from, to) {
            const fromAbs = resolve(from);
            const toAbs = resolve(to);
            if (fromAbs === toAbs) return '';

            const fromParts = fromAbs.split(/[\/\\]+/).filter(Boolean);
            const toParts = toAbs.split(/[\/\\]+/).filter(Boolean);

            let sameParts = 0;
            while (sameParts < fromParts.length && sameParts < toParts.length &&
                   fromParts[sameParts] === toParts[sameParts]) {
                sameParts++;
            }

            const upCount = fromParts.length - sameParts;
            const resParts = [];
            for (let i = 0; i < upCount; i++) resParts.push('..');
            for (let i = sameParts; i < toParts.length; i++) resParts.push(toParts[i]);
            return resParts.join(sep);
        }

        const pathModule = {
            join,
            resolve,
            normalize,
            isAbsolute,
            dirname,
            basename,
            extname,
            relative,
            sep,
            delimiter,
            posix: null,
            win32: null,
        };
        pathModule.posix = pathModule;
        pathModule.win32 = pathModule;

        globalThis.path = pathModule;
    })();
    "#
}
