//! 收费平台只读 API
//!
//! 端点与请求体取自线上移动端 bundle（2026-10-23，`/mobile/js/app.43b046a6.js` + chunks），
//! 设置 `schoolCode = "10001"`、`dataSource = "PAY"`。这里只有查询类接口：不下单、不支付、
//! 不关单、不改发票抬头。

use crate::client::{self, CWPAY_BASE};
use crate::crypto::{decode_body, envelope_message, envelope_state, EnvelopeState};
use crate::login;
use crate::parse::{self, Order};
use anyhow::{bail, Context, Result};
use pkuinfo_common::session::{Session, Store};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub const SCHOOL_CODE: &str = "10001";
pub const DATA_SOURCE: &str = "PAY";
const PAGE: u32 = 50;

pub struct Api {
    http: reqwest::Client,
    store: Store,
    session: Session,
}

impl Api {
    pub fn new() -> Result<Self> {
        let (store, session) = login::load_session()?;
        Ok(Self {
            http: client::build()?,
            store,
            session,
        })
    }

    /// 学号（dealcas jsonData 的 idserial，或登录用户名）
    pub fn idserial(&self) -> Result<String> {
        self.session
            .uid
            .clone()
            .filter(|s| !s.is_empty())
            .context("会话里没有学号，请用 `cwpay login -p` 重新登录")
    }

    /// 发请求，返回解密后的信封；处理 -3 续期（保存新 token 并重放）与登出。
    async fn envelope(&mut self, path: &str, body: Option<&Value>) -> Result<Value> {
        let url = format!("{CWPAY_BASE}/api/{}", path.trim_start_matches('/'));
        for _ in 0..3 {
            let req = match body {
                Some(b) => self.http.post(&url).json(b),
                None => self.http.get(&url),
            };
            let resp = req
                .header("X-Token", &self.session.token)
                .send()
                .await
                .with_context(|| format!("请求收费平台失败：{path}"))?;
            let status = resp.status();
            let text = resp.text().await.context("读取收费平台响应失败")?;
            let plain = decode_body(&text);
            let env: Value = match serde_json::from_str(&plain) {
                Ok(v) => v,
                Err(_) if status.is_server_error() => bail!("收费平台暂时不可用（HTTP {status}）"),
                Err(_) => bail!("收费平台返回了无法识别的内容（HTTP {status}，{path}）"),
            };
            match envelope_state(&env) {
                EnvelopeState::Renew => {
                    let Some(token) = env
                        .get("data")
                        .and_then(Value::as_str)
                        .filter(|t| !t.is_empty())
                    else {
                        bail!("收费平台续期 token 失败，请重新运行 `cwpay login -p`");
                    };
                    self.session.token = token.to_string();
                    if let Some(exp) = crate::crypto::jwt_exp(token) {
                        self.session.expires_at = Some(exp);
                    }
                    self.store.save_session(&self.session)?;
                }
                EnvelopeState::SignedOut => bail!(
                    "收费平台会话已失效：{}。请重新运行 `cwpay login -p`",
                    envelope_message(&env)
                ),
                _ => return Ok(env),
            }
        }
        bail!("收费平台反复要求续期 token，请重新运行 `cwpay login -p`")
    }

    /// 只接受 "0"/"6003"，返回 data
    async fn data(&mut self, path: &str, body: Option<&Value>, what: &str) -> Result<Value> {
        let env = self.envelope(path, body).await?;
        match envelope_state(&env) {
            EnvelopeState::Ok => Ok(env.get("data").cloned().unwrap_or(Value::Null)),
            _ => bail!("收费平台：{}（{what}）", envelope_message(&env)),
        }
    }

    /// 全部缴费项目（含暂停状态）
    pub async fn catalog(&mut self) -> Result<Vec<parse::Project>> {
        let data = self
            .data(
                "pay/project/getAllProjectNoType",
                Some(&json!({ "schoolCode": SCHOOL_CODE })),
                "缴费项目",
            )
            .await?;
        Ok(parse::projects(&data))
    }

    /// 学费住宿费（先在目录里找 tuitionAndDorm 项目）
    pub async fn tuition(&mut self) -> Result<(String, parse::Tuition)> {
        let project = self
            .catalog()
            .await?
            .into_iter()
            .find(|p| p.kind == "tuitionAndDorm")
            .context("收费平台目前没有“学费住宿费”缴费项目")?;
        let id = self.idserial()?;
        let env = self
            .envelope(
                &format!(
                    "pay/web/tuitionAndDorm/getTuitionAndDormList/{id}/{}",
                    project.id
                ),
                None,
            )
            .await?;
        if parse::is_no_arrears(&env) {
            return Ok((project.name, parse::tuition(&Value::Null)));
        }
        match envelope_state(&env) {
            EnvelopeState::Ok => Ok((
                project.name,
                parse::tuition(env.get("data").unwrap_or(&Value::Null)),
            )),
            _ => bail!("收费平台：{}（学费住宿费）", envelope_message(&env)),
        }
    }

    /// 待缴费列表（全部项目；最后一段 "undefined" 与站点一致）
    pub async fn pending(&mut self) -> Result<Vec<parse::Pending>> {
        let id = self.idserial()?;
        let data = self
            .data(
                &format!("pay/pay/waitingPay/getWaitingPayList/{SCHOOL_CODE}/{id}/undefined"),
                None,
                "待缴费",
            )
            .await?;
        Ok(parse::pending(&data))
    }

    /// 订单状态名（字典；失败用内置）
    pub async fn status_names(&mut self) -> BTreeMap<String, String> {
        let path = format!(
            "common/com/dict/getDictDatasByTypes/TRADE_DISPLAY_STATUS/{SCHOOL_CODE}/{DATA_SOURCE}"
        );
        match self.data(&path, None, "订单状态").await {
            Ok(d) => parse::status_dict(&d),
            Err(_) => parse::default_status_names(),
        }
    }

    /// 订单列表一页
    pub async fn orders_page(
        &mut self,
        page: u32,
        size: u32,
        from: Option<&str>,
        to: Option<&str>,
        project: Option<&str>,
        names: &BTreeMap<String, String>,
    ) -> Result<(u64, Vec<Order>)> {
        let body = json!({
            "pageCurrent": page,
            "pageSize": size,
            "projectName": project.unwrap_or(""),
            "projectId": "",
            "displayStatus": "",
            "startCreateDate": from.unwrap_or(""),
            "endCreateDate": to.unwrap_or(""),
            "schoolCode": SCHOOL_CODE,
        });
        let data = self
            .data("pay/web/order/pageOrderlist", Some(&body), "订单记录")
            .await?;
        Ok(parse::orders(&data, names))
    }

    /// 翻页取订单（最多 max_pages 页，每页 50）；stop 命中即停。返回 (订单, 是否截断)
    pub async fn all_orders(
        &mut self,
        from: Option<&str>,
        to: Option<&str>,
        project: Option<&str>,
        max_pages: u32,
        stop: Option<&str>,
        names: &BTreeMap<String, String>,
    ) -> Result<(Vec<Order>, bool)> {
        let mut out: Vec<Order> = Vec::new();
        for page in 1..=max_pages {
            let (total, list) = self
                .orders_page(page, PAGE, from, to, project, names)
                .await?;
            let n = list.len();
            let hit = stop.is_some_and(|no| list.iter().any(|o| o.order_no == no));
            out.extend(list);
            if hit || n < PAGE as usize || out.len() as u64 >= total {
                return Ok((out, false));
            }
            if page == max_pages {
                let truncated = (out.len() as u64) < total;
                return Ok((out, truncated));
            }
        }
        Ok((out, false))
    }

    /// 订单详情 + 电子票据链接
    pub async fn order_detail(&mut self, order_no: &str) -> Result<parse::OrderDetail> {
        let names = self.status_names().await;
        let (list, _) = self
            .all_orders(None, None, None, 6, Some(order_no), &names)
            .await?;
        let row = list
            .into_iter()
            .find(|o| o.order_no == order_no)
            .with_context(|| format!("在最近的订单里没有找到订单号 {order_no}"))?;
        let detail = if row.id.is_empty() {
            Value::Null
        } else {
            self.data(
                &format!("pay/pay/orderTrade/getOrderById/{}", row.id),
                None,
                "订单详情",
            )
            .await
            .unwrap_or(Value::Null)
        };
        let mut d = parse::order_detail(&row, &detail, &names);
        if let (Some(bill_no), Some(bill_type)) = (&row.bill_no, &row.bill_type) {
            if let Some(path) = parse::invoice_endpoint(bill_type) {
                let body =
                    json!({ "billType": bill_type, "billNo": bill_no, "schoolCode": SCHOOL_CODE });
                if let Ok(data) = self.data(path, Some(&body), "电子票据").await {
                    d.invoice_url = parse::invoice_url(&data);
                }
            }
        }
        Ok(d)
    }

    /// 首页公告
    pub async fn notice(&mut self) -> Result<Option<parse::Notice>> {
        let data = self
            .data(
                "common/com/notice/getStuNotice",
                Some(&json!({ "schoolCode": SCHOOL_CODE, "dataSource": DATA_SOURCE })),
                "平台公告",
            )
            .await?;
        Ok(parse::notice(&data))
    }

    /// 个人信息（个人中心）
    pub async fn profile(&mut self) -> Result<parse::Profile> {
        let id = self.idserial()?;
        let body =
            json!({ "accountNum": id, "schoolCode": SCHOOL_CODE, "dataSource": DATA_SOURCE });
        let data = self
            .data(
                "pay/web/personalCenter/findUserInfoByAccountNum",
                Some(&body),
                "个人信息",
            )
            .await?;
        Ok(parse::profile(&data))
    }
}
