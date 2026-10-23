//! Wingsoft "WF" 平台通用协议层
//!
//! 财务门户的每个菜单项都是一个"窗口"（winno），窗口里是若干"功能"（funcno）：
//! `F` 表单、`Q` 表格、`C` 自定义页、`WG` 组合窗口。一次查询的完整链路是：
//!
//! 1. `loadDefinition.action {projid, winno, type=W}` → 窗口定义（JS 对象字面量）。
//!    里面有表单字段、按钮绑定的存储过程引用 `` `$<md5>:#<func>-<field>#;#0-<var>#;… ``、
//!    字段联动函数表 `funcMap`、下拉框数据源 `bindMap`。哈希随会话变化，必须每次动态解析。
//! 2. `commonUpdate_responseButtonEvent.action {cmd, projid, userContext, async}`
//!    → 执行"查询"按钮绑定的存储过程。`cmd.data0` 按引用里的参数顺序传值；
//!    `userContext` 里需要带上表单字段（`WF_CWBS$<func>-<FIELD>`），服务端会把它存进会话，
//!    表格 SQL 里的 `#<func>-<field>#` 就从这里取值。
//! 3. （交叉表）`crossfieldrebuild.action` → 动态列；
//! 4. `commonQuery_doQuery.action {funcno, …}` → 表格数据。
//!
//! 所有请求体都要走 [`crate::encrypt`] 的混淆。本模块只放协议和纯解析函数，
//! 具体业务查询见 [`crate::queries`]。

use crate::{
    api::{decode_numeric_entities, DoQueryResp},
    client::CWFW_BASE,
    context::UserContext,
    encrypt,
};
use anyhow::{anyhow, Context, Result};
use regex::Regex;
use reqwest::Client;
use serde_json::{json, Value};

/// 统一的加密 POST：`a=<uid>` 放最前，其余字段按调用方顺序，所有 key/value 加密。
///
/// `path` 是子系统的 URL 路径（`WF_CWBS`、智能报销是 `YBX`）。
pub async fn post_encrypted(
    client: &Client,
    uid: &str,
    path: &str,
    action: &str,
    extra_pairs: &[(&str, &str)],
) -> Result<String> {
    let url = format!("{CWFW_BASE}/{path}/{action}");
    let mut pairs: Vec<(&str, &str)> = Vec::with_capacity(extra_pairs.len() + 1);
    pairs.push(("a", uid));
    pairs.extend_from_slice(extra_pairs);
    let body = encrypt::encrypt_form(pairs.iter().copied());

    let resp = client
        .post(&url)
        .header("x-requested-with", "XMLHttpRequest")
        .header(
            "content-type",
            "application/x-www-form-urlencoded; charset=UTF-8",
        )
        .header("accept", "application/json, text/javascript, */*; q=0.01")
        .header("origin", CWFW_BASE)
        .header("referer", format!("{CWFW_BASE}/{path}/main.jsp"))
        .body(body)
        .send()
        .await
        .with_context(|| format!("{action} 请求失败"))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .with_context(|| format!("读取 {action} 响应失败"))?;
    if !status.is_success() {
        return Err(anyhow!(
            "{action} HTTP {status}: {}",
            text.chars().take(300).collect::<String>()
        ));
    }
    Ok(text)
}

/// 某个子系统上的 WF 客户端
pub struct Wf {
    pub client: Client,
    pub uid: String,
    /// URL 路径，如 `WF_CWBS` / `YBX`
    pub path: String,
    /// projid，如 `WF_CWBS` / `WF_YBX`
    pub projid: String,
}

/// 存储过程实参
#[derive(Debug, Clone, PartialEq)]
pub struct ProcArg {
    pub name: String,
    pub value: String,
    pub ty: &'static str,
}

impl Wf {
    pub fn new(client: Client, uid: impl Into<String>, path: &str, projid: &str) -> Self {
        Self {
            client,
            uid: uid.into(),
            path: path.to_string(),
            projid: projid.to_string(),
        }
    }

    pub async fn post(&self, action: &str, pairs: &[(&str, &str)]) -> Result<String> {
        post_encrypted(&self.client, &self.uid, &self.path, action, pairs).await
    }

    /// 加载窗口定义
    pub async fn load_window(&self, winno: &str) -> Result<String> {
        self.post(
            "loadDefinition.action",
            &[("projid", &self.projid), ("winno", winno), ("type", "W")],
        )
        .await
    }

    /// 下拉框数据源：返回 `value:label;value:label`
    pub async fn binding(&self, sqlid: &str, cvstr: &str) -> Result<String> {
        self.post(
            "common_getBindingDataBackend.action",
            &[("sqlid", sqlid), ("cvstr", cvstr), ("projid", &self.projid)],
        )
        .await
    }

    /// 字段联动函数（`funcMap`）：服务端算出某个字段的值
    pub async fn check(&self, hash: &str, data: &[ProcArg], ctx: &UserContext) -> Result<String> {
        let cmd = wf_cmd(json!({ "check_func": hash, "check_data": args_json(data) }));
        let ctx_str = ctx.to_json_string();
        let text = self
            .post(
                "commonUpdate_responseButtonEvent.action",
                &[
                    ("cmd", &cmd),
                    ("projid", &self.projid),
                    ("userContext", &ctx_str),
                    ("async", "false"),
                ],
            )
            .await?;
        Ok(text.trim().to_string())
    }

    /// 执行按钮绑定的存储过程，可选带前置校验函数（check_func）。
    ///
    /// 服务端成功返回 `pass`（或 `success: …` / `pass;local(…)`），否则返回提示文字。
    pub async fn button(
        &self,
        proc_hash: &str,
        data0: &[ProcArg],
        check: Option<(&str, &[ProcArg])>,
        ctx: &UserContext,
    ) -> Result<String> {
        let mut obj = json!({ "check_func": "escape" });
        if let Some((hash, data)) = check {
            obj["check_func"] = Value::String(hash.to_string());
            obj["check_data"] = args_json(data);
        }
        obj["proc0"] = Value::String(proc_hash.to_string());
        obj["data0"] = args_json(data0);
        let cmd = wf_cmd(obj);
        let ctx_str = ctx.to_json_string();
        let text = self
            .post(
                "commonUpdate_responseButtonEvent.action",
                &[
                    ("cmd", &cmd),
                    ("projid", &self.projid),
                    ("userContext", &ctx_str),
                    ("async", "false"),
                ],
            )
            .await?;
        let text = text.trim().to_string();
        if !button_ok(&text) {
            return Err(anyhow!("财务门户拒绝了查询：{}", short(&text)));
        }
        Ok(text)
    }

    /// 交叉表的动态列：返回 (字段名, 列标题)，顺序与 doQuery 的 cell 顺序一致
    pub async fn cross_fields(
        &self,
        funcno: &str,
        ctx: &UserContext,
    ) -> Result<Vec<(String, String)>> {
        let ctx_str = ctx.to_json_string();
        let text = self
            .post(
                "crossfieldrebuild.action",
                &[
                    ("projId", &self.projid),
                    ("funcno", funcno),
                    ("dynColSwitch", "true"),
                    ("data", &ctx_str),
                ],
            )
            .await?;
        parse_cross_fields(&text)
    }

    /// 取表格数据。`user_data` 为 None 时只发最小的用户标识。
    pub async fn do_query(
        &self,
        funcno: &str,
        sidx: &str,
        rows: u32,
        user_data: Option<&UserContext>,
    ) -> Result<DoQueryResp> {
        let user_data = match user_data {
            Some(c) => c.to_json_string(),
            None => json!({ "0-USERINFO.USERID": self.uid, "0-UNINO": self.uid }).to_string(),
        };
        let nd = chrono::Utc::now().timestamp_millis().to_string();
        let rows = rows.to_string();
        let text = self
            .post(
                "commonQuery_doQuery.action",
                &[
                    ("_search", "false"),
                    ("nd", &nd),
                    ("rows", &rows),
                    ("page", "1"),
                    ("sidx", sidx),
                    ("sord", ""),
                    ("userData", &user_data),
                    ("itemArr", ""),
                    ("sectionArr", ""),
                    ("funcno", funcno),
                    ("projid", &self.projid),
                    ("needcount", "true"),
                ],
            )
            .await?;
        DoQueryResp::parse(&text)
    }
}

fn wf_cmd(mut obj: Value) -> String {
    for k in [
        "wfName",
        "wfAct",
        "wfNodeId",
        "wfState",
        "wfUniKey",
        "wfVars",
        "wfKeyVal",
        "wfComment",
        "wfCollab",
        "wfPdf",
    ] {
        obj[k] = Value::String(String::new());
    }
    obj.to_string()
}

fn args_json(args: &[ProcArg]) -> Value {
    Value::Array(
        args.iter()
            .map(|a| json!({ "name": a.name, "value": a.value, "type": a.ty }))
            .collect(),
    )
}

fn short(s: &str) -> String {
    let s = decode_numeric_entities(s);
    let s: String = s.chars().take(120).collect();
    s.replace('\n', " ")
}

/// 按钮执行结果是否算成功
pub fn button_ok(text: &str) -> bool {
    let t = text.trim();
    t.eq_ignore_ascii_case("pass")
        || t.starts_with("pass;")
        || t.starts_with("success")
        || t.starts_with("[{")
}

// ───────────────────────── 纯解析函数 ─────────────────────────

/// 存储过程引用：`` `$<hash> `` + 参数列表（`#5574-d.year#`、`#0-userinfo.userid#`，空参数为 ""）
#[derive(Debug, Clone, PartialEq)]
pub struct ProcRef {
    pub hash: String,
    pub params: Vec<String>,
}

fn proc_ref(hash: &str, params: &str) -> ProcRef {
    ProcRef {
        hash: hash.to_string(),
        params: params
            .split(';')
            .map(|p| p.trim_matches('#').to_string())
            .collect(),
    }
}

/// 列出窗口定义里按钮绑定的所有存储过程（`procs:[…]`）
pub fn procs(def: &str) -> Vec<ProcRef> {
    let list = Regex::new(r"procs:\[([^\]]*)\]").expect("静态正则");
    let item = Regex::new(r#""(`\$[a-f0-9]{32}):([^"]*)""#).expect("静态正则");
    list.captures_iter(def)
        .flat_map(|l| {
            item.captures_iter(&l[1])
                .map(|c| proc_ref(&c[1], &c[2]))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// 找到第一个参数为 `first` 的存储过程（大小写不敏感），如 `5574-d.year`
pub fn find_proc(def: &str, first: &str) -> Option<ProcRef> {
    procs(def).into_iter().find(|p| {
        p.params
            .first()
            .is_some_and(|f| f.eq_ignore_ascii_case(first))
    })
}

/// 按钮的前置校验函数（`check_func:"`$hash:…"`），第一个参数为 `first`
pub fn find_check(def: &str, first: &str) -> Option<ProcRef> {
    let re = Regex::new(r#"check_func:"(`\$[a-f0-9]{32}):([^"]*)""#).expect("静态正则");
    let found = re
        .captures_iter(def)
        .map(|c| proc_ref(&c[1], &c[2]))
        .find(|p| {
            p.params
                .first()
                .is_some_and(|f| f.eq_ignore_ascii_case(first))
        });
    found
}

/// 窗口里 `beforeLoadProcs` 的第一个存储过程
pub fn before_load_proc(def: &str) -> Option<ProcRef> {
    let re = Regex::new(r#"beforeLoadProcs:\["(`\$[a-f0-9]{32}):([^"]*)""#).expect("静态正则");
    re.captures(def).map(|c| proc_ref(&c[1], &c[2]))
}

/// 从 `funcMap` / `bindMap` 中取某个字段的条目，如
/// `"d.code":"`$f930…|#5574-d.incometype#,#5574-d.funcno#"` → (hash, ["5574-d.incometype", "5574-d.funcno"])。
///
/// 返回的 hash 保留原样前缀（funcMap 为 `` `$ ``，bindMap 为 `@$`，静态选项则是 `0:a;1:b` 原文、参数为空）。
pub fn map_entry(def: &str, map: &str, field: &str) -> Option<(String, Vec<String>)> {
    let block = Regex::new(&format!(r#"{}:\{{([^}}]*)\}}"#, regex::escape(map))).expect("静态正则");
    let item = Regex::new(&format!(r#""{}":"([^"]*)""#, regex::escape(field))).expect("静态正则");
    for b in block.captures_iter(def) {
        if let Some(c) = item.captures(&b[1]) {
            let raw = c[1].to_string();
            let (head, params) = match raw.split_once('|') {
                Some((h, p)) => (
                    h.to_string(),
                    p.split(',')
                        .map(|x| x.trim().trim_matches('#').to_string())
                        .filter(|x| !x.is_empty())
                        .collect(),
                ),
                None => (raw, Vec::new()),
            };
            return Some((head, params));
        }
    }
    None
}

/// `value:label;value:label` → [(value, label)]
pub fn parse_options(s: &str) -> Vec<(String, String)> {
    s.split(';')
        .filter_map(|kv| {
            let (k, v) = kv.split_once(':')?;
            let k = k.trim();
            (!k.is_empty()).then(|| (k.to_string(), v.trim().to_string()))
        })
        .collect()
}

/// `pass;local(0-a:1;0-b:x;)` → [(a, 1), (b, x)]（去掉 `0-` 前缀）
pub fn parse_local(s: &str) -> Vec<(String, String)> {
    let Some(start) = s.find("local(") else {
        return Vec::new();
    };
    let inner = &s[start + 6..];
    let inner = inner.strip_suffix(')').unwrap_or(inner);
    let inner = inner.rfind(')').map_or(inner, |i| &inner[..i]);
    inner
        .split(';')
        .filter_map(|kv| {
            let (k, v) = kv.split_once(':')?;
            let k = k.trim();
            let k = k.strip_prefix("0-").unwrap_or(k);
            (!k.is_empty()).then(|| (k.to_string(), v.trim().to_string()))
        })
        .collect()
}

/// `crossfieldrebuild.action` 响应 → (字段, 标题)。普通字段（`qq.ym`）的标题为空。
pub fn parse_cross_fields(text: &str) -> Result<Vec<(String, String)>> {
    let v: Value = serde_json::from_str(text.trim()).context("解析动态列失败")?;
    let fields = v["prjfields"]
        .as_str()
        .ok_or_else(|| anyhow!("动态列响应缺少 prjfields"))?;
    let mut names = std::collections::HashMap::new();
    if let Some(models) = v["colModel"].as_object() {
        for m in models.values() {
            if let Some(cols) = m["data"].as_array() {
                for c in cols {
                    if let (Some(k), Some(n)) = (c["colName"].as_str(), c["name"].as_str()) {
                        names.insert(k.to_string(), n.to_string());
                    }
                }
            }
        }
    }
    Ok(fields
        .split(',')
        .map(|f| {
            let f = f.trim().to_string();
            let n = names.get(&f).cloned().unwrap_or_default();
            (f, n)
        })
        .collect())
}

/// 菜单项
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct MenuEntry {
    pub title: String,
    pub winno: String,
    /// 所在分组，如 `网上查询-收入查询`
    pub group: String,
    pub node_id: String,
}

/// `loadMenu.action` / `loadRolesMenu.action` 的菜单 HTML →
/// `globalMenuEvent')('<标题>','<winno>','<分组>')` … `setData('currMenuNodeId',<id>)`
pub fn parse_menu(html: &str) -> Vec<MenuEntry> {
    let re = Regex::new(
        r"globalMenuEvent'\)\('([^']*)','(\d+)','([^']*)'\).*?setData\('currMenuNodeId',(\d+)\)",
    )
    .expect("静态正则");
    re.captures_iter(html)
        .map(|c| MenuEntry {
            title: c[1].to_string(),
            winno: c[2].to_string(),
            group: c[3].to_string(),
            node_id: c[4].to_string(),
        })
        .collect()
}

/// 把存储过程参数映射成实参。
///
/// - `<func>-<field>`：取表单值（`form`，字段名大小写不敏感）
/// - `0-<var>`：取 userContext 的 `0-<VAR>`
/// - 空参数（`;;`）：空名空值
pub fn build_args(params: &[String], form: &[(&str, String)], ctx: &UserContext) -> Vec<ProcArg> {
    params
        .iter()
        .map(|p| {
            if p.is_empty() {
                return ProcArg {
                    name: String::new(),
                    value: String::new(),
                    ty: "CHAR",
                };
            }
            let (scope, name) = p.split_once('-').unwrap_or(("0", p.as_str()));
            let value = if scope == "0" {
                ctx.get_str(&format!("0-{}", name.to_uppercase()))
            } else {
                form.iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(name))
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default()
            };
            // 前端对全大写的系统变量用 "char"，其他用 "CHAR"（与抓包一致）
            let ty = if scope == "0" && name.chars().all(|c| !c.is_lowercase()) {
                "char"
            } else {
                "CHAR"
            };
            ProcArg {
                name: name.to_string(),
                value,
                ty,
            }
        })
        .collect()
}

/// 把表单字段写进 userContext：`<projid>$<func>-<FIELD>`
pub fn seed_form(ctx: &mut UserContext, projid: &str, func: &str, form: &[(&str, String)]) {
    for (k, v) in form {
        ctx.set(
            format!("{projid}${func}-{}", k.to_uppercase()),
            Value::String(v.clone()),
        );
    }
    ctx.set("0-CURRFUNC".into(), json!(func.parse::<u64>().unwrap_or(0)));
    ctx.set("0-CURRBTN".into(), Value::String("查询".into()));
    ctx.set("0-CURRFUNCTYPE".into(), Value::String("3".into()));
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEF: &str = r#"{mainWinno:5609,wins:[{winno:5609,funcno:5574,func_btns:[{caption:"查询",procs:["`$238b97ccfadb6466f0e40e90e17763c9:#5574-d.year#;#5574-d.pubdate#;#5574-d.month_2#;#5574-d.month_1#;#5574-d.incometype#;#5574-d.funcno#;#5574-d.dbcode#;#5574-d.code#;#0-SHOWZEROGZ#;#0-userinfo.userid#;#0-currfunctype#"],affects:"1==1:6240^"}],beforeLoadProcs:[]},{winno:6240,funcno:6160,func_btns:[],beforeLoadProcs:["`$9baa856ef90181037dc0520df583f1a3:#0-userinfo.userid#"]}],funcMap:{5609:{func:5574,type:"F",url:"",title:"个人工资查询"},6240:{func:6160,type:"Q"}},funcs:[{funcno:"5574",typeMap:{"d.funcno":"CHAR"},funcMap:{"d.funcno":"","d.incometype":"`$25b8177ea318643bd726a5a11dc176fc|#5574-d.funcno#","d.code":"`$f930ffe8da00b8be0eb90ea0ce374454|#5574-d.incometype#,#5574-d.funcno#"},bindMap:{"d.funcno":"@$5e59f45e66d097068bc3db16762ea3f6|#0-USERINFO.USERID#","d.stype":"0:学生发放;2:奖助学金"}}]}"#;

    #[test]
    fn finds_procs() {
        let p = find_proc(DEF, "5574-d.year").expect("proc");
        assert_eq!(p.hash, "`$238b97ccfadb6466f0e40e90e17763c9");
        assert_eq!(p.params.len(), 11);
        assert_eq!(p.params[8], "0-SHOWZEROGZ");
        let b = before_load_proc(DEF).expect("before");
        assert_eq!(b.hash, "`$9baa856ef90181037dc0520df583f1a3");
        assert!(find_proc(DEF, "9999-x").is_none());
        // beforeLoadProcs 不算按钮存储过程
        assert!(find_proc(DEF, "0-userinfo.userid").is_none());
    }

    #[test]
    fn finds_check_func() {
        let def = r#"{func_btns:[{caption:"查询",check_func:"`$1d919a3aa838816987db6d806443dbb0:#10819-t.year2#;#10819-t.funcno#",procs:["`$09c151a719811efa039315b40bc840a0:#10819-t.year2#;#0-userinfo.userid#"]}]}"#;
        let c = find_check(def, "10819-t.year2").expect("check");
        assert_eq!(c.hash, "`$1d919a3aa838816987db6d806443dbb0");
        assert_eq!(c.params[1], "10819-t.funcno");
        let p = find_proc(def, "10819-t.year2").expect("proc");
        assert_eq!(p.hash, "`$09c151a719811efa039315b40bc840a0");
    }

    #[test]
    fn map_entries() {
        let (h, p) = map_entry(DEF, "funcMap", "d.code").expect("code");
        assert_eq!(h, "`$f930ffe8da00b8be0eb90ea0ce374454");
        assert_eq!(p, vec!["5574-d.incometype", "5574-d.funcno"]);
        let (h, p) = map_entry(DEF, "bindMap", "d.funcno").expect("bind");
        assert_eq!(h, "@$5e59f45e66d097068bc3db16762ea3f6");
        assert_eq!(p, vec!["0-USERINFO.USERID"]);
        let (h, p) = map_entry(DEF, "bindMap", "d.stype").expect("static");
        assert_eq!(parse_options(&h).len(), 2);
        assert!(p.is_empty());
        assert!(map_entry(DEF, "funcMap", "d.nope").is_none());
    }

    #[test]
    fn args_from_form_and_ctx() {
        let mut ctx = UserContext::new();
        ctx.set("0-USERINFO.USERID".into(), json!("2000000000"));
        ctx.set("0-SHOWZEROGZ".into(), json!("N"));
        let form = [
            ("d.year", "2026".to_string()),
            ("d.funcno", "6".to_string()),
        ];
        let params: Vec<String> = [
            "5574-d.year",
            "5574-d.funcno",
            "0-SHOWZEROGZ",
            "0-userinfo.userid",
            "",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let a = build_args(&params, &form, &ctx);
        assert_eq!(
            a[0],
            ProcArg {
                name: "d.year".into(),
                value: "2026".into(),
                ty: "CHAR"
            }
        );
        assert_eq!(a[1].value, "6");
        assert_eq!(
            (a[2].name.as_str(), a[2].value.as_str(), a[2].ty),
            ("SHOWZEROGZ", "N", "char")
        );
        assert_eq!((a[3].value.as_str(), a[3].ty), ("2000000000", "CHAR"));
        assert_eq!(a[4].name, "");
    }

    #[test]
    fn seeds_form() {
        let mut ctx = UserContext::new();
        seed_form(
            &mut ctx,
            "WF_CWBS",
            "13562",
            &[("t.start", "2026-01-01".to_string())],
        );
        assert_eq!(ctx.get_str("WF_CWBS$13562-T.START"), "2026-01-01");
        assert_eq!(ctx.get_str("0-CURRFUNC"), "13562");
    }

    #[test]
    fn local_vars() {
        let v = parse_local("pass;local(0-BX_YBXALL_count:3;0-BX_YBXALL_amt:120.5;0-tks_count:0;)");
        assert_eq!(v[0], ("BX_YBXALL_count".to_string(), "3".to_string()));
        assert_eq!(v[1].1, "120.5");
        assert_eq!(v.len(), 3);
        assert!(parse_local("pass").is_empty());
    }

    #[test]
    fn cross_fields() {
        let raw = r#"{"prjfields":"qq.ym,mon3,mon4,sum,qq.ord","colModel":{"qq.dtl":{"index":1,"data":[{"colName":"mon3","name":"3月"},{"colName":"mon4","name":"4月"},{"colName":"sum","name":"小计"}]}}}"#;
        let f = parse_cross_fields(raw).expect("parse");
        assert_eq!(f[0], ("qq.ym".to_string(), String::new()));
        assert_eq!(f[1], ("mon3".to_string(), "3月".to_string()));
        assert_eq!(f[3].1, "小计");
    }

    #[test]
    fn menu() {
        let html = r#"<li><a onclick="if(typeof(globalMenuEvent) == 'function'){var result = eval('globalMenuEvent')('个人工资查询','6941','网上查询-收入查询');if(!result){return;}};$.userContext.setData('currMenuNodeId',219);$.page.openExternal('WF_CWBS','6941','个人工资查询')"></a></li><li><a onclick="eval('globalMenuEvent')('汇率查询','7700','网上查询');$.userContext.setData('currMenuNodeId',253);"></a></li>"#;
        let m = parse_menu(html);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].title, "个人工资查询");
        assert_eq!(m[0].winno, "6941");
        assert_eq!(m[0].group, "网上查询-收入查询");
        assert_eq!(m[1].node_id, "253");
    }

    #[test]
    fn button_results() {
        assert!(button_ok("pass"));
        assert!(button_ok("success: 执行成功."));
        assert!(button_ok("pass;local(0-a:1;)"));
        assert!(!button_ok("金额范围必填"));
    }
}
