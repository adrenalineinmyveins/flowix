# FD-11 跨用户 Agent 网络建设总结报告

> **项目**：Flowix 跨用户个人 Agent 网络（FD-11）
> **周期**：2026-10-19 ~ 2026-10-21
> **分支**：`trae/agent-dRBdg4`（已推送至 4511a837，P0/P1/P2 改动待提交）
> **关联文档**：[F5-CROSS-USER-NETWORK.md](F5-CROSS-USER-NETWORK.md)（契约）、
> [AGENT-NETWORK-BACKLOG.md](AGENT-NETWORK-BACKLOG.md)（待办台账）、
> [AGENT-NETWORK-PLAN.md](AGENT-NETWORK-PLAN.md)（总规划）

---

## 一、项目目标与范围

在既有 F5 网络互咨（wire v1 单中继、匿名设备）基础上，交付跨用户网络 v2：
多租户中继、身份与寻址、目录发现、信封签名、滥用防护，并完成云端部署
与客户端全链路接入。F5 §1 明确挡在 v2 之外的四项（计费/联邦/推送/E2E）
后经确认提前交付。

## 二、交付总览（时间线）

| 阶段 | 日期 | 内容 |
| --- | --- | --- |
| M1 契约定稿 | 10-19 | 命名空间 `alice/researcher`、允许匿名投递、明文脱敏稿过公网（三决策） |
| M2 中继多租户 | 10-19 | 账号（Ed25519 密钥即身份）/ 命名空间 / SQLite WAL 七表 / 配额执法（投递 30/h·匿名 3/h·拉取 120/h·目录写 10/天） |
| M3 目录 | 10-19 | 三档可见性（public/unlisted/private）+ 客户端卡发布（L1 过闸 + 确认门）+ 搜索 UI |
| M4 客户端签名 | 10-19 | wire 可选 signature 块（旧端忽略）；byte-equal 纪律；中继信任锚验签；v1 零回退 |
| M5 三路验收 | 10-19~20 | 本机模拟（cross_user_m5）→ 真机 CDP → **云端真中继两轮复跑**（201 注册 + 挑战登录幂等） |
| 云端部署 | 10-20 | 阿里云 8.160.162.251 systemd 常驻（DynamicUser 加固 + firewalld/安全组双层放行） |
| 客户端接入实测 | 10-21 | CDP 直调 IPC 全链路：注册/挑战登录幂等/relay_url 自动对齐/配额读数/目录搜索 |
| **P0 补全** | 10-21 | 举报 UI（中继端点+客户端按钮）、发起身份选择器 |
| **P1 增强** | 10-21 | F4 编排自动回答、confirm 档 toolGrants 逐命令执法 |
| **P2 提前交付** | 10-21 | 计量报表、联邦一跳转发、SSE 推送、E2E 盲转加密 |

## 三、本轮（P0/P1/P2）交付明细

### P0 —— 补全闭环

1. **举报**：中继新增 `POST /v2/reports`（reports 表、Bearer 账号鉴权、
   同举报方同 receipt 幂等 409，进人工审核队列不自动封禁）；客户端
   `redaction_report` 命令 + 收件卡片旗帜按钮 + 双语提示
2. **发起身份选择器**：确认门发起侧（非回程）增加下拉，数据源
   `registeredAgents`，默认第一个注册 agent；回程仍锁定被咨询专家

### P1 —— 功能增强

3. **F4 编排自动回答**：收件箱 accepted 咨询卡片「询问本机专家」——
   `consult_initiate`（to=命名空间 slug 段，from=本机另一 agent）+ 事件轮询
   （Result 填草稿 / Askback 显示反问 / Failed 报错）。核心纪律：
   **编排自动化、出门人审**——草稿发送仍走确认门脱敏
4. **confirm 档执法**：ToolPolicy 升级三态判决（Allow/Deny/Confirm）——
   allowlist 非空逐命令执法（表外 Deny）；表内未批准 → 终止 run +
   `plugin-tool-confirm` 审批事件 → 前端批准对话框写进程级批准集
   （本会话有效），重跑放行。DSH runtime 无挂起闸门，事件层首用拦截
   为既定形态，dsh-appserver 零改动

### P2 —— 原远期四项（提前交付）

5. **计量报表层**（FD-1「只计量不结算」维持）：`GET /v2/usage/summary?days=N`
   按天聚合（投递/拉取/目录写）+ 套餐档；客户端近 7 天报表懒加载展开。
   无支付集成
6. **联邦一跳转发**：`--peers` 启动参数；本中继无此 agent → 逐对端转发
   （任一 2xx 即成功，伪 queueId `fed:<peer>`）；`X-Forwarded-From` 防环；
   转发计量主体 `fed:<第一跳>`（伪造收益上限=注册账号，已记录）。
   不做路由表/多跳/全网目录
7. **SSE 推送**：`GET /v2/events`（Bearer + 按 token 拥有的 agent 过滤 +
   20s keep-alive）；客户端进程级单订阅（fetch 流式 + 5s→60s 指数退避
   重连）；收件箱监听 `redaction-inbound` 后 3s 去抖自动拉取。
   **消费语义不变**（at-least-once + ack）
8. **E2E 盲转加密**（F5 §6 边界严格保持）：wire `cipherText` 可选字段；
   NaCl box（X25519+XSalsa20-Poly1305），密钥零新增体系（账号 Ed25519
   经标准 NaCl 映射转换，libsodium 互操作）；preview 预取收件方公钥固化
   留档（无卡拒开）→ 确认时**先加密后签名**（签名覆盖密文、密文随档固化，
   byte-equal 不变）→ 中继只见密文；收端解密（公钥=签名 keyId，回退
   目录卡），失败留占位人审，`encrypted` 留档标记 + UI 标识

## 四、关键决策记录（本轮）

| 决策 | 结论 | 依据 |
| --- | --- | --- |
| D1 DSH approval 类别 | 复用 permissions，disclosure 不在 DSH 层建模 | 应用层确认门是唯一出门口；双审批入口破坏留档一致性；dsh-appserver 零改动 |
| 计费形态 | 计量报表层，不做支付 | FD-1 外接模型契约维持；个人项目无支付资质 |
| 联邦形态 | 最小一跳转发 | 完整联邦（路由表/多跳）工程量与真实需求不匹配 |
| 推送形态 | SSE 而非 WebSocket/CF 橙云 | axum 生态最轻；CF 免费版大陆回源延迟劣化 200ms+ 且回源端口白名单不含 8788 |
| E2E 密钥 | Ed25519→X25519 标准映射 | 不引入第二密钥体系；目录卡公钥即加密公钥；libsodium 互操作 |

## 五、质量验证

- **中继**：31 测全绿（账号全流程/命名空间/配额执法/重启恢复/三档可见性/
  验签/举报幂等/联邦转发+防环）
- **客户端 Rust**：redaction 53 测全绿（e2e roundtrip/防篡改/签名共存/
  盲转留档/byte-equal）；plugin 30 测（confirm 门+批准集隔离）；
  cross_user_m5 + network_roundtrip 不回退
- **前端**：tsc 0 错；redaction 相关 19 测全绿
- **真中继实测**：注册/挑战登录幂等/relay_url 对齐/配额/目录全过
  （CDP 全链路）
- 基线说明：`agent-tasks-recent` 4 失败为并行改动（会话列表）既有基线，
  与本轮无关

## 六、遗留事项与风险

| 事项 | 状态 | 说明 |
| --- | --- | --- |
| **TLS 补齐** | 阻塞域名 | 唯一功能待办。路径已定（Caddy+LE 直签），域名就位后 6 步约 5 分钟；物料已备（scripts/relay-deploy/）。当前明文 HTTP 与 F5 §6 有差距，已记录 §8.1 |
| 云端二进制落后 | 待重部署 | 服务器 relay 二进制不含本轮 P2 端点，重编译上传即可（源码包流程已验证 3m17s） |
| 发版签名 | 用户侧 | `TAURI_SIGNING_PRIVATE_KEY` 未配，当前安装包为本地未签名 |
| 联邦信任 | 已知边界 | `X-Forwarded-From` 可伪造（收益上限=认证档配额，等同注册账号），TLS 后缓解，记录在案 |
| 本轮改动 | 待提交 | relay 6 + desktop 10 + web 12 + 文档 3，共 31 个文件 |

## 七、结论

FD-11 跨用户网络从契约到落地全周期收官：**中继多租户 → 云端常驻 →
客户端接入 → 防滥用闭环 → 增强能力 → 原远期四项全部提前交付**。安全
纪律贯穿始终——byte-equal 出口唯一、确认门人审不可绕过、E2E 加密不
破坏计量与跳数执法边界。测试覆盖三层（单元/内嵌中继集成/公网真中继），
契约文档与待办台账同步更新。项目仅余 TLS 补齐一项功能待办，等域名
注册即可闭环。
