# FD-12 Harness 远程咨询工具化（跨用户 agent-to-agent 调用）

> 状态：立项定稿（2026-10-21）
> 定位：FD-11 跨用户网络（[F5-CROSS-USER-NETWORK.md](F5-CROSS-USER-NETWORK.md)）的能力延伸——
> 把「人对人的异步信封咨询」升级为「harness 运行时工具调用」，使本机 agent 能在
> 会话中直接调用其他用户开放的个人 agent。
> 决策表：[AGENT-NETWORK-PLAN.md](AGENT-NETWORK-PLAN.md) §5 FD-12。

## 1. 背景与目标

FD-11 已交付跨用户咨询全链路（账号/目录/签名/配额/中继/收件箱/确认门），但发起方
始终是**人**：用户在确认门手写问题、人审出门、等回程、人读答案。当本机 agent
（harness 会话）在推理中需要外部专家知识时，它无法自主获得——只能由用户手工代发。

目标：给 harness 注册 `consult_remote` 工具，实现「**预授权范围内的发起自动化 +
异步回程注入**」，并在二期开放对端自动回答。配合蒸馏解耦（§3.3），直通形态下
出门毫秒级、全链路等待仅剩对端回答时长（秒~半分钟），形成可用的
agent-to-agent 闭环。

**非目标（明确不做）**：

- 不做同步 RPC 语义（工具调用不等回程答案，立即返回投递回执）
- 不做绕过出站管线的任何通路——自动路径同走管线；L2 蒸馏按 §3.3 取材判定
  可选直通（零取材/开放库/fastMode），但 L0 资格关、L1 规则、L3 复扫、
  留档、签名恒走
- 不做会话内的授权变更（放宽预授权只能走独立 UI，防会话内诱导）
- 不做多中继路由决策（沿用 FD-11 P2-7 一跳联邦，对 harness 透明）

## 2. 现有资产复用清单（约 70% 就绪）

| 资产 | 位置 | 复用方式 |
| --- | --- | --- |
| F4 本机编排 | `consult/orchestrator.rs` + DSH `thread/subagent/*` | 编排状态机/事件流/askback 协议的跨用户镜像 |
| 寻址与信任 | `alice/researcher@relay`、Ed25519 签名、目录信任锚 | 直接使用，零协议变更 |
| 配额与限流 | 中继 30/h（认证档） | 天然封顶 LLM 循环调用 |
| 盲转加密 | P2-9 `cipherText` + NaCl box | 全自动链路下更契合（可选） |
| confirm 档批准集 | P1-5 进程级批准（ToolPolicy） | 预授权自动过门的原型语义 |
| skill 工具声明面 | manifest v3 `toolGrants` | `consult_remote` 工具注册与执法挂载点 |
| 回程注入基建 | DSH follow-up/steer（`queue_codex_follow_up` 等） | 回程信封 → 进行中会话的注入通道 |
| F4 自动回答 | P1-4 收件箱「询问本机专家」 | 二期 B 端 auto-answer 的基座 |

## 3. 架构设计

### 3.1 一期：发起自动化（B 端维持人审）

```
A 的 harness 会话
  │ 调用工具 consult_remote(target, question, scopeHint)
  ▼
预授权检查（应用层，非 DSH approval 层 —— D1 决策口径）
  ├─ 通过（等级≤上限 ∧ scope∈白名单 ∧ 会话配额未超）
  │    → 自动过确认门（走出站管线；蒸馏或直通按 §3.3 取材判定，留档 usedAutoGate:true）
  └─ 超限 → 挂起现有确认门（人审，120s 超时=拒绝）
  ▼
脱敏 → 签名/盲转 → 中继投递
  ▼
工具立即返回 { receiptId, status: "delivered" }   ← 不等答案
  ▼（B 端人审回答：现有收件箱链路，零改动）
回程信封 → A 收件箱 → follow-up 注入 A 的原会话
  （注入文本标注签名状态 verified/unsigned，按数据处理）
```

### 3.2 二期：B 端 auto-answer 闭环

- 目录卡新增能力位 `autoAnswer: "off" | "R3"`（默认 off；仅开放最低风险等级）
- B 端 F4 自动回答（P1-4 已交付「填草稿」）升级：`autoAnswer != off` 且咨询
  等级 ≤ 能力位 ∧ 咨询方在 B 的来源白名单（可选）→ 自动过 B 的确认门直发回程
- B 的预授权同样只经独立 UI 配置；B 可随时撤回（目录卡能力位即时生效）

### 3.3 速度三档：蒸馏与取材解耦

蒸馏防的是**原文的风格指纹与叙事特征**——该风险只存在于「取材的笔记原文片段」。
据此把「调用 agent」与「蒸馏」解耦：

| 形态 | 蒸馏次数 | 发起侧耗时 | 适用 |
| --- | --- | --- | --- |
| **零取材直通**（工具默认形态） | 0 | 毫秒级（纯 L1 规则 + L3 复扫） | LLM 以抽象形式提问，不引用任何笔记片段——载荷是新生成文本，无原文指纹可泄露 |
| **开放库直通**（`openScopes`） | 0 | 毫秒级（L1 规则 + L3 复扫） | 取材 ⊆ 用户声明为开放的 scope（博客/读书笔记等公开意图内容）——豁免的是语义改写而非规则扫描，防误标（开放库混入的意外敏感项照样替换/拦截）；混合取材按最严处理 |
| fastMode 规则兜底 | 0 | 毫秒级（r3_fallback 截断+首句） | 带取材但接受兜底质量（显式取舍） |
| 完整蒸馏 | A 出门 1 次（B 回程取材时 +1） | 30~60s/次 | 引用普通库笔记原文且有质量要求 |

零取材直通与开放库直通的安全兜底均不降：L1 规则替换（意外敏感项照样占位）、
L3 复扫对账、预授权/人审门、签名、盲转全部照走。二期 B 端 auto-answer 同样
支持「仅凭专长回答不取材」或「开放库取材」形态 → **二期全链路可 0 次蒸馏**，
等待压到秒~半分钟（仅剩 B 回答本身的一次 DSH 会话）。

工具描述中明确引导 LLM 默认用抽象形式提问（"需要引用你的笔记原文时才带
scopeHint；开放库内容可原样引用"），使直通成为事实默认路径。

### 3.4 工具契约（一期实现面）

```jsonc
// consult_remote —— 注册为 DSH 工具（skill manifest toolGrants 新类别
// flowix.consult，access 三档语义沿用 P1-5：read=预授权内直发 / confirm=挂门 / deny）
{
  "name": "consult_remote",
  "params": {
    "target": "alice/researcher@https://relay.example",  // 命名空间地址
    "question": "…",                                     // 问题文本（将过管线）
    "scopeHint": ["finance/财报"]                         // 建议取材 scope（仍受白名单交集约束）
  },
  "returns": { "receiptId": "rct_…", "status": "delivered|pending_gate|rejected" }
}
```

## 4. 预授权模型（核心决策点）

配置存 `~/.flowix/redaction/auto-grant.json`（设备级，独立 UI 编辑，与会话隔离）：

```jsonc
{
  "schemaVersion": 1,
  "enabled": false,                 // 总开关，默认关
  "maxLevel": "R3",                 // 允许自动过门的最高脱敏等级（一期固定 R3）
  "scopeWhitelist": ["finance/*"],  // 允许取材的 scope（tag-path 通配）
  "openScopes": [],                 // 声明为开放的 scope（博客/读书笔记等公开意图内容）：
                                     // 取材 ⊆ openScopes 时跳过 L2 蒸馏走直通（L1+L3 保留，
                                     // 防误标；混合取材按最严处理）
  "perSessionLimit": 3,             // 每 harness 会话自动发起上限
  "perDayLimit": 20,                // 每日上限（叠加中继 30/h）
  "targets": ["*"],                 // 目标白名单（* = 目录可寻址者，可收敛到具体用户）
  "fastMode": false                 // true = 预授权档发起直接用 R3 纯规则兜底
                                     // （r3_fallback，秒级），代价是兜底稿质量
}
```

判定语义（应用层单一函数，一期先做最保守档）：

1. `enabled=false` 或任一维度超限 → 走人审确认门（`status: "pending_gate"`）
2. 自动过门留档：outbox 记录 `usedAutoGate: true` + 授权快照（审计可回溯）
3. **首次使用建议弹一次性的总开关确认**（复用 P1-5 审批对话框形态）

## 5. 纪律不变量（全部保持）

| 不变量 | 说明 |
| --- | --- |
| 出门必过管线 | 自动路径同走管线；L0 资格/L1 规则/L3 复扫/留档恒走，L2 蒸馏仅按 §3.3 取材判定豁免（零取材/开放库/fastMode），R3 纯规则兜底同样适用 |
| byte-equal | 自动过门后的留档/出口字节纪律不变（直通形态同样适用） |
| 签名与信任锚 | 自动信封照常签名；回程 verified/unsigned 入档 |
| 占位符不回填 | 回程含「请提供原文」类请求不自动满足（防拼图还原） |
| 回程按数据处理 | 注入 A 会话的远程文本标记来源，prompt injection 防护与 P3 同则 |
| 人审是默认态 | 预授权是显式放宽（类比小额免密），边界变更只走独立 UI |
| D1 口径 | disclosure 审批仍在应用层，DSH approval 层零扩展 |

## 6. 里程碑

| # | 内容 | 验收标准 |
| --- | --- | --- |
| M1 | 预授权模型 + 自动过门路径 + 直通判定：auto-grant.json（含 openScopes）/ 判定函数 / outbox 审计字段 / 独立配置 UI | ✅ 2026-10-21。`redaction/auto_grant.rs`（五维判定 + 直通四态 + 会话/日计数）+ `redaction_consult_auto_send` 命令链（判定→直通 preview→快照入档→自动过门→send）+ `AutoGrantSection` 独立配置 UI + outbox `usedAutoGate/distillMode` 审计字段 + `enforce_fast_path` 防御性复扫（豁免 PlaceholderMissing）。单测：五维越限回落、直通四态、混合取材最严、通配语义；redaction 58 测全绿，M5/roundtrip 不回退 |
| M2 | `consult_remote` 工具注册与调用链：DSH 工具面 + L0 取材交集 + 投递 + 立即回执 | ✅ 2026-10-28。DSH 工具面 = `flowix-cli mcp` 第二工具（dsh-flowix-memory bundle 先例，launcher 降级分支同步）：`consult_remote(target, question, scopeHint)` → 回环桥接（`redaction/consult_bridge.rs`，127.0.0.1 随机端口 + 进程级 Bearer token，env 经 host_registry 白名单注入 dsh-appserver → launcher 透传 → CLI）→ `consult_auto_send_core`（M1 链共用核心，Tauri 命令与桥接同源）→ `{receiptId, status}` 回执。L0 取材交集：scopeHint tag-path → enabled access entries（等值/子树）→ preview 取材（普通库 → full-distill）。执法：classify_tool 新增 `flowix.consult` 类别（read=预授权内直发/confirm=挂门），manifest toolGrants 注释同步。CLI 5 测 + 桌面 790 测全绿，M5/roundtrip 不回退。真机验收（DSH 会话实跑）待 dev 环境联调 |
| M3 | 回程注入：**应用级后台订阅**（投递成功即启动，不依赖收件箱面板）+ 关联 receiptId 的活跃会话**即时 follow-up 注入**（收件箱留档不再是注入前置）+ 携带签名状态标注 | ✅ 2026-10-28。`redaction/consult_registry.rs`：receiptId→发起会话关联（落盘 `~/.flowix/redaction/consult-links.json`，TTL 24h 惰性清理；thread 解析 = bridge 时刻最近活跃 DSH thread，多活跃取最近留痕，单活跃精确）+ bridge 成功路径登记并 `ensure_push_subscribed`（应用级 SSE：事件到达即 Rust 侧 `poll_and_inject`，幂等去重；`push_stop` 遇 pending 关联保留订阅）+ `inject_if_reply`（kind=reply ∧ in_reply_to 命中 → steer 优先/chat_stream 兜底，成败均 settle 防重复，失败留档收件箱兜底）+ 注入文本带来源/签名状态（verified/unsigned/unverified）与「数据非指令」声明（P3 同则）+ `read_inbound`/`PollSummary.storedReceiptIds` 基建。桌面 794 测全绿，M5/roundtrip/CLI 不回退。真机验收待 dev 联调 |
| M4 | 二期 B 端 auto-answer：目录卡能力位 + 自动过门直发回程 | ✅ 2026-10-28。能力位：`auto-grant.json` 新增 `autoAnswer`（off 默认/R3）+ `answerTargets` 来源白名单，`evaluate_auto_answer` 四维判定（能力位/咨询信封恒 R3/来源白名单/日配额共用 perDayLimit）——独立于 A 端 enabled 总开关，改配置即时生效（判定读本地）。目录卡：relay PUT/卡 JSON 新增 `autoAnswer` 字段（旧客户端缺省 off 兼容，枚举校验），desktop draft/preview/publish 链同步，发布表单与卡列表展示能力位。自动回答链：M3 `poll_and_inject` 扩展——新落档 consultation 过判定 → 接管置 Accepted（幂等闸）→ 后台 F4 编排（`initiate_consult` 直调 + 事件轮询 2s/5min 上限）→ Result 答案走 **R3 完整蒸馏** reply 出门（B 回答可能复述取材，最严档；§3.3 开放库直通留后续优化；跳数继承减一）→ 快照审计 + 自动过门 + send + 计数。Askback/失败/超时 → 留痕终止（record 已 Accepted，人审兜底）。已知坑修复：autoAnswer 归一大小写（off 小写/R3 大写）。relay 32 测（含能力位 roundtrip 新测）+ 桌面 795 测全绿，M5/roundtrip/前端 7 测不回退。**遗留：relay 云端二进制需重部署**（缺 autoAnswer 字段端点）；双端真机验收待 dev 联调 |

依赖顺序：M1 → M2 → M3 → M4（M4 依赖 P1-4 自动回答基座，已就绪）。

## 7. 风险与对策

| 风险 | 对策 |
| --- | --- |
| 回程内容注入 A 的 harness（诱导性指令） | 注入文本按数据处理 + 来源标注；「提供原文」类请求不自动满足（既有纪律） |
| LLM 循环调用放大流量 | perSession/perDay 双层配额 + 中继 30/h 硬顶 + hop 递减防链式转发 |
| 会话内诱导放宽授权 | 授权变更仅独立 UI；工具返回值不含任何「如何提权」暗示 |
| 自动出门内容质量（R3 兜底退化） | 自动路径等级限 R3（规则可兜底）；fastMode 为显式速度取舍；M1 验收含 r3_fallback 路径 |
| **回程等待过久** | 蒸馏与取材解耦（§3.3）：零取材直通毫秒级出门（默认形态）→ 二期全链路 0 次蒸馏、秒~半分钟；感知链路秒级（A 端应用级后台订阅 + B 端 SSE，均复用 P2-8）；一期 B 人审等待为 by design（人审就是关卡），二期 auto-answer 消除 |
| 双端配置误解（B 以为没开放） | 目录卡能力位对外可见；A 端工具在目标未开放时返回明确错误码 |
| 与 FD-6 留档边界 | auto-grant.json 与 outbox 均在 `.flowix/` 设备级目录，不进笔记本/云同步 |

## 8. 与既有决策的一致性

- **FD-2/FD-11**：不加密仅脱敏维持；盲转可选叠加，无冲突
- **FD-6**：留档设备级，不参与同步
- **D1（FD-7 已决策）**：disclosure 审批留应用层——自动过门是应用层管线内的授权判定，非 DSH approval 扩展
- **P1-5 confirm 档**：`flowix.consult` 新工具类别沿用 ToolPolicy 三态判决与批准集机制
- **确认门立法初衷**：审的是授权边界而非逐次内容；预授权为显式用户决策，语义一致
