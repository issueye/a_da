/// 生成注入微内核环境的 `node:url` / `url` 模块 Polyfill 脚本
pub fn get_url_polyfill_script() -> &'static str {
    r#"
    (function() {
        const isWindows = typeof process !== 'undefined' && process.platform === 'win32';

        class URLSearchParams {
            constructor(init) {
                this._list = [];
                if (!init) return;

                if (typeof init === 'string') {
                    let str = init;
                    if (str.startsWith('?')) str = str.slice(1);
                    if (str) {
                        const pairs = str.split('&');
                        for (let i = 0; i < pairs.length; i++) {
                            const pair = pairs[i];
                            if (!pair) continue;
                            const idx = pair.indexOf('=');
                            if (idx !== -1) {
                                const k = decodeURIComponent(pair.slice(0, idx).replace(/\+/g, ' '));
                                const v = decodeURIComponent(pair.slice(idx + 1).replace(/\+/g, ' '));
                                this._list.push([k, v]);
                            } else {
                                const k = decodeURIComponent(pair.replace(/\+/g, ' '));
                                this._list.push([k, '']);
                            }
                        }
                    }
                } else if (Array.isArray(init)) {
                    for (let i = 0; i < init.length; i++) {
                        const item = init[i];
                        if (Array.isArray(item) && item.length >= 2) {
                            this._list.push([String(item[0]), String(item[1])]);
                        }
                    }
                } else if (init instanceof URLSearchParams) {
                    for (const [k, v] of init.entries()) {
                        this._list.push([k, v]);
                    }
                } else if (typeof init === 'object' && init !== null) {
                    for (const k of Object.keys(init)) {
                        this._list.push([k, String(init[k])]);
                    }
                }
            }

            append(name, value) {
                this._list.push([String(name), String(value)]);
                this._onChange && this._onChange();
            }

            delete(name) {
                const n = String(name);
                this._list = this._list.filter(([k]) => k !== n);
                this._onChange && this._onChange();
            }

            get(name) {
                const n = String(name);
                for (let i = 0; i < this._list.length; i++) {
                    if (this._list[i][0] === n) return this._list[i][1];
                }
                return null;
            }

            getAll(name) {
                const n = String(name);
                const res = [];
                for (let i = 0; i < this._list.length; i++) {
                    if (this._list[i][0] === n) res.push(this._list[i][1]);
                }
                return res;
            }

            has(name) {
                const n = String(name);
                for (let i = 0; i < this._list.length; i++) {
                    if (this._list[i][0] === n) return true;
                }
                return false;
            }

            set(name, value) {
                const n = String(name);
                const v = String(value);
                let replaced = false;
                const next = [];
                for (let i = 0; i < this._list.length; i++) {
                    if (this._list[i][0] === n) {
                        if (!replaced) {
                            next.push([n, v]);
                            replaced = true;
                        }
                    } else {
                        next.push(this._list[i]);
                    }
                }
                if (!replaced) {
                    next.push([n, v]);
                }
                this._list = next;
                this._onChange && this._onChange();
            }

            sort() {
                this._list.sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0));
                this._onChange && this._onChange();
            }

            forEach(callback, thisArg) {
                for (let i = 0; i < this._list.length; i++) {
                    const [k, v] = this._list[i];
                    callback.call(thisArg, v, k, this);
                }
            }

            *keys() {
                for (let i = 0; i < this._list.length; i++) yield this._list[i][0];
            }

            *values() {
                for (let i = 0; i < this._list.length; i++) yield this._list[i][1];
            }

            *entries() {
                for (let i = 0; i < this._list.length; i++) yield [this._list[i][0], this._list[i][1]];
            }

            [Symbol.iterator]() {
                return this.entries();
            }

            toString() {
                return this._list
                    .map(([k, v]) => encodeURIComponent(k).replace(/%20/g, '+') + '=' + encodeURIComponent(v).replace(/%20/g, '+'))
                    .join('&');
            }
        }

        class URL {
            constructor(input, base) {
                if (typeof input !== 'string' && !(input instanceof URL)) {
                    throw new TypeError('Invalid URL: ' + input);
                }
                let rawInput = String(input);
                let fullUrl = rawInput;

                if (base !== undefined) {
                    let baseUrl = base instanceof URL ? base.href : String(base);
                    if (!rawInput.includes('://')) {
                        // 相对 URL 结合 base 进行解析
                        const baseParsed = parseUrlComponents(baseUrl);
                        if (!baseParsed) {
                            throw new TypeError('Invalid base URL: ' + baseUrl);
                        }
                        if (rawInput.startsWith('//')) {
                            fullUrl = baseParsed.protocol + rawInput;
                        } else if (rawInput.startsWith('/')) {
                            fullUrl = baseParsed.origin + rawInput;
                        } else if (rawInput.startsWith('?')) {
                            fullUrl = baseParsed.origin + baseParsed.pathname + rawInput;
                        } else if (rawInput.startsWith('#')) {
                            fullUrl = baseParsed.origin + baseParsed.pathname + (baseParsed.search || '') + rawInput;
                        } else {
                            // 相对同级目录
                            let baseDir = baseParsed.pathname;
                            const lastSlash = baseDir.lastIndexOf('/');
                            if (lastSlash !== -1) {
                                baseDir = baseDir.slice(0, lastSlash + 1);
                            } else {
                                baseDir = '/';
                            }
                            fullUrl = baseParsed.origin + baseDir + rawInput;
                        }
                    }
                }

                const components = parseUrlComponents(fullUrl);
                if (!components) {
                    throw new TypeError('Invalid URL string: ' + fullUrl);
                }

                this._protocol = components.protocol;
                this._username = components.username || '';
                this._password = components.password || '';
                this._hostname = components.hostname || '';
                this._port = components.port || '';
                this._pathname = components.pathname || '/';
                this._hash = components.hash || '';

                this._searchParams = new URLSearchParams(components.search || '');
                this._searchParams._onChange = () => {};
            }

            get protocol() { return this._protocol; }
            set protocol(val) {
                let p = String(val);
                if (!p.endsWith(':')) p += ':';
                this._protocol = p;
            }

            get host() {
                return this._port ? this._hostname + ':' + this._port : this._hostname;
            }
            set host(val) {
                const parts = String(val).split(':');
                this._hostname = parts[0];
                this._port = parts[1] || '';
            }

            get hostname() { return this._hostname; }
            set hostname(val) { this._hostname = String(val); }

            get port() { return this._port; }
            set port(val) { this._port = String(val); }

            get pathname() { return this._pathname; }
            set pathname(val) {
                let p = String(val);
                if (!p.startsWith('/')) p = '/' + p;
                this._pathname = p;
            }

            get search() {
                const str = this._searchParams.toString();
                return str ? '?' + str : '';
            }
            set search(val) {
                let s = String(val);
                if (s.startsWith('?')) s = s.slice(1);
                this._searchParams = new URLSearchParams(s);
            }

            get searchParams() {
                return this._searchParams;
            }

            get hash() { return this._hash; }
            set hash(val) {
                let h = String(val);
                if (h && !h.startsWith('#')) h = '#' + h;
                this._hash = h;
            }

            get origin() {
                if (this._protocol === 'file:') return 'null';
                return this._protocol + '//' + this.host;
            }

            get href() {
                let authority = '';
                if (this._username || this._password) {
                    authority += this._username;
                    if (this._password) authority += ':' + this._password;
                    authority += '@';
                }
                authority += this.host;

                let res = this._protocol + (authority ? '//' + authority : (this._protocol === 'file:' ? '///' : ''));
                if (this._protocol === 'file:') {
                    let pn = this._pathname;
                    if (pn.startsWith('/')) pn = pn.slice(1);
                    res = 'file:///' + pn;
                } else {
                    res += this._pathname;
                }
                res += this.search;
                res += this.hash;
                return res;
            }
            set href(val) {
                const next = new URL(val);
                this._protocol = next._protocol;
                this._hostname = next._hostname;
                this._port = next._port;
                this._pathname = next._pathname;
                this._hash = next._hash;
                this._searchParams = next._searchParams;
            }

            toString() { return this.href; }
            toJSON() { return this.href; }
        }

        function parseUrlComponents(urlStr) {
            // 标准 URL 匹配正则
            // scheme://[user:pass@]host[:port]/path?query#hash
            // 或 file:///C:/path...
            const match = urlStr.match(/^([a-zA-Z][a-zA-Z0-9+.-]*:)(?:\/\/([^\/?#]*))?([^?#]*)(?:\?([^#]*))?(?:#(.*))?$/);
            if (!match) return null;

            const protocol = match[1].toLowerCase();
            const rawHost = match[2] || '';
            let pathname = match[3] || '/';
            const search = match[4] !== undefined ? '?' + match[4] : '';
            const hash = match[5] !== undefined ? '#' + match[5] : '';

            let username = '';
            let password = '';
            let host = rawHost;

            const atIdx = host.indexOf('@');
            if (atIdx !== -1) {
                const userinfo = host.slice(0, atIdx);
                host = host.slice(atIdx + 1);
                const colonIdx = userinfo.indexOf(':');
                if (colonIdx !== -1) {
                    username = userinfo.slice(0, colonIdx);
                    password = userinfo.slice(colonIdx + 1);
                } else {
                    username = userinfo;
                }
            }

            let hostname = host;
            let port = '';
            const portIdx = host.lastIndexOf(':');
            if (portIdx !== -1 && !host.includes(']')) {
                hostname = host.slice(0, portIdx);
                port = host.slice(portIdx + 1);
            }

            return {
                protocol,
                username,
                password,
                hostname,
                port,
                pathname,
                search,
                hash
            };
        }

        function isWindowsPath(p) {
            if (typeof process !== 'undefined' && process.platform === 'win32') return true;
            if (typeof p === 'string' && p.length >= 2 && p.charCodeAt(1) === 58) {
                const c = p.charCodeAt(0);
                if ((c >= 65 && c <= 90) || (c >= 97 && c <= 122)) return true;
            }
            return false;
        }

        function isWindowsUrl(parsed) {
            if (typeof process !== 'undefined' && process.platform === 'win32') return true;
            const pn = parsed.pathname;
            if (pn && pn.length >= 3 && pn.charCodeAt(0) === 47 && pn.charCodeAt(2) === 58) {
                return true;
            }
            return false;
        }

        /// Node.js 兼容：fileURLToPath
        function fileURLToPath(url) {
            let parsed = url;
            if (typeof url === 'string') {
                parsed = new URL(url);
            } else if (!(url instanceof URL)) {
                throw new TypeError('fileURLToPath 期望参数为 string 或 URL 实例');
            }

            if (parsed.protocol !== 'file:') {
                throw new TypeError('URL 协议必须为 "file:", 实际为: ' + parsed.protocol);
            }

            let pathname = decodeURIComponent(parsed.pathname);

            if (isWindowsUrl(parsed)) {
                // Windows 下 file:///C:/path 或 file://localhost/C:/path
                // 如果是 /C:/... 开头，去掉首个 /
                if (pathname.length >= 3 && pathname.charCodeAt(0) === 47 /* / */ &&
                    pathname.charCodeAt(2) === 58 /* : */) {
                    pathname = pathname.slice(1);
                } else if (parsed.hostname && parsed.hostname !== 'localhost') {
                    // UNC 路径：file://server/share/file -> \\server\share\file
                    return '\\\\' + parsed.hostname + pathname.replace(/\//g, '\\');
                }
                return pathname.replace(/\//g, '\\');
            } else {
                return pathname;
            }
        }

        /// Node.js 兼容：pathToFileURL
        function pathToFileURL(filepath) {
            if (typeof filepath !== 'string') {
                throw new TypeError('pathToFileURL 期望字符串路径参数');
            }

            let resolved = filepath;
            if (typeof path !== 'undefined' && path.resolve) {
                resolved = path.resolve(filepath);
            }

            // 处理末尾斜杠
            const hasTrailingSlash = filepath.endsWith('/') || filepath.endsWith('\\');

            let pathUrl = '';
            if (isWindowsPath(resolved)) {
                // 转换 Windows 盘符 C:\dir\file -> /C:/dir/file
                let normalized = resolved.replace(/\\/g, '/');
                if (normalized.length >= 2 && normalized.charCodeAt(1) === 58) {
                    normalized = '/' + normalized;
                }
                // 对每一段进行 encodeURI 处理
                pathUrl = 'file://' + encodeURI(normalized);
            } else {
                pathUrl = 'file://' + encodeURI(resolved);
            }

            if (hasTrailingSlash && !pathUrl.endsWith('/')) {
                pathUrl += '/';
            }

            return new URL(pathUrl);
        }

        const urlModule = {
            URL,
            URLSearchParams,
            fileURLToPath,
            pathToFileURL,
            format: function(urlObj) {
                if (typeof urlObj === 'string') return urlObj;
                if (urlObj instanceof URL) return urlObj.href;
                return (new URL(urlObj.href || '', 'http://localhost')).href;
            },
            parse: function(urlStr, parseQueryString) {
                const u = new URL(urlStr, 'http://localhost');
                return {
                    href: u.href,
                    protocol: u.protocol,
                    host: u.host,
                    hostname: u.hostname,
                    port: u.port,
                    pathname: u.pathname,
                    search: u.search,
                    query: parseQueryString ? Object.fromEntries(u.searchParams) : (u.search.startsWith('?') ? u.search.slice(1) : u.search),
                    hash: u.hash
                };
            },
            resolve: function(from, to) {
                return (new URL(to, new URL(from, 'http://localhost'))).href;
            }
        };

        // 注入全局
        globalThis.URL = URL;
        globalThis.URLSearchParams = URLSearchParams;
        globalThis.url = urlModule;
    })();
    "#
}

#[cfg(test)]
mod tests {
    use super::*;
    use boa_engine::{Context, Source};

    #[test]
    fn test_url_and_search_params_in_boa() {
        let mut ctx = Context::default();
        let script = get_url_polyfill_script();
        ctx.eval(Source::from_bytes(script)).expect("注入 url polyfill 失败");

        // 1. 验证 URL 解析与 searchParams
        let eval_res = ctx.eval(Source::from_bytes(r#"
            const u = new URL("https://example.com:8080/api/v1?user=alice&tag=rust#section-1");
            u.searchParams.append("tag", "typescript");
            const tags = u.searchParams.getAll("tag").join(",");
            const port = u.port;
            const pathname = u.pathname;
            const origin = u.origin;
            tags + '|' + port + '|' + pathname + '|' + origin;
        "#)).expect("eval url 失败");
        assert_eq!(
            eval_res.to_string(&mut ctx).unwrap().to_std_string_escaped(),
            "rust,typescript|8080|/api/v1|https://example.com:8080"
        );

        // 2. 验证 fileURLToPath 与 pathToFileURL (Windows 与 Unix 语义)
        let eval_file_url = ctx.eval(Source::from_bytes(r#"
            (function() {
                const uFile = new URL("file:///C:/projects/a_da/src/index.ts");
                const p = url.fileURLToPath(uFile);
                const backUrl = url.pathToFileURL("C:\\projects\\a_da\\src\\index.ts").href;
                return p + '|' + backUrl;
            })();
        "#)).expect("eval fileURLToPath 失败");
        let out = eval_file_url.to_string(&mut ctx).unwrap().to_std_string_escaped();
        assert!(out.contains("C:\\projects\\a_da\\src\\index.ts") || out.contains("C:/projects/a_da/src/index.ts"));
        assert!(out.contains("file:///C:/projects/a_da/src/index.ts") || out.contains("file:///C%3A/projects/a_da/src/index.ts") || out.contains("file:///C:"));
    }
}
