//! 终端输出

use crate::parse::{Notice, Order, OrderDetail, Pending, Profile, Project, Spending, Tuition};
use colored::Colorize;

fn yuan(v: Option<f64>) -> String {
    v.map(|x| format!("¥{x:.2}")).unwrap_or_else(|| "-".into())
}

pub fn tuition(project: &str, t: &Tuition) {
    println!("{} {}", "■".cyan(), project.bold());
    if t.items.is_empty() {
        println!("  {} 没有欠费记录（无欠费）", "✓".green());
        return;
    }
    for i in &t.items {
        let unpaid = i.unpaid.unwrap_or(0.0);
        let flag = if unpaid > 0.0 {
            format!("未缴 {}", yuan(i.unpaid)).red().to_string()
        } else {
            "已缴清".green().to_string()
        };
        println!(
            "  {}  {:<8} 应缴 {:>10}  已缴 {:>10}  {flag}  {}",
            i.year,
            i.item,
            yuan(i.due),
            yuan(i.paid),
            i.note.dimmed()
        );
    }
    println!(
        "  合计：应缴 {}，已缴 {}，{}",
        yuan(Some(t.due)),
        yuan(Some(t.paid)),
        if t.unpaid > 0.0 {
            format!("未缴 {}", yuan(Some(t.unpaid)))
                .red()
                .bold()
                .to_string()
        } else {
            "全部缴清".green().to_string()
        }
    );
}

pub fn pending(items: &[Pending]) {
    if items.is_empty() {
        println!("{} 收费平台上没有待缴费账单", "✓".green());
        return;
    }
    let total: f64 = items.iter().map(|p| p.unpaid.unwrap_or(0.0).max(0.0)).sum();
    for p in items {
        println!(
            "  {} {}  批次 {}  应缴 {}  未缴 {}{}",
            "•".yellow(),
            p.project.bold(),
            if p.batch.is_empty() { "-" } else { &p.batch },
            yuan(p.due),
            yuan(p.unpaid).red(),
            if p.partial_allowed {
                "  （可分次缴）".dimmed().to_string()
            } else {
                String::new()
            }
        );
    }
    println!("  未缴合计 {}", yuan(Some(total)).red().bold());
}

pub fn orders(total: u64, list: &[Order]) {
    println!("{} 共 {total} 笔订单", "■".cyan());
    for o in list {
        let status = match o.status_code.as_str() {
            "103" => o.status.green().to_string(),
            "101" | "102" => o.status.yellow().to_string(),
            _ => o.status.dimmed().to_string(),
        };
        println!(
            "  {}  {:<16} {:>10}  {}  {}{}",
            o.created_at,
            o.project,
            yuan(o.amount),
            status,
            o.order_no.dimmed(),
            if o.invoiced {
                "  [已开票]".cyan().to_string()
            } else {
                String::new()
            }
        );
    }
}

pub fn order_detail(d: &OrderDetail) {
    println!(
        "{} {}  {}",
        "■".cyan(),
        d.project.bold(),
        d.order_no.dimmed()
    );
    println!("  状态     {}", d.status);
    println!("  金额     {}（应付 {}）", yuan(d.amount), yuan(d.payable));
    if d.refunded.is_some() {
        println!("  已退款   {}", yuan(d.refunded));
    }
    println!("  下单时间 {}", d.created_at);
    if !d.closed_at.is_empty() {
        println!("  支付/关闭 {}", d.closed_at);
    }
    if let Some(c) = &d.paid_by {
        println!("  支付方式 {c}");
    }
    println!(
        "  票据     {}{}",
        if d.invoice_issued {
            "已开具"
        } else if d.invoice_required {
            "需开票，尚未开具"
        } else {
            "无需开票 / 未开具"
        },
        d.invoice_url
            .as_deref()
            .map(|u| format!("\n  查看链接 {u}"))
            .unwrap_or_default()
    );
}

pub fn spending(range: &str, s: &Spending, truncated: bool) {
    println!("{} 收费平台支出 {range}", "■".cyan());
    println!(
        "  已支付 {} 笔，共 {}{}",
        s.paid_count,
        yuan(Some(s.paid_total)).bold(),
        if s.pending > 0 {
            format!("；另有 {} 笔待支付", s.pending)
        } else {
            String::new()
        }
    );
    if !s.by_project.is_empty() {
        println!("  按项目：");
        for b in &s.by_project {
            println!(
                "    {:<18} {:>3} 笔  {:>10}",
                b.key,
                b.count,
                yuan(Some(b.total))
            );
        }
        println!("  按月份：");
        for b in &s.by_month {
            println!(
                "    {:<18} {:>3} 笔  {:>10}",
                b.key,
                b.count,
                yuan(Some(b.total))
            );
        }
    }
    if truncated {
        println!("  {}", "（只统计了最近 500 笔订单）".dimmed());
    }
}

pub fn projects(total: usize, list: &[&Project]) {
    println!("{} 共 {total} 个项目，匹配 {} 个", "■".cyan(), list.len());
    for p in list {
        let state = if p.open {
            "开放".green().to_string()
        } else {
            "暂停".red().to_string()
        };
        println!(
            "  [{state}] {:<24} {:<10} {}{}",
            p.name,
            p.kind_label.dimmed(),
            p.standard_amount
                .map(|a| format!("标准 ¥{a:.2}"))
                .unwrap_or_default(),
            p.suspended_note
                .as_deref()
                .map(|n| format!("\n         {}", n.yellow()))
                .unwrap_or_default()
        );
    }
}

pub fn notice(n: Option<&Notice>) {
    match n {
        None => println!("{} 收费平台目前没有公告", "○".dimmed()),
        Some(n) => {
            println!(
                "{} {}  {}",
                "■".cyan(),
                n.title.bold(),
                n.updated_at.dimmed()
            );
            println!("{}", n.content);
        }
    }
}

pub fn profile(p: &Profile) {
    println!("  账号   {}", p.account);
    println!("  姓名   {}", p.name);
    println!("  学工号 {}", p.idserial);
    println!("  学院   {}", p.dept);
}
