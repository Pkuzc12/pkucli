//! 收费平台响应解析（纯函数 + 单测）
//!
//! 字段名取自线上移动端 bundle 的各页面 chunk（2026-10，见各函数注释）。金额单位是分
//! （SPA 用 `fenToYuan` 换算）。与 pku-harness `plugins/finance/src/cwpay-parse.ts` /
//! `cwpay-extra-parse.ts` 一一对应。

use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

// ── 通用 ─────────────────────────────────────────────────────────────────────

/// 分 → 元（数字或数字字符串；空/非法为 None）
pub fn fen_to_yuan(v: &Value) -> Option<f64> {
    let n = match v {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) if !s.trim().is_empty() => s.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    n.is_finite().then(|| n.round() / 100.0)
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// 服务端文本：去标签、去控制字符、压缩空白、截断
pub fn clean(v: &Value, max: usize) -> String {
    let raw = match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    };
    let mut out = String::with_capacity(raw.len());
    let mut in_tag = false;
    for c in raw.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if in_tag => {}
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    let collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.chars().take(max).collect()
}

fn s(v: &Value, key: &str, max: usize) -> String {
    clean(v.get(key).unwrap_or(&Value::Null), max)
}

fn arr(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}

// ── 订单状态 ─────────────────────────────────────────────────────────────────

/// TRADE_DISPLAY_STATUS（2026-10-23 线上字典；运行时以 `getDictDatasByTypes` 为准）
pub const ORDER_STATUS: &[(&str, &str)] = &[
    ("101", "待支付"),
    ("102", "支付中"),
    ("103", "支付成功"),
    ("104", "支付失败"),
    ("107", "部分退款"),
    ("108", "全额退款"),
    ("005", "已过期"),
    ("006", "已取消"),
];

pub fn default_status_names() -> BTreeMap<String, String> {
    ORDER_STATUS
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// `getDictDatasByTypes/TRADE_DISPLAY_STATUS/…` → `{TRADE_DISPLAY_STATUS: [{key, value}]}`
pub fn status_dict(data: &Value) -> BTreeMap<String, String> {
    let mut out = default_status_names();
    for d in arr(data.get("TRADE_DISPLAY_STATUS").unwrap_or(&Value::Null)) {
        let key = clean(d.get("key").unwrap_or(&Value::Null), 10);
        let value = s(d, "value", 20);
        if !key.is_empty() && !value.is_empty() {
            out.insert(key, value);
        }
    }
    out
}

fn status_name(names: &BTreeMap<String, String>, code: &str) -> String {
    names.get(code).cloned().unwrap_or_else(|| {
        if code.is_empty() {
            "未知".into()
        } else {
            format!("状态 {code}")
        }
    })
}

/// 支付渠道（`pay/channel/getChannelList` {schoolCode, partnerId "PT000004"}，2026-10-23）
pub fn channel_name(code: &str) -> Option<String> {
    let name = match code {
        "" => return None,
        "01" => "支付宝",
        "02" => "微信",
        "08" => "建行网银",
        "09" => "建行大额转账",
        "61" => "中行外卡",
        "71" => "数字货币",
        other => return Some(format!("渠道 {other}")),
    };
    Some(name.into())
}

// ── 缴费项目目录：POST pay/project/getAllProjectNoType {schoolCode} ────────────

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub name_en: String,
    /// proModelUrl：支付页 `/<kind>Pay`
    pub kind: String,
    pub kind_label: String,
    /// 标准金额（元），payamt > 0 时
    pub standard_amount: Option<f64>,
    /// status=YES 且未暂停
    pub open: bool,
    /// 仅暂停时：平台维护说明（suspendTip）
    pub suspended_note: Option<String>,
    pub invoice_required: bool,
}

pub fn kind_label(kind: &str) -> &str {
    match kind {
        "tuitionAndDorm" => "学费住宿费",
        "bathRecharge" => "浴卡充值",
        "net" => "网费",
        "eCardRecharge" => "一卡通充值",
        "hospital" => "校园卡就诊账户",
        "party" => "党费",
        "medicalParty" => "党费（医学部）",
        "union" => "工会会费",
        "carFaculty" | "carCooperation" => "车证",
        "heating" => "供暖费",
        "xiaojiaheRecharge" => "肖家河租金",
        "newPark" => "中关新园",
        "scene" => "现场/窗口收费",
        "common" => "其他缴费",
        "meeting" => "会议费",
        other => other,
    }
}

pub fn projects(data: &Value) -> Vec<Project> {
    arr(data)
        .iter()
        .filter(|p| {
            !p.get("id").unwrap_or(&Value::Null).is_null() && !s(p, "projectName", 60).is_empty()
        })
        .map(|p| {
            let kind = s(p, "proModelUrl", 40);
            let suspended = p.get("isSuspend").and_then(Value::as_str) == Some("YES");
            let status = p.get("status").and_then(Value::as_str).unwrap_or("YES");
            let amount = fen_to_yuan(p.get("payamt").unwrap_or(&Value::Null)).filter(|a| *a > 0.0);
            let note = s(p, "suspendTip", 200);
            Project {
                id: clean(&p["id"], 40),
                name: s(p, "projectName", 60),
                name_en: s(p, "engName", 80),
                kind_label: kind_label(&kind).to_string(),
                kind,
                standard_amount: amount,
                open: status == "YES" && !suspended,
                suspended_note: (suspended && !note.is_empty()).then_some(note),
                invoice_required: p.get("receiptMust").and_then(Value::as_str) == Some("YES"),
            }
        })
        .collect()
}

pub fn filter_projects<'a>(
    list: &'a [Project],
    search: Option<&str>,
    kind: Option<&str>,
    only_open: bool,
) -> Vec<&'a Project> {
    let q = search
        .map(|q| q.trim().to_lowercase())
        .filter(|q| !q.is_empty());
    list.iter()
        .filter(|p| kind.is_none_or(|k| p.kind == k))
        .filter(|p| !only_open || p.open)
        .filter(|p| {
            q.as_ref().is_none_or(|q| {
                p.name.to_lowercase().contains(q)
                    || p.name_en.to_lowercase().contains(q)
                    || p.kind_label.to_lowercase().contains(q)
            })
        })
        .collect()
}

// ── 学费住宿费：GET pay/web/tuitionAndDorm/getTuitionAndDormList/<idserial>/<projectId> ──
// chunk-34087c56：academicYear, feeTypeName, shouldpay, paid, note（分）

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TuitionItem {
    pub year: String,
    pub item: String,
    pub due: Option<f64>,
    pub paid: Option<f64>,
    pub unpaid: Option<f64>,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Tuition {
    pub items: Vec<TuitionItem>,
    pub due: f64,
    pub paid: f64,
    pub unpaid: f64,
}

/// 没欠费时接口回 `{"messageCode":"0001","message":"无欠费##","data":null}`（2026-10-23 实测，
/// SPA 只是 toast 一下）：这是空列表，不是错误。
pub fn is_no_arrears(env: &Value) -> bool {
    crate::crypto::message_code(env) == "0001"
        && env
            .get("message")
            .and_then(Value::as_str)
            .is_some_and(|m| m.contains("无欠费"))
}

pub fn tuition(rows: &Value) -> Tuition {
    let items: Vec<TuitionItem> = arr(rows)
        .iter()
        .map(|r| {
            let due = fen_to_yuan(&r["shouldpay"]);
            let paid = fen_to_yuan(&r["paid"]);
            TuitionItem {
                year: s(r, "academicYear", 20),
                item: s(r, "feeTypeName", 40),
                due,
                paid,
                unpaid: due.map(|d| round2(d - paid.unwrap_or(0.0))),
                note: s(r, "note", 80),
            }
        })
        .collect();
    let sum = |f: fn(&TuitionItem) -> Option<f64>| {
        round2(items.iter().map(|i| f(i).unwrap_or(0.0).max(0.0)).sum())
    };
    Tuition {
        due: sum(|i| i.due),
        paid: sum(|i| i.paid),
        unpaid: sum(|i| i.unpaid),
        items,
    }
}

// ── 订单：POST /pay/web/order/pageOrderlist；GET pay/pay/orderTrade/getOrderById/<id> ──

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Order {
    /// 内部 id（getOrderById 用）
    #[serde(skip)]
    pub id: String,
    pub order_no: String,
    pub project: String,
    pub amount: Option<f64>,
    pub created_at: String,
    pub status_code: String,
    pub status: String,
    pub invoiced: bool,
    #[serde(skip)]
    pub bill_no: Option<String>,
    #[serde(skip)]
    pub bill_type: Option<String>,
    #[serde(skip)]
    pub refund: Option<f64>,
}

fn project_name(o: &Value) -> String {
    let name = s(o, "projectName", 60);
    let name = if name.is_empty() {
        s(o, "productDesc", 60)
    } else {
        name
    };
    name.trim_end_matches('&').to_string()
}

fn opt_str(v: &Value, key: &str, max: usize) -> Option<String> {
    Some(s(v, key, max)).filter(|x| !x.is_empty())
}

/// pageOrderlist data → (total, orders)
pub fn orders(data: &Value, names: &BTreeMap<String, String>) -> (u64, Vec<Order>) {
    let records = arr(data.get("records").unwrap_or(&Value::Null));
    let total = data
        .get("total")
        .and_then(Value::as_u64)
        .unwrap_or(records.len() as u64);
    let list = records
        .iter()
        .map(|o| {
            let code = clean(o.get("displayStatus").unwrap_or(&Value::Null), 10);
            Order {
                id: clean(o.get("id").unwrap_or(&Value::Null), 40),
                order_no: s(o, "orderNo", 40),
                project: project_name(o),
                amount: fen_to_yuan(&o["amount"]),
                created_at: s(o, "createDate", 20),
                status: status_name(names, &code),
                status_code: code,
                invoiced: opt_str(o, "billNo", 60).is_some(),
                bill_no: opt_str(o, "billNo", 60),
                bill_type: opt_str(o, "billType", 20),
                refund: fen_to_yuan(&o["refundAmount"]).filter(|r| *r > 0.0),
            }
        })
        .collect();
    (total, list)
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct OrderDetail {
    pub order_no: String,
    pub project: String,
    pub amount: Option<f64>,
    pub payable: Option<f64>,
    pub refunded: Option<f64>,
    pub status: String,
    pub created_at: String,
    /// actualCloseTime：已支付订单为支付时间，过期/取消订单为关闭时间
    pub closed_at: String,
    pub paid_by: Option<String>,
    pub invoice_required: bool,
    pub invoice_issued: bool,
    pub invoice_url: Option<String>,
}

/// 列表行（有 projectName/billNo/billType）+ getOrderById（有渠道、时间等）
pub fn order_detail(row: &Order, detail: &Value, names: &BTreeMap<String, String>) -> OrderDetail {
    let d = |k: &str| detail.get(k).filter(|v| !v.is_null());
    let code = d("displayStatus")
        .map(|v| clean(v, 10))
        .unwrap_or_else(|| row.status_code.clone());
    OrderDetail {
        order_no: row.order_no.clone(),
        project: row.project.clone(),
        amount: d("amount").and_then(fen_to_yuan).or(row.amount),
        payable: d("payableAmount").and_then(fen_to_yuan),
        refunded: d("refundAmount")
            .and_then(fen_to_yuan)
            .filter(|r| *r > 0.0)
            .or(row.refund),
        status: status_name(names, &code),
        created_at: row.created_at.clone(),
        closed_at: d("actualCloseTime")
            .map(|v| clean(v, 20))
            .unwrap_or_default(),
        paid_by: d("tradeChannel").and_then(|v| channel_name(&clean(v, 8))),
        invoice_required: d("receiptMust").and_then(Value::as_str) == Some("YES"),
        invoice_issued: row.bill_no.is_some(),
        invoice_url: None,
    }
}

/// 查看发票（orderList `lookInvoice`）：HANGXIN → getEBillPicUrl，BOSI → getCouldEBillPicUrl；
/// body {billType, billNo, schoolCode}，data = URL
pub fn invoice_endpoint(bill_type: &str) -> Option<&'static str> {
    match bill_type {
        "HANGXIN" => Some("pay/pay/invoiced/getEBillPicUrl"),
        "BOSI" => Some("pay/pay/invoiced/getCouldEBillPicUrl"),
        _ => None,
    }
}

pub fn invoice_url(data: &Value) -> Option<String> {
    let u = data.as_str()?.trim();
    let ok = (u.starts_with("https://") || u.starts_with("http://"))
        && u.len() <= 2000
        && !u
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '<' | '>' | '"'));
    ok.then(|| u.to_string())
}

// ── 待缴费：GET pay/pay/waitingPay/getWaitingPayList/<schoolCode>/<idserial>/undefined ──
// PC「待缴费列表」与移动端 balanceInquiry 都不传项目，最后一段就是字面量 "undefined"。
// chunk-5dcd1bc5：projectName, batchno, payamt, unpaidamt, payLimit

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Pending {
    pub project: String,
    pub batch: String,
    pub due: Option<f64>,
    pub unpaid: Option<f64>,
    /// payLimit 为 -1 / 0 / null 时必须一次缴清（SPA `check()`）
    pub partial_allowed: bool,
}

pub fn pending(rows: &Value) -> Vec<Pending> {
    arr(rows)
        .iter()
        .map(|r| {
            let limit = clean(r.get("payLimit").unwrap_or(&Value::Null), 20);
            Pending {
                project: s(r, "projectName", 60),
                batch: s(r, "batchno", 60),
                due: fen_to_yuan(&r["payamt"]),
                unpaid: fen_to_yuan(&r["unpaidamt"]),
                partial_allowed: !limit.is_empty() && limit != "-1" && limit != "0",
            }
        })
        .collect()
}

// ── 支出汇总（站点自带的「缴费账单」统计接口对所有人回 -1 token 失效，只能本地算）──

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct Bucket {
    pub key: String,
    pub count: u32,
    pub total: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct Spending {
    pub orders: usize,
    pub paid_count: u32,
    pub paid_total: f64,
    pub pending: u32,
    pub by_project: Vec<Bucket>,
    pub by_month: Vec<Bucket>,
}

/// 计入：支付成功(103)、部分退款(107，扣除退款)；待支付/支付中计入 pending
pub fn spending(orders: &[Order]) -> Spending {
    let mut by_project: BTreeMap<String, Bucket> = BTreeMap::new();
    let mut by_month: BTreeMap<String, Bucket> = BTreeMap::new();
    let mut out = Spending {
        orders: orders.len(),
        ..Default::default()
    };
    for o in orders {
        match o.status_code.as_str() {
            "101" | "102" => out.pending += 1,
            "103" | "107" => {
                let amt = (o.amount.unwrap_or(0.0) - o.refund.unwrap_or(0.0)).max(0.0);
                out.paid_count += 1;
                out.paid_total = round2(out.paid_total + amt);
                let project = if o.project.is_empty() {
                    "未知项目".to_string()
                } else {
                    o.project.clone()
                };
                let month: String = o.created_at.chars().take(7).collect();
                for (map, key) in [(&mut by_project, project), (&mut by_month, month)] {
                    let b = map.entry(key.clone()).or_insert_with(|| Bucket {
                        key,
                        ..Default::default()
                    });
                    b.count += 1;
                    b.total = round2(b.total + amt);
                }
            }
            _ => {}
        }
    }
    out.by_project = by_project.into_values().collect();
    out.by_project.sort_by(|a, b| b.total.total_cmp(&a.total));
    out.by_month = by_month.into_values().collect();
    out
}

// ── 公告：POST common/com/notice/getStuNotice {schoolCode, dataSource} ─────────

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Notice {
    pub title: String,
    pub content: String,
    pub updated_at: String,
}

pub fn notice(data: &Value) -> Option<Notice> {
    let title = s(data, "title", 120);
    if title.is_empty() {
        return None;
    }
    let content = data
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .replace("<br>", "\n")
        .replace("<br/>", "\n")
        .replace("</p>", "\n");
    Some(Notice {
        title,
        content: clean(&Value::String(content), 1500),
        updated_at: s(data, "updateDate", 20),
    })
}

// ── 个人信息：POST pay/web/personalCenter/findUserInfoByAccountNum ─────────────

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Profile {
    pub account: String,
    pub name: String,
    pub idserial: String,
    pub dept: String,
}

pub fn profile(data: &Value) -> Profile {
    Profile {
        account: s(data, "accountNum", 20),
        name: s(data, "name", 40),
        idserial: s(data, "idserial", 20),
        dept: s(data, "deptName", 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Value {
        let text = match name {
            "catalog" => include_str!("../fixtures/catalog.json"),
            "tuition" => include_str!("../fixtures/tuition.json"),
            "orders" => include_str!("../fixtures/orders.json"),
            "order-detail" => include_str!("../fixtures/order-detail.json"),
            "pending" => include_str!("../fixtures/pending.json"),
            "notice" => include_str!("../fixtures/notice.json"),
            "dict" => include_str!("../fixtures/dict.json"),
            _ => unreachable!(),
        };
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn yuan_and_clean() {
        assert_eq!(fen_to_yuan(&Value::from(32050)), Some(320.5));
        assert_eq!(fen_to_yuan(&Value::from("100")), Some(1.0));
        assert_eq!(fen_to_yuan(&Value::from("")), None);
        assert_eq!(fen_to_yuan(&Value::Null), None);
        assert_eq!(clean(&Value::from("<b>已缴</b>\n  ok"), 20), "已缴 ok");
        assert_eq!(clean(&Value::from("一二三四五"), 3), "一二三");
    }

    #[test]
    fn catalog_open_suspended_amount() {
        let list = projects(&fixture("catalog")["data"]);
        let rows: Vec<_> = list
            .iter()
            .map(|p| {
                (
                    p.id.as_str(),
                    p.kind_label.as_str(),
                    p.open,
                    p.standard_amount,
                )
            })
            .collect();
        assert_eq!(
            rows,
            vec![
                ("c-tuition", "学费住宿费", false, None),
                ("c-bath", "浴卡充值", true, None),
                ("c-car", "其他缴费", true, Some(540.0)),
                ("c-copy", "现场/窗口收费", true, None),
            ]
        );
        assert!(list[0]
            .suspended_note
            .as_deref()
            .unwrap()
            .contains("临时关闭"));
        assert_eq!(
            list[1].suspended_note, None,
            "stale suspendTip on an open project is hidden"
        );
        assert_eq!(list[3].name, "图书馆复印打印费");
        assert_eq!(filter_projects(&list, Some("车证"), None, false).len(), 1);
        assert_eq!(
            filter_projects(&list, Some("BATH"), None, false)[0].id,
            "c-bath"
        );
        assert_eq!(
            filter_projects(&list, None, Some("scene"), false)[0].id,
            "c-copy"
        );
        assert_eq!(filter_projects(&list, None, None, true).len(), 3);
    }

    #[test]
    fn tuition_rows_and_no_arrears() {
        let t = tuition(&fixture("tuition")["data"]);
        assert_eq!(
            t.items[0],
            TuitionItem {
                year: "2026-2027".into(),
                item: "学费".into(),
                due: Some(5000.0),
                paid: Some(0.0),
                unpaid: Some(5000.0),
                note: String::new()
            }
        );
        assert_eq!(t.items[2].note, "已缴");
        assert_eq!((t.due, t.paid, t.unpaid), (11200.0, 6200.0, 5000.0));
        let none: Value =
            serde_json::from_str(r#"{"messageCode":"0001","message":"无欠费##","data":null}"#)
                .unwrap();
        assert!(is_no_arrears(&none));
        let other: Value =
            serde_json::from_str(r#"{"messageCode":"0001","message":"其他"}"#).unwrap();
        assert!(!is_no_arrears(&other));
        assert_eq!(tuition(&Value::Null).items.len(), 0);
    }

    #[test]
    fn orders_status_and_dict() {
        let names = status_dict(&fixture("dict")["data"]);
        assert_eq!(names.get("103").unwrap(), "支付成功");
        assert_eq!(names.get("999").unwrap(), "测试状态");
        let (total, list) = orders(&fixture("orders")["data"], &names);
        assert_eq!(total, 4);
        let rows: Vec<_> = list
            .iter()
            .map(|o| (o.project.as_str(), o.amount, o.status.as_str(), o.invoiced))
            .collect();
        assert_eq!(
            rows,
            vec![
                ("校园卡就诊账户充值", Some(100.0), "支付成功", false),
                ("浴卡充值", Some(50.0), "支付成功", true),
                ("学费住宿费", Some(1020.0), "已过期", false),
                ("网费充值", Some(30.0), "部分退款", false),
            ]
        );
        assert_eq!(list[3].refund, Some(10.0));
    }

    #[test]
    fn order_detail_merge() {
        let names = default_status_names();
        let (_, list) = orders(&fixture("orders")["data"], &names);
        let d = order_detail(&list[1], &fixture("order-detail")["data"], &names);
        assert_eq!(d.paid_by.as_deref(), Some("微信"));
        assert_eq!(d.closed_at, "2026-08-31 20:53:05");
        assert_eq!(d.payable, Some(50.0));
        assert!(d.invoice_required && d.invoice_issued);
        assert_eq!(
            invoice_endpoint(list[1].bill_type.as_deref().unwrap()),
            Some("pay/pay/invoiced/getEBillPicUrl")
        );
        assert_eq!(
            invoice_endpoint("BOSI"),
            Some("pay/pay/invoiced/getCouldEBillPicUrl")
        );
        assert_eq!(
            invoice_url(&Value::from("https://bill.example.com/e?id=1")).as_deref(),
            Some("https://bill.example.com/e?id=1")
        );
        assert_eq!(invoice_url(&Value::from("javascript:alert(1)")), None);
        assert_eq!(channel_name("99").as_deref(), Some("渠道 99"));
        assert_eq!(channel_name(""), None);
    }

    #[test]
    fn pending_rows() {
        let p = pending(&fixture("pending")["data"]);
        assert_eq!(
            p,
            vec![
                Pending {
                    project: "教材费".into(),
                    batch: "2026秋季教材".into(),
                    due: Some(320.5),
                    unpaid: Some(320.5),
                    partial_allowed: false
                },
                Pending {
                    project: "会议注册费".into(),
                    batch: "第一批".into(),
                    due: Some(1000.0),
                    unpaid: Some(400.0),
                    partial_allowed: true
                },
            ]
        );
    }

    #[test]
    fn spending_buckets() {
        let (_, list) = orders(&fixture("orders")["data"], &default_status_names());
        let s = spending(&list);
        assert_eq!(
            (s.orders, s.paid_count, s.paid_total, s.pending),
            (4, 3, 170.0, 0)
        );
        assert_eq!(
            s.by_project[0],
            Bucket {
                key: "校园卡就诊账户充值".into(),
                count: 1,
                total: 100.0
            }
        );
        let months: Vec<_> = s
            .by_month
            .iter()
            .map(|b| (b.key.as_str(), b.total))
            .collect();
        assert_eq!(months, vec![("2026-08", 50.0), ("2026-09", 120.0)]);
    }

    #[test]
    fn notice_and_profile() {
        let n = notice(&fixture("notice")["data"]).unwrap();
        assert_eq!(n.title, "系统维护通知");
        assert_eq!(n.content, "本周六 0:00-6:00 系统维护 期间暂停缴费");
        assert_eq!(notice(&Value::Null), None);
        let p = profile(
            &serde_json::json!({"accountNum": "2600000000", "name": "张三", "idserial": "2600000000", "deptName": "示例学院"}),
        );
        assert_eq!(p.dept, "示例学院");
    }
}
