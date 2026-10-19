---
name: bdkj
description: "北京大学空间 (bdkj.pku.edu.cn) 学术研讨教室预约 CLI 工具。当用户提及 bdkj、北大空间、学术研讨教室、教室预约、研讨间、借教室 或想要查询/预约/取消 二教/四教/地学 教学楼里的空闲学术研讨教室时使用此 skill。支持保存固定参与人分组（如课题组），重复发起预约时只需 `--group <名字>`。Also use when dealing with bdkj IAAA SSO callback (app_id=bdkj), `/space` / `/space/schedule` / `/space/lockStatus` / `/classRoom/seachStudent` / `/classRoom/handle/submit` 这些端点，或 bdkj groups.json 持久化。"
version: 1.1.0
---

# bdkj - 北大空间学术研讨教室预约 CLI

A CLI client for PKU's academic discussion room reservation system at bdkj.pku.edu.cn.

## Architecture

- **Crate location**: `crates/bdkj/`
- **Auth flow**: IAAA SSO (`app_id="bdkj"`, redirect `https://bdkj.pku.edu.cn/login/oauth`) → `GET /login/oauth?_rand=…&token=…` → 302 `/home`，种 `SESSION` cookie。2026-10 改版后 `/personal/user` 对所有人 404，会话有效的判据是最终 URL 不落在 `/oauth`、`/login*`（回调除外）或 IAAA
- **教室**：`GET /space` HTML —— 楼栋面板 `[data-building-id]`（`span.cr-panel-title` 二教/四教/地学，`span.sp-building-code` (02)/(04)/(09)），教室卡片 `div.sp-card[data-room-id][data-room-name]`（`span.sp-code` 如 02514，楼层 / 可容纳 N 人 / 至少 N 人起约，门/屏幕状态）。教室 id 是长雪花 id
- **门锁**：`GET /space/lockStatus`（XHR）→ `{ roomId: 0|1|2 }`，仅供参考
- **空闲时段**：`GET /space/schedule?roomId=&startDate=YYYY-MM-DD`（XHR）→ 一周内已占用时段 `[{beginTime,endTime}]`。每天 13 节、每节 50 分钟（08:00–22:30），一次最多连续 2 节
- **我的预约**：`GET /classRoom` HTML，`div#results` 内是记录，空时为 `div.cr-empty`（暂无申请记录）
- **查询学生 / 提交 / 取消**：`/classRoom/seachStudent`、`/classRoom/handle/submit`、`/classRoom/cancelApply/<id>` 仍是旧版流程，**尚未在 2026 新站点上验证**
- 旧的 `/room/classRoom`（对所有楼返回 `[]`）和 `/classRoom/historyTime` 已弃用

## Key Source Files

- `src/main.rs` — tokio::main，调用 `pku_bdkj::run()`
- `src/lib.rs` — Clap CLI 定义 + dispatch
- `src/client.rs` — reqwest client 构造
- `src/login.rs` — IAAA 登录 + JWTUser cookie 提取
- `src/api.rs` — 教室/时段/学生/提交/取消/列出的 HTTP 调用（统一检查会话失效与 /personal 重定向）
- `src/parse.rs` — 纯函数解析（`/space`、lockStatus、schedule、空闲节次、/classRoom 列表、URL 判定），含单元测试
- `src/commands.rs` — 各子命令实现
- `src/display.rs` — 终端渲染（colored）
- `src/groups.rs` — 参与人分组持久化（`~/.config/info/bdkj/groups.json`）

## CLI Commands

| Command | 用途 |
|---------|-----|
| `login -p` | IAAA 密码登录（keyring → env → interactive） |
| `status` / `logout` | 会话管理 |
| `rooms [building] [--min-capacity N]` | 列出研讨教室（二教/四教/地学 或 02/04/09；不填列全部），含容量、起约人数、门状态 |
| `schedule <room_id> [--date YYYY-MM-DD] [--days N]` | 每天空闲节次 + 已占用时段（别名 `history` / `free`） |
| `student <serial> <name>` | 查询学生信息 |
| `list` / `ls` | 列出当前用户的预约记录 |
| `reserve --room-id ... --begin ... --end ... --reason ... -g <group>` | 提交一次预约（用分组） |
| `reserve ... -p 学号:姓名 -p ...` | 提交一次预约（手动列参与人） |
| `cancel <apply_id>` | 取消一次预约 |
| `group list` / `show <name>` / `set <name> -p ...` / `remove <name>` | 管理参与人分组 |

`--group` 与 `--participant` 互斥；两者必须至少指定一个。申请人自己也必须包含在参与人列表中。

`reserve` 提交前会本地校验：至少 3 人且不少于教室「起约」人数、不超过容量；同一天、最多连续 2 节；与 `/space/schedule` 已占用时段不冲突；逐个校验参与人学号+姓名。

**首次预约**：如果北大空间个人资料不完整，站点会把请求重定向到 `/personal`，CLI 报错「首次预约前需要在北大空间「个人信息」页完善资料（https://bdkj.pku.edu.cn/personal）」。原样转告用户去浏览器完成，不要反复重试。

**提交步骤未验证**：2026 改版后的提交端点尚未抓到，`reserve` 沿用旧版 `/classRoom/handle/submit` 表单；若失败请告知用户可能需要在网页上预约。

## Auto-Login for AI Agents

```bash
info-auth check
bdkj login -p              # 读 keyring，不需要密码
bdkj group set lab -p 2200011523:张三 -p 2200011524:李四
bdkj rooms 二教 --min-capacity 3
bdkj schedule <room_id> --days 3
bdkj reserve --room-id <room_id> \
  --begin "2026-04-20 14:00" --end "2026-04-20 15:50" \
  --reason "课题组讨论" -g lab
```

## Development Notes

- 所有文案中文
- 错误用 `anyhow::Result` + `.context("中文描述")`
- Session 持久化 `~/.config/info/bdkj/`；groups 在同目录下 `groups.json`
- `submit_apply` 的成功判据是重定向后 URL path 为 `/classRoom`，而不是扫 `layer.msg`（页面模板里有无关的 layer.msg 调用会误命中）
- 教学楼 / 教室 ID 都从 `/space` 页面动态解析，不再硬编码
