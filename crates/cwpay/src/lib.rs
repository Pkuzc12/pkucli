//! 北京大学收费平台（cwpay.pku.edu.cn，"收费大厅"）只读 CLI 客户端
//!
//! - IAAA 登录（app_id 由站点 `getIAAALoginParam` 给出，现为 `CWSF`）→ dealcas → X-Token
//! - 学费住宿费、待缴费、订单记录/详情（含电子票据链接）、支出汇总、缴费项目目录、公告、个人信息
//!
//! 全部只读：不下单、不支付、不关单、不开票、不改资料。

pub mod api;
mod client;
pub mod crypto;
mod display;
pub mod login;
pub mod parse;

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use serde::Serialize;

#[derive(Parser)]
#[command(
    name = "cwpay",
    about = "北京大学收费平台（学费住宿费 / 订单 / 待缴费）只读 CLI",
    version
)]
pub struct Cli {
    /// 以 JSON 输出（便于 AI Agent 解析）
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// 登录收费平台（通过 IAAA 统一身份认证）
    Login {
        /// 使用用户名密码登录（默认为扫码登录）
        #[arg(short, long)]
        password: bool,
        /// 学号/职工号（仅密码登录时需要）
        #[arg(short, long)]
        username: Option<String>,
        /// 用系统图片查看器打开二维码（默认终端渲染）
        #[arg(long)]
        open: bool,
        /// 手动指定一次性 OTP 码
        #[arg(long)]
        otp: Option<String>,
    },
    /// 查看当前登录状态
    Status,
    /// 清除本地会话
    Logout,

    /// 学费住宿费：按学年列出应缴/已缴/未缴
    #[command(alias = "fee")]
    Tuition,
    /// 待缴费列表（平台导入的待缴账单，所有项目）
    #[command(alias = "due")]
    Pending,
    /// 缴费订单记录（新的在前）
    #[command(alias = "list")]
    Orders {
        /// 页码，从 1 开始
        #[arg(long, default_value_t = 1)]
        page: u32,
        /// 每页条数
        #[arg(short = 'n', long, default_value_t = 20)]
        size: u32,
        /// 起始日期 YYYY-MM-DD（下单日期）
        #[arg(long)]
        from: Option<String>,
        /// 结束日期 YYYY-MM-DD
        #[arg(long)]
        to: Option<String>,
        /// 项目名关键词，如 学费 / 浴卡
        #[arg(short, long)]
        project: Option<String>,
    },
    /// 订单详情（支付方式、支付时间、电子票据链接）
    Order {
        /// 订单号
        order_no: String,
    },
    /// 支出汇总：按项目和月份统计已支付金额
    #[command(alias = "stats")]
    Spending {
        /// 起始日期 YYYY-MM-DD
        #[arg(long)]
        from: Option<String>,
        /// 结束日期 YYYY-MM-DD
        #[arg(long)]
        to: Option<String>,
        /// 项目名关键词
        #[arg(short, long)]
        project: Option<String>,
    },
    /// 缴费项目目录（含是否开放、暂停说明、标准金额）
    #[command(alias = "catalog")]
    Projects {
        /// 名称关键词（中/英）
        #[arg(short, long)]
        search: Option<String>,
        /// 类型（proModelUrl），如 tuitionAndDorm / bathRecharge / net / party / common / scene
        #[arg(short, long)]
        kind: Option<String>,
        /// 只看当前开放的项目
        #[arg(long)]
        open_only: bool,
    },
    /// 平台首页公告
    Notice,
    /// 个人信息（个人中心）
    Me,
}

fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();
}

pub async fn run() -> Result<()> {
    init_tracing();
    let cli = Cli::parse();
    dispatch(cli).await
}

pub async fn run_from<I, T>(args: I) -> Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let cli = Cli::try_parse_from(args)?;
    dispatch(cli).await
}

fn print_json<T: Serialize>(v: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(v)?);
    Ok(())
}

fn check_date(d: &Option<String>) -> Result<()> {
    if let Some(d) = d {
        if chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").is_err() {
            bail!("日期格式应为 YYYY-MM-DD：{d}");
        }
    }
    Ok(())
}

pub async fn dispatch(cli: Cli) -> Result<()> {
    let json = cli.json;
    match cli.command {
        Commands::Login {
            password,
            username,
            open,
            otp,
        } => {
            if password {
                login::login_with_password(username.as_deref(), otp.as_deref()).await?;
            } else {
                let mode = if open {
                    pkuinfo_common::qr::QrDisplayMode::Open
                } else {
                    pkuinfo_common::qr::QrDisplayMode::Terminal
                };
                login::login_with_qrcode(mode).await?;
            }
        }
        Commands::Status => login::status()?,
        Commands::Logout => login::logout()?,

        Commands::Tuition => {
            let (project, t) = api::Api::new()?.tuition().await?;
            if json {
                print_json(
                    &serde_json::json!({ "project": project, "settled": t.unpaid <= 0.0, "tuition": t }),
                )?;
            } else {
                display::tuition(&project, &t);
            }
        }
        Commands::Pending => {
            let items = api::Api::new()?.pending().await?;
            if json {
                print_json(&items)?
            } else {
                display::pending(&items)
            }
        }
        Commands::Orders {
            page,
            size,
            from,
            to,
            project,
        } => {
            check_date(&from)?;
            check_date(&to)?;
            let mut api = api::Api::new()?;
            let names = api.status_names().await;
            let (total, list) = api
                .orders_page(
                    page.max(1),
                    size.clamp(1, 100),
                    from.as_deref(),
                    to.as_deref(),
                    project.as_deref(),
                    &names,
                )
                .await?;
            if json {
                print_json(&serde_json::json!({ "total": total, "orders": list }))?;
            } else {
                display::orders(total, &list);
            }
        }
        Commands::Order { order_no } => {
            let d = api::Api::new()?.order_detail(order_no.trim()).await?;
            if json {
                print_json(&d)?
            } else {
                display::order_detail(&d)
            }
        }
        Commands::Spending { from, to, project } => {
            check_date(&from)?;
            check_date(&to)?;
            let mut api = api::Api::new()?;
            let names = api.status_names().await;
            let (list, truncated) = api
                .all_orders(
                    from.as_deref(),
                    to.as_deref(),
                    project.as_deref(),
                    10,
                    None,
                    &names,
                )
                .await?;
            let s = parse::spending(&list);
            let range = format!(
                "{}..{}",
                from.as_deref().unwrap_or("…"),
                to.as_deref().unwrap_or("…")
            );
            if json {
                print_json(
                    &serde_json::json!({ "range": range, "truncated": truncated, "summary": s }),
                )?;
            } else {
                display::spending(&range, &s, truncated);
            }
        }
        Commands::Projects {
            search,
            kind,
            open_only,
        } => {
            let all = api::Api::new()?.catalog().await?;
            let hits = parse::filter_projects(&all, search.as_deref(), kind.as_deref(), open_only);
            if json {
                print_json(&hits)?
            } else {
                display::projects(all.len(), &hits)
            }
        }
        Commands::Notice => {
            let n = api::Api::new()?.notice().await?;
            if json {
                print_json(&n)?
            } else {
                display::notice(n.as_ref())
            }
        }
        Commands::Me => {
            let p = api::Api::new()?.profile().await?;
            if json {
                print_json(&p)?
            } else {
                display::profile(&p)
            }
        }
    }
    Ok(())
}

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
