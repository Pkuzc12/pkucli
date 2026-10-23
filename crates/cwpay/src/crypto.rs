//! 收费平台响应解密 + 登录跳转解析（纯函数，可单测）
//!
//! 来自线上站点移动端 bundle（2026-10，`/mobile/js/app.43b046a6.js` 模块 `49b9` 的 `h()`，
//! axios 响应拦截器在 `content-type: application/json;charset=UTF-8` 时调用）：
//!
//! ```text
//! body = rand(10) + reverse(key)(32) + rand(10) + base64(AES-256-ECB/PKCS7(key, json)) + rand(10)
//! ```
//!
//! 即 key = `body[10..42]` 反转（32 个 ASCII 字符），密文 = `body[52..len-10]`。错误响应
//! （`{"messageCode":"8100",…}`、`-1` token 失效等）是明文 JSON。请求体不加密。
//! 与 pku-harness `src/core/campus/adapters/cwpay-crypto.ts` 同一套逻辑。

use aes::cipher::{generic_array::GenericArray, BlockDecrypt, KeyInit};
use aes::Aes256;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde_json::Value;

/// 解密响应体；不是密文（明文 JSON / HTML / 太短）或解密失败时原样返回。
pub fn decode_body(body: &str) -> String {
    let text = body.trim();
    if text.is_empty()
        || text.starts_with('{')
        || text.starts_with('[')
        || text.starts_with('<')
        || text.len() < 64
        || !text.is_ascii()
    {
        return body.to_string();
    }
    try_decrypt(text).unwrap_or_else(|| body.to_string())
}

fn try_decrypt(text: &str) -> Option<String> {
    let key: Vec<u8> = text.as_bytes()[10..42].iter().rev().copied().collect();
    let cipher_b64 = &text[52..text.len() - 10];
    let mut data = B64.decode(cipher_b64).ok()?;
    if data.is_empty() || data.len() % 16 != 0 {
        return None;
    }
    let cipher = Aes256::new_from_slice(&key).ok()?;
    for block in data.chunks_mut(16) {
        cipher.decrypt_block(GenericArray::from_mut_slice(block));
    }
    // PKCS7
    let pad = *data.last()? as usize;
    if pad == 0
        || pad > 16
        || pad > data.len()
        || !data[data.len() - pad..].iter().all(|&b| b as usize == pad)
    {
        return None;
    }
    data.truncate(data.len() - pad);
    String::from_utf8(data).ok()
}

/// `{messageCode, message, code, data}` 信封的状态（axios 拦截器 `b775`）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvelopeState {
    /// "0" / "6003"
    Ok,
    /// "-3"：token 已续期，data = 新 token，需重放请求
    Renew,
    /// "-1" / "-2" / "-4" / code 50014：已登出
    SignedOut,
    /// 其他业务错误码
    Error(String),
    /// 不是信封
    Unknown,
}

/// messageCode 有时是字符串有时是数字，统一成字符串。
pub fn message_code(env: &Value) -> String {
    match env.get("messageCode") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

pub fn envelope_state(env: &Value) -> EnvelopeState {
    if !env.is_object() {
        return EnvelopeState::Unknown;
    }
    let code = message_code(env);
    match code.as_str() {
        "0" | "6003" => EnvelopeState::Ok,
        "-3" => EnvelopeState::Renew,
        "-1" | "-2" | "-4" => EnvelopeState::SignedOut,
        _ if env.get("code").and_then(Value::as_i64) == Some(50014) => EnvelopeState::SignedOut,
        "" => EnvelopeState::Unknown,
        other => EnvelopeState::Error(other.to_string()),
    }
}

/// 错误信息：data 是非空字符串时优先（SPA 也这样显示），否则 message。
pub fn envelope_message(env: &Value) -> String {
    if let Some(s) = env.get("data").and_then(Value::as_str) {
        if !s.trim().is_empty() {
            return s.trim().to_string();
        }
    }
    env.get("message")
        .and_then(Value::as_str)
        .unwrap_or("请求失败")
        .trim()
        .to_string()
}

/// hash 路由里的查询参数（`https://…/#/middle_page?token=…&jsonData=…`）
pub fn hash_query(url: &str) -> Vec<(String, String)> {
    let hash = url.split_once('#').map(|(_, h)| h).unwrap_or(url);
    let Some((_, q)) = hash.split_once('?') else {
        return Vec::new();
    };
    q.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (percent_decode(k), percent_decode(&v.replace('+', " ")))
        })
        .collect()
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 3 <= bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(b) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `/dealcas/?token=<iaaa>` 跳转链的结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dealcas {
    /// `/#/middle_page?token=<X-Token>&jsonData=…`
    Ok {
        token: String,
        json_data: Option<String>,
    },
    /// `/#/middle_page_error?errorMsg=…`
    Err(String),
    /// 还没到最后一跳
    NotYet,
}

pub fn parse_dealcas_location(location: &str) -> Dealcas {
    let q = hash_query(location);
    let get = |k: &str| q.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    if location.contains("#/middle_page_error") {
        return Dealcas::Err(
            get("errorMsg")
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "收费平台登录失败".into()),
        );
    }
    let is_middle = location
        .find("#/middle_page")
        .map(|i| {
            let rest = &location[i + "#/middle_page".len()..];
            rest.is_empty() || rest.starts_with('?') || rest.starts_with('/')
        })
        .unwrap_or(false);
    if !is_middle {
        return Dealcas::NotYet;
    }
    match get("token").filter(|t| !t.is_empty()) {
        Some(token) => Dealcas::Ok {
            token,
            json_data: get("jsonData").filter(|s| !s.is_empty()),
        },
        None => Dealcas::Err("收费平台没有返回登录凭证".into()),
    }
}

/// X-Token 是 HS256 JWT；读 payload 的 `exp`（秒）。读不到返回 None。
pub fn jwt_exp(token: &str) -> Option<i64> {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    serde_json::from_slice::<Value>(&bytes)
        .ok()?
        .get("exp")?
        .as_i64()
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes::cipher::BlockEncrypt;

    /// 按站点格式加密（测试用）：rand10 + reverse(key) + rand10 + b64(AES-ECB) + rand10
    fn encrypt(json: &str, key: &str) -> String {
        let cipher = Aes256::new_from_slice(key.as_bytes()).unwrap();
        let mut data = json.as_bytes().to_vec();
        let pad = 16 - data.len() % 16;
        data.extend(std::iter::repeat_n(pad as u8, pad));
        for block in data.chunks_mut(16) {
            cipher.encrypt_block(GenericArray::from_mut_slice(block));
        }
        let rev: String = key.chars().rev().collect();
        format!("abcdefghij{rev}klmnopqrst{}uvwxyz0123", B64.encode(data))
    }

    #[test]
    fn roundtrip_decrypts_site_format() {
        let key = "0123456789abcdefghijklmnopqrstuv";
        let json = r#"{"messageCode":"0","message":"成功","data":[{"projectName":"浴卡充值"}]}"#;
        assert_eq!(decode_body(&encrypt(json, key)), json);
    }

    #[test]
    fn plain_and_garbage_pass_through() {
        let plain = r#"{"data":"","messageCode":"-1","message":"user-token权限验证失败、失效"}"#;
        assert_eq!(decode_body(plain), plain);
        assert_eq!(decode_body("<html></html>"), "<html></html>");
        let junk = "x".repeat(80);
        assert_eq!(decode_body(&junk), junk);
    }

    #[test]
    fn envelope_states() {
        let v = |s: &str| serde_json::from_str::<Value>(s).unwrap();
        assert_eq!(
            envelope_state(&v(r#"{"messageCode":"0"}"#)),
            EnvelopeState::Ok
        );
        assert_eq!(
            envelope_state(&v(r#"{"messageCode":"6003"}"#)),
            EnvelopeState::Ok
        );
        assert_eq!(
            envelope_state(&v(r#"{"messageCode":"-3","data":"t2"}"#)),
            EnvelopeState::Renew
        );
        assert_eq!(
            envelope_state(&v(r#"{"messageCode":"-1"}"#)),
            EnvelopeState::SignedOut
        );
        assert_eq!(
            envelope_state(&v(r#"{"code":50014}"#)),
            EnvelopeState::SignedOut
        );
        assert_eq!(
            envelope_state(&v(r#"{"messageCode":"8100"}"#)),
            EnvelopeState::Error("8100".into())
        );
        assert_eq!(envelope_state(&v("[1]")), EnvelopeState::Unknown);
        assert_eq!(
            envelope_message(&v(
                r#"{"messageCode":"1","data":"项目未开放","message":"x"}"#
            )),
            "项目未开放"
        );
        assert_eq!(
            envelope_message(&v(
                r#"{"messageCode":"8100","data":"","message":"服务异常，请稍后再试"}"#
            )),
            "服务异常，请稍后再试"
        );
    }

    #[test]
    fn dealcas_locations() {
        assert_eq!(
            parse_dealcas_location("https://cwpay.pku.edu.cn/#/middle_page?token=a.b.c&jsonData=%7B%22idserial%22%3A%222600000000%22%7D"),
            Dealcas::Ok { token: "a.b.c".into(), json_data: Some(r#"{"idserial":"2600000000"}"#.into()) }
        );
        assert_eq!(
            parse_dealcas_location("https://cwpay.pku.edu.cn/#/middle_page_error?errorMsg=%E7%B3%BB%E7%BB%9F%E9%94%99%E8%AF%AF"),
            Dealcas::Err("系统错误".into())
        );
        assert_eq!(
            parse_dealcas_location("https://cwpay.pku.edu.cn/dealcas/?token=x"),
            Dealcas::NotYet
        );
        assert_eq!(
            parse_dealcas_location("https://cwpay.pku.edu.cn/#/middle_page?x=1"),
            Dealcas::Err("收费平台没有返回登录凭证".into())
        );
    }

    #[test]
    fn jwt_exp_reads_payload() {
        // {"alg":"HS256"} . {"jti":"x","iat":1700000000,"exp":1700021600}
        let token =
            "eyJhbGciOiJIUzI1NiJ9.eyJqdGkiOiJ4IiwiaWF0IjoxNzAwMDAwMDAwLCJleHAiOjE3MDAwMjE2MDB9.sig";
        assert_eq!(jwt_exp(token), Some(1_700_021_600));
        assert_eq!(jwt_exp("nope"), None);
    }
}
