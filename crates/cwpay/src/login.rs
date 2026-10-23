//! 收费平台登录流程
//!
//! 1. `GET /api/pay/iaaa/getIAAALoginParam`（公开，响应加密）→ `{appId: "CWSF", redirectUrl:
//!    "http://cwpay.pku.edu.cn/dealcas"}`；取不到就用这组常量
//! 2. IAAA 登录（该 appId / redirectUrl）→ iaaa token
//! 3. `GET https://cwpay.pku.edu.cn/dealcas?_rand=…&token=<iaaa>` → 302 若干跳 →
//!    `/#/middle_page?token=<X-Token>&jsonData={idserial,…}`（失败为 `/#/middle_page_error?errorMsg=…`）
//! 4. 之后所有 API：`/api/<path>` + 头 `X-Token`，见 `api.rs`
//!
//! X-Token 是 HS256 JWT（实测有效期 6 小时），保存为 session.token；过期需重新登录。

use crate::client::{self, CWPAY_BASE};
use crate::crypto::{decode_body, jwt_exp, parse_dealcas_location, Dealcas};
use anyhow::{anyhow, bail, Context, Result};
use colored::Colorize;
use pkuinfo_common::{
    credential,
    iaaa::{self, IaaaConfig},
    session::{Session, Store},
};

pub const APP_NAME: &str = "cwpay";

const FALLBACK_APP_ID: &str = "CWSF";
const FALLBACK_REDIRECT: &str = "http://cwpay.pku.edu.cn/dealcas";

/// 登录第 1 步：站点自己要求的 IAAA 应用参数（拿不到用常量）
async fn iaaa_config(http: &reqwest::Client) -> IaaaConfig {
    let fetched = async {
        let body = http
            .get(format!("{CWPAY_BASE}/api/pay/iaaa/getIAAALoginParam"))
            .send()
            .await
            .ok()?
            .text()
            .await
            .ok()?;
        let env: serde_json::Value = serde_json::from_str(&decode_body(&body)).ok()?;
        let data = env.get("data")?;
        Some(IaaaConfig {
            app_id: data.get("appId")?.as_str()?.to_string(),
            redirect_url: data.get("redirectUrl")?.as_str()?.to_string(),
        })
    }
    .await;
    fetched.unwrap_or_else(|| IaaaConfig {
        app_id: FALLBACK_APP_ID.into(),
        redirect_url: FALLBACK_REDIRECT.into(),
    })
}

/// 用户名密码登录
pub async fn login_with_password(username: Option<&str>, otp_override: Option<&str>) -> Result<()> {
    let store = Store::new(APP_NAME)?;
    let cred = credential::resolve_credential(username)?;
    let http = client::build()?;
    let config = iaaa_config(&http).await;

    let otp = match otp_override {
        Some(code) => Some(code.to_string()),
        None => pkuinfo_common::otp::get_current_otp(store.config_dir())?,
    };
    let simple = client::build_simple()?;
    let token = iaaa::login_password(
        &simple,
        &config,
        &cred.username,
        &cred.password,
        otp.as_deref(),
    )
    .await?;
    complete_login(&store, &http, &config, &token.token, &cred.username).await
}

/// 扫码登录
pub async fn login_with_qrcode(mode: pkuinfo_common::qr::QrDisplayMode) -> Result<()> {
    let store = Store::new(APP_NAME)?;
    let http = client::build()?;
    let config = iaaa_config(&http).await;
    let simple = client::build_simple()?;
    let token = iaaa::login_qrcode(&simple, &config, store.config_dir(), mode).await?;
    complete_login(&store, &http, &config, &token.token, "").await
}

/// 登录第 3 步：IAAA token 换 X-Token（手动跟随 dealcas 的重定向）
async fn complete_login(
    store: &Store,
    http: &reqwest::Client,
    config: &IaaaConfig,
    iaaa_token: &str,
    username: &str,
) -> Result<()> {
    println!("{} 完成收费平台登录...", "[+]".green());
    let rand_val: f64 = rand::random();
    let mut url = format!(
        "{}?_rand={rand_val}&token={iaaa_token}",
        config.redirect_url
    )
    .replacen("http://", "https://", 1);
    for _ in 0..6 {
        let resp = http
            .get(&url)
            .header("accept", "text/html,*/*")
            .send()
            .await
            .context("访问收费平台 dealcas 失败")?;
        let status = resp.status();
        let Some(loc) = resp.headers().get("location").and_then(|v| v.to_str().ok()) else {
            bail!("收费平台登录失败：HTTP {status}，没有跳转");
        };
        let abs = if loc.starts_with("http") {
            loc.to_string()
        } else {
            format!(
                "{CWPAY_BASE}{}{loc}",
                if loc.starts_with('/') { "" } else { "/" }
            )
        };
        match parse_dealcas_location(&abs) {
            Dealcas::Ok { token, json_data } => {
                let mut session = Session::new(token.clone());
                session.expires_at =
                    jwt_exp(&token).or(Some(chrono::Utc::now().timestamp() + 6 * 3600));
                let uid = json_data
                    .as_deref()
                    .and_then(|j| serde_json::from_str::<serde_json::Value>(j).ok())
                    .and_then(|v| v.get("idserial").and_then(|s| s.as_str()).map(String::from))
                    .unwrap_or_else(|| username.to_string());
                if !uid.is_empty() {
                    session.uid = Some(uid);
                }
                store.save_session(&session)?;
                println!();
                println!("{} 收费平台登录成功！", "[done]".green().bold());
                println!("  配置目录 = {}", store.config_dir().display());
                return Ok(());
            }
            Dealcas::Err(msg) => bail!("收费平台登录失败：{msg}"),
            Dealcas::NotYet => url = abs.replacen("http://", "https://", 1),
        }
    }
    Err(anyhow!("收费平台登录跳转次数过多"))
}

/// 查看当前登录状态
pub fn status() -> Result<()> {
    let store = Store::new(APP_NAME)?;
    match store.load_session()? {
        Some(s) if !s.is_expired() => {
            println!("{} 已登录", "●".green());
            println!(
                "  创建时间 = {}",
                s.created_at.format("%Y-%m-%d %H:%M:%S UTC")
            );
            if let Some(exp) = s
                .expires_at
                .and_then(|e| chrono::DateTime::from_timestamp(e, 0))
            {
                println!("  过期时间 = {}", exp.format("%Y-%m-%d %H:%M:%S UTC"));
            }
            println!("  配置目录 = {}", store.config_dir().display());
        }
        Some(_) => println!(
            "{} 会话已过期。运行 `cwpay login -p` 重新登录。",
            "○".yellow()
        ),
        None => println!("{} 未登录。运行 `cwpay login -p` 开始。", "○".red()),
    }
    Ok(())
}

/// 退出登录（只清本地会话，不调用站点的 pay/logout）
pub fn logout() -> Result<()> {
    Store::new(APP_NAME)?.clear()?;
    println!("{} 已清除本地会话", "✓".green());
    Ok(())
}

/// 已保存且未过期的会话
pub fn load_session() -> Result<(Store, Session)> {
    let store = Store::new(APP_NAME)?;
    let session = store
        .load_session()?
        .ok_or_else(|| anyhow!("未登录。请先运行 `cwpay login -p`"))?;
    if session.is_expired() {
        bail!("收费平台会话已过期（X-Token 有效期约 6 小时）。请重新运行 `cwpay login -p`");
    }
    Ok((store, session))
}
