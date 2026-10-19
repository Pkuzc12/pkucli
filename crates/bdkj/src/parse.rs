//! 北大空间页面 / 接口解析（纯函数：HTML / JSON 进，数据出）
//!
//! 站点 2026-10 改版：教室列表来自 `/space` 页面，门锁状态来自 `/space/lockStatus`，
//! 已占用时段来自 `/space/schedule`。旧的 `/room/classRoom` 对所有楼都返回 `[]`，
//! `/personal/user` 对所有人都是 404，均已弃用。

use anyhow::{anyhow, Context, Result};
use chrono::NaiveDateTime;
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 首次预约前资料不完整时，站点会把请求重定向到 `/personal`
pub const PROFILE_MSG: &str =
    "首次预约前需要在北大空间「个人信息」页完善资料（https://bdkj.pku.edu.cn/personal）";

/// 每天可预约的 13 节（每节 50 分钟，08:00–22:30）。1–12 节与北大课表一致，13 节为 21:40–22:30。
pub const PERIODS: [(&str, &str); 13] = [
    ("08:00", "08:50"),
    ("09:00", "09:50"),
    ("10:10", "11:00"),
    ("11:10", "12:00"),
    ("13:00", "13:50"),
    ("14:00", "14:50"),
    ("15:10", "16:00"),
    ("16:10", "17:00"),
    ("17:10", "18:00"),
    ("18:40", "19:30"),
    ("19:40", "20:30"),
    ("20:40", "21:30"),
    ("21:40", "22:30"),
];

/// 一次预约最多连续的节数
pub const MAX_CONSECUTIVE_PERIODS: usize = 2;

/// 教学楼别名（拼音 / 全称）
const BUILDING_ALIASES: [(&str, &[&str]); 3] = [
    ("二教", &["erjiao", "二教学楼", "第二教学楼"]),
    ("四教", &["sijiao", "四教学楼", "第四教学楼"]),
    ("地学", &["dixue", "地学楼"]),
];

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Building {
    pub id: String,
    pub name: String,
    /// 两位楼号，如 `02`
    pub code: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Room {
    pub id: String,
    pub name: String,
    /// 5 位教室编号，如 `02514`
    pub code: String,
    pub building_id: String,
    pub building: String,
    pub floor: Option<u32>,
    pub capacity: Option<u32>,
    pub min_people: Option<u32>,
    /// 卡片上的门状态文字（关门 / 开门 / 常开 …），没有则为空
    pub door: String,
    /// 卡片上的屏幕状态文字
    pub screen: String,
    /// 「去申请」按钮的 href（如果有）
    pub apply_href: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BookedSlot {
    pub begin: String,
    pub end: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FreePeriod {
    /// 第几节（从 1 开始）
    pub period: usize,
    /// `HH:MM`
    pub begin: String,
    /// `HH:MM`
    pub end: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DaySlots {
    /// `YYYY-MM-DD`
    pub date: String,
    pub free: Vec<FreePeriod>,
    pub taken: Vec<BookedSlot>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchStudentResp {
    pub success: bool,
    #[serde(default)]
    pub message: String,
    pub data: Option<StudentInfo>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StudentInfo {
    pub id: String,
    pub serial: String,
    pub name: String,
    #[serde(default)]
    pub college: Option<String>,
    #[serde(rename = "mobilePhone", default)]
    pub mobile_phone: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
}

/// 一条已有的预约申请（从 /classRoom 页面抽取）
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Application {
    pub id: String,
    pub room_name: String,
    pub status: String,
    pub applicant: String,
    pub apply_time: String,
    pub begin_end: String,
    pub reason: String,
    pub participants: Vec<String>,
    pub can_cancel: bool,
}

fn sel(s: &str) -> Selector {
    Selector::parse(s).expect("static selector")
}

fn re(s: &str) -> Regex {
    Regex::new(s).expect("static regex")
}

fn clean(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn el_text(el: ElementRef<'_>) -> String {
    clean(&el.text().collect::<Vec<_>>().join(" "))
}

fn first_int(s: &str) -> Option<u32> {
    re(r"\d+").find(s).and_then(|m| m.as_str().parse().ok())
}

/// 解析 `GET /space`：楼栋面板 `[data-building-id]` 与教室卡片 `div.sp-card[data-room-id]`
pub fn parse_space(html: &str) -> (Vec<Building>, Vec<Room>) {
    let doc = Html::parse_document(html);
    let title_sel = sel(".cr-panel-title");
    let bcode_sel = sel(".sp-building-code");
    let name_sel = sel(".cr-room-name");
    let code_sel = sel(".sp-code");
    let feat_sel = sel(".sp-features li");
    let door_sel = sel(r#"[data-dev-kind="door"] .sp-dev-state"#);
    let screen_sel = sel(r#"[data-dev-kind="screen"] .sp-dev-state"#);
    let apply_sel = sel("a.sp-btn-apply");

    let mut buildings: Vec<Building> = Vec::new();
    for el in doc.select(&sel("[data-building-id]")) {
        let id = el.value().attr("data-building-id").unwrap_or_default();
        let name = el
            .select(&title_sel)
            .next()
            .map(el_text)
            .unwrap_or_default();
        if id.is_empty() || name.is_empty() || buildings.iter().any(|b| b.id == id) {
            continue;
        }
        let code = el
            .select(&bcode_sel)
            .next()
            .map(el_text)
            .and_then(|t| re(r"\d+").find(&t).map(|m| m.as_str().to_string()))
            .unwrap_or_default();
        buildings.push(Building {
            id: id.to_string(),
            name,
            code,
        });
    }

    let mut rooms: Vec<Room> = Vec::new();
    for card in doc.select(&sel("div.sp-card[data-room-id]")) {
        let id = card.value().attr("data-room-id").unwrap_or_default();
        if id.is_empty() || rooms.iter().any(|r| r.id == id) {
            continue;
        }
        let name = card
            .select(&name_sel)
            .next()
            .map(el_text)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| clean(card.value().attr("data-room-name").unwrap_or_default()));
        let code = card
            .select(&code_sel)
            .next()
            .map(el_text)
            .unwrap_or_default();

        // 卡片位于所属楼栋面板内；否则按编号前缀匹配（"02514" → 02）
        let parent_bid = card
            .ancestors()
            .filter_map(ElementRef::wrap)
            .find_map(|a| a.value().attr("data-building-id"));
        let building = parent_bid
            .and_then(|bid| buildings.iter().find(|b| b.id == bid))
            .or_else(|| {
                buildings
                    .iter()
                    .find(|b| !b.code.is_empty() && code.starts_with(&b.code))
            });

        let (mut floor, mut capacity, mut min_people) = (None, None, None);
        for li in card.select(&feat_sel) {
            let t = el_text(li);
            if t.contains("可容纳") {
                capacity = first_int(&t);
            } else if t.contains("起约") || t.contains("至少") {
                min_people = first_int(&t);
            } else if t.contains('层') || t.contains('楼') {
                floor = first_int(&t);
            }
        }
        let apply_href = card
            .select(&apply_sel)
            .next()
            .and_then(|a| a.value().attr("href"))
            .map(str::trim)
            .filter(|h| {
                !h.is_empty() && !h.starts_with('#') && !h.to_lowercase().starts_with("javascript:")
            })
            .map(str::to_string);

        rooms.push(Room {
            id: id.to_string(),
            name,
            code,
            building_id: building.map(|b| b.id.clone()).unwrap_or_default(),
            building: building.map(|b| b.name.clone()).unwrap_or_default(),
            floor,
            capacity,
            min_people,
            door: card
                .select(&door_sel)
                .next()
                .map(el_text)
                .unwrap_or_default(),
            screen: card
                .select(&screen_sel)
                .next()
                .map(el_text)
                .unwrap_or_default(),
            apply_href,
        });
    }
    (buildings, rooms)
}

/// 按楼名（二教 / 二教楼）、楼号（02 / 2）或拼音匹配教学楼
pub fn match_building<'a>(query: &str, buildings: &'a [Building]) -> Option<&'a Building> {
    let raw = query.trim();
    let q = raw.to_lowercase();
    let q = q.trim_end_matches('楼');
    if q.is_empty() {
        return None;
    }
    let q_num: Option<u32> = q.parse().ok();
    buildings.iter().find(|b| {
        let name = b.name.to_lowercase();
        let aliases: &[&str] = BUILDING_ALIASES
            .iter()
            .find(|(k, _)| name.contains(k))
            .map(|(_, a)| *a)
            .unwrap_or(&[]);
        name == q
            || name.trim_end_matches('楼') == q
            || (q_num.is_some() && b.code.parse::<u32>().ok() == q_num)
            || aliases.contains(&q)
            || aliases.contains(&raw)
    })
}

/// 解析 `GET /space/lockStatus`：`{ "<roomId>": 0|1|2 }`；格式不对时返回空表
pub fn parse_lock_status(body: &str) -> BTreeMap<String, i64> {
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(body) else {
        return BTreeMap::new();
    };
    map.into_iter()
        .filter_map(|(k, v)| v.as_i64().map(|n| (k, n)))
        .collect()
}

/// 解析 `GET /space/schedule`：`[{"beginTime": "...", "endTime": "..."}]`
pub fn parse_schedule(body: &str) -> Result<Vec<BookedSlot>> {
    let v: serde_json::Value = serde_json::from_str(body)
        .context("已预约时段返回的不是 JSON（北大空间可能暂时不可用或会话失效）")?;
    let list = v
        .as_array()
        .ok_or_else(|| anyhow!("已预约时段格式不对：{body}"))?;
    let as_str = |x: &serde_json::Value| match x {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Null => None,
        other => Some(other.to_string()),
    };
    Ok(list
        .iter()
        .filter_map(|h| {
            Some(BookedSlot {
                begin: as_str(h.get("beginTime")?)?,
                end: as_str(h.get("endTime")?)?,
            })
        })
        .collect())
}

/// `YYYY-MM-DD HH:MM[:SS]`（也接受 `T` 分隔）→ `YYYY-MM-DD HH:MM:SS`；非法输入返回 None
pub fn normalize_time(input: &str) -> Option<String> {
    let s = input.trim().replacen('T', " ", 1);
    ["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M"]
        .iter()
        .find_map(|f| NaiveDateTime::parse_from_str(&s, f).ok())
        .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
}

/// `[begin, end)`（规范化时间）是否与已占用时段重叠；格式未知的时段视为不冲突
pub fn overlaps(begin: &str, end: &str, slot: &BookedSlot) -> bool {
    match (normalize_time(&slot.begin), normalize_time(&slot.end)) {
        (Some(b), Some(e)) => begin < e.as_str() && b.as_str() < end,
        _ => false,
    }
}

/// 按天计算空闲节次。`now`（`YYYY-MM-DD HH:MM:SS`）之前已开始的节次不再提供。
pub fn free_slots(dates: &[String], taken: &[BookedSlot], now: Option<&str>) -> Vec<DaySlots> {
    dates
        .iter()
        .map(|date| {
            let day_taken: Vec<BookedSlot> = taken
                .iter()
                .filter(|s| s.begin.starts_with(date.as_str()))
                .cloned()
                .collect();
            let free = PERIODS
                .iter()
                .enumerate()
                .filter_map(|(i, (b, e))| {
                    let begin = format!("{date} {b}:00");
                    let end = format!("{date} {e}:00");
                    if now.is_some_and(|n| begin.as_str() < n)
                        || day_taken.iter().any(|s| overlaps(&begin, &end, s))
                    {
                        return None;
                    }
                    Some(FreePeriod {
                        period: i + 1,
                        begin: b.to_string(),
                        end: e.to_string(),
                    })
                })
                .collect();
            DaySlots {
                date: date.clone(),
                free,
                taken: day_taken,
            }
        })
        .collect()
}

/// URL 的 path 部分（解析失败时返回原串）
fn url_path(url: &str) -> String {
    reqwest::Url::parse(url)
        .map(|u| u.path().to_string())
        .unwrap_or_else(|_| url.to_string())
}

/// 最终 URL 是否为「个人信息」页（资料不完整时预约会被重定向到这里）
pub fn is_profile_page(final_url: &str) -> bool {
    let path = url_path(final_url);
    path == "/personal" || (path.starts_with("/personal/") && !path.starts_with("/personal/user"))
}

/// 会话是否已失效：最终落在 IAAA、`/oauth` 登录入口或 `/login*`（`/login/oauth` 回调除外）
pub fn is_logged_out(status: u16, final_url: &str, body: &str) -> bool {
    let path = url_path(final_url);
    status == 401
        || final_url.contains("iaaa.pku.edu.cn")
        || path == "/oauth"
        || path.starts_with("/oauth/")
        || (path.starts_with("/login") && !path.starts_with("/login/oauth"))
        || (body.contains("iaaa.pku.edu.cn") && body.contains("oauth.jsp"))
}

/// /classRoom 页面是否为空状态（`div.cr-empty` / 暂无申请记录）
pub fn is_empty_applications(html: &str) -> bool {
    re(r#"class="[^"]*\bcr-empty\b"#).is_match(html) || html.contains("暂无申请记录")
}

/// 解析 `POST /classRoom/seachStudent`
pub fn parse_student(body: &str) -> Result<StudentInfo> {
    let parsed: SearchStudentResp = serde_json::from_str(body)
        .with_context(|| format!("解析学生信息失败（北大空间可能暂时不可用）: {body}"))?;
    if !parsed.success {
        let msg = if parsed.message.is_empty() {
            "查无此人".to_string()
        } else {
            parsed.message
        };
        return Err(anyhow!("seachStudent 失败: {msg}"));
    }
    parsed
        .data
        .ok_or_else(|| anyhow!("seachStudent 返回 data 为空"))
}

/// 从 HTML 中抠出第一条 `layer.msg("...")` 的文字
pub fn extract_layer_msg(html: &str) -> Option<String> {
    re(r#"layer\.msg\(["']([^"']+)["']"#)
        .captures(html)
        .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
}

/// 提交结果：成功 = 重定向到 `/classRoom` 列表；资料不完整 = `/personal`；
/// 其他情况读 `layer.msg(...)`
pub fn submit_outcome(final_url: &str, body: &str) -> Result<(bool, String)> {
    if is_profile_page(final_url) {
        return Err(anyhow!(PROFILE_MSG));
    }
    let path = url_path(final_url);
    let path = path.trim_end_matches('/');
    if path == "/classRoom" || path.ends_with("/classRoom") {
        return Ok((true, "预约提交成功".into()));
    }
    let msg = extract_layer_msg(body).unwrap_or_else(|| format!("未知返回: path={path}"));
    Ok((false, msg))
}

fn normalize_status(raw: &str) -> String {
    if raw.contains("申请成功") {
        "申请成功".into()
    } else if raw.contains("已取消") {
        "申请已取消".into()
    } else if raw.contains("已结束") {
        "已结束".into()
    } else if !raw.is_empty() {
        raw.to_string()
    } else {
        "未知".into()
    }
}

/// 解析 /classRoom 页面中的申请列表
///
/// 记录在 `div#results` 内；为空时页面显示 `div.cr-empty`（暂无申请记录）。
/// 2026 版行内结构尚未见到非空样例，仍按旧版 `div#results > div.row` 读取。
pub fn parse_applications(html: &str) -> Vec<Application> {
    if is_empty_applications(html) {
        return Vec::new();
    }
    let doc = Html::parse_document(html);
    let row_sel = sel("div#results > div.row");
    let h5_sel = sel("h5");

    let cancel_re = re(r"/classRoom/cancelApply/(\d+)");
    let begin_end_re = re(r"(\d{4}-\d{2}-\d{2} \d{2}:\d{2}\s*-\s*\d{2}:\d{2})");
    let apply_time_re = re(r"(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2})");
    let applicant_re = re(r"(\S+)\s+申请$");
    let reason_re = re(r"(?s)fa-comment-dots[^<]*</em>\s*([^<]+)");
    let status_re = re(r#"(?s)color-red"><strong>([^<]+)"#);

    let mut out = Vec::new();
    for node in doc.select(&row_sel) {
        let html_s = node.html();
        let text = el_text(node);

        let room_name = node.select(&h5_sel).next().map(el_text).unwrap_or_default();
        if room_name.is_empty() {
            continue;
        }
        let cap = |r: &Regex, s: &str| {
            r.captures(s)
                .and_then(|c| c.get(1).map(|m| m.as_str().trim().to_string()))
                .unwrap_or_default()
        };
        let id = cap(&cancel_re, &html_s);
        let status = normalize_status(&cap(&status_re, &html_s));
        let apply_time = cap(&apply_time_re, &text);
        let begin_end = cap(&begin_end_re, &text);
        // 申请人在 "xxx 申请 <time>" 的前面
        let applicant = if apply_time.is_empty() {
            String::new()
        } else {
            text.split_once(&apply_time)
                .map(|(prefix, _)| cap(&applicant_re, prefix.trim_end()))
                .unwrap_or_default()
        };
        let reason = cap(&reason_re, &html_s);

        // 参与人：文本形如 "张三[ 未签到 ] 李四[ 已签到 ]"，取每个 "[" 前的最后一个词
        let participants = text
            .split(']')
            .filter_map(|seg| {
                seg.rsplit_once('[')
                    .and_then(|(left, _)| left.split_whitespace().last())
                    .map(str::to_string)
            })
            .filter(|n| n != "申请")
            .collect::<Vec<_>>();

        out.push(Application {
            can_cancel: !id.is_empty(),
            id,
            room_name,
            status,
            applicant,
            apply_time,
            begin_end,
            reason,
            participants,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPACE_HTML: &str = r#"
<html><body>
<div class="cr-panel" data-building-id="1000000000000000001">
  <div class="cr-panel-head">
    <span class="cr-panel-title">二教</span>
    <span class="sp-building-code">(02)</span>
    <span class="cr-panel-count">共 <b>2</b> 间可预约</span>
  </div>
  <div class="sp-card" data-room-id="2000000000000000011" data-room-name="514">
    <div class="cr-room-name">514</div>
    <span class="sp-code">02514</span>
    <ul class="sp-features">
      <li>5层</li>
      <li>可容纳 <b>12</b> 人</li>
      <li>至少 <b>3</b> 人起约</li>
    </ul>
    <div class="sp-devs">
      <span data-dev-kind="door">门 <b class="sp-dev-state">关门</b></span>
      <span data-dev-kind="screen">屏幕 <b class="sp-dev-state">正常</b></span>
    </div>
    <a class="sp-btn-apply" href="/space/apply?roomId=2000000000000000011">去申请</a>
  </div>
  <div class="sp-card" data-room-id="2000000000000000012" data-room-name="520">
    <span class="sp-code">02520</span>
    <ul class="sp-features"><li>可容纳 <b>8</b> 人</li></ul>
    <a class="sp-btn-apply" href="javascript:void(0)">去申请</a>
  </div>
</div>
<div class="cr-panel" data-building-id="1000000000000000002">
  <span class="cr-panel-title">四教</span>
  <span class="sp-building-code">(04)</span>
</div>
<div class="cr-panel" data-building-id="1000000000000000003">
  <span class="cr-panel-title">地学</span>
  <span class="sp-building-code">(09)</span>
</div>
<div class="sp-card" data-room-id="2000000000000000041" data-room-name="401">
  <span class="sp-code">04401</span>
  <ul class="sp-features"><li>4层</li><li>可容纳 <b>20</b> 人</li></ul>
</div>
</body></html>"#;

    #[test]
    fn space_buildings_and_rooms() {
        let (buildings, rooms) = parse_space(SPACE_HTML);
        assert_eq!(buildings.len(), 3);
        assert_eq!(buildings[0].name, "二教");
        assert_eq!(buildings[0].code, "02");
        assert_eq!(buildings[2].code, "09");

        assert_eq!(rooms.len(), 3);
        let r = &rooms[0];
        assert_eq!(r.id, "2000000000000000011");
        assert_eq!(r.name, "514");
        assert_eq!(r.code, "02514");
        assert_eq!(r.building, "二教");
        assert_eq!(r.floor, Some(5));
        assert_eq!(r.capacity, Some(12));
        assert_eq!(r.min_people, Some(3));
        assert_eq!(r.door, "关门");
        assert_eq!(r.screen, "正常");
        assert_eq!(
            r.apply_href.as_deref(),
            Some("/space/apply?roomId=2000000000000000011")
        );

        // name falls back to data-room-name, javascript: href dropped
        assert_eq!(rooms[1].name, "520");
        assert_eq!(rooms[1].apply_href, None);
        assert_eq!(rooms[1].min_people, None);

        // outside any panel → matched by code prefix
        assert_eq!(rooms[2].building, "四教");
        assert_eq!(rooms[2].building_id, "1000000000000000002");
    }

    #[test]
    fn building_matching() {
        let (buildings, _) = parse_space(SPACE_HTML);
        let id = |q: &str| match_building(q, &buildings).map(|b| b.name.as_str());
        assert_eq!(id("二教"), Some("二教"));
        assert_eq!(id("二教楼"), Some("二教"));
        assert_eq!(id("02"), Some("二教"));
        assert_eq!(id("4"), Some("四教"));
        assert_eq!(id("dixue"), Some("地学"));
        assert_eq!(id("第二教学楼"), Some("二教"));
        assert_eq!(id("理教"), None);
        assert_eq!(id(""), None);
    }

    #[test]
    fn lock_status() {
        let m = parse_lock_status(r#"{"1":0,"2":2,"3":"x"}"#);
        assert_eq!(m.len(), 2);
        assert_eq!(m["2"], 2);
        assert!(parse_lock_status("<html>").is_empty());
        assert!(parse_lock_status("[]").is_empty());
    }

    #[test]
    fn schedule_and_free_slots() {
        let body = r#"[{"beginTime":"2026-10-20 08:00:00","endTime":"2026-10-20 09:50:00"},
                      {"beginTime":"2026-10-21 20:40:00","endTime":"2026-10-21 21:30:00"},
                      {"foo":1}]"#;
        let taken = parse_schedule(body).unwrap();
        assert_eq!(taken.len(), 2);
        assert!(parse_schedule("<html>").is_err());

        let dates = vec!["2026-10-20".to_string(), "2026-10-21".to_string()];
        let days = free_slots(&dates, &taken, None);
        assert_eq!(days[0].taken.len(), 1);
        assert_eq!(days[0].free.len(), 11);
        assert_eq!(days[0].free[0].period, 3);
        assert_eq!(days[1].free.len(), 12);
        assert!(days[1].free.iter().all(|p| p.period != 12));

        let days = free_slots(&dates[..1], &taken, Some("2026-10-20 13:30:00"));
        assert_eq!(days[0].free.first().map(|p| p.period), Some(6));
        assert_eq!(days[0].free.len(), 8);
    }

    #[test]
    fn time_helpers() {
        assert_eq!(
            normalize_time("2026-10-20 14:00").as_deref(),
            Some("2026-10-20 14:00:00")
        );
        assert_eq!(
            normalize_time("2026-10-20T14:00:30").as_deref(),
            Some("2026-10-20 14:00:30")
        );
        assert_eq!(normalize_time("2026-02-30 14:00"), None);
        assert_eq!(normalize_time("tomorrow"), None);

        let slot = BookedSlot {
            begin: "2026-10-20 14:00:00".into(),
            end: "2026-10-20 14:50:00".into(),
        };
        assert!(overlaps(
            "2026-10-20 13:00:00",
            "2026-10-20 14:10:00",
            &slot
        ));
        assert!(!overlaps(
            "2026-10-20 14:50:00",
            "2026-10-20 15:40:00",
            &slot
        ));
    }

    #[test]
    fn url_classification() {
        assert!(is_profile_page("https://bdkj.pku.edu.cn/personal"));
        assert!(is_profile_page("https://bdkj.pku.edu.cn/personal/edit?x=1"));
        assert!(!is_profile_page("https://bdkj.pku.edu.cn/personal/user"));
        assert!(!is_profile_page("https://bdkj.pku.edu.cn/space"));

        assert!(is_logged_out(200, "https://bdkj.pku.edu.cn/oauth", ""));
        assert!(is_logged_out(200, "https://bdkj.pku.edu.cn/login", ""));
        assert!(is_logged_out(
            200,
            "https://iaaa.pku.edu.cn/iaaa/oauth.jsp?appID=bdkj",
            ""
        ));
        assert!(is_logged_out(401, "https://bdkj.pku.edu.cn/space", ""));
        assert!(!is_logged_out(200, "https://bdkj.pku.edu.cn/home", ""));
        assert!(!is_logged_out(
            200,
            "https://bdkj.pku.edu.cn/login/oauth?token=x",
            ""
        ));
    }

    #[test]
    fn submit_outcomes() {
        let (ok, _) = submit_outcome("https://bdkj.pku.edu.cn/classRoom", "").unwrap();
        assert!(ok);
        let (ok, msg) = submit_outcome(
            "https://bdkj.pku.edu.cn/classRoom/apply",
            r#"<script>layer.msg("时间冲突")</script>"#,
        )
        .unwrap();
        assert!(!ok);
        assert_eq!(msg, "时间冲突");
        let err = submit_outcome("https://bdkj.pku.edu.cn/personal", "").unwrap_err();
        assert!(err.to_string().contains("个人信息"));
    }

    #[test]
    fn student() {
        let s = parse_student(
            r#"{"success":true,"data":{"id":"9","serial":"2200000001","name":"张三"}}"#,
        )
        .unwrap();
        assert_eq!(s.id, "9");
        assert!(parse_student(r#"{"success":false,"message":"查无此人"}"#).is_err());
        assert!(parse_student("<html>").is_err());
    }

    #[test]
    fn applications_empty_state() {
        let html = r#"<div id="results"><div class="cr-empty">暂无申请记录</div></div>"#;
        assert!(is_empty_applications(html));
        assert!(parse_applications(html).is_empty());
    }

    #[test]
    fn applications_rows() {
        let html = r#"
<div id="results">
  <div class="row">
    <h5> 二教 514 </h5>
    <span class="color-red"><strong>申请成功</strong></span>
    <p>张三 申请 2026-10-18 10:00:00</p>
    <p>2026-10-20 14:00 - 15:50</p>
    <p><em class="fa fa-comment-dots"></em> 课题组讨论</p>
    <p>张三[ 未签到 ] 李四[ 未签到 ] 王五[ 已签到 ]</p>
    <a href="/classRoom/cancelApply/3000000000000000001">取消</a>
  </div>
  <div class="row"><p>no title</p></div>
</div>"#;
        let apps = parse_applications(html);
        assert_eq!(apps.len(), 1);
        let a = &apps[0];
        assert_eq!(a.id, "3000000000000000001");
        assert!(a.can_cancel);
        assert_eq!(a.room_name, "二教 514");
        assert_eq!(a.status, "申请成功");
        assert_eq!(a.applicant, "张三");
        assert_eq!(a.apply_time, "2026-10-18 10:00:00");
        assert_eq!(a.begin_end, "2026-10-20 14:00 - 15:50");
        assert_eq!(a.reason, "课题组讨论");
        assert_eq!(a.participants, vec!["张三", "李四", "王五"]);
    }
}
