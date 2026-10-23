---
name: cwpay
description: "北京大学收费平台 / 收费大厅 (cwpay.pku.edu.cn) 只读 CLI 工具。当用户问 学费、住宿费、学宿费 交了没有 / 还欠多少、收费平台待缴费、缴费订单/缴费记录、某笔订单的支付方式或电子票据（发票）链接、在收费平台一共花了多少（浴卡充值/网费/校园卡就诊账户等）、收费平台有哪些缴费项目、学费缴费通道是否暂停、收费平台公告 时使用此 skill。不负责缴费/下单（只读）；个人酬金/工资请用 cwfw skill，校园卡余额请用 campuscard skill。Also use when dealing with cwpay IAAA (app_id=CWSF) → /dealcas → X-Token 登录、响应 AES-256-ECB 解密、messageCode -3 续期 token。"
version: 1.0.0
---

# cwpay - 北京大学收费平台只读 CLI

A read-only CLI for PKU's payment platform (收费大厅, vendor datalook) at cwpay.pku.edu.cn.

## Architecture

- **Crate location**: `crates/cwpay/`
- **Auth flow**: `GET /api/pay/iaaa/getIAAALoginParam`（公开）→ `{appId: "CWSF", redirectUrl: "http://cwpay.pku.edu.cn/dealcas"}` → IAAA → `GET https://cwpay.pku.edu.cn/dealcas?_rand=…&token=<iaaa>` → 302 → `/#/middle_page?token=<X-Token>&jsonData={idserial,…}`（失败 `/#/middle_page_error?errorMsg=…`）
- **API**: `/api/<path>` + 请求头 `X-Token`（HS256 JWT，约 6 小时有效）；请求体是明文 JSON
- **响应加密**：`rand10 + reverse(key32) + rand10 + base64(AES-256-ECB/PKCS7) + rand10`；错误响应是明文 JSON
- **信封**：`{messageCode, message, data}`；`"0"`/`"6003"` 成功，`"-3"` 续期（data = 新 token，重放请求），`"-1"/"-2"/"-4"` 登出
- 站点设置：`schoolCode = "10001"`，`dataSource = "PAY"`，`partnerId = "PT000004"`

## Key Source Files

- `src/lib.rs` — Clap CLI + dispatch（全局 `--json`）
- `src/client.rs` — reqwest client（不跟随重定向）
- `src/login.rs` — IAAA → dealcas → X-Token，会话存 `~/.config/info/cwpay/session.json`
- `src/crypto.rs` — 响应解密、信封状态、dealcas 跳转解析、JWT exp（含单测）
- `src/api.rs` — 只读接口调用（处理 -3 续期并写回会话）
- `src/parse.rs` — 纯函数解析（含单测，fixtures 在 `fixtures/`，全部合成数据）
- `src/display.rs` — 终端渲染

## CLI Commands

| Command | 用途 | 端点 |
|---------|-----|-----|
| `login -p` / `status` / `logout` | 会话管理 | 见上 |
| `tuition` | 学费住宿费按学年 应缴/已缴/未缴 | `GET pay/web/tuitionAndDorm/getTuitionAndDormList/<学号>/<projectId>`（无欠费时回 `messageCode "0001"` `无欠费##`，按空列表处理） |
| `pending` | 待缴费列表（所有项目） | `GET pay/pay/waitingPay/getWaitingPayList/10001/<学号>/undefined`（末段字面量 undefined 与站点一致） |
| `orders [--from --to -p 项目 --page -n]` | 缴费订单记录 | `POST pay/web/order/pageOrderlist` + 状态字典 `common/com/dict/getDictDatasByTypes/TRADE_DISPLAY_STATUS/10001/PAY` |
| `order <订单号>` | 订单详情：支付方式、支付时间、退款、电子票据链接 | `GET pay/pay/orderTrade/getOrderById/<id>`；票据 `POST pay/pay/invoiced/getEBillPicUrl`（HANGXIN）/ `getCouldEBillPicUrl`（BOSI） |
| `spending [--from --to -p]` | 按项目/月份汇总已支付金额（最多最近 500 笔） | 本地按 pageOrderlist 统计（站点自带「缴费账单」统计接口对所有人回 -1） |
| `projects [-s 关键词] [-k kind] [--open-only]` | 约 160 个缴费项目，是否开放、暂停说明、标准金额 | `POST pay/project/getAllProjectNoType` |
| `notice` | 平台首页公告 | `POST common/com/notice/getStuNotice` |
| `me` | 个人中心信息 | `POST pay/web/personalCenter/findUserInfoByAccountNum` |

订单状态码：101 待支付、102 支付中、103 支付成功、104 支付失败、107 部分退款、108 全额退款、005 已过期、006 已取消。支付渠道：01 支付宝、02 微信、08 建行网银、09 建行大额转账、61 中行外卡、71 数字货币。金额单位均为分，CLI 输出为元。

## Auto-Login for AI Agents

```bash
info-auth check
cwpay login -p          # 读 keyring，不需要密码
cwpay tuition
cwpay --json orders --from 2026-01-01
cwpay order <订单号>
```

会话过期（约 6 小时）后报「会话已过期」，重新 `cwpay login -p` 即可。

## Not supported / known dead ends (2026-10-23)

- 只读：不创建订单、不支付、不关单、不开票、不改发票抬头/个人信息（这些接口在 bundle 里存在，CLI 故意不接）。
- `pay/pay/orderTrade/getTrade*`（缴费账单统计）、`pay/web/queryPayment/*`（缴费查询）、`pay/web/transfer/*` 与 `common/com/transfer/*`（退费申请）、`pay/web/netFee/queryNetBalance`：对 IAAA 登录的 X-Token 回 `-1 user-token权限验证失败`（站点自己的页面也一样），应是另一套账号体系。
- `system/message/pageList`（消息中心）、`reimburse/*`（报销进度）：服务端 404。
- `pay/pay/orderTrade/getOrderTradeDetailByIds`：回「系统正在维护中」。
- 党费 / 供暖费 / 肖家河租金 查询需要对应身份（非党员、无协议的账号会返回业务错误）。
