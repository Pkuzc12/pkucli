---
name: cwfw
description: "北京大学财务综合信息门户 (cwfw.pku.edu.cn) CLI 工具，只读。当用户提及 cwfw、财务门户、个人酬金、助教/助研/劳务费、奖学金/助学金按月发放、工资查询、谁给我打了钱、入卡/银行卡到账、报销进度/报销被退回、借款/暂借款、外汇到款/境外汇款、财务部汇率 时使用此 skill。Also use when dealing with cwfw IAAA 登录 (app_id=IIPF)、home2.jsp→findpages_postData.action→home3.jsp 多步 bootstrap、WF_CWBS / YBX 子系统入口、Wingsoft loadDefinition/responseButtonEvent/doQuery 协议或 cwfw 的加密表单字段。学费住宿费、缴费订单不在这里（见 cwpay）。"
version: 1.1.0
---

# cwfw - 北大财务综合信息门户 CLI

A read-only CLI client for PKU's financial information portal at cwfw.pku.edu.cn
(Wingsoft "WF" platform). Campus network only — call it directly, never through a VPN/proxy.

## CLI Commands

| Command (alias) | 用途 |
|---|---|
| `login -p` / `status` / `logout` | IAAA 登录 + 多步 bootstrap，会话管理 |
| `cj` (`reward`) `[-y 年] [-f 月] [-t 月]` | 个人酬金：按笔发放的助教/助研/劳务费等（经办人、发放部门、扣税、实发） |
| `gz` (`salary`) `[-y] [-f] [-t] [-k 代码] [--list]` | 个人工资查询：学生为按月发放的奖助学金、生活物价补贴（每月金额 + 合计） |
| `rk` (`deposits`) `[-s YYYY-MM-DD] [-u …]` | 我的入卡：发到本人银行卡的每一笔（摘要、发放部门、经办人、入卡金额、支付状态/失败代码） |
| `bx` (`reimburse`) | 智能报销：报销单各状态笔数与金额（草稿/审批中/待投递/待入账/被退回/支付失败/待我审批） |
| `jk` (`loans`) | 个人未核销借款 |
| `wh` (`remittance`) `<关键词> [--all] [--since YYYY-MM] [--until …] [-c USD] [--min] [--max]` | 外汇到款：学校收到的境外汇款，按汇款单位英文关键词；金额区间不能超过 10 万 |
| `hl` (`fx`) `[-c 美元] [-s] [-u]` | 财务部外币汇率（中行折算价，每 100 外币） |
| `xc` (`payroll`) `[-k 0-5] [-s] [-u] [-p 项目号]` | 薪酬劳务查询（经办/项目负责人视角，学生通常为空；不输出证件号/卡号） |
| `menu` | 列出本账号"网上查询"可见菜单及窗口号 |

所有新查询命令都支持 `--json`，便于 Agent 解析。

"谁给我打了钱" → 先 `cj`（按笔酬金）+ `rk`（入卡），按月的奖助学金看 `gz`。

## Auto-Login for AI Agents

```bash
info-auth check
cwfw login -p
cwfw rk --json
```

## Architecture

- **Crate**: `crates/cwfw/`
- **Auth**: IAAA (`app_id="IIPF"`) → `WFManager/home2.jsp` → `findpages_postData.action` (必须回 `ok`)
  → `home3.jsp`（每个子系统 `<div id="WF_*" url="WINOPEN../<path>/main2.jsp?…&token=…">`）→ 子系统入口。
  登录只进 WF_CWBS；`bx` 需要时用 `login::enter_subsystem("WF_YBX")` 重新从 home3 进入 YBX。
- **Protocol** (`src/wf.rs`):
  1. `loadDefinition.action {projid, winno, type=W}` → 窗口定义（JS 字面量）：按钮 `procs:["`$<md5>:#<func>-<field>#;#0-<var>#…"]`、
     `check_func`、`beforeLoadProcs`、字段联动 `funcMap`、下拉数据源 `bindMap`。哈希随会话变化，每次动态解析。
  2. `commonUpdate_responseButtonEvent.action {cmd:{check_func, proc0, data0[]}, projid, userContext, async}`；
     `userContext` 里要带表单字段 `WF_CWBS$<func>-<FIELD>`，服务端存进会话，表格 SQL 从这里取值（只改 data0 不生效）。
  3. 交叉表先 `crossfieldrebuild.action` 拿动态列；4. `commonQuery_doQuery.action` 取行（无数据时可能直接返回 `[]`）。
  所有请求体走 `encrypt.rs` 混淆（`a=<uid>` 放第一个字段）。
- **窗口号**（静态配置）：个人工资 5609(表单 5574/交叉表 6160)，个人酬金 5802(5743/5746)，借款 6320(6241/6243)，
  薪酬劳务 7320(7181/7183)，外汇信息 7600(10819/13522)，汇率 7700(13562/13583)，我的入卡 7987(15030/15028，无存储过程)，
  智能报销首页 YBX winno 6（beforeLoad 存储过程返回 `pass;local(0-BX_YBXALL_count:…)`）。

## Key Source Files

- `src/login.rs` — IAAA → bootstrap → WF_CWBS；`enter_subsystem` 进入其他子系统
- `src/context.rs` — userContext（loadRolesMenu 的 `_@` 转义 JSON + loadInitFunction）
- `src/encrypt.rs` — 表单混淆（picList.encode/encode2）
- `src/wf.rs` — 通用协议 + 纯解析函数（procs/funcMap/bindMap/local()/菜单/动态列）
- `src/queries.rs` — 各查询流程与行映射（含单元测试，合成数据）
- `src/api.rs` — 个人酬金（最早的实现）与 doQuery 响应解析
- `src/commands.rs` / `src/display.rs` — 子命令与终端渲染

## Development Notes

- 只读：不要调用任何保存/提交/认领/修改卡号等按钮的存储过程
- 隐私：外汇到款不输出联系电话、结汇人、经办人；薪酬劳务不输出工号、证件号、银行卡号、开户行
- 测试数据全部合成，不要提交真实姓名/学号/金额
- Session 持久化 `~/.config/info/cwfw/`；所有文案中文，`anyhow::Result` + `.context("中文描述")`
