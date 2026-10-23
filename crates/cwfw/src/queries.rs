//! 财务门户的只读查询（个人工资/奖助学金、银行入卡、借款、外汇到款、汇率、薪酬劳务、
//! 智能报销概览、菜单）。协议细节见 [`crate::wf`]。
//!
//! 窗口号 / 功能号是门户的静态配置（2026-10 抓包，学生账号）：
//!
//! | 菜单 | winno | 表单 func | 表格 func |
//! |---|---|---|---|
//! | 收入查询-个人工资查询（判断页 6941 → 5609） | 5609 | 5574 | 6160（交叉表） |
//! | 收入查询-个人酬金查询（判断页 7081 → 5802） | 5802 | 5743 | 5746 |
//! | 借款查询 | 6320 | 6241 | 6243 |
//! | 薪酬劳务查询 | 7320 | 7181 | 7183 |
//! | 外汇信息查询（外汇到款） | 7600 → 7640 | 10819 | 13522 |
//! | 汇率查询 | 7700 → 7722 | 13562 | 13583 |
//! | 银行入卡-我的入卡 | 7987 | 15030 | 15028 |
//! | 智能报销 个人首页（YBX 子系统） | 6 | — | beforeLoad 存储过程返回 `local(…)` |
//!
//! 存储过程 / 下拉数据源的哈希都按会话动态从窗口定义里解析。

use crate::{
    api::{DoQueryResp, DoQueryRow},
    client::{CWFW_BASE, CWFW_PROJECT},
    context::{self, UserContext},
    login,
    wf::{self, ProcArg, Wf},
};
use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use serde_json::{json, Value};

fn pad2(n: u32) -> String {
    format!("{n:02}")
}

fn cell(row: &DoQueryRow, i: usize) -> String {
    row.cell
        .get(i)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// "1,234.50" / "" → Some(1234.5) / None
pub fn amount(s: &str) -> Option<f64> {
    let t: String = s
        .chars()
        .filter(|c| !matches!(c, ',' | '，' | ' ' | '¥'))
        .collect();
    if t.is_empty() {
        return None;
    }
    t.parse().ok()
}

/// `2026-07-25 00:00:00.0` → `2026-07-25`
fn date_only(s: &str) -> String {
    s.split_whitespace().next().unwrap_or("").to_string()
}

fn is_total(row: &DoQueryRow) -> bool {
    row.cell
        .iter()
        .take(2)
        .any(|c| c.contains("合计") || c.contains("总计"))
}

/// cvstr：`#0-USERINFO.USERID#::<uid>$$#…#::…`
fn cvstr(params: &[String], form: &[(&str, String)], ctx: &UserContext) -> String {
    wf::build_args(params, form, ctx)
        .iter()
        .zip(params)
        .map(|(a, p)| format!("#{p}#::{}", a.value))
        .collect::<Vec<_>>()
        .join("$$")
}

/// 下拉框选项（`bindMap`）：动态数据源走 getBindingDataBackend，静态选项直接解析
async fn field_options(
    wf: &Wf,
    def: &str,
    field: &str,
    form: &[(&str, String)],
    ctx: &UserContext,
) -> Result<Vec<(String, String)>> {
    let Some((head, params)) = wf::map_entry(def, "bindMap", field) else {
        return Ok(Vec::new());
    };
    match head.strip_prefix("@$") {
        Some(sqlid) => {
            let text = wf.binding(sqlid, &cvstr(&params, form, ctx)).await?;
            Ok(wf::parse_options(&text))
        }
        None => Ok(wf::parse_options(&head)),
    }
}

/// 联动字段（`funcMap`）：服务端根据其他字段算出该字段的值；没有联动返回 None
async fn resolve_field(
    wf: &Wf,
    def: &str,
    field: &str,
    form: &[(&str, String)],
    ctx: &UserContext,
) -> Result<Option<String>> {
    let Some((hash, params)) = wf::map_entry(def, "funcMap", field) else {
        return Ok(None);
    };
    if !hash.starts_with("`$") {
        return Ok(None);
    }
    let args = wf::build_args(&params, form, ctx);
    let v = wf.check(&hash, &args, ctx).await?;
    Ok((v != "error").then_some(v))
}

/// 新开 userContext 副本，写入表单字段和菜单节点
fn query_ctx(
    base: &UserContext,
    func: &str,
    menu_node: u32,
    form: &[(&str, String)],
) -> UserContext {
    let mut ctx = base.clone();
    wf::seed_form(&mut ctx, CWFW_PROJECT, func, form);
    ctx.set("CURRMENUNODEID".into(), json!(menu_node));
    ctx
}

/// 跑一个"表单 + 查询按钮 + 表格"的标准窗口
struct FormQuery<'a> {
    winno: &'a str,
    form_func: &'a str,
    first_param: &'a str,
    menu_node: u32,
    with_check: bool,
}

impl FormQuery<'_> {
    async fn run(
        &self,
        wf: &Wf,
        base: &UserContext,
        def: Option<&str>,
        form: &[(&str, String)],
    ) -> Result<UserContext> {
        let owned;
        let def = match def {
            Some(d) => d,
            None => {
                owned = wf.load_window(self.winno).await?;
                owned.as_str()
            }
        };
        let proc = wf::find_proc(def, self.first_param)
            .ok_or_else(|| anyhow!("没能解析窗口 {} 的查询按钮（门户可能已改版）", self.winno))?;
        let ctx = query_ctx(base, self.form_func, self.menu_node, form);
        let args = wf::build_args(&proc.params, form, &ctx);
        let check = if self.with_check {
            wf::find_check(def, self.first_param)
                .map(|c| (c.hash.clone(), wf::build_args(&c.params, form, &ctx)))
        } else {
            None
        };
        let check_ref = check.as_ref().map(|(h, a)| (h.as_str(), a.as_slice()));
        wf.button(&proc.hash, &args, check_ref, &ctx).await?;
        Ok(ctx)
    }
}

// ───────────────────────── 菜单 ─────────────────────────

/// 当前账号在"网上查询"(WF_CWBS) 里能看到的全部菜单项
pub async fn menu(wf: &Wf) -> Result<Vec<wf::MenuEntry>> {
    let text = wf
        .client
        .post(format!("{CWFW_BASE}/{}/loadMenu.action", wf.path))
        .header("x-requested-with", "XMLHttpRequest")
        .header(
            "content-type",
            "application/x-www-form-urlencoded; charset=UTF-8",
        )
        .body(String::new())
        .send()
        .await
        .context("loadMenu 请求失败")?
        .text()
        .await?;
    let entries = wf::parse_menu(&text);
    if entries.is_empty() {
        return Err(anyhow!(
            "没有拿到菜单（会话可能已过期，请重新 `cwfw login`）"
        ));
    }
    Ok(entries)
}

// ───────────────────────── 个人工资 / 奖助学金 ─────────────────────────

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SalaryItem {
    /// 工资项，如 研究生岗位奖学金 / 应发合计
    pub name: String,
    /// (月份, 金额)，只含有金额的月份
    pub months: Vec<(u32, f64)>,
    pub total: Option<f64>,
    pub item_code: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SalarySection {
    /// 查询功能代码，如 6
    pub code: String,
    /// 查询功能名称，如 校长奖学金/博士生岗位奖学金/学业奖学金
    pub title: String,
    pub items: Vec<SalaryItem>,
}

/// 交叉表 6160 → 工资项。`fields` 为 crossfieldrebuild 的 (字段, 标题)，
/// 典型为 `qq.ym, mon1..monN, sum, qq.ord, qq.gztype, qq.remark, qq.itemcode, qq.unino`。
pub fn salary_items(resp: &DoQueryResp, fields: &[(String, String)]) -> Vec<SalaryItem> {
    let idx = |name: &str| fields.iter().position(|(f, _)| f == name);
    let name_i = idx("qq.ym").unwrap_or(0);
    let sum_i = idx("sum");
    let code_i = idx("qq.itemcode");
    resp.rows
        .iter()
        .map(|r| {
            let months = fields
                .iter()
                .enumerate()
                .filter_map(|(i, (f, _))| {
                    let m: u32 = f.strip_prefix("mon")?.parse().ok()?;
                    let v = amount(&cell(r, i))?;
                    Some((m, v))
                })
                .collect();
            SalaryItem {
                name: cell(r, name_i),
                months,
                total: sum_i.and_then(|i| amount(&cell(r, i))),
                item_code: code_i.map(|i| cell(r, i)).unwrap_or_default(),
            }
        })
        .filter(|i| !i.name.is_empty())
        .collect()
}

/// crossfieldrebuild 失败时按月份区间推出的默认列
pub fn default_salary_fields(from: u32, to: u32) -> Vec<(String, String)> {
    let mut f = vec![("qq.ym".to_string(), String::new())];
    for m in from..=to {
        f.push((format!("mon{m}"), format!("{m}月")));
    }
    for k in [
        "sum",
        "qq.ord",
        "qq.gztype",
        "qq.remark",
        "qq.itemcode",
        "qq.unino",
    ] {
        f.push((k.to_string(), String::new()));
    }
    f
}

/// 个人工资查询（学生账号为 奖助学金 / 生活物价补贴 等按月发放的款项）
///
/// `only` 为查询功能代码（如 "6"），None = 账号可见的全部功能。
pub async fn salary(
    wf: &Wf,
    base: &UserContext,
    year: u32,
    from: u32,
    to: u32,
    only: Option<&str>,
) -> Result<Vec<SalarySection>> {
    const WIN: &str = "5609";
    const FORM: &str = "5574";
    const GRID: &str = "6160";
    let def = wf.load_window(WIN).await?;
    let mut options = field_options(wf, &def, "d.funcno", &[], base).await?;
    if let Some(code) = only {
        options.retain(|(c, _)| c == code);
        if options.is_empty() {
            return Err(anyhow!(
                "没有代码为 {code} 的工资查询功能（用 `cwfw gz --list` 查看）"
            ));
        }
    }
    let q = FormQuery {
        winno: WIN,
        form_func: FORM,
        first_param: "5574-d.year",
        menu_node: 219,
        with_check: false,
    };
    let mut out = Vec::new();
    for (code, title) in options {
        let mut form: Vec<(&str, String)> = vec![("d.funcno", code.clone())];
        // 联动字段：收入类型 → 代码/库 → 发布方式
        for f in ["d.incometype", "d.code", "d.dbcode", "d.pubdate"] {
            let v = resolve_field(wf, &def, f, &form, base)
                .await?
                .unwrap_or_default();
            form.push((f, v));
        }
        form.push(("d.year", year.to_string()));
        form.push(("d.month_1", pad2(from)));
        form.push(("d.month_2", pad2(to)));
        // 表格窗口的 beforeLoad 存储过程（前端打开窗口时先跑一次），失败不致命
        if let Some(b) = wf::before_load_proc(&def) {
            let args = wf::build_args(&b.params, &form, base);
            let _ = wf.button(&b.hash, &args, None, base).await;
        }
        let ctx = q.run(wf, base, Some(&def), &form).await?;
        let fields = match wf.cross_fields(GRID, &ctx).await {
            Ok(f) if f.iter().any(|(k, _)| k.starts_with("mon")) => f,
            _ => default_salary_fields(from, to),
        };
        let resp = wf.do_query(GRID, "qq.ord ASC", 10000, None).await?;
        out.push(SalarySection {
            code,
            title,
            items: salary_items(&resp, &fields),
        });
    }
    Ok(out)
}

/// 工资查询功能列表（代码, 名称）
pub async fn salary_kinds(wf: &Wf, base: &UserContext) -> Result<Vec<(String, String)>> {
    let def = wf.load_window("5609").await?;
    field_options(wf, &def, "d.funcno", &[], base).await
}

// ───────────────────────── 银行入卡 ─────────────────────────

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CardDeposit {
    /// 预约单号
    pub req_no: String,
    /// 经办人
    pub handler: String,
    /// 发放部门
    pub unit: String,
    /// 摘要
    pub summary: String,
    /// 发放金额
    pub amount: Option<f64>,
    /// 入卡金额
    pub paid: Option<f64>,
    pub tax: Option<f64>,
    /// 支付状态
    pub status: String,
    /// 错误代码（入卡失败时有值）
    pub error_code: String,
}

/// 表格 15028：req_no, req_name, depart, remark, amt, paid_amt, tax, status, caozuo, errir_id
pub fn card_deposits_from(resp: &DoQueryResp) -> Vec<CardDeposit> {
    resp.rows
        .iter()
        .filter(|r| !is_total(r))
        .map(|r| CardDeposit {
            req_no: cell(r, 0),
            handler: cell(r, 1),
            unit: cell(r, 2),
            summary: cell(r, 3),
            amount: amount(&cell(r, 4)),
            paid: amount(&cell(r, 5)),
            tax: amount(&cell(r, 6)),
            status: cell(r, 7),
            error_code: cell(r, 9),
        })
        .collect()
}

/// 我的入卡：发到本人银行卡的款项（学生发放等），按日期区间（YYYY-MM-DD）
pub async fn card_deposits(
    wf: &Wf,
    base: &UserContext,
    since: &str,
    until: &str,
) -> Result<Vec<CardDeposit>> {
    // 这个窗口的"查询"按钮不绑存储过程：表格 SQL 直接读 userContext 里的表单值
    let form: Vec<(&str, String)> = vec![
        ("d.req_no", String::new()),
        ("d.jbrno", String::new()),
        ("d.status", String::new()),
        ("d.date1", since.to_string()),
        ("d.date2", until.to_string()),
        ("d.min_amt", String::new()),
        ("d.max_amt", String::new()),
    ];
    let mut ctx = query_ctx(base, "15030", 271, &form);
    ctx.set("0-REFFUNC15028".into(), json!(15030));
    ctx.set("0-CURRWIN".into(), json!(7987));
    let ctx_str = ctx.to_json_string();
    let _ = wf
        .post(
            "common_updateUserContext.action",
            &[("userContext", &ctx_str)],
        )
        .await;
    let resp = wf.do_query("15028", "", 500, Some(&ctx)).await?;
    Ok(card_deposits_from(&resp))
}

// ───────────────────────── 借款 ─────────────────────────

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Loan {
    pub first_date: String,
    pub summary: String,
    pub project: String,
    /// 核销号
    pub clear_no: String,
    pub first_amount: Option<f64>,
    /// 已核销额
    pub cleared: Option<f64>,
    pub balance: Option<f64>,
}

/// 表格 6243：first_clr_date, sabstract, uni_prj_order, clr_order, first_jf_amt, nxt_amt, jf_amt, unino, pid
pub fn loans_from(resp: &DoQueryResp) -> Vec<Loan> {
    resp.rows
        .iter()
        .filter(|r| !is_total(r))
        .map(|r| Loan {
            first_date: date_only(&cell(r, 0)),
            summary: cell(r, 1),
            project: cell(r, 2),
            clear_no: cell(r, 3),
            first_amount: amount(&cell(r, 4)),
            cleared: amount(&cell(r, 5)),
            balance: amount(&cell(r, 6)),
        })
        .collect()
}

/// 个人未核销借款
pub async fn loans(wf: &Wf, base: &UserContext) -> Result<Vec<Loan>> {
    let def = wf.load_window("6320").await?;
    let mut options = field_options(wf, &def, "d.func", &[], base).await?;
    if options.is_empty() {
        options.push(("1".into(), String::new()));
    }
    if let Some(b) = wf::before_load_proc(&def) {
        let args = wf::build_args(&b.params, &[], base);
        let _ = wf.button(&b.hash, &args, None, base).await;
    }
    let q = FormQuery {
        winno: "6320",
        form_func: "6241",
        first_param: "6241-d.func",
        menu_node: 239,
        with_check: false,
    };
    let mut out = Vec::new();
    for (code, _) in options {
        let form = vec![("d.func", code)];
        q.run(wf, base, Some(&def), &form).await?;
        let resp = wf
            .do_query("6243", "to_number(t.pid) ASC", 500, None)
            .await?;
        out.extend(loans_from(&resp));
    }
    Ok(out)
}

// ───────────────────────── 汇率 ─────────────────────────

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct FxRate {
    pub currency: String,
    pub date: String,
    /// 中行折算价：每 100 外币折合人民币
    pub rate_per_100: Option<f64>,
}

/// 表格 13583：wh_type, create_date, wh_rate, …
pub fn fx_rates_from(resp: &DoQueryResp) -> Vec<FxRate> {
    resp.rows
        .iter()
        .map(|r| FxRate {
            currency: cell(r, 0),
            date: date_only(&cell(r, 1)),
            rate_per_100: amount(&cell(r, 2)),
        })
        .collect()
}

/// 外币汇率（中行折算价），currency 为中文币种名（美元/欧元/…/全部）
pub async fn fx_rates(
    wf: &Wf,
    base: &UserContext,
    currency: &str,
    since: &str,
    until: &str,
) -> Result<Vec<FxRate>> {
    let form = vec![
        ("t.start", since.to_string()),
        ("t.end", until.to_string()),
        ("t.btype", currency.to_string()),
    ];
    FormQuery {
        winno: "7700",
        form_func: "13562",
        first_param: "13562-t.start",
        menu_node: 253,
        with_check: false,
    }
    .run(wf, base, None, &form)
    .await?;
    let resp = wf.do_query("13583", "", 2000, None).await?;
    Ok(fx_rates_from(&resp))
}

// ───────────────────────── 外汇到款 ─────────────────────────

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Remittance {
    /// 收报日期
    pub date: String,
    /// 来款单位（汇款人）
    pub remitter: String,
    /// 结汇金额
    pub settled_amount: Option<f64>,
    /// 来款金额
    pub amount: Option<f64>,
    pub currency: String,
    /// 来款地
    pub origin: String,
    pub remark: String,
    /// 结汇状态
    pub status: String,
    /// 送银行日期
    pub bank_date: String,
}

/// 表格 13522：notify_date, income_unit, exchange_amt, income_amt, currency, income_addr, remark,
/// status, tel, bank_date, exchange_user, opt_code, opt_name, log_id, userid, log_date。
/// 联系电话、结汇人、经办人等第三方个人信息不输出。
pub fn remittances_from(resp: &DoQueryResp) -> Vec<Remittance> {
    resp.rows
        .iter()
        .map(|r| Remittance {
            date: date_only(&cell(r, 0)),
            remitter: cell(r, 1),
            settled_amount: amount(&cell(r, 2)),
            amount: amount(&cell(r, 3)),
            currency: cell(r, 4),
            origin: cell(r, 5),
            remark: cell(r, 6),
            status: cell(r, 7),
            bank_date: date_only(&cell(r, 9)),
        })
        .collect()
}

pub struct RemittanceQuery<'a> {
    /// 汇入单位关键词（门户要求大写英文）
    pub keyword: &'a str,
    /// true = 包含全部字词；false = 包含任意字词
    pub all_words: bool,
    pub from: (u32, u32),
    pub to: (u32, u32),
    /// AUD/CAD/CHF/CNY/EUR/GBD/HKD/RMB/SEK/SGD/USD/其他/全部
    pub currency: &'a str,
    pub min: f64,
    pub max: f64,
}

/// 门户要求金额范围的差额不超过 10 万
pub const REMITTANCE_MAX_SPAN: f64 = 100_000.0;

/// 外汇信息查询：学校收到的境外汇款（到款），按汇款单位关键词查
pub async fn remittances(
    wf: &Wf,
    base: &UserContext,
    q: &RemittanceQuery<'_>,
) -> Result<Vec<Remittance>> {
    let kw = q.keyword.trim().to_uppercase();
    if kw.is_empty() {
        return Err(anyhow!("请提供汇入单位关键词（英文）"));
    }
    if q.max < q.min {
        return Err(anyhow!("最大金额不能小于最小金额"));
    }
    if q.max - q.min > REMITTANCE_MAX_SPAN {
        return Err(anyhow!("财务门户要求金额范围的差额不超过 10 万"));
    }
    let (any, all) = if q.all_words {
        (String::new(), kw)
    } else {
        (kw, String::new())
    };
    let form = vec![
        ("t.year", q.from.0.to_string()),
        ("t.year2", q.to.0.to_string()),
        ("t.month_1", pad2(q.from.1)),
        ("t.month_2", pad2(q.to.1)),
        ("t.abstractAny", any),
        ("t.abstractAll", all),
        ("t.outAmt1", trim_num(q.min)),
        ("t.outAmt2", trim_num(q.max)),
        ("t.currency", q.currency.to_string()),
        ("t.funcno", String::new()),
    ];
    let ran = FormQuery {
        winno: "7600",
        form_func: "10819",
        first_param: "10819-t.year2",
        menu_node: 252,
        with_check: true,
    }
    .run(wf, base, None, &form)
    .await;
    match ran {
        Ok(_) => {}
        // 校验函数在无结果时直接回 "没有合适的结果"
        Err(e) if e.to_string().contains("没有合适的结果") => return Ok(Vec::new()),
        Err(e) => return Err(e),
    }
    let resp = wf.do_query("13522", "", 1000, None).await?;
    Ok(remittances_from(&resp))
}

fn trim_num(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{v:.0}")
    } else {
        v.to_string()
    }
}

// ───────────────────────── 薪酬劳务（经办/项目视角） ─────────────────────────

/// 查询类别（7181-t.stype）
pub const PAYROLL_KINDS: [(&str, &str); 6] = [
    ("0", "学生发放"),
    ("1", "薪酬劳务"),
    ("2", "奖助学金"),
    ("3", "合同制工资"),
    ("4", "博士后工资"),
    ("5", "研究技术系列人员工资"),
];

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PayrollRecord {
    pub date: String,
    /// 凭证号-分录
    pub voucher: String,
    pub summary: String,
    pub project_code: String,
    pub project_name: String,
    /// 分录支出金额
    pub amount: Option<f64>,
    /// 收款人姓名
    pub payee: String,
    /// 人员类型
    pub person_type: String,
    /// 应发金额
    pub gross: Option<f64>,
    /// 发放方式
    pub method: String,
}

/// 表格 7183：ym, pz_unino, sabstract, prj_code, prj_name, j_amount, sno, cercode_name, personid,
/// card, branchname, sname, m_name, sk_amt, ff_type, …。工号、证件号、银行卡号、开户行不输出。
pub fn payroll_from(resp: &DoQueryResp) -> Vec<PayrollRecord> {
    resp.rows
        .iter()
        .filter(|r| !is_total(r))
        .map(|r| PayrollRecord {
            date: date_only(&cell(r, 0)),
            voucher: cell(r, 1),
            summary: cell(r, 2),
            project_code: cell(r, 3),
            project_name: cell(r, 4),
            amount: amount(&cell(r, 5)),
            payee: cell(r, 11),
            person_type: cell(r, 12),
            gross: amount(&cell(r, 13)),
            method: cell(r, 14),
        })
        .collect()
}

/// 薪酬劳务查询（经办人 / 项目负责人视角：从自己经办或负责的项目发出的款项）
pub async fn payroll(
    wf: &Wf,
    base: &UserContext,
    kind: &str,
    since: &str,
    until: &str,
    project: &str,
) -> Result<Vec<PayrollRecord>> {
    let form = vec![
        ("t.stype", kind.to_string()),
        ("t.startdate", since.to_string()),
        ("t.sno", String::new()),
        ("t.sname", String::new()),
        ("t.prjcode", project.to_string()),
        ("t.enddate", until.to_string()),
    ];
    FormQuery {
        winno: "7320",
        form_func: "7181",
        first_param: "7181-t.stype",
        menu_node: 246,
        with_check: false,
    }
    .run(wf, base, None, &form)
    .await?;
    let resp = wf
        .do_query("7183", "ccd.ym ASC, ccd.pz_unino ASC", 2000, None)
        .await?;
    Ok(payroll_from(&resp))
}

// ───────────────────────── 智能报销概览（YBX） ─────────────────────────

/// 智能报销首页的计数项：(变量前缀, 中文名)
pub const REIMBURSE_KEYS: [(&str, &str); 10] = [
    ("BX_YBXALL", "全部报销单"),
    ("BX_YBXCG", "草稿"),
    ("BX_YBXSPZ", "审批中"),
    ("BX_DTD", "待投递"),
    ("BX_YBXWC", "待入账（财务处理中）"),
    ("BX_YBXTH", "被退回/待补充"),
    ("BX_YBXZFSB", "支付失败"),
    ("V2_dsp", "待我审批"),
    ("tks", "票夹中待报销票据"),
    ("BX_HIS", "历史报销单（含网报）"),
];

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ReimburseStat {
    pub key: String,
    pub label: String,
    pub count: Option<f64>,
    pub amount: Option<f64>,
}

/// `pass;local(…)` → 各状态的笔数/金额。变量名大小写不一（`_count` / `_COUNT`，`_AMOUT` 拼写错误）
pub fn reimburse_stats(local: &[(String, String)]) -> Vec<ReimburseStat> {
    let find = |prefix: &str, suffixes: &[&str]| {
        local.iter().find_map(|(k, v)| {
            let rest = k.strip_prefix(prefix)?.strip_prefix('_')?;
            suffixes
                .iter()
                .any(|s| rest.eq_ignore_ascii_case(s))
                .then(|| amount(v))
                .flatten()
        })
    };
    REIMBURSE_KEYS
        .iter()
        .map(|(k, label)| ReimburseStat {
            key: (*k).to_string(),
            label: (*label).to_string(),
            count: find(k, &["count"]),
            amount: find(k, &["amt", "amount", "amout"]),
        })
        .filter(|s| s.count.is_some() || s.amount.is_some())
        .collect()
}

/// 智能报销（YBX 子系统）个人首页的报销单统计
pub async fn reimbursements(client: &reqwest::Client, uid: &str) -> Result<Vec<ReimburseStat>> {
    const PATH: &str = "YBX";
    const PROJID: &str = "WF_YBX";
    let ctx = match context::fetch_user_context_at(client, PATH).await {
        Ok(c) => c,
        Err(_) => {
            login::enter_subsystem(client, PROJID).await?;
            context::fetch_user_context_at(client, PATH)
                .await
                .context("进入智能报销系统失败")?
        }
    };
    let wf = Wf::new(client.clone(), uid, PATH, PROJID);
    let def = wf.load_window("6").await?;
    let proc = wf::before_load_proc(&def)
        .ok_or_else(|| anyhow!("没能解析智能报销首页（系统可能已改版）"))?;
    let mut args: Vec<ProcArg> = wf::build_args(&proc.params, &[], &ctx);
    for a in &mut args {
        if a.name.eq_ignore_ascii_case("userinfo.userid") && a.value.is_empty() {
            a.value = uid.to_string();
        }
    }
    let text = wf.button(&proc.hash, &args, None, &ctx).await?;
    Ok(reimburse_stats(&wf::parse_local(&text)))
}

/// 会话：WF_CWBS 的 Wf + 初始 userContext
pub async fn open_cwbs(client: reqwest::Client, uid: &str) -> Result<(Wf, UserContext)> {
    let mut ctx = context::fetch_user_context(&client)
        .await
        .map_err(|e| anyhow!("{e}。会话可能已过期，请重新 `cwfw login`"))?;
    let _ = context::load_init_function(&client, &mut ctx).await;
    ctx.set("0-USERINFO.USERID".into(), Value::String(uid.to_string()));
    ctx.set("0-UNINO".into(), Value::String(uid.to_string()));
    Ok((Wf::new(client, uid, CWFW_PROJECT, CWFW_PROJECT), ctx))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resp(raw: &str) -> DoQueryResp {
        DoQueryResp::parse(raw).expect("parse")
    }

    #[test]
    fn salary_cross_table() {
        // 合成数据：奖学金 9/10 月各一笔
        let raw = r#"{total:1,page:1,records:0,rows:[{id:"6160_0",cell:["&#30740;&#31350;&#29983;&#22870;&#23398;&#37329;","","","","","","","","","1200","800","2000","9","&#26222;&#36890;&#23383;&#27573;","A01","A01","2000000000"]}]}"#;
        let items = salary_items(&resp(raw), &default_salary_fields(1, 10));
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "研究生奖学金");
        assert_eq!(items[0].months, vec![(9, 1200.0), (10, 800.0)]);
        assert_eq!(items[0].total, Some(2000.0));
        assert_eq!(items[0].item_code, "A01");
    }

    #[test]
    fn salary_partial_range() {
        let fields = default_salary_fields(3, 5);
        assert_eq!(fields[1].0, "mon3");
        let raw = r#"{total:1,page:1,records:0,rows:[{id:"6160_0",cell:["X","100","","300","400","9","","","B02","2000000000"]}]}"#;
        let items = salary_items(&resp(raw), &fields);
        assert_eq!(items[0].months, vec![(3, 100.0), (5, 300.0)]);
        assert_eq!(items[0].total, Some(400.0));
    }

    #[test]
    fn empty_grids() {
        assert!(card_deposits_from(&resp("[]")).is_empty());
        assert!(loans_from(&resp("{total:0,page:0,records:0,rows:[]}")).is_empty());
    }

    #[test]
    fn card_deposit_rows() {
        let raw = r#"{total:1,page:1,records:1,rows:[{id:"15028_0",cell:["YY0001","&#24352;&#32769;&#24072;","&#29289;&#29702;&#23398;&#38498;","&#21161;&#25945;&#36153;","1,500.00","1500.00","0.00","&#24050;&#25903;&#20184;","","",""]}]}"#;
        let d = card_deposits_from(&resp(raw));
        assert_eq!(d[0].handler, "张老师");
        assert_eq!(d[0].unit, "物理学院");
        assert_eq!(d[0].summary, "助教费");
        assert_eq!(d[0].amount, Some(1500.0));
        assert_eq!(d[0].status, "已支付");
    }

    #[test]
    fn remittance_rows_drop_contacts() {
        let raw = r#"{total:1,page:1,records:1,rows:[{id:"13522_0",cell:["2026-03-02 00:00:00.0","EXAMPLE FOUNDATION","9990","10000","USD","&#32654;&#22269;","&#31185;&#30740;&#32463;&#36153;","&#24050;&#32467;&#27719;","13800000000","2026-03-05 00:00:00.0","&#26576;&#32769;&#24072;","AB","&#26576;&#20154;","1","2000000000","2026-03-06 10:00:00.0"]}]}"#;
        let r = remittances_from(&resp(raw));
        assert_eq!(r[0].remitter, "EXAMPLE FOUNDATION");
        assert_eq!(r[0].date, "2026-03-02");
        assert_eq!(r[0].amount, Some(10000.0));
        assert_eq!(r[0].origin, "美国");
        assert_eq!(r[0].bank_date, "2026-03-05");
        let json = serde_json::to_string(&r).expect("json");
        assert!(!json.contains("13800000000"));
    }

    #[test]
    fn fx_rows() {
        let raw = r#"{total:1,page:1,records:2,rows:[{id:"13583_0",cell:["&#27431;&#20803;","2026-10-01 21:26:14.0","762.15","","","","2000000000"]}]}"#;
        let r = fx_rates_from(&resp(raw));
        assert_eq!(r[0].currency, "欧元");
        assert_eq!(r[0].date, "2026-10-01");
        assert_eq!(r[0].rate_per_100, Some(762.15));
    }

    #[test]
    fn payroll_rows_drop_ids() {
        let raw = r#"{total:1,page:1,records:1,rows:[{id:"7183_0",cell:["2026-09","V-12-3","&#21171;&#21153;&#36153;","P001","&#31034;&#20363;&#39033;&#30446;","800","S001","&#36523;&#20221;&#35777;","110101200001010000","6222000000000000000","&#38134;&#34892;","&#26446;&#22235;","&#23398;&#29983;","800","&#36716;&#21345;","2026","0","1","R1","1"]}]}"#;
        let p = payroll_from(&resp(raw));
        assert_eq!(p[0].payee, "李四");
        assert_eq!(p[0].project_name, "示例项目");
        assert_eq!(p[0].gross, Some(800.0));
        let json = serde_json::to_string(&p).expect("json");
        assert!(!json.contains("110101200001010000"));
        assert!(!json.contains("6222000000000000000"));
    }

    #[test]
    fn reimburse_local() {
        let local = wf::parse_local("pass;local(0-tks_amt:0;0-tks_count:0;0-BX_YBXALL_count:2;0-BX_YBXALL_amt:350.5;0-BX_YBXSPZ_count:1;0-BX_YBXSPZ_amt:200;0-BX_DTD_COUNT:1;0-BX_DTD_AMOUT:150.5;0-PZ_50_PT_amt:99999;)");
        let s = reimburse_stats(&local);
        let all = s.iter().find(|x| x.key == "BX_YBXALL").expect("all");
        assert_eq!((all.count, all.amount), (Some(2.0), Some(350.5)));
        let dtd = s.iter().find(|x| x.key == "BX_DTD").expect("dtd");
        assert_eq!(dtd.amount, Some(150.5));
        assert!(s.iter().all(|x| x.key != "PZ_50_PT"));
    }

    #[test]
    fn amounts() {
        assert_eq!(amount("1,234.50"), Some(1234.5));
        assert_eq!(amount(""), None);
        assert_eq!(amount("abc"), None);
        assert_eq!(trim_num(100000.0), "100000");
    }
}
