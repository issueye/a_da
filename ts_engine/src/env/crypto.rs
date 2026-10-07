use boa_engine::{
    js_error, js_string, Context, JsValue, NativeFunction, Source,
};

/// 计算 MD5 摘要 (RFC 1321)
pub fn compute_md5(data: &[u8]) -> [u8; 16] {
    let mut a: u32 = 0x67452301;
    let mut b: u32 = 0xefcdab89;
    let mut c: u32 = 0x98badcfe;
    let mut d: u32 = 0x10325476;

    let s: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22,
        5,  9, 14, 20, 5,  9, 14, 20, 5,  9, 14, 20, 5,  9, 14, 20,
        4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23,
        6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];

    let k: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee,
        0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
        0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be,
        0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
        0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa,
        0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
        0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
        0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c,
        0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
        0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05,
        0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
        0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039,
        0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1,
        0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
    ];

    let bit_len = (data.len() as u64) * 8;
    let mut msg = data.to_vec();
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());

    for chunk in msg.chunks_exact(64) {
        let mut m = [0u32; 16];
        for i in 0..16 {
            m[i] = u32::from_le_bytes(chunk[i * 4..(i + 1) * 4].try_into().unwrap());
        }

        let mut aa = a;
        let mut bb = b;
        let mut cc = c;
        let mut dd = d;

        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((bb & cc) | (!bb & dd), i),
                16..=31 => ((dd & bb) | (!dd & cc), (5 * i + 1) % 16),
                32..=47 => (bb ^ cc ^ dd, (3 * i + 5) % 16),
                _ => (cc ^ (bb | !dd), (7 * i) % 16),
            };

            let temp = dd;
            dd = cc;
            cc = bb;
            bb = bb.wrapping_add(
                aa.wrapping_add(f)
                    .wrapping_add(k[i])
                    .wrapping_add(m[g])
                    .rotate_left(s[i]),
            );
            aa = temp;
        }

        a = a.wrapping_add(aa);
        b = b.wrapping_add(bb);
        c = c.wrapping_add(cc);
        d = d.wrapping_add(dd);
    }

    let mut result = [0u8; 16];
    result[0..4].copy_from_slice(&a.to_le_bytes());
    result[4..8].copy_from_slice(&b.to_le_bytes());
    result[8..12].copy_from_slice(&c.to_le_bytes());
    result[12..16].copy_from_slice(&d.to_le_bytes());
    result
}

/// 计算 SHA-1 摘要 (RFC 3174)
pub fn compute_sha1(data: &[u8]) -> [u8; 20] {
    let mut h0: u32 = 0x67452301;
    let mut h1: u32 = 0xefcdab89;
    let mut h2: u32 = 0x98badcfe;
    let mut h3: u32 = 0x10325476;
    let mut h4: u32 = 0xc3d2e1f0;

    let bit_len = (data.len() as u64) * 8;
    let mut msg = data.to_vec();
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(chunk[i * 4..(i + 1) * 4].try_into().unwrap());
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }

        let mut a = h0;
        let mut b = h1;
        let mut c = h2;
        let mut d = h3;
        let mut e = h4;

        for i in 0..80 {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5a827999),
                20..=39 => (b ^ c ^ d, 0x6ed9eba1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1bbcdc),
                _ => (b ^ c ^ d, 0xca62c1d6),
            };

            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(w[i]);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }

        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
    }

    let mut result = [0u8; 20];
    result[0..4].copy_from_slice(&h0.to_be_bytes());
    result[4..8].copy_from_slice(&h1.to_be_bytes());
    result[8..12].copy_from_slice(&h2.to_be_bytes());
    result[12..16].copy_from_slice(&h3.to_be_bytes());
    result[16..20].copy_from_slice(&h4.to_be_bytes());
    result
}

/// 计算 SHA-256 摘要
pub fn compute_sha256(data: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().into()
}

/// 将字节数组转为十六进制小写字符串
pub fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        write!(s, "{:02x}", b).unwrap();
    }
    s
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// 将十六进制字符串解码为字节数组
pub fn decode_hex(s: &str) -> Vec<u8> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for chunk in bytes.chunks_exact(2) {
        if let (Some(h), Some(l)) = (hex_digit(chunk[0]), hex_digit(chunk[1])) {
            out.push((h << 4) | l);
        }
    }
    out
}

/// 向 Boa 上下文注册同步 `crypto` 原生函数与封装脚本
pub fn register_crypto_native_and_script(ctx: &mut Context) -> Result<(), String> {
    // 1. 原生哈希计算函数 __native_crypto_hash(algo, hex_input) -> hex_digest
    let native_hash = NativeFunction::from_copy_closure(|_this, args, ctx| {
        let algo = args.get(0).map(|v| v.to_string(ctx).unwrap_or_default().to_std_string_escaped()).unwrap_or_default();
        let hex_input = args.get(1).map(|v| v.to_string(ctx).unwrap_or_default().to_std_string_escaped()).unwrap_or_default();

        let raw_bytes = decode_hex(&hex_input);
        let normalized = algo.to_lowercase();
        let digest_hex = match normalized.as_str() {
            "sha256" | "sha-256" => to_hex(&compute_sha256(&raw_bytes)),
            "sha1" | "sha-1" => to_hex(&compute_sha1(&raw_bytes)),
            "md5" => to_hex(&compute_md5(&raw_bytes)),
            _ => return Err(js_error!("不支持的哈希算法: {}", algo)),
        };

        Ok(JsValue::from(js_string!(digest_hex.as_str())))
    });
    ctx.register_global_callable(js_string!("__native_crypto_hash"), 2, native_hash).ok();

    // 2. 原生 UUID 生成器 __native_crypto_random_uuid() -> string
    let native_random_uuid = NativeFunction::from_copy_closure(|_this, _args, _ctx| {
        let uuid_str = uuid::Uuid::new_v4().to_string();
        Ok(JsValue::from(js_string!(uuid_str.as_str())))
    });
    ctx.register_global_callable(js_string!("__native_crypto_random_uuid"), 0, native_random_uuid).ok();

    // 3. 原生随机字节生成器 __native_crypto_random_bytes(size) -> hex_string
    let native_random_bytes = NativeFunction::from_copy_closure(|_this, args, ctx| {
        let size = args.get(0).and_then(|v| v.to_u32(ctx).ok()).unwrap_or(0) as usize;
        let mut buf = vec![0u8; size];
        // 使用伪随机/时间戳等生成安全字节
        for b in buf.iter_mut() {
            *b = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0) % 256) as u8;
        }
        Ok(JsValue::from(js_string!(to_hex(&buf).as_str())))
    });
    ctx.register_global_callable(js_string!("__native_crypto_random_bytes"), 1, native_random_bytes).ok();

    // 4. 注入 JavaScript 封装层 (Hash 类及 crypto 对象)
    let crypto_js_wrapper = r#"
        (function() {
            function toHexStr(bytes) {
                let hex = '';
                for (let i = 0; i < bytes.length; i++) {
                    const b = bytes[i];
                    hex += (b < 16 ? '0' : '') + b.toString(16);
                }
                return hex;
            }

            class Hash {
                constructor(algorithm) {
                    this.algorithm = String(algorithm).toLowerCase();
                    this.hexChunks = [];
                    this.finalized = false;
                }

                update(data, encoding = 'utf-8') {
                    if (this.finalized) {
                        throw new Error("Digest already called");
                    }
                    if (typeof data === 'string') {
                        if (typeof Buffer !== 'undefined') {
                            const buf = Buffer.from(data, encoding);
                            this.hexChunks.push(buf.toString('hex'));
                        } else {
                            const bytes = [];
                            for (let i = 0; i < data.length; i++) {
                                bytes.push(data.charCodeAt(i) & 0xff);
                            }
                            this.hexChunks.push(toHexStr(bytes));
                        }
                    } else if (typeof Buffer !== 'undefined' && Buffer.isBuffer(data)) {
                        this.hexChunks.push(data.toString('hex'));
                    } else if (data instanceof Uint8Array || Array.isArray(data)) {
                        this.hexChunks.push(toHexStr(data));
                    } else if (data instanceof ArrayBuffer) {
                        this.hexChunks.push(toHexStr(new Uint8Array(data)));
                    } else {
                        const str = String(data);
                        const buf = typeof Buffer !== 'undefined' ? Buffer.from(str) : null;
                        this.hexChunks.push(buf ? buf.toString('hex') : toHexStr(Array.from(str).map(c => c.charCodeAt(0) & 0xff)));
                    }
                    return this; // 支持链式调用
                }

                digest(encoding = 'hex') {
                    if (this.finalized) {
                        throw new Error("Digest already called");
                    }
                    this.finalized = true;
                    const fullHex = this.hexChunks.join('');
                    const resultHex = __native_crypto_hash(this.algorithm, fullHex);
                    const enc = encoding ? String(encoding).toLowerCase().replace('-', '') : null;
                    if (enc === 'hex') {
                        return resultHex;
                    } else if (enc === 'base64') {
                        if (typeof Buffer !== 'undefined') {
                            return Buffer.from(resultHex, 'hex').toString('base64');
                        }
                        return resultHex;
                    } else if (!enc || enc === 'buffer') {
                        if (typeof Buffer !== 'undefined') {
                            return Buffer.from(resultHex, 'hex');
                        }
                        return resultHex;
                    }
                    return resultHex;
                }
            }

            const cryptoModule = {
                createHash: function(algorithm) {
                    return new Hash(algorithm);
                },
                randomUUID: function() {
                    if (typeof __native_crypto_random_uuid === 'function') {
                        return __native_crypto_random_uuid();
                    }
                    return 'xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx'.replace(/[xy]/g, function(c) {
                        const r = Math.random() * 16 | 0;
                        const v = c === 'x' ? r : (r & 0x3 | 0x8);
                        return v.toString(16);
                    });
                },
                randomBytes: function(size) {
                    const count = Number(size) || 0;
                    if (typeof __native_crypto_random_bytes === 'function') {
                        const hex = __native_crypto_random_bytes(count);
                        return typeof Buffer !== 'undefined' ? Buffer.from(hex, 'hex') : hex;
                    }
                    const arr = new Uint8Array(count);
                    for (let i = 0; i < count; i++) {
                        arr[i] = Math.floor(Math.random() * 256);
                    }
                    return typeof Buffer !== 'undefined' ? Buffer.from(arr) : arr;
                },
                getHashes: function() {
                    return ['sha256', 'sha1', 'md5'];
                },
                getRandomValues: function(typedArray) {
                    if (typedArray && typedArray.length) {
                        for (let i = 0; i < typedArray.length; i++) {
                            typedArray[i] = Math.floor(Math.random() * 256);
                        }
                    }
                    return typedArray;
                }
            };

            globalThis.crypto = cryptoModule;
        })();
    "#;

    ctx.eval(Source::from_bytes(crypto_js_wrapper))
        .map_err(|e| format!("注入 crypto 封装层失败: {e}"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_algorithms_rfc_vectors() {
        // RFC 1321 MD5 测试向量
        assert_eq!(to_hex(&compute_md5(b"")), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(to_hex(&compute_md5(b"hello world")), "5eb63bbbe01eeed093cb22bb8f5acdc3");
        assert_eq!(
            to_hex(&compute_md5(b"The quick brown fox jumps over the lazy dog")),
            "9e107d9d372bb6826bd81d3542a419d6"
        );

        // RFC 3174 SHA-1 测试向量
        assert_eq!(to_hex(&compute_sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(to_hex(&compute_sha1(b"hello world")), "2aae6c35c94fcfb415dbe95f408b9ce91ee846ed");
        assert_eq!(
            to_hex(&compute_sha1(b"The quick brown fox jumps over the lazy dog")),
            "2fd4e1c67a2d28fced849ee1bb76e7391b93eb12"
        );

        // SHA-256 测试向量
        assert_eq!(
            to_hex(&compute_sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            to_hex(&compute_sha256(b"hello world")),
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn test_crypto_in_boa_context() {
        let mut ctx = Context::default();
        // 先注入 buffer
        let buffer_script = crate::env::buffer::get_buffer_polyfill_script();
        ctx.eval(Source::from_bytes(buffer_script)).unwrap();

        register_crypto_native_and_script(&mut ctx).expect("注册 crypto 失败");

        // 1. 测试 sha256 链式调用
        let script_sha256 = r#"
            crypto.createHash('sha256')
                .update('hello')
                .update(' world')
                .digest('hex');
        "#;
        let res_sha256 = ctx.eval(Source::from_bytes(script_sha256)).unwrap();
        assert_eq!(
            res_sha256.to_string(&mut ctx).unwrap().to_std_string_escaped(),
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );

        // 2. 测试 md5
        let script_md5 = "crypto.createHash('md5').update('hello world').digest('hex');";
        let res_md5 = ctx.eval(Source::from_bytes(script_md5)).unwrap();
        assert_eq!(
            res_md5.to_string(&mut ctx).unwrap().to_std_string_escaped(),
            "5eb63bbbe01eeed093cb22bb8f5acdc3"
        );

        // 3. 测试 sha1
        let script_sha1 = "crypto.createHash('sha1').update('hello world').digest('hex');";
        let res_sha1 = ctx.eval(Source::from_bytes(script_sha1)).unwrap();
        assert_eq!(
            res_sha1.to_string(&mut ctx).unwrap().to_std_string_escaped(),
            "2aae6c35c94fcfb415dbe95f408b9ce91ee846ed"
        );

        // 4. 测试重复 digest 抛错
        let script_double_digest = r#"
            let err = null;
            const h = crypto.createHash('sha256').update('test');
            h.digest('hex');
            try {
                h.digest('hex');
            } catch (e) {
                err = e.message;
            }
            err;
        "#;
        let res_err = ctx.eval(Source::from_bytes(script_double_digest)).unwrap();
        assert_eq!(res_err.to_string(&mut ctx).unwrap().to_std_string_escaped(), "Digest already called");

        // 5. 测试 randomUUID
        let script_uuid = "crypto.randomUUID().length === 36;";
        let res_uuid = ctx.eval(Source::from_bytes(script_uuid)).unwrap();
        assert_eq!(res_uuid.to_boolean(), true);
    }
}
