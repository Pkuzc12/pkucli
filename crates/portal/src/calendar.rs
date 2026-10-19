//! 校历查询
//!
//! 数据源：<https://simso.pku.edu.cn/pages/ccSchoolCalendar.html>
//!
//! 这是一个 Vue 2 SPA，校历内容被编译到带哈希的 JS bundle 中。每次发布哈希会变。
//! 2026 年起 simso 从 webpack 换成了 Vite 构建，两种形态都要支持：
//!
//! | | webpack（旧） | Vite（2026 起） |
//! |---|---|---|
//! | bundle | `js/ccSchoolCalendar.<hex>.js` | `../js/ccSchoolCalendar.<MixedCase-_>.js`（另有 `-legacy` 副本，跳过） |
//! | tab | `{attrs:{label:"2025-2026学年",name:"2526"}}` | `label:"2026-2027学年",name:"2627"` |
//! | 装配 | `<mod>=(…),Object(o["a"])(<data>,<render>,…)` | `<mod>=<f>(<data>,<render>,…)`（normalizeComponent，任意短名） |
//! | render | `<r>=function(){var t=this,e=t.$createElement…` | `<r>=function(){var e=this,t=e._self._c;…` |
//! | 分支 | `"第一学期"===t.xq?[` | `e.xq==="第一学期"?[`，之后还有 `e.xq==="上课时间"?[` |
//!
//! 流程：
//! 1. GET 页面 HTML，正则提取 bundle 文件名；
//! 2. GET 该 JS。从 tab label 取得学年与 pane name，再从 Home 组件注册
//!    `components:{Calendar<tag>:<var>}` 拿到该 tab 绑定的导出变量名；
//! 3. 顺装配链反查 render 函数：`<var>=<mod>.exports` → `<mod>=…(<data>,<render>,…)` →
//!    `<render>=function(){var <self>=this…`。**真正的 `<self>._v("…")` 文字节点都在
//!    render 函数里**，组件 data 块只放 props/methods。simso 的开发者经常原地复用老
//!    .vue 文件、不改 `name:`，所以不能按内部 `name:"Calendar…"` 匹配；
//! 4. render 函数体内按学期三元分支切出上 / 下学期段（“上课时间”分支结束第二段），
//!    再各自按源码顺序抽 `<self>._v("…")` 文字节点和 `label:"…"` 小节标题
//!    （新版把“六、上课”放在 `attrs:{label:…}`，日期在下一个文字节点）。

use crate::client::{self, SIMSO_BASE};
use anyhow::{anyhow, Context, Result};
use colored::Colorize;
use regex::Regex;
use std::collections::BTreeSet;

const ENTRY_PATH: &str = "/pages/ccSchoolCalendar.html";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Semester {
    First,
    Second,
    All,
}

impl Semester {
    pub fn parse(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "first" | "1" | "一" | "上" | "第一学期" => Ok(Self::First),
            "second" | "2" | "二" | "下" | "第二学期" => Ok(Self::Second),
            "all" | "both" => Ok(Self::All),
            other => Err(anyhow!(
                "学期参数无效：{other}（可选 first/second/all 或 1/2/上/下）"
            )),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::First => "第一学期",
            Self::Second => "第二学期",
            Self::All => "全部",
        }
    }
}

pub struct Calendar {
    pub year: String,
    pub first_semester: Vec<String>,
    pub second_semester: Vec<String>,
}

pub async fn fetch() -> Result<Vec<Calendar>> {
    let client = client::build_simple()?;

    let html = client
        .get(format!("{SIMSO_BASE}{ENTRY_PATH}"))
        .send()
        .await
        .context("访问 simso 校历页面失败")?
        .text()
        .await?;

    let js_rel = bundle_path(&html)
        .ok_or_else(|| anyhow!("未在页面中找到校历 JS bundle 文件名，simso 可能已改版"))?;

    // 页面引用 `../js/…`（Vite）或 `js/…`（webpack），都在站点根下
    let js_url = format!("{SIMSO_BASE}/{js_rel}");
    let js = client
        .get(&js_url)
        .send()
        .await
        .context("下载校历 JS bundle 失败")?
        .text()
        .await?;

    parse_bundle(&js)
}

/// 标识符
const ID: &str = r"[A-Za-z_$][A-Za-z0-9_$]*";
/// 标识符左边界：`\b` 不把 `$` 当作单词字符
const B: &str = r"(?:^|[^A-Za-z0-9_$])";

/// 从入口页 HTML 提取 bundle 相对路径 `js/ccSchoolCalendar.<hash>.js`。
///
/// webpack 用十六进制哈希；Vite 用大小写混合并含 `-`/`_` 的哈希。`-legacy` 副本
/// 因名字后不是紧跟 `.` 而被跳过。
fn bundle_path(html: &str) -> Option<String> {
    Regex::new(r"(js/ccSchoolCalendar\.[A-Za-z0-9_-]+\.js)")
        .expect("static regex")
        .captures(html)
        .map(|c| c[1].to_string())
}

/// 学期分支，两种构建：`"第一学期"===t.xq?[`（webpack）或 `e.xq==="第一学期"?[`（Vite）。
fn branch(sem: &str) -> Regex {
    let sem = regex::escape(sem);
    Regex::new(&format!(r#"(?:"{sem}"==={ID}\.xq|{ID}\.xq==="{sem}")\?\["#)).expect("branch regex")
}

fn parse_bundle(js: &str) -> Result<Vec<Calendar>> {
    // tab pane: 学年 + 短 tag（如 2025-2026 / "2526"），按学年排序去重
    let tab_re = Regex::new(r#"label:"(\d{4}-\d{4})学年",name:"(\d+)""#)?;
    let tabs: BTreeSet<(String, String)> = tab_re
        .captures_iter(js)
        .map(|c| (c[1].to_string(), c[2].to_string()))
        .collect();
    if tabs.is_empty() {
        return Err(anyhow!("JS bundle 中没找到学年 tab，simso 可能已改版"));
    }

    let s1_re = branch("第一学期");
    let s2_re = branch("第二学期");
    // Vite 版在两个学期后还有“上课时间”tab，它结束第二学期段
    let s3_re = branch("上课时间");

    let mut out = Vec::new();
    for (year, tag) in &tabs {
        let Some((seg_start, seg_end)) = locate_render(js, tag)? else {
            continue;
        };
        let segment = &js[seg_start..seg_end];

        let s1 = s1_re.find(segment).map(|m| m.end());
        let s2 = s2_re.find(segment).map(|m| m.end());
        let s3 = s3_re.find(segment).map(|m| m.end());

        let first_slice = s1.map(|s1| match s2 {
            Some(s2) if s2 > s1 => &segment[s1..s2],
            _ => &segment[s1..],
        });
        let second_slice = s2.map(|s2| match s3 {
            Some(s3) if s3 > s2 => &segment[s2..s3],
            _ => &segment[s2..],
        });

        let first = first_slice.map(extract_text).unwrap_or_default();
        let second = second_slice.map(extract_text).unwrap_or_default();

        if first.is_empty() && second.is_empty() {
            continue;
        }

        out.push(Calendar {
            year: year.clone(),
            first_semester: first,
            second_semester: second,
        });
    }

    if out.is_empty() {
        return Err(anyhow!(
            "解析 bundle 后没拿到任何学年内容，simso 可能已改版"
        ));
    }
    Ok(out)
}

/// 根据 tab 短 tag 定位实际的 **render 函数体** `[start, end)`。
///
/// 链路：Home `Calendar<tag>:<var>` → `<var>=<mod>.exports` →
/// `<mod>=…(<data>,<render>,…)` → `<render>=function(){var <self>=this…`。
fn locate_render(js: &str, tag: &str) -> Result<Option<(usize, usize)>> {
    let first_cap = |pattern: String, group: usize| -> Result<Option<String>> {
        Ok(Regex::new(&pattern)?
            .captures(js)
            .and_then(|c| c.get(group))
            .map(|m| m.as_str().to_string()))
    };

    // 1. Home: `Calendar<tag>:<var>`
    let Some(export_var) = first_cap(format!(r"Calendar{}:({ID})", regex::escape(tag)), 1)? else {
        return Ok(None);
    };

    // 2. `<export_var>=<mod>.exports`
    let Some(mod_var) = first_cap(
        format!(
            r"{B}{}=({ID})\.exports(?:[^A-Za-z0-9_$]|$)",
            regex::escape(&export_var)
        ),
        1,
    )?
    else {
        return Ok(None);
    };

    // 3. 装配调用，第 2 个参数是 render 函数
    let m = regex::escape(&mod_var);
    let candidates = [
        // webpack: `<mod>=(i("…"),…),Object(o["a"])(<data>,<render>,…)`
        format!(r#"{B}{m}=\([^)]*\),?\s*Object\(o\["a"\]\)\(\s*({ID})\s*,\s*({ID})\s*,"#),
        // webpack 退路：不带前置逗号表达式
        format!(r#"{B}{m}=Object\(o\["a"\]\)\(\s*({ID})\s*,\s*({ID})\s*,"#),
        // Vite: normalizeComponent 任意短名，`k=s(u,h,f,!1,null,"<scope>")`
        format!(r"{B}{m}={ID}\(\s*({ID})\s*,\s*({ID})\s*,"),
    ];
    let mut render_var = None;
    for pattern in candidates {
        if let Some(v) = first_cap(pattern, 2)? {
            render_var = Some(v);
            break;
        }
    }
    let Some(render_var) = render_var else {
        return Ok(None);
    };

    // 4. `<render>=function(){var t=this,…`（webpack）/ `var e=this,t=e._self._c`（Vite）
    let start_re = Regex::new(&format!(
        r"{B}({}=function\(\)\{{var {ID}=this)",
        regex::escape(&render_var)
    ))?;
    let Some(head) = start_re.captures(js).and_then(|c| c.get(1)) else {
        return Ok(None);
    };
    let start = head.start();

    // 5. 终点：render 头部之后的下一个 render 函数定义（任何变量名）或 EOF。
    //    从头部末尾开始搜，避免多字符变量名（如 `_x`）的尾部被当成下一个定义。
    let next_re = Regex::new(&format!(r"{B}({ID}=function\(\)\{{var {ID}=this)"))?;
    let end = next_re
        .captures_at(js, head.end())
        .and_then(|c| c.get(1))
        .map(|m| m.start())
        .unwrap_or(js.len());
    Ok(Some((start, end)))
}

/// 按源码顺序抽取文字节点 `<self>._v("…")` 与小节标题 `label:"…"`：
/// 反转义、去首尾空白、丢弃空串、合并相邻重复。
fn extract_text(s: &str) -> Vec<String> {
    let re = Regex::new(r#"(?:[A-Za-z_$][A-Za-z0-9_$]*\._v\(|label:)"((?:\\.|[^"\\])*)""#)
        .expect("static regex");
    let mut out: Vec<String> = re
        .captures_iter(s)
        .map(|c| unescape(&c[1]).trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    out.dedup();
    out
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let Some(next) = chars.next() else {
            out.push('\\');
            break;
        };
        match next {
            'n' => out.push('\n'),
            't' => out.push('\t'),
            '"' | '\\' | '/' | '\'' => out.push(next),
            'u' => {
                let hex: String = chars.clone().take(4).collect();
                match (hex.len() == 4)
                    .then(|| u32::from_str_radix(&hex, 16).ok())
                    .flatten()
                    .and_then(char::from_u32)
                {
                    Some(ch) => {
                        out.push(ch);
                        for _ in 0..4 {
                            chars.next();
                        }
                    }
                    None => out.push_str("\\u"),
                }
            }
            _ => {
                out.push('\\');
                out.push(next);
            }
        }
    }
    out
}

pub fn render(calendars: &[Calendar], year_filter: Option<&str>, semester: Semester) {
    let mut matched = false;
    for cal in calendars {
        if let Some(y) = year_filter {
            if !cal.year.contains(y) {
                continue;
            }
        }
        matched = true;
        println!(
            "{} {} {}",
            "==".cyan(),
            format!("{} 学年校历", cal.year).bold(),
            format!("[{}]", semester.label()).dimmed()
        );
        println!();
        if matches!(semester, Semester::First | Semester::All) {
            print_semester("第一学期", &cal.first_semester);
        }
        if matches!(semester, Semester::Second | Semester::All) {
            print_semester("第二学期", &cal.second_semester);
        }
    }
    if !matched {
        eprintln!(
            "{}",
            "未找到匹配的学年，可用 `portal calendar` 看全部学年".yellow()
        );
    }
}

fn print_semester(title: &str, lines: &[String]) {
    println!("  {}", title.bold().yellow());
    if lines.is_empty() {
        println!("    {}", "(无内容)".dimmed());
    } else {
        for line in lines {
            println!("    {line}");
        }
    }
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// webpack 形态：Home 注册、`Object(o["a"])` 装配、`"第一学期"===t.xq?[` 分支。
    /// 2526 tab 指向一个仍*叫* "Calendar2425" 的数据块 —— 必须忽略 name。
    const WEBPACK: &str = concat!(
        r#"var Home={name:"Home",components:{Calendar2425:p,Calendar2526:g},render:function(){var t=this;return t._c("el-tabs",[t._c("el-tab-pane",{attrs:{label:"2024-2025学年",name:"2425"}}),t._c("el-tab-pane",{attrs:{label:"2025-2026学年",name:"2526"}})])}};"#,
        r#"a={name:"Calendar2425",props:["xq"]},"#,
        r#"r=function(){var t=this,e=t.$createElement,n=t._self._c||e;return n("div",["第一学期"===t.xq?[n("p",[t._v("2024年9月9日（星期一）开始上课")]),n("p",[t._v("旧学年")])]:t._e(),"第二学期"===t.xq?[n("p",[t._v("2025年2月17日开始上课")])]:t._e()],2)},"#,
        r#"u=(i("x1"),Object(o["a"])(a,r,[],!1,null,null,null)),p=u.exports,"#,
        r#"c={name:"Calendar2425",props:["xq"],methods:{}},"#,
        r#"_x=function(){var t=this,e=t.$createElement,n=t._self._c||e;return n("div",["#,
        r#""第一学期"===t.xq?[n("p",[t._v("9月8日（星期一）开始上课")]),n("p",[t._v("国庆节放假：10月1日至8日")]),n("p",[t._v("国庆节放假：10月1日至8日")]),n("p",[t._v("  ")])]:t._e(),"#,
        r#""第二学期"===t.xq?[n("p",[t._v("2026年2月23日开始上课")]),n("p",[t._v("说明：\"校历\"以第教务部通知为准")])]:t._e()"#,
        r#"],2)},"#,
        r#"m=(i("x2"),Object(o["a"])(c,_x,[],!1,null,"abc",null)),g=m.exports,"#,
        r#"zz=function(){var t=this,e=t.$createElement;return t._v("不属于校历")};"#,
    );

    /// Vite 形态（simso 2026-10 实际 bundle 的结构）。
    const VITE: &str = concat!(
        r#"label:"2026-2027学年",name:"2627""#,
        r#"components:{Calendar2627:S}"#,
        r#"h=function(){var e=this,t=e._self._c;return t("div",[e.xq==="第一学期"?[t("el-descriptions-item",{attrs:{label:"六、上课"}},[t("li",[e._v("校本部：9月7日")]),t("li",[e._v("医学部：")])]),"#,
        r#"t("el-descriptions-item",{attrs:{label:"十二、停课复习考试"}},[t("li",[e._v("校本部、医学部：12月28日至1月10日")])])]:e._e(),"#,
        r#"e.xq==="第二学期"?[t("el-descriptions-item",{attrs:{label:"一、上课"}},[t("li",[e._v("校本部：2月22日")])])]:e._e(),"#,
        r#"e.xq==="上课时间"?[e._v("第一节 8:00")]:e._e()])},f=[],k=s(u,h,f,!1,null,"b2890cff");const S=k.exports,"#,
        r#"x=function(){var e=this,t=e._self._c;return t("div",[e._v("不属于校历")])}"#,
    );

    #[test]
    fn bundle_path_webpack_and_vite() {
        assert_eq!(
            bundle_path(r#"<script src=js/chunk-vendors.1a2b.js></script><script src=js/ccSchoolCalendar.0f3e9a.js></script>"#).as_deref(),
            Some("js/ccSchoolCalendar.0f3e9a.js")
        );
        assert_eq!(
            bundle_path(r#"<script src="../js/ccSchoolCalendar-legacy.D2anCUWM.js"></script><script src="../js/ccSchoolCalendar.DP5SXWAy.js">"#).as_deref(),
            Some("js/ccSchoolCalendar.DP5SXWAy.js")
        );
        assert_eq!(bundle_path("<html></html>"), None);
    }

    #[test]
    fn webpack_follows_wiring_not_names() {
        let cals = parse_bundle(WEBPACK).unwrap();
        let years: Vec<&str> = cals.iter().map(|c| c.year.as_str()).collect();
        assert_eq!(years, ["2024-2025", "2025-2026"]);
        assert_eq!(
            cals[0].first_semester,
            ["2024年9月9日（星期一）开始上课", "旧学年"]
        );
        assert_eq!(cals[0].second_semester, ["2025年2月17日开始上课"]);
        let cur = &cals[1];
        // 相邻重复合并、空白丢弃
        assert_eq!(
            cur.first_semester,
            ["9月8日（星期一）开始上课", "国庆节放假：10月1日至8日"]
        );
        // 多字符 render 变量名 `_x`：终点不能落在自身头部
        assert_eq!(
            cur.second_semester,
            ["2026年2月23日开始上课", "说明：\"校历\"以第教务部通知为准"]
        );
    }

    #[test]
    fn vite_sections_and_third_tab() {
        let cals = parse_bundle(VITE).unwrap();
        assert_eq!(cals.len(), 1);
        let y = &cals[0];
        assert_eq!(y.year, "2026-2027");
        assert_eq!(
            y.first_semester,
            [
                "六、上课",
                "校本部：9月7日",
                "医学部：",
                "十二、停课复习考试",
                "校本部、医学部：12月28日至1月10日"
            ]
        );
        assert_eq!(y.second_semester, ["一、上课", "校本部：2月22日"]);
    }

    #[test]
    fn errors_when_simso_changed() {
        assert!(parse_bundle("var x=1").is_err());
    }

    #[test]
    fn extract_and_unescape() {
        assert_eq!(
            extract_text(r#"t._v("a"),t._v("a"),t._v(" b "),t._v("a")"#),
            ["a", "b", "a"]
        );
        assert_eq!(unescape(r#"第\n\"x\"\\"#), "第\n\"x\"\\");
    }
}
