//! 北大空间登录流程
//!
//! 完整流程：
//! 1. IAAA 登录（appID=bdkj）→ 获得 iaaa_token
//! 2. GET `/login/oauth?_rand=…&token=<iaaa_token>` → 302 → `/home`，种下 `SESSION` cookie
//! 3. 以最终落点校验会话：没有回到 `/oauth`、`/login*`（回调本身除外）或 IAAA 即为成功
//!    （2026 改版后 `/personal/user` 对所有人都是 404，不能再用来校验）

use crate::client::{self, BDKJ_BASE};
use crate::parse;
use anyhow::{anyhow, Context, Result};
use colored::Colorize;
use pkuinfo_common::{
    credential,
    iaaa::{self, IaaaConfig},
    session::{Session, Store},
};

pub const APP_NAME: &str = "bdkj";

const IAAA_APP_ID: &str = "bdkj";
const IAAA_REDIRECT: &str = "https://bdkj.pku.edu.cn/login/oauth";

fn iaaa_config() -> IaaaConfig {
    IaaaConfig {
        app_id: IAAA_APP_ID.to_string(),
        redirect_url: IAAA_REDIRECT.to_string(),
    }
}

pub async fn login_with_password(username: Option<&str>) -> Result<()> {
    let store = Store::new(APP_NAME)?;
    check_existing_session(&store)?;

    let cred = credential::resolve_credential(username)?;

    let simple_client = client::build_simple()?;
    let config = iaaa_config();

    let otp_code = pkuinfo_common::otp::get_current_otp(store.config_dir())?;
    if otp_code.is_some() {
        println!("{} 已自动填入手机令牌", "[otp]".cyan());
    }

    let iaaa_token = iaaa::login_password(
        &simple_client,
        &config,
        &cred.username,
        &cred.password,
        otp_code.as_deref(),
    )
    .await?;

    complete_bdkj_login(&store, &iaaa_token.token, &cred.username).await
}

pub async fn login_with_qrcode(qr_mode: pkuinfo_common::qr::QrDisplayMode) -> Result<()> {
    let store = Store::new(APP_NAME)?;
    check_existing_session(&store)?;

    let simple_client = client::build_simple()?;
    let config = iaaa_config();

    let iaaa_token =
        iaaa::login_qrcode(&simple_client, &config, store.config_dir(), qr_mode).await?;

    complete_bdkj_login(&store, &iaaa_token.token, "").await
}

async fn complete_bdkj_login(store: &Store, iaaa_token: &str, username: &str) -> Result<()> {
    println!("{} 完成北大空间登录...", "[+]".green());

    let cookie_store = store.load_cookie_store()?;
    let client = client::build(cookie_store.clone())?;

    // 与 IAAA 自己的跳转一致：/login/oauth?_rand=…&token=… → 302 → /home
    let rand = format!("0.{}", chrono::Utc::now().timestamp_subsec_nanos());
    let resp = client
        .get(format!("{BDKJ_BASE}/login/oauth"))
        .query(&[("_rand", rand.as_str()), ("token", iaaa_token)])
        .send()
        .await
        .context("北大空间 oauth 回调失败")?;
    let status = resp.status();
    let final_url = resp.url().to_string();
    let body = resp.text().await.context("读取 oauth 回调响应失败")?;
    if status.is_client_error() || status.is_server_error() {
        return Err(anyhow!("oauth 回调 HTTP {status}"));
    }
    if parse::is_logged_out(status.as_u16(), &final_url, &body) {
        return Err(anyhow!("北大空间拒绝了 IAAA token，请重新登录"));
    }

    // 旧版 JWTUser cookie 里带了 account；新版可能没有，此时回退到登录用户名
    let uid_from_cookie = extract_account_from_cookies(&cookie_store);
    let uid = if !username.is_empty() {
        username.to_string()
    } else {
        uid_from_cookie.clone().unwrap_or_default()
    };

    let mut session = Session::new(iaaa_token.to_string());
    session.expires_at = Some(chrono::Utc::now().timestamp() + 24 * 3600);
    if !uid.is_empty() {
        session.uid = Some(uid.clone());
    }
    store.save_session(&session)?;
    store.save_cookie_store(&cookie_store)?;

    println!();
    println!("{} 北大空间登录成功！", "[done]".green().bold());
    if !uid.is_empty() {
        println!("  学号     = {uid}");
    }
    println!("  配置目录 = {}", store.config_dir().display());
    Ok(())
}

fn extract_account_from_cookies(
    store: &std::sync::Arc<reqwest_cookie_store::CookieStoreMutex>,
) -> Option<String> {
    let store = store.lock().ok()?;
    for c in store.iter_any() {
        if c.name() == "JWTUser" {
            let raw = urlencoding_decode(c.value());
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                if let Some(acc) = v.get("account").and_then(|x| x.as_str()) {
                    return Some(acc.to_string());
                }
            }
        }
    }
    None
}

fn urlencoding_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) =
                u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
            {
                out.push(byte as char);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn check_existing_session(store: &Store) -> Result<()> {
    if let Some(old) = store.load_session()? {
        if !old.is_expired() {
            println!("{} 检测到已有登录会话，继续将覆盖。", "[info]".cyan());
        }
    }
    Ok(())
}

pub fn status() -> Result<()> {
    let store = Store::new(APP_NAME)?;
    match store.load_session()? {
        Some(s) => {
            println!("{} 已登录", "●".green());
            println!(
                "  创建时间 = {}",
                s.created_at.format("%Y-%m-%d %H:%M:%S UTC")
            );
            if let Some(uid) = &s.uid {
                println!("  学号     = {uid}");
            }
            println!("  配置目录 = {}", store.config_dir().display());
        }
        None => {
            println!("{} 未登录。运行 `bdkj login` 开始。", "○".red());
        }
    }
    Ok(())
}

pub fn logout() -> Result<()> {
    let store = Store::new(APP_NAME)?;
    store.clear()?;
    println!("{} 已清除本地会话", "✓".green());
    Ok(())
}

pub fn load_session() -> Result<Session> {
    let store = Store::new(APP_NAME)?;
    let session = store
        .load_session()?
        .ok_or_else(|| anyhow!("未登录。请先运行 `bdkj login`"))?;
    if session.is_expired() {
        return Err(anyhow!("会话已过期。请重新 `bdkj login`"));
    }
    Ok(session)
}
