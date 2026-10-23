//! 北大财务综合信息门户 CLI 客户端
//!
//! 目前实现了（全部只读）：
//! - IAAA 登录（app_id = `IIPF`）
//! - 网上查询(WF_CWBS)：菜单、个人酬金、个人工资（奖助学金）、我的入卡、借款、外汇到款、
//!   汇率、薪酬劳务
//! - 智能报销(YBX)：报销单状态概览

pub mod api;
mod client;
pub mod commands;
pub mod context;
mod display;
pub mod encrypt;
pub mod login;
pub mod queries;
pub mod wf;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "cwfw", about = "北大财务综合信息门户 CLI 客户端", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// 登录财务门户（通过 IAAA 统一身份认证）
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
        /// 手动指定一次性 OTP 码（覆盖本地保存的 TOTP secret，用于 secret 失效时）
        #[arg(long)]
        otp: Option<String>,
    },
    /// 查看当前登录状态
    Status,
    /// 退出登录
    Logout,

    /// 个人酬金查询（默认当前年份、1 月到当前月）
    #[command(alias = "reward")]
    Cj {
        /// 年份，默认当前年
        #[arg(short, long)]
        year: Option<u32>,
        /// 起始月份 (1-12)，默认 1
        #[arg(short, long)]
        from: Option<u32>,
        /// 结束月份 (1-12)，默认当前月
        #[arg(short, long)]
        to: Option<u32>,
    },

    /// 列出当前账号"网上查询"里可见的菜单（窗口号）
    Menu {
        /// 输出 JSON
        #[arg(long)]
        json: bool,
    },

    /// 个人工资查询（学生为奖助学金、生活补贴等按月发放的款项）
    #[command(alias = "salary")]
    Gz {
        /// 年份，默认当前年
        #[arg(short, long)]
        year: Option<u32>,
        /// 起始月份 (1-12)，默认 1
        #[arg(short, long)]
        from: Option<u32>,
        /// 结束月份 (1-12)，默认当前月（往年为 12）
        #[arg(short, long)]
        to: Option<u32>,
        /// 只查某个查询功能（代码，见 --list）
        #[arg(short, long)]
        kind: Option<String>,
        /// 列出可用的查询功能
        #[arg(long)]
        list: bool,
        /// 输出 JSON
        #[arg(long)]
        json: bool,
    },

    /// 我的入卡：发放到本人银行卡的款项（谁给我打了钱）
    #[command(alias = "deposits")]
    Rk {
        /// 起始日期 YYYY-MM-DD，默认今年 1 月 1 日
        #[arg(short, long)]
        since: Option<String>,
        /// 结束日期 YYYY-MM-DD，默认今天
        #[arg(short, long)]
        until: Option<String>,
        /// 输出 JSON
        #[arg(long)]
        json: bool,
    },

    /// 借款查询：个人未核销借款
    #[command(alias = "loans")]
    Jk {
        /// 输出 JSON
        #[arg(long)]
        json: bool,
    },

    /// 外汇到款查询：学校收到的境外汇款，按汇款单位关键词（英文）
    #[command(alias = "remittance")]
    Wh {
        /// 汇入单位关键词（英文，自动转大写）
        keyword: String,
        /// 要求包含全部字词（默认包含任意字词）
        #[arg(long)]
        all: bool,
        /// 起始年月 YYYY-MM，默认去年 1 月
        #[arg(long)]
        since: Option<String>,
        /// 结束年月 YYYY-MM，默认本月
        #[arg(long)]
        until: Option<String>,
        /// 币种：USD/EUR/GBD/HKD/…/全部
        #[arg(short, long, default_value = "全部")]
        currency: String,
        /// 最小金额
        #[arg(long, default_value_t = 0.0)]
        min: f64,
        /// 最大金额，默认 min + 100000（门户要求区间不超过 10 万）
        #[arg(long)]
        max: Option<f64>,
        /// 输出 JSON
        #[arg(long)]
        json: bool,
    },

    /// 外币汇率查询（中行折算价，每 100 外币）
    #[command(alias = "fx")]
    Hl {
        /// 币种中文名：美元/欧元/英镑/日元/港币/…/全部
        #[arg(short, long, default_value = "美元")]
        currency: String,
        /// 起始日期 YYYY-MM-DD，默认 30 天前
        #[arg(short, long)]
        since: Option<String>,
        /// 结束日期 YYYY-MM-DD，默认今天
        #[arg(short, long)]
        until: Option<String>,
        /// 输出 JSON
        #[arg(long)]
        json: bool,
    },

    /// 薪酬劳务查询（经办/项目负责人视角：从自己负责的项目发出的款项）
    #[command(alias = "payroll")]
    Xc {
        /// 查询类别：0 学生发放 / 1 薪酬劳务 / 2 奖助学金 / 3 合同制工资 / 4 博士后工资 / 5 研究技术系列人员工资
        #[arg(short, long, default_value = "1")]
        kind: String,
        /// 起始日期 YYYY-MM-DD，默认今年 1 月 1 日
        #[arg(short, long)]
        since: Option<String>,
        /// 结束日期 YYYY-MM-DD，默认今天
        #[arg(short, long)]
        until: Option<String>,
        /// 项目号
        #[arg(short, long, default_value = "")]
        project: String,
        /// 输出 JSON
        #[arg(long)]
        json: bool,
    },

    /// 智能报销：报销单状态概览（草稿/审批中/待投递/待入账/退回/支付失败…）
    #[command(alias = "reimburse")]
    Bx {
        /// 输出 JSON
        #[arg(long)]
        json: bool,
    },

    /// 手机令牌 (OTP) 管理
    Otp {
        #[command(subcommand)]
        action: OtpAction,
    },
}

#[derive(Subcommand)]
pub enum OtpAction {
    /// 绑定手机令牌（默认交互式；支持 --send / --verify 两阶段绑定）
    Bind {
        /// 学号/职工号
        #[arg(short, long)]
        username: Option<String>,
        /// 只发送短信验证码并保存会话，不等待输入（供 AI Agent 使用）
        #[arg(long, conflicts_with = "verify")]
        send: bool,
        /// 用已保存的会话和指定短信验证码完成绑定
        #[arg(long, value_name = "CODE", conflicts_with = "send")]
        verify: Option<String>,
    },
    /// 手动设置 TOTP secret
    Set {
        /// Base32 编码的 TOTP secret
        secret: String,
        /// 学号/职工号
        #[arg(short, long)]
        username: Option<String>,
    },
    /// 查看当前 OTP 码
    Show,
    /// 清除已保存的 OTP 配置
    Clear,
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
    dispatch(cli.command).await
}

pub async fn run_from<I, T>(args: I) -> Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let cli = Cli::try_parse_from(args)?;
    dispatch(cli.command).await
}

pub async fn dispatch(command: Commands) -> Result<()> {
    match command {
        Commands::Login {
            password,
            username,
            open,
            otp,
        } => {
            if password {
                login::login_with_password(username.as_deref(), otp.as_deref()).await?;
            } else {
                let qr_mode = if open {
                    pkuinfo_common::qr::QrDisplayMode::Open
                } else {
                    pkuinfo_common::qr::QrDisplayMode::Terminal
                };
                login::login_with_qrcode(qr_mode).await?;
            }
        }
        Commands::Status => login::status()?,
        Commands::Logout => login::logout()?,

        Commands::Cj { year, from, to } => commands::cmd_reward(year, from, to).await?,
        Commands::Menu { json } => commands::cmd_menu(json).await?,
        Commands::Gz {
            year,
            from,
            to,
            kind,
            list,
            json,
        } => commands::cmd_salary(year, from, to, kind, list, json).await?,
        Commands::Rk { since, until, json } => commands::cmd_deposits(since, until, json).await?,
        Commands::Jk { json } => commands::cmd_loans(json).await?,
        Commands::Wh {
            keyword,
            all,
            since,
            until,
            currency,
            min,
            max,
            json,
        } => {
            commands::cmd_remittance(commands::RemittanceArgs {
                keyword,
                all,
                since,
                until,
                currency,
                min,
                max,
                json,
            })
            .await?
        }
        Commands::Hl {
            currency,
            since,
            until,
            json,
        } => commands::cmd_fx(currency, since, until, json).await?,
        Commands::Xc {
            kind,
            since,
            until,
            project,
            json,
        } => commands::cmd_payroll(kind, since, until, project, json).await?,
        Commands::Bx { json } => commands::cmd_reimburse(json).await?,

        Commands::Otp { action } => {
            let store = pkuinfo_common::session::Store::new(login::APP_NAME)?;
            handle_otp(action, store.config_dir()).await?;
        }
    }
    Ok(())
}

async fn handle_otp(action: OtpAction, config_dir: &std::path::Path) -> anyhow::Result<()> {
    use colored::Colorize;
    match action {
        OtpAction::Bind {
            username,
            send,
            verify,
        } => {
            if send {
                pkuinfo_common::otp::bind_otp_send_sms(config_dir, username.as_deref()).await?;
            } else if let Some(code) = verify {
                pkuinfo_common::otp::bind_otp_verify(config_dir, &code).await?;
            } else {
                pkuinfo_common::otp::bind_otp_interactive(config_dir, username.as_deref()).await?;
            }
        }
        OtpAction::Set { secret, username } => {
            let uid = username.unwrap_or_default();
            pkuinfo_common::otp::set_otp_secret(config_dir, &secret, &uid)?;
        }
        OtpAction::Show => match pkuinfo_common::otp::get_current_otp(config_dir)? {
            Some(code) => {
                let config = pkuinfo_common::otp::load_otp_config(config_dir)?
                    .ok_or_else(|| anyhow::anyhow!("OTP 配置文件缺失，请先运行 `otp bind`"))?;
                println!(
                    "{} {} ({})",
                    "OTP:".green().bold(),
                    code.bold(),
                    config.user_id
                );
            }
            None => {
                println!(
                    "{} 未配置 OTP。使用 `otp bind` 绑定或 `otp set <SECRET>` 手动设置",
                    "○".red()
                );
            }
        },
        OtpAction::Clear => {
            pkuinfo_common::otp::clear_otp_config(config_dir)?;
            println!("{} OTP 配置已清除", "✓".green());
        }
    }
    Ok(())
}

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
