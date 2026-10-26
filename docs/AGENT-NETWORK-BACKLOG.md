# Agent 网络遗留事项（Backlog）

> 状态快照：2026-10-21（FD-11 跨用户网络 v2 全量交付后重整）。
> F6 脱敏管线 M1-M3 与 F5 FD-10（wire v1）Rust 侧早已全部完成；
> FD-11 v2（多租户中继/目录/签名/云端部署/客户端接入实测）于
> 2026-10-19 ~ 10-21 交付完毕，本文记录跨域完成度与后续待办。
> 关联设计文档：[AGENT-NETWORK-PLAN.md](AGENT-NETWORK-PLAN.md)（总规划）、
> [F5-CROSS-USER-NETWORK.md](F5-CROSS-USER-NETWORK.md)（FD-11 v2 契约）、
> [REDACTION-PIPELINE.md](REDACTION-PIPELINE.md)（F6）、
> [AGENT-PROFILE-MODEL.md](AGENT-PROFILE-MODEL.md)（F1）、
> [AGENT-NURTURE-PIPELINE.md](AGENT-NURTURE-PIPELINE.md)（F2）、
> [AGENT-SKILL-SYSTEM.md](AGENT-SKILL-SYSTEM.md)（F3）、
> [AGENT-LOCAL-CONSULT.md](AGENT-LOCAL-CONSULT.md)（F4）。

## 1. 完成度矩阵

| 域 | 里程碑 | 状态 | 关键落点 |
| --- | --- | --- | --- |
| F1 Profile | 数据模型 + 接线 + 卡片 UI | ✅ | `flowix-core/src/agent/`、`.agents/` 双数据面、AgentState 记账（含 token，2026-10-18 接线）；卡片 UI（2026-10-17）：`agent_profile_update` 扩展隐私边界/model 可选参数，前端 `agent-profile-cards.tsx` 列表+详情+隐私三字段编辑，入口在会话标题栏「个人 Agent…」；（2026-10-18）面板并入中列视图切换组第四 tab，与访问空间/卡片/对话并列，并对齐中列统一格式（--list-bg、避让标题行、OverlayScrollbar、i18n、Radix 删除确认、@[440px] 容器查询），全视图标题避让 pl-[100px]→pl-[126px] |
| F2 喂养 | N1-N3 + 多 agent 映射 | ✅ | `nurture/{mod,runner}.rs`、disclosure_scope 门控、水位增量 |
| F3 技能 | manifest v3 + knowledgeScope + P2 导入导出 + P4 执法 | ✅ | `flowix-plugin-runtime/src/manifest.rs`、skill_registry；P2（2026-10-18）：`plugin_export/plugin_import`（zip 往返 byte-equal + zip-slip 防护）；P4：`ToolPolicy` 类别级黑名单执行器（越权 tool_call 终止 run 留痕）+ `effective_target_level` 与 preview 联动 |
| F4 本机互咨 | mailbox + BudgetGuard + AskBackGate + DSH 编排 + 咨询记账 | ✅ | `consult/{mod,orchestrator}.rs`、`commands/consult.rs`；被咨询次数/lastConsultedAt 落 AgentState |
| F6 脱敏 | M1 规则地基 | ✅ 验收过 | `flowix-redaction` crate：对抗样本命中率 100%、占位符防毒化 |
| F6 脱敏 | M2 蒸馏 + L3 复扫 | ✅ 验收过 | `redaction/mod.rs`：注入样本拦截 100%、回炉上限 2 |
| F6 脱敏 | M3 确认门 + outbox | ✅ 验收过 | `redaction/outbox.rs`：UNCONFIRMED_SEND_BLOCKED 兜底、byte-equal、FIFO 200 |
| F5 网络互咨 | FD-10 全里程碑 M1–M4b | ✅ | M1 协议 v1 + M2 `flowix-relay` 参考实现（14 测试）+ M3a 入站收件 + M3b 出站 wire v1（byte-equal）+ M3 前端收件 UI + M3c 回程信封（kind=reply）+ M4a 真机收件闭环（注册/拉取/接受/ack）+ M4b 网络往返测试（`tests/network_roundtrip.rs`：内嵌 relay 全链路 A 发起→B 收件→回程→A 收 reply + 跳数递减；顺带修复 wire redactionLevel 小写被 422 拒收、发起侧 sender 自动取注册 agent）。遗留（v2）：F4 编排自动回答、发起身份选择器 UI、E2E 加密 |
| F5 跨用户网络 | FD-11 全里程碑 M1–M5 | ✅ | 2026-10-19~21 交付：中继 v2（账号/命名空间/SQLite 七表/配额执法）+ 目录三档可见性 + 客户端 Ed25519 信封签名（byte-equal）+ M5 三路验收（本机模拟/真机 CDP/**云端真中继**两轮复跑）+ 云端部署（8.160.162.251 systemd 常驻，见 F5 §8.1）+ 客户端接入实测（CDP 全链路：注册/挑战登录幂等/relay_url 自动对齐/目录搜索）。后续待办见 §6 |

## 2. P0 —— 端到端闭环缺口

### 2.1 前端确认门卡片与 scope 选择器

- **状态**：✅ 已实现（2026-10-17，真机全流程点检待做）
- **现状**：IPC 全部就绪——`redaction_preview`（question/scope/level 全管线）、
  `redaction_confirm`、`redaction_confirm_wait`（gate 挂起）、`redaction_send`
  （唯一出口）、`redaction_outbox_list`、`redaction_config_get/set`、
  `redaction_l1_preview`（诊断）。
- **已落地**：
  - 类型镜像与 8 个 IPC 绑定：`lib/types/redaction.ts`、
    `platform/tauri/client/redaction.ts`；
  - 确认门对话框 `features/agent/components/redaction-send-gate.tsx`
    （compose → preview 报告卡片 → 确认/取消 → send；倒计时到 0 自动按拒绝
    结算、draft 留档重开可恢复、错误码归一文案），入口在会话标题栏
    更多菜单「脱敏发送…」；
  - scope 选择器标签档：`AgentAccessKind` 补 `"tag"` + `tagPath` 字段，
    store 新增 `addTagScope`（乐观写 + already-tracked 去重），对话框内
    可从笔记本标签树添加标签级授权；
  - 测试：`redaction-gate-logic.test.ts` / `redaction-send-gate.test.tsx`
    （preview→confirm→send、取消回落、draft 恢复、超时自动取消、错误展示）/
    `agent-access-store.test.ts`（addTagScope 四例）。全量 vitest 中该批
    全绿（Windows 环境另有 42 个与本线无关的既有失败，基线对照一致）。
- **待做（真机）**：真机上 preview → 人审 → confirm → send 全程可点，
  取消/超时卡片状态正确回落；需 dsh-appserver 可用（见 §2.3）。

### 2.2 F5 网络传输层

- ✅ 已闭环（FD-11 v2，2026-10-19~21）：客户端传输切片（2026-10-18）之上，
  对端协议与云端中继已全量交付——`flowix-relay` v2 多租户中继（账号/命名空间/
  SQLite/配额/目录/验签）、客户端签名、云端部署（8.160.162.251）与三路验收，
  详见 [F5-CROSS-USER-NETWORK.md](F5-CROSS-USER-NETWORK.md) §8。
  `disclosure.hopsRemaining` 消费契约已在中继落地（回程跳数递减、0 拒投，
  M5 断言）。
- FD-7 approval 类别：见 §3.3（已决策复用 permissions）。

### 2.3 dsh-appserver 构建与模型凭证

- **现状**：L2 蒸馏接线完成（复用 `run_distill_once`，title=`redaction:distill`），
  但沙箱内 `dsh-appserver` 仅有源码未构建、无模型 API 凭证。
- **完成判据**：真机跑通一次真实 L2 蒸馏（任意等级），验证蒸馏稿过 L3 复扫。

## 3. P1 —— 规格内未完成项

### 3.1 token 用量 → AgentState 记账

✅ 已接线（2026-10-18）：`run_distill_once` 在删除蒸馏线程前取 DSH 会话
usage，nurture 喂养落账 `record_tokens`；咨询完成对专家落
`consultations_received / last_consulted_at`。非 DSH 运行时暂无会话级
usage 查询，不计。

### 3.2 manifest v3 运行时消费

✅ 已接线（2026-10-18）：
- `minRedactionLevel` 经 `redaction_preview` 新参数 `min_levels` 与
  `target_level` 取 max（`effective_target_level`，大小写归一）；
- `toolGrants` 在 `plugin_run` 事件层执法（`ToolPolicy` 类别级黑名单：
  已识别类别未授权 → 终止 run + `TOOL_GRANT_VIOLATION` 留痕；未识别类别
  放行避免误杀）。confirm 档逐命令 allowlist 执法待 DSH approval 集成。

### 3.3 DSH approval 的 disclosure 审批类别（规格 §9 未决 1）

✅ 已决策（2026-10-21，F5 v2 交付后回看）：**复用 permissions 类别**，
disclosure 审批不在 DSH approval 层建模。理由：

1. 权威 disclosure 审批已在应用层单点执法——F5 确认门（`redaction_confirm_wait`
   挂起 + 人审 + outbox 留档 + byte-equal）是唯一出门口，所有数据出设备收敛于此；
2. permissions（能力授予）与 disclosure（数据流出）语义正交，DSH 层另建
   disclosure 类别会形成双审批入口，破坏留档一致性纪律；
3. confirm 档 toolGrants 逐命令审批走既有 permission request/response 通路，
   dsh-appserver 零协议扩展（实施项见 §6 P1）。

### 3.4 蒸馏 Agent 类型选择（规格 §9 未决 4）

✅ FD-8 已落地（2026-10-18）：`redaction/config.json` 新增
`distillAgentType` 字段，空 = 默认 deepseek-harness，可覆盖。

### 3.5 占位符「查看回填」本地预览（规格 §9 未决 2）

默认不回填已实现且被测试锁定。是否提供用户显式开启的本地回填预览
（仅本机展示、不回传）待产品决策。

## 4. P2 —— 优化与上游

| 事项 | 说明 |
| --- | --- |
| 词表扫描性能 | `apply_word_rules` 为 O(words × text) 逐词扫描；词表 > 数百条时可优化为单遍多模式（Aho-Corasick） |
| 检索上限联动 | L0 检索行数 200 / 行内材料上限等常量当前硬编码，未接 `redaction/config.json` |
| watcher::whitelist 3 个上游失败 | 非本线引入（相关文件未改动），基线 756 passed / 3 failed 待上游修复 |

## 5. 验收记录（F6）

| 里程碑 | 验收标准 | 结果 |
| --- | --- | --- |
| M1 | 对抗样本集内规则类别命中率 100% | ✅ 31 crate 测试，含 kitchen-sink 全类别断言 |
| M1 | scope 外内容在 L0 输出不可达（fuzz） | ✅ 200 轮确定性 LCG fuzz |
| M2 | LLM 写回敏感词被复扫拦截率 100% | ✅ 9 类注入样本全拦截 |
| M2 | 占位符被破坏时映射校验失败并回炉 | ✅ Unknown/Missing 双向对账 + 回炉上限 2 |
| M3 | 无确认时发送被兜底拦截 | ✅ draft/cancelled/failed/不存在四态全 `UNCONFIRMED_SEND_BLOCKED` |
| M3 | 发送字节与留档 byte-equal | ✅ 取出字节 == 留档重读序列化，幂等 |
| M3 | 取消/超时留档状态正确 | ✅ 均记 cancelled + settledAt，超时报 confirm_timeout |

调试入口：`cargo run -p flowix-redaction --example demo`（内置对抗样本 +
L3 复扫拦截演示；支持 `--file <文本> --words <词表>` 脱敏任意文件）。

## 6. FD-11 v2 交付后待办（2026-10-21 整理，同日 P0/P1 批量执行完毕）

### P0 —— 小活补全闭环

| # | 事项 | 状态 |
| --- | --- | --- |
| 1 | TLS 补齐 | **暂缓（用户指示）**。路径已定：Caddy + Let's Encrypt 直签（CF 橙云因大陆回源延迟劣化 + 回源端口白名单被否）。阻塞：域名注册（阿里云 `.top`/`.xyz` 首年 ¥10-20 即可）。域名就位后 6 步：装 Caddy → 部署 `scripts/relay-deploy/Caddyfile.relay` → relay unit 切回 `127.0.0.1:8788` → firewalld/安全组收 8788 → 三层验收（端点/M5 复跑/客户端重注册）→ 物料更新提交 |
| 2 | 举报 UI | ✅ 2026-10-21。中继侧补齐 `POST /v2/reports`（`reports` 表 + Bearer 账号鉴权 + 同举报方同 receipt 幂等 409，进人工审核队列不自动封禁）；客户端 `redaction_report` 命令 + 收件卡片旗帜按钮 + i18n 双语 |
| 3 | 发起身份选择器 | ✅ 2026-10-21。确认门发起侧（非回程）加「发起身份」下拉，数据源 `registeredAgents`，默认第一个注册 agent；回程仍由 `presetSender` 锁定 |

### P1 —— 功能增强

| # | 事项 | 状态 |
| --- | --- | --- |
| 4 | F4 编排自动回答 | ✅ 2026-10-21。收件箱 accepted 咨询卡片加「询问本机专家」：`consult_initiate`（to = 命名空间 toAgent 的 slug 段，from = 本机另一 agent）+ 事件轮询（Result 填草稿 / Askback 提示反问 / Failed 报错）。纪律：**编排自动化、出门人审**——结果只填回答草稿框，发送仍走确认门脱敏 |
| 5 | confirm 档 toolGrants 逐命令执法 | ✅ 2026-10-21。事件层实现：`ToolPolicy` 三态判决（Allow/Deny/Confirm）——confirm 档先过 `allowlist`（非空逐命令执法，表外 Deny），表内未批准 → 终止 run + `plugin-tool-confirm` 审批事件；前端对话框批准写进程级批准集（本会话有效），重跑放行。DSH runtime 无逐命令挂起闸门，事件层首用拦截 + 批准重跑为既定形态（dsh-appserver 零改动，符合 §3.3 决策） |

### P2 —— 原远期单独立项（2026-10-21 经用户确认提前交付，形态见 F5 §8.2）

| # | 事项 | 状态 |
| --- | --- | --- |
| 6 | 计量/经济模型 | ✅ 计量报表层（`/v2/usage/summary` 按天聚合 + 客户端近 7 天报表 + 套餐档展示）。FD-1 维持「只计量不结算」，无支付集成 |
| 7 | 多中继联邦 | ✅ 最小一跳转发（`--peers` 配置 + 未知 agent 转发对端 + `X-Forwarded-From` 防环 + `fed:` 计量主体）。不做路由表/多跳/全网目录同步 |
| 8 | Feed/云推送 | ✅ SSE 推送（`/v2/events` Bearer 过滤 + 客户端流式订阅退避重连 + 收件箱 3s 去抖自动拉取）。消费语义不变（at-least-once + ack） |
| 9 | E2E 加密 | ✅ 盲转模式（NaCl box + Ed25519→X25519 标准映射，libsodium 互操作；确认时先加密后签名，密文随档固化 byte-equal 不变；收端解密 + encrypted 留档标记）。F5 §6 边界不破 |

### 工程优化（沿袭 §4）

词表 Aho-Corasick、检索上限配置化——性能尚未成为瓶颈，顺手做时带上。
