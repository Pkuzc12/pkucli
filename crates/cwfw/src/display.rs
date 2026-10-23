//! 结果渲染

use crate::api::{DoQueryResp, DoQueryRow};
use crate::queries::{
    CardDeposit, FxRate, Loan, PayrollRecord, ReimburseStat, Remittance, SalarySection,
};
use crate::wf::MenuEntry;
use colored::Colorize;

/// 渲染个人酬金查询结果
pub fn render_reward_query(resp: &DoQueryResp, year: u32, month_from: u32, month_to: u32) {
    println!();
    println!(
        "{} {}年{:02}月 - {:02}月 个人酬金查询",
        "═══".cyan(),
        year,
        month_from,
        month_to
    );
    println!();

    if resp.rows.is_empty() {
        println!("{} 未查询到记录", "○".yellow());
        return;
    }

    // 最后一行通常是 "总计"，单独显示
    let (data_rows, total_row) = split_total(&resp.rows);

    for (idx, row) in data_rows.iter().enumerate() {
        println!(
            "{} {}",
            format!("[{}]", idx + 1).dimmed(),
            cell(row, 0).bold().cyan()
        );
        println!(
            "    {}  {}  {}",
            format!("项目代码: {}", cell(row, 1)).dimmed(),
            format!("发放类型: {}", cell(row, 2)).dimmed(),
            format!("发放时间: {}", cell(row, 7)).dimmed()
        );
        println!(
            "    {}  {}  {}",
            colored_amount("发放金额", &cell(row, 3), "white"),
            colored_amount("扣税金额", &cell(row, 4), "yellow"),
            colored_amount("实发金额", &cell(row, 5), "green")
        );
        println!(
            "    {}  {}  {}",
            format!("经办人: {}", cell(row, 8)).dimmed(),
            format!("发放部门: {}", cell(row, 10)).dimmed(),
            format!("录入时间: {}", cell(row, 6)).dimmed()
        );
        println!();
    }

    if let Some(total) = total_row {
        println!("{}", "─".repeat(60).dimmed());
        println!(
            "{}: 发放 {}  扣税 {}  实发 {}",
            "总计".bold(),
            cell(total, 3).bold(),
            cell(total, 4).bold().yellow(),
            cell(total, 5).bold().green()
        );
        println!("共 {} 条记录", format!("{}", data_rows.len()).bold());
    }
}

fn cell(row: &DoQueryRow, index: usize) -> String {
    row.cell.get(index).cloned().unwrap_or_default()
}

fn colored_amount(label: &str, value: &str, color: &str) -> String {
    let text = format!("{label}: ¥{value}");
    match color {
        "green" => text.green().to_string(),
        "yellow" => text.yellow().to_string(),
        _ => text.to_string(),
    }
}

fn split_total(rows: &[DoQueryRow]) -> (&[DoQueryRow], Option<&DoQueryRow>) {
    match rows.last() {
        Some(last) if cell(last, 0) == "总计" || cell(last, 0).contains("总计") => {
            (&rows[..rows.len() - 1], Some(last))
        }
        _ => (rows, None),
    }
}

// ───────────────────────── 新增查询的渲染 ─────────────────────────

fn money(v: Option<f64>) -> String {
    v.map(|x| format!("¥{x:.2}")).unwrap_or_else(|| "-".into())
}

fn header(title: &str) {
    println!();
    println!("{} {}", "═══".cyan(), title.bold());
    println!();
}

pub fn render_menu(entries: &[MenuEntry]) {
    header("网上查询 菜单");
    let mut last = String::new();
    for e in entries {
        if e.group != last {
            println!("{}", e.group.cyan());
            last = e.group.clone();
        }
        println!(
            "  {:<12} {}",
            e.title,
            format!("winno={} node={}", e.winno, e.node_id).dimmed()
        );
    }
}

pub fn render_salary(sections: &[SalarySection], year: u32, from: u32, to: u32) {
    header(&format!("{year}年{from:02}月-{to:02}月 个人工资/奖助学金"));
    let mut any = false;
    for s in sections {
        println!(
            "{} {}",
            format!("[{}]", s.code).dimmed(),
            s.title.cyan().bold()
        );
        if s.items.is_empty() {
            println!("    {}", "无记录".dimmed());
            continue;
        }
        any = true;
        for i in &s.items {
            println!("    {}  合计 {}", i.name.bold(), money(i.total).green());
            let months: Vec<String> = i
                .months
                .iter()
                .map(|(m, v)| format!("{m}月 ¥{v:.2}"))
                .collect();
            if !months.is_empty() {
                println!("      {}", months.join("  ").dimmed());
            }
        }
    }
    if !any {
        println!("{} 未查询到记录", "○".yellow());
    }
}

pub fn render_deposits(list: &[CardDeposit], since: &str, until: &str) {
    header(&format!("{since} ~ {until} 我的入卡"));
    if list.is_empty() {
        println!("{} 未查询到入卡记录", "○".yellow());
        return;
    }
    for (i, d) in list.iter().enumerate() {
        println!(
            "{} {}  {}",
            format!("[{}]", i + 1).dimmed(),
            d.summary.bold().cyan(),
            d.status
        );
        println!(
            "    发放 {}  入卡 {}  扣税 {}",
            money(d.amount),
            money(d.paid).green(),
            money(d.tax).yellow()
        );
        println!(
            "    {}",
            format!(
                "发放部门: {}  经办人: {}  预约单号: {}",
                d.unit, d.handler, d.req_no
            )
            .dimmed()
        );
        if !d.error_code.is_empty() {
            println!("    {} {}", "入卡失败:".red(), d.error_code);
        }
    }
    let total: f64 = list.iter().filter_map(|d| d.paid).sum();
    println!("{}", "─".repeat(60).dimmed());
    println!(
        "共 {} 笔，入卡合计 {}",
        list.len(),
        format!("¥{total:.2}").green().bold()
    );
}

pub fn render_loans(list: &[Loan]) {
    header("个人未核销借款");
    if list.is_empty() {
        println!("{} 没有未核销借款", "✓".green());
        return;
    }
    for l in list {
        println!("{}  {}", l.first_date, l.summary.bold());
        println!(
            "    首次发生 {}  已核销 {}  余额 {}",
            money(l.first_amount),
            money(l.cleared),
            money(l.balance).yellow()
        );
        println!(
            "    {}",
            format!("项目号: {}  核销号: {}", l.project, l.clear_no).dimmed()
        );
    }
}

pub fn render_remittances(list: &[Remittance], keyword: &str) {
    header(&format!("外汇到款：{}", keyword.to_uppercase()));
    if list.is_empty() {
        println!("{} 未查询到到款记录", "○".yellow());
        return;
    }
    for r in list {
        println!("{}  {}", r.date, r.remitter.bold().cyan());
        println!(
            "    来款 {} {}  结汇 {}  {}",
            r.amount.map(|v| format!("{v:.2}")).unwrap_or_default(),
            r.currency,
            r.settled_amount
                .map(|v| format!("{v:.2}"))
                .unwrap_or_default(),
            r.status
        );
        println!(
            "    {}",
            format!("来款地: {}  备注: {}", r.origin, r.remark).dimmed()
        );
    }
    println!("共 {} 笔", list.len());
}

pub fn render_fx(list: &[FxRate]) {
    header("外币汇率（中行折算价，每 100 外币）");
    if list.is_empty() {
        println!("{} 未查询到汇率", "○".yellow());
        return;
    }
    for r in list {
        println!(
            "{}  {:<6} {}",
            r.date,
            r.currency,
            r.rate_per_100
                .map(|v| format!("{v:.2}"))
                .unwrap_or_default()
        );
    }
}

pub fn render_payroll(list: &[PayrollRecord]) {
    header("薪酬劳务查询");
    if list.is_empty() {
        println!("{} 未查询到记录", "○".yellow());
        return;
    }
    for p in list {
        println!(
            "{}  {}  {}",
            p.date,
            p.summary.bold(),
            money(p.gross).green()
        );
        println!(
            "    {}",
            format!(
                "收款人: {}（{}）  项目: {} {}  凭证: {}  {}",
                p.payee, p.person_type, p.project_code, p.project_name, p.voucher, p.method
            )
            .dimmed()
        );
    }
}

pub fn render_reimburse(stats: &[ReimburseStat]) {
    header("智能报销 概览");
    if stats.is_empty() {
        println!("{} 没有拿到报销统计", "○".yellow());
        return;
    }
    for s in stats {
        let count = s.count.map(|c| format!("{c:.0} 笔")).unwrap_or_default();
        println!("  {:<16} {:>6}  {}", s.label, count, money(s.amount));
    }
}
