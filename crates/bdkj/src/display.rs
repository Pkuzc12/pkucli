//! 终端渲染

use crate::parse::{Application, DaySlots, Room};
use colored::Colorize;
use std::collections::BTreeMap;

fn opt(n: Option<u32>) -> String {
    n.map(|v| v.to_string()).unwrap_or_else(|| "-".into())
}

fn or_dash(s: &str) -> &str {
    if s.is_empty() {
        "-"
    } else {
        s
    }
}

pub fn render_rooms(title: &str, rooms: &[Room], locks: &BTreeMap<String, i64>) {
    println!(
        "{} {} 共 {} 间研讨教室",
        "●".green(),
        title.bold(),
        rooms.len()
    );
    println!(
        "  {:<20} {:<6} {:<8} {:<6} {:<4} {:<6} {:<6} {:<6} 锁",
        "ID", "楼", "教室", "编号", "层", "容量", "起约", "门"
    );
    for r in rooms {
        let lock = locks
            .get(&r.id)
            .map(|v| v.to_string())
            .unwrap_or_else(|| "-".into());
        println!(
            "  {:<20} {:<6} {:<8} {:<6} {:<4} {:<6} {:<6} {:<6} {}",
            r.id,
            or_dash(&r.building),
            r.name.bold(),
            or_dash(&r.code),
            opt(r.floor),
            opt(r.capacity),
            opt(r.min_people),
            or_dash(&r.door),
            lock
        );
    }
}

pub fn render_schedule(room: &str, days: &[DaySlots]) {
    println!(
        "{} 教室 {} 空闲节次（每节 50 分钟，一次最多连续 2 节）",
        "●".green(),
        room.bold()
    );
    for d in days {
        println!("  {}", d.date.bold().cyan());
        if d.free.is_empty() {
            println!("    空闲：{}", "无".red());
        } else {
            let free = d
                .free
                .iter()
                .map(|p| format!("第{}节 {}-{}", p.period, p.begin, p.end))
                .collect::<Vec<_>>()
                .join("  ");
            println!("    空闲：{}", free.green());
        }
        for t in &d.taken {
            println!("    {} {} ~ {}", "已占用".yellow(), t.begin, t.end);
        }
    }
}

pub fn render_applications(apps: &[Application]) {
    println!("{} 共 {} 条预约记录", "●".green(), apps.len());
    for a in apps {
        let status_colored = match a.status.as_str() {
            "申请成功" | "待签到" => a.status.green(),
            "已结束" => a.status.dimmed(),
            "申请已取消" => a.status.red(),
            _ => a.status.yellow(),
        };
        println!(
            "  [{}] {} {}  —  {}",
            status_colored,
            a.room_name.bold().cyan(),
            a.begin_end,
            a.reason
        );
        if !a.id.is_empty() {
            print!("    id={} 申请人={}", a.id, a.applicant);
            if a.can_cancel {
                print!("  {}", "[可取消]".yellow());
            }
            println!();
        }
    }
}
