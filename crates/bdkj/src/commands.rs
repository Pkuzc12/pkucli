//! 命令处理器

use crate::{
    api::{BdkjApi, Participant},
    client, display, groups,
    login::{self, APP_NAME},
    parse, GroupAction,
};
use anyhow::{anyhow, Context, Result};
use colored::Colorize;
use pkuinfo_common::session::Store;

fn http() -> Result<reqwest::Client> {
    let _session = login::load_session()?; // 会话过期提示
    let store = Store::new(APP_NAME)?;
    let cookie_store = store.load_cookie_store()?;
    let client = client::build(cookie_store)?;
    Ok(client)
}

pub async fn cmd_rooms(building: Option<&str>, min_capacity: Option<u32>) -> Result<()> {
    let api = BdkjApi::new(http()?);
    let (buildings, mut rooms) = api.space().await?;
    // 门锁状态只作参考，失败不影响列表
    let locks = api.lock_status().await.unwrap_or_default();

    let mut title = "全部教学楼".to_string();
    if let Some(q) = building {
        let b = parse::match_building(q, &buildings).ok_or_else(|| {
            let names = buildings
                .iter()
                .map(|b| b.name.as_str())
                .collect::<Vec<_>>()
                .join(" / ");
            anyhow!("未知教学楼：{q}。支持：{names}")
        })?;
        rooms.retain(|r| r.building_id == b.id);
        title = b.name.clone();
    }
    if let Some(n) = min_capacity {
        rooms.retain(|r| r.capacity.is_none_or(|c| c >= n));
    }
    display::render_rooms(&title, &rooms, &locks);
    Ok(())
}

pub async fn cmd_schedule(room_id: &str, date: Option<&str>, days: u32) -> Result<()> {
    if !(1..=7).contains(&days) {
        return Err(anyhow!("--days 取值 1–7"));
    }
    let now = chrono::Local::now().naive_local();
    let start = match date {
        Some(d) => chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d")
            .with_context(|| format!("日期格式应为 YYYY-MM-DD：{d}"))?,
        None => now.date(),
    };
    let dates: Vec<String> = start
        .iter_days()
        .take(days as usize)
        .map(|d| d.format("%Y-%m-%d").to_string())
        .collect();

    let api = BdkjApi::new(http()?);
    let start_s = start.format("%Y-%m-%d").to_string();
    let taken = api.schedule(room_id, &start_s).await?;
    let now_s = now.format("%Y-%m-%d %H:%M:%S").to_string();
    let slots = parse::free_slots(&dates, &taken, Some(&now_s));
    display::render_schedule(room_id, &slots);
    Ok(())
}

pub async fn cmd_list() -> Result<()> {
    let api = BdkjApi::new(http()?);
    let apps = api.list_applications().await?;
    display::render_applications(&apps);
    Ok(())
}

pub async fn cmd_search_student(serial: &str, name: &str) -> Result<()> {
    let api = BdkjApi::new(http()?);
    let info = api.search_student(serial, name).await?;
    println!("{} {} ({})", "●".green(), info.name.bold(), info.serial);
    println!("  id       = {}", info.id);
    if let Some(c) = &info.college {
        println!("  学院     = {c}");
    }
    if let Some(p) = &info.mobile_phone {
        println!("  电话     = {p}");
    }
    if let Some(e) = &info.email {
        println!("  邮箱     = {e}");
    }
    Ok(())
}

pub async fn cmd_reserve(
    room_id: &str,
    begin_time: &str,
    end_time: &str,
    reason: &str,
    group: Option<&str>,
    participants: &[String],
) -> Result<()> {
    // 参与人来源：--group 或 --participant（两者至少给一个）
    let mut members = match (group, participants.is_empty()) {
        (Some(_), false) => {
            return Err(anyhow!("--group 与 --participant 不能同时使用"));
        }
        (Some(name), true) => groups::get(name)?,
        (None, false) => groups::parse_members(participants)?,
        (None, true) => {
            return Err(anyhow!(
                "请通过 --group <name> 或 --participant 学号:姓名 指定参与人"
            ))
        }
    };
    let mut seen = std::collections::HashSet::new();
    members.retain(|m| seen.insert(m.serial.clone()));
    if members.len() < 3 {
        return Err(anyhow!(
            "申请教室要求不少于 3 人（包含申请人），当前 {} 人",
            members.len()
        ));
    }

    // 时间：同一天、开始早于结束、最多连续 2 节
    let begin = parse::normalize_time(begin_time)
        .ok_or_else(|| anyhow!("开始时间格式应为 YYYY-MM-DD HH:MM：{begin_time}"))?;
    let end = parse::normalize_time(end_time)
        .ok_or_else(|| anyhow!("结束时间格式应为 YYYY-MM-DD HH:MM：{end_time}"))?;
    if begin[..10] != end[..10] || begin >= end {
        return Err(anyhow!("结束时间要晚于开始时间，且在同一天"));
    }
    let fmt = "%Y-%m-%d %H:%M:%S";
    let minutes = (chrono::NaiveDateTime::parse_from_str(&end, fmt)?
        - chrono::NaiveDateTime::parse_from_str(&begin, fmt)?)
    .num_minutes();
    if minutes > 120 {
        return Err(anyhow!(
            "一次最多预约连续 {} 节（约 2 小时以内）",
            parse::MAX_CONSECUTIVE_PERIODS
        ));
    }

    let api = BdkjApi::new(http()?);

    // 教室存在性与人数限制
    let (_, rooms) = api.space().await?;
    let room = rooms
        .iter()
        .find(|r| r.id == room_id)
        .ok_or_else(|| anyhow!("北大空间没有这个教室 id：{room_id}，先用 `bdkj rooms` 查询"))?;
    let label = format!("{}{}", room.building, room.name);
    if let Some(min) = room.min_people {
        if (members.len() as u32) < min {
            return Err(anyhow!(
                "{label} 至少 {min} 人起约，当前 {} 人",
                members.len()
            ));
        }
    }
    if let Some(cap) = room.capacity {
        if members.len() as u32 > cap {
            return Err(anyhow!(
                "{label} 最多容纳 {cap} 人，当前 {} 人",
                members.len()
            ));
        }
    }
    // 资料不完整时这里会被重定向到 /personal
    api.check_can_apply(room.apply_href.as_deref()).await?;

    // 时段冲突
    let taken = api.schedule(room_id, &begin[..10]).await?;
    if let Some(c) = taken.iter().find(|s| parse::overlaps(&begin, &end, s)) {
        return Err(anyhow!("这个时段已被占用：{} ~ {}", c.begin, c.end));
    }

    let mut ps: Vec<Participant> = Vec::new();
    for m in &members {
        let info = api.search_student(&m.serial, &m.name).await?;
        ps.push(Participant {
            student_id: info.id,
            serial: info.serial,
            name: info.name,
        });
    }

    println!(
        "{} 提交预约 {label} ({room_id})，{begin} ~ {end}，参与人 {} 人",
        "[*]".cyan(),
        ps.len()
    );
    let result = api.submit_apply(room_id, &begin, &end, reason, &ps).await?;
    if result.success {
        println!("{} {}", "[done]".green().bold(), result.message);
    } else {
        return Err(anyhow!("预约失败：{}", result.message));
    }
    Ok(())
}

pub async fn cmd_cancel(apply_id: &str) -> Result<()> {
    let api = BdkjApi::new(http()?);
    let after = api.cancel_apply(apply_id).await?;
    let status = after
        .map(|a| a.status)
        .unwrap_or_else(|| "申请已取消".into());
    println!(
        "{} 已取消预约 {apply_id}（状态：{status}）",
        "[done]".green().bold()
    );
    Ok(())
}

pub async fn cmd_group(action: GroupAction) -> Result<()> {
    match action {
        GroupAction::List => {
            let file = groups::load()?;
            if file.groups.is_empty() {
                println!(
                    "{} 没有任何分组。使用 `bdkj group set <name> -p 学号:姓名 ...`",
                    "○".yellow()
                );
                return Ok(());
            }
            println!("{} 共 {} 个分组", "●".green(), file.groups.len());
            for (name, members) in &file.groups {
                println!(
                    "  {} ({} 人) — {}",
                    name.bold().cyan(),
                    members.len(),
                    members
                        .iter()
                        .map(|m| format!("{}:{}", m.serial, m.name))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        }
        GroupAction::Show { name } => {
            let members = groups::get(&name)?;
            println!("{} 分组 {}", "●".green(), name.bold().cyan());
            for m in &members {
                println!("  {}  {}", m.serial, m.name);
            }
        }
        GroupAction::Set { name, participants } => {
            let members = groups::parse_members(&participants)?;
            groups::upsert(&name, members)?;
            println!("{} 已保存分组 {}", "✓".green(), name.bold());
        }
        GroupAction::Remove { name } => {
            if groups::remove(&name)? {
                println!("{} 已删除分组 {}", "✓".green(), name.bold());
            } else {
                println!("{} 分组 {} 不存在", "○".yellow(), name);
            }
        }
    }
    Ok(())
}
