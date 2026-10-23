//! HTTP 客户端构建

use reqwest::header::{HeaderMap, HeaderValue};

pub const CWPAY_BASE: &str = "https://cwpay.pku.edu.cn";

const UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/147.0.0.0 Safari/537.36";

fn headers() -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert("user-agent", HeaderValue::from_static(UA));
    h.insert(
        "accept",
        HeaderValue::from_static("application/json, text/plain, */*"),
    );
    h.insert(
        "accept-language",
        HeaderValue::from_static("zh-CN,zh;q=0.9"),
    );
    h.insert(
        "referer",
        HeaderValue::from_static("https://cwpay.pku.edu.cn/mobile/"),
    );
    h
}

/// 业务 API 客户端：鉴权只靠 `X-Token` 头，不需要 cookie；不自动跟随重定向（dealcas 要手动读 Location）
pub fn build() -> anyhow::Result<reqwest::Client> {
    let builder = reqwest::Client::builder()
        .default_headers(headers())
        .redirect(reqwest::redirect::Policy::none());
    Ok(pkuinfo_common::tls::apply_extra_roots(builder)?.build()?)
}

/// IAAA 登录用客户端（内置 cookie jar）
pub fn build_simple() -> anyhow::Result<reqwest::Client> {
    let mut h = HeaderMap::new();
    h.insert("user-agent", HeaderValue::from_static(UA));
    let builder = reqwest::Client::builder()
        .cookie_store(true)
        .default_headers(h);
    Ok(pkuinfo_common::tls::apply_extra_roots(builder)?.build()?)
}
