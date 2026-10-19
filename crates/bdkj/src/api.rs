//! 北大空间 API 客户端
//!
//! 教室列表 / 已占用时段 / 我的预约按 2026-10 改版后的站点实现（`/space`、`/space/lockStatus`、
//! `/space/schedule`、`/classRoom`）。查询学生、提交和取消仍是旧版流程，尚未在新站点上验证。

use crate::client::BDKJ_BASE;
use crate::parse::{self, Application, BookedSlot, Building, Room, StudentInfo, PROFILE_MSG};
use anyhow::{anyhow, Context, Result};
use std::collections::BTreeMap;

const XHR_ACCEPT: &str = "application/json, text/javascript, */*; q=0.01";

#[derive(Debug, Clone)]
pub struct Participant {
    pub student_id: String,
    pub serial: String,
    pub name: String,
}

#[derive(Debug)]
pub struct SubmitResult {
    pub success: bool,
    pub message: String,
}

/// 一次请求跟随重定向后的结果
struct Page {
    url: String,
    body: String,
}

pub struct BdkjApi {
    http: reqwest::Client,
}

impl BdkjApi {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }

    /// 发送请求并做统一检查：HTTP 错误、会话失效、被重定向到「个人信息」页
    async fn send(&self, req: reqwest::RequestBuilder, what: &str) -> Result<Page> {
        let resp = req
            .send()
            .await
            .with_context(|| format!("{what}请求失败"))?;
        let status = resp.status();
        let url = resp.url().to_string();
        let body = resp
            .text()
            .await
            .with_context(|| format!("读取{what}响应失败"))?;
        if parse::is_logged_out(status.as_u16(), &url, &body) {
            return Err(anyhow!("北大空间会话已失效，请重新运行 `bdkj login`"));
        }
        if parse::is_profile_page(&url) {
            return Err(anyhow!(PROFILE_MSG));
        }
        if !status.is_success() {
            return Err(anyhow!("{what} HTTP {status}"));
        }
        Ok(Page { url, body })
    }

    fn xhr(&self, path: &str) -> reqwest::RequestBuilder {
        self.http
            .get(format!("{BDKJ_BASE}{path}"))
            .header("x-requested-with", "XMLHttpRequest")
            .header("accept", XHR_ACCEPT)
            .header("referer", format!("{BDKJ_BASE}/space"))
    }

    /// `GET /space`：教学楼 + 教室卡片
    pub async fn space(&self) -> Result<(Vec<Building>, Vec<Room>)> {
        let page = self
            .send(self.http.get(format!("{BDKJ_BASE}/space")), "教室列表")
            .await?;
        let (buildings, rooms) = parse::parse_space(&page.body);
        if buildings.is_empty() && rooms.is_empty() {
            return Err(anyhow!("没有从北大空间读到教室列表（页面结构可能又改了）"));
        }
        Ok((buildings, rooms))
    }

    /// `GET /space/lockStatus`：`{ roomId: 0|1|2 }`，仅供参考（含义未验证）
    pub async fn lock_status(&self) -> Result<BTreeMap<String, i64>> {
        let page = self.send(self.xhr("/space/lockStatus"), "门锁状态").await?;
        Ok(parse::parse_lock_status(&page.body))
    }

    /// `GET /space/schedule`：教室从 `start_date`（`YYYY-MM-DD`）起一周内已被占用的时段
    pub async fn schedule(&self, room_id: &str, start_date: &str) -> Result<Vec<BookedSlot>> {
        let req = self
            .xhr("/space/schedule")
            .query(&[("roomId", room_id), ("startDate", start_date)]);
        let page = self.send(req, "已预约时段").await?;
        parse::parse_schedule(&page.body)
    }

    /// 像浏览器一样打开教室的「去申请」页面；资料不完整时会被重定向到 /personal
    pub async fn check_can_apply(&self, apply_href: Option<&str>) -> Result<()> {
        let Some(href) = apply_href else {
            return Ok(());
        };
        let url = if href.starts_with("http://") || href.starts_with("https://") {
            if !href.contains("://bdkj.pku.edu.cn/") {
                return Ok(());
            }
            href.to_string()
        } else if href.starts_with('/') {
            format!("{BDKJ_BASE}{href}")
        } else {
            format!("{BDKJ_BASE}/{href}")
        };
        let req = self
            .http
            .get(url)
            .header("referer", format!("{BDKJ_BASE}/space"));
        self.send(req, "申请页").await.map(|_| ())
    }

    /// 根据学号+姓名查询学生信息
    pub async fn search_student(&self, serial: &str, name: &str) -> Result<StudentInfo> {
        let req = self
            .http
            .post(format!("{BDKJ_BASE}/classRoom/seachStudent"))
            .header("x-requested-with", "XMLHttpRequest")
            .header("accept", XHR_ACCEPT)
            .form(&[("serial", serial), ("name", name)]);
        let page = self.send(req, "查询学生").await?;
        parse::parse_student(&page.body).with_context(|| format!("参与人 {serial} {name} 校验失败"))
    }

    /// 提交预约（旧版流程，未在 2026 新站点上验证）
    ///
    /// `begin_time` / `end_time` 格式 `YYYY-MM-DD HH:MM:SS`
    pub async fn submit_apply(
        &self,
        room_id: &str,
        begin_time: &str,
        end_time: &str,
        reason: &str,
        participants: &[Participant],
    ) -> Result<SubmitResult> {
        // 表单字段与旧版浏览器 step4 提交完全一致
        let mut form: Vec<(String, String)> = vec![
            ("id".into(), "".into()),
            ("beginTime".into(), begin_time.into()),
            ("endTime".into(), end_time.into()),
            ("roomId".into(), room_id.into()),
            ("step".into(), "3".into()),
            ("switchover".into(), "true".into()),
            ("reason".into(), reason.into()),
        ];
        for (i, p) in participants.iter().enumerate() {
            form.push((format!("students[{i}].studentId"), p.student_id.clone()));
            form.push((format!("students[{i}].serial"), p.serial.clone()));
            form.push((format!("students[{i}].name"), p.name.clone()));
            form.push((format!("students[{i}].sort"), i.to_string()));
        }

        let req = self
            .http
            .post(format!("{BDKJ_BASE}/classRoom/handle/submit"))
            .header("referer", format!("{BDKJ_BASE}/classRoom/apply"))
            .form(&form);
        // send() 已把 /personal 重定向转成 PROFILE_MSG 错误
        let page = self.send(req, "提交预约").await?;
        let (success, message) = parse::submit_outcome(&page.url, &page.body)?;
        Ok(SubmitResult { success, message })
    }

    /// 取消预约，并以申请列表确认（页面上的 layer.msg 不足为凭）
    pub async fn cancel_apply(&self, apply_id: &str) -> Result<Option<Application>> {
        let req = self
            .http
            .get(format!("{BDKJ_BASE}/classRoom/cancelApply/{apply_id}"));
        let page = self.send(req, "取消预约").await?;
        let msg = parse::extract_layer_msg(&page.body);
        let list = if page.body.contains(r#"id="results""#) {
            parse::parse_applications(&page.body)
        } else {
            self.list_applications().await?
        };
        let after = list.into_iter().find(|a| a.id == apply_id);
        if let Some(a) = &after {
            if a.can_cancel && a.status != "申请已取消" {
                return Err(anyhow!(
                    "取消没有生效{}",
                    msg.map(|m| format!("：{m}")).unwrap_or_default()
                ));
            }
        }
        Ok(after)
    }

    /// 拉取 /classRoom 页面并抽取申请列表
    pub async fn list_applications(&self) -> Result<Vec<Application>> {
        let page = self
            .send(self.http.get(format!("{BDKJ_BASE}/classRoom")), "预约记录")
            .await?;
        Ok(parse::parse_applications(&page.body))
    }
}
