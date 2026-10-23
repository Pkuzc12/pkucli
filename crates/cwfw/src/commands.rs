//! 命令处理器

use crate::{
    api::CwfwApi,
    client, context,
    context::UserContext,
    display,
    login::{self, APP_NAME},
    queries,
    wf::Wf,
};
use anyhow::{anyhow, Result};
use colored::Colorize;
use pkuinfo_common::session::Store;
use serde::Serialize;

/// `cwfw reward` —— 个人酬金查询
///
/// - `year`:      年份，默认当前年
/// - `month_from` / `month_to`: 起止月份，默认 1 月到当前月
pub async fn cmd_reward(
    year: Option<u32>,
    month_from: Option<u32>,
    month_to: Option<u32>,
) -> Result<()> {
    let session = login::load_session()?;
    let uid = session
        .uid
        .clone()
        .ok_or_else(|| anyhow!("会话中缺少 uid，请重新 `cwfw login`"))?;

    let store = Store::new(APP_NAME)?;
    let cookie_store = store.load_cookie_store()?;
    let http = client::build(cookie_store.clone())?;

    // 构建 userContext：先从 loadRolesMenu 拉初始值，再合并 loadInitFunction 的用户变量，
    // 最后塞进 "个人酬金查询" 的静态字段。
    println!("{} 初始化会话上下文...", "[*]".cyan());
    let mut ctx = match context::fetch_user_context(&http).await {
        Ok(c) => c,
        Err(e) => {
            // 会话可能过期，提示用户重登
            return Err(anyhow!("{e}。会话可能已过期，请重新 `cwfw login`"));
        }
    };
    let _ = context::load_init_function(&http, &mut ctx).await; // 失败不致命
    context::seed_reward_query_context(&mut ctx, &uid);

    // 解析默认年份 / 月份
    let now = chrono::Local::now();
    let year = year.unwrap_or_else(|| now.format("%Y").to_string().parse().unwrap_or(2025));
    let month_from = month_from.unwrap_or(1);
    let month_to = month_to.unwrap_or_else(|| now.format("%m").to_string().parse().unwrap_or(12));

    validate_month(month_from, "起始月份")?;
    validate_month(month_to, "结束月份")?;
    if month_from > month_to {
        return Err(anyhow!("起始月份 {month_from} 不能大于结束月份 {month_to}"));
    }

    // 保存更新后的 cookies（loadRolesMenu 可能刷新过）
    let _ = store.save_cookie_store(&cookie_store);

    let api = CwfwApi::new(http, ctx, uid);

    println!(
        "{} 查询 {}年{:02}月-{:02}月 个人酬金...",
        "[*]".cyan(),
        year,
        month_from,
        month_to
    );
    let resp = api.query_reward(year, month_from, month_to).await?;

    display::render_reward_query(&resp, year, month_from, month_to);
    Ok(())
}

fn validate_month(month: u32, label: &str) -> Result<()> {
    if !(1..=12).contains(&month) {
        return Err(anyhow!("{label} {month} 不在 1-12 范围内"));
    }
    Ok(())
}

// ───────────────────────── 新增只读查询 ─────────────────────────

/// 打开 WF_CWBS 会话：(Wf, userContext, 保存 cookie 的闭包所需的 store/jar)
async fn open_session() -> Result<(
    Wf,
    UserContext,
    Store,
    std::sync::Arc<reqwest_cookie_store::CookieStoreMutex>,
)> {
    let session = login::load_session()?;
    let uid = session
        .uid
        .clone()
        .ok_or_else(|| anyhow!("会话中缺少 uid，请重新 `cwfw login`"))?;
    let store = Store::new(APP_NAME)?;
    let jar = store.load_cookie_store()?;
    let http = client::build(jar.clone())?;
    let (wf, ctx) = queries::open_cwbs(http, &uid).await?;
    Ok((wf, ctx, store, jar))
}

fn print_json<T: Serialize>(v: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(v)?);
    Ok(())
}

fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

fn year_start() -> String {
    chrono::Local::now().format("%Y-01-01").to_string()
}

fn check_date(s: &str) -> Result<String> {
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map(|d| d.format("%Y-%m-%d").to_string())
        .map_err(|_| anyhow!("日期格式应为 YYYY-MM-DD：{s}"))
}

fn date_range(
    since: Option<String>,
    until: Option<String>,
    default_since: String,
) -> Result<(String, String)> {
    let since = check_date(&since.unwrap_or(default_since))?;
    let until = check_date(&until.unwrap_or_else(today))?;
    if since > until {
        return Err(anyhow!("起始日期 {since} 晚于结束日期 {until}"));
    }
    Ok((since, until))
}

fn year_month(s: &str) -> Result<(u32, u32)> {
    let (y, m) = s
        .split_once('-')
        .ok_or_else(|| anyhow!("年月格式应为 YYYY-MM：{s}"))?;
    let y: u32 = y
        .parse()
        .map_err(|_| anyhow!("年月格式应为 YYYY-MM：{s}"))?;
    let m: u32 = m
        .parse()
        .map_err(|_| anyhow!("年月格式应为 YYYY-MM：{s}"))?;
    validate_month(m, "月份")?;
    Ok((y, m))
}

/// `cwfw menu`
pub async fn cmd_menu(json: bool) -> Result<()> {
    let (wf, _ctx, store, jar) = open_session().await?;
    let entries = queries::menu(&wf).await?;
    let _ = store.save_cookie_store(&jar);
    if json {
        return print_json(&entries);
    }
    display::render_menu(&entries);
    Ok(())
}

/// `cwfw gz`
pub async fn cmd_salary(
    year: Option<u32>,
    from: Option<u32>,
    to: Option<u32>,
    kind: Option<String>,
    list: bool,
    json: bool,
) -> Result<()> {
    let (wf, ctx, store, jar) = open_session().await?;
    if list {
        let kinds = queries::salary_kinds(&wf, &ctx).await?;
        let _ = store.save_cookie_store(&jar);
        if json {
            return print_json(&kinds);
        }
        for (code, name) in kinds {
            println!("{}  {}", code.bold(), name);
        }
        return Ok(());
    }
    let now = chrono::Local::now();
    let this_year: u32 = now.format("%Y").to_string().parse().unwrap_or(2026);
    let year = year.unwrap_or(this_year);
    let from = from.unwrap_or(1);
    let to = to.unwrap_or_else(|| {
        if year == this_year {
            now.format("%m").to_string().parse().unwrap_or(12)
        } else {
            12
        }
    });
    validate_month(from, "起始月份")?;
    validate_month(to, "结束月份")?;
    if from > to {
        return Err(anyhow!("起始月份 {from} 不能大于结束月份 {to}"));
    }
    if !json {
        println!(
            "{} 查询 {year}年{from:02}月-{to:02}月 个人工资/奖助学金...",
            "[*]".cyan()
        );
    }
    let sections = queries::salary(&wf, &ctx, year, from, to, kind.as_deref()).await?;
    let _ = store.save_cookie_store(&jar);
    if json {
        return print_json(&sections);
    }
    display::render_salary(&sections, year, from, to);
    Ok(())
}

/// `cwfw rk`
pub async fn cmd_deposits(since: Option<String>, until: Option<String>, json: bool) -> Result<()> {
    let (since, until) = date_range(since, until, year_start())?;
    let (wf, ctx, store, jar) = open_session().await?;
    let list = queries::card_deposits(&wf, &ctx, &since, &until).await?;
    let _ = store.save_cookie_store(&jar);
    if json {
        return print_json(&list);
    }
    display::render_deposits(&list, &since, &until);
    Ok(())
}

/// `cwfw jk`
pub async fn cmd_loans(json: bool) -> Result<()> {
    let (wf, ctx, store, jar) = open_session().await?;
    let list = queries::loans(&wf, &ctx).await?;
    let _ = store.save_cookie_store(&jar);
    if json {
        return print_json(&list);
    }
    display::render_loans(&list);
    Ok(())
}

pub struct RemittanceArgs {
    pub keyword: String,
    pub all: bool,
    pub since: Option<String>,
    pub until: Option<String>,
    pub currency: String,
    pub min: f64,
    pub max: Option<f64>,
    pub json: bool,
}

/// `cwfw wh`
pub async fn cmd_remittance(a: RemittanceArgs) -> Result<()> {
    let now = chrono::Local::now();
    let this_year: u32 = now.format("%Y").to_string().parse().unwrap_or(2026);
    let from = match &a.since {
        Some(s) => year_month(s)?,
        None => (this_year - 1, 1),
    };
    let to = match &a.until {
        Some(s) => year_month(s)?,
        None => (
            this_year,
            now.format("%m").to_string().parse().unwrap_or(12),
        ),
    };
    if from > to {
        return Err(anyhow!("起始年月晚于结束年月"));
    }
    let (wf, ctx, store, jar) = open_session().await?;
    let q = queries::RemittanceQuery {
        keyword: &a.keyword,
        all_words: a.all,
        from,
        to,
        currency: &a.currency,
        min: a.min,
        max: a.max.unwrap_or(a.min + queries::REMITTANCE_MAX_SPAN),
    };
    let list = queries::remittances(&wf, &ctx, &q).await?;
    let _ = store.save_cookie_store(&jar);
    if a.json {
        return print_json(&list);
    }
    display::render_remittances(&list, &a.keyword);
    Ok(())
}

/// `cwfw hl`
pub async fn cmd_fx(
    currency: String,
    since: Option<String>,
    until: Option<String>,
    json: bool,
) -> Result<()> {
    let default_since = (chrono::Local::now() - chrono::Duration::days(30))
        .format("%Y-%m-%d")
        .to_string();
    let (since, until) = date_range(since, until, default_since)?;
    let (wf, ctx, store, jar) = open_session().await?;
    let list = queries::fx_rates(&wf, &ctx, &currency, &since, &until).await?;
    let _ = store.save_cookie_store(&jar);
    if json {
        return print_json(&list);
    }
    display::render_fx(&list);
    Ok(())
}

/// `cwfw xc`
pub async fn cmd_payroll(
    kind: String,
    since: Option<String>,
    until: Option<String>,
    project: String,
    json: bool,
) -> Result<()> {
    if !queries::PAYROLL_KINDS.iter().any(|(k, _)| *k == kind) {
        return Err(anyhow!("查询类别应为 0-5（0 学生发放 / 1 薪酬劳务 / 2 奖助学金 / 3 合同制工资 / 4 博士后工资 / 5 研究技术系列人员工资）"));
    }
    let (since, until) = date_range(since, until, year_start())?;
    let (wf, ctx, store, jar) = open_session().await?;
    let list = queries::payroll(&wf, &ctx, &kind, &since, &until, &project).await?;
    let _ = store.save_cookie_store(&jar);
    if json {
        return print_json(&list);
    }
    display::render_payroll(&list);
    Ok(())
}

/// `cwfw bx`
pub async fn cmd_reimburse(json: bool) -> Result<()> {
    let session = login::load_session()?;
    let uid = session
        .uid
        .clone()
        .ok_or_else(|| anyhow!("会话中缺少 uid，请重新 `cwfw login`"))?;
    let store = Store::new(APP_NAME)?;
    let jar = store.load_cookie_store()?;
    let http = client::build(jar.clone())?;
    let stats = queries::reimbursements(&http, &uid).await?;
    let _ = store.save_cookie_store(&jar);
    if json {
        return print_json(&stats);
    }
    display::render_reimburse(&stats);
    Ok(())
}
