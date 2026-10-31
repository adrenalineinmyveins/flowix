# Flowix Agent 体系评估：系统内 Agent 与外部 Agent 的差异

| 项 | 内容 |
| --- | --- |
| 评估日期 | 2026-10-29 |
| 基线代码 | `af9c1d8c`（trae/agent-dRBdg4） |
| 评估范围 | agent runtime 归属、人格资产管理、市场发布/订阅/咨询能力（AGENT-MARKET M1-M4）、记忆与授权模型 |
| 方法 | 代码事实核查（Rust desktop src + flowix-web types），无推测结论 |

## 0. 结论摘要

1. **runtime 层已无"内置"与"外部"之分**：[commands/agent/runtime.rs:7-9](../app/flowix-desktop/src/commands/agent/runtime.rs#L7-L9) 明确注释 *"The former built-in runtime was removed; every supported agent is an external CLI runtime now"*。历史上基于 rllm / `openai_compatible` provider 的进程内 runtime 已整体移除（`src/providers/` 目录不复存在，`ExternalRuntimeKind` 是唯一 chat 派发目标）。CLAUDE.md 架构图中 `providers/` 一节已过时。
2. **真正的差异在"人格资产管理层"**：所有对话 agent 共享同一套外部 runtime 注册表；区别是 Flowix 是否为其维护 **nurture profile**（`.agents/*/profile.md`）。有 profile 的成为「系统内个人 Agent」（可喂养、可发身份卡、可被订阅与咨询）；无 profile 的只是普通外部会话（thread card）。
3. **市场订阅的对象是"身份卡"，两类入口的订阅能力因此不对等**：只有 profile 化的 agent 可发布/被订阅；直开的外部 agent 会话不构成可订阅对象（无中继地址）。M4 的 `@` 提及与「添加内容」引用均以订阅快照为数据源，能力边界与订阅一致。

## 1. 分类澄清

当前代码中与"agent"相关的三个类别：

| 类别 | 定义 | 代码锚点 |
| --- | --- | --- |
| A. 系统内个人 Agent | notebook `.agents/<id>/profile.md` 定义的 nurture agent；runtime 仍为外部 CLI，但被 Flowix 全生命周期管理（人格/喂养/发布/应答） | [nurture/mod.rs:113](../app/flowix-desktop/src/nurture/mod.rs#L113)、[nurture/runner.rs:184-282](../app/flowix-desktop/src/nurture/runner.rs#L184-L282) |
| B. 外部 agent 会话 | 笔记/对话列表直接新建的 codex / claude / pi / dsh 等 thread card 会话；无 profile、无人格沉淀 | [types/agent.ts:14-24](../app/flowix-web/types/agent.ts#L14-L24) |
| C. Mobile 云同步会话 | 跨端只读回放会话，OpenAI 兼容 SSE 直连，无本地 CLI 子进程 | [runtime_registry.rs:37-38](../app/flowix-desktop/src/agent_external/runtime_registry.rs#L37-L38) |

附：前端 `AgentTypeKey` 另含 `gemini` / `openclaw`，但 Rust 侧 `ExternalRuntimeKind::parse` 不接受（仅 7 种），二者为 `coming-soon` 预告位，不可用。

## 2. Runtime 层事实（无差异区）

`ExternalRuntimeKind` 注册表（[runtime_registry.rs:30-75](../app/flowix-desktop/src/agent_external/runtime_registry.rs#L30-L75)）：

| key | runtime | 形态 |
| --- | --- | --- |
| `codex` | Codex | 本地 CLI 子进程（app_server 协议） |
| `claude` | Claude | 本地 CLI 子进程 |
| `hermes` | Hermes | 本地 CLI 子进程 |
| `opencode` | OpenCode | 本地 CLI 子进程 |
| `pi` | Pi | 本地 runtime |
| `deepseek-harness`（`dsh`） | DeepSeek Harness | 本地 harness（launcher → flowix-cli MCP 工具桥） |
| `mobile` | Mobile | 云同步会话直连（非子进程） |

关键事实：

- **chat 派发单出口**：`dispatch_chat(thread_id, runtime, message, app)` 统一走 `ExternalCliRuntime::chat_stream` trait（[runtime_registry.rs:79-98](../app/flowix-desktop/src/agent_external/runtime_registry.rs#L79-L98)），A 类与 B 类在此层完全同构。
- **profile 的 `agent_type` 同样映射到该注册表**：[nurture/runner.rs:281-282](../app/flowix-desktop/src/nurture/runner.rs#L281-L282) `ExternalRuntimeKind::parse(profile.agent_type)`，留空回落 `default_agent_id()`。即"系统内个人 Agent"的应答引擎就是外部 CLI。
- **数据落盘差异（runtime 自身属性，非 A/B 差异）**：provider-owned runtime（DSH/Codex 等）的 live chunk 不落本地（[runner.rs:361](../app/flowix-desktop/src/nurture/runner.rs#L361) 注释），会话数据主权在 provider 侧。

## 3. 管理层差异对比（核心差异区）

| 维度 | A. 系统内个人 Agent | B. 外部 agent 直开会话 |
| --- | --- | --- |
| 本质 | 人格资产（profile）+ 按需会话 | 一次性/持续会话 |
| 人格定义 | `flowix_display_name / bio / expertise / system / tools / skills / model / agent_type` 固化于 profile.md（[agent/mod.rs:84-109](../app/flowix-core/src/agent/mod.rs#L84-L109)） | 会话级配置（模型/权限/工作区），无跨会话人格 |
| 记忆与喂养 | nurture 事件喂养，`flowix_memory` tag 闸门（D-4：喂养输入范围 = 记忆库 = 对外披露范围，[nurture/mod.rs:96-110](../app/flowix-desktop/src/nurture/mod.rs#L96-L110)） | 无喂养、无记忆沉淀 |
| 身份卡发布 | ✅ `market_my_cards` 遍历全部 profile，不按 agent_type 过滤（[directory.rs:339-345](../app/flowix-desktop/src/redaction/directory.rs#L339-L345)）；支持 发布/更新/下架（private 重发） | ❌ 无 profile 即无发布入口 |
| 被订阅 | ✅ 发布后其他用户可搜索订阅（M3，快照含 relayUrl/公钥/autoAnswer） | ❌ 无中继地址 |
| 被咨询（consult_remote） | ✅ 异步投递 → 卡主侧由该 profile 起外部 runtime 会话应答；`autoAnswer=off` 人工确认 / `R3` 满足披露级自动答（[auto_grant.rs:224-231](../app/flowix-desktop/src/redaction/auto_grant.rs#L224-L231)） | ❌ |
| 编辑器引用（M4） | ✅ 订阅方 `@` 提及 / 「添加内容」注入 `agentReference` chip，点击复制完整地址 | ❌（非订阅对象） |
| consult_targets 枚举 | ✅ 订阅快照并入 targets（M3），本地个人 agent 短地址自动补全 | ❌ |
| 入口 | 「个人 Agent」页 / 收件箱咨询 / 市场 | 笔记内 thread card / 「对话」列表新建 |
| 披露控制 | `flowix_redaction_level` + 记忆库授权（非对称闸门，写权执法） | thread 级权限（完全访问/确认等），无披露级概念 |
| 应答成本 | 卡主侧 runtime 自负（provider-owned） | 使用者自负 |

## 4. 市场与订阅能力评估

承接"外部 agent 可以订阅吗"的结论并展开：

1. **问题等价转换**：由于 runtime 层无差异，"订阅一个外部 agent" 实际等价于 "订阅一张由外部 runtime 驱动的身份卡"。判断标准只有一条——**卡主是否把它 profile 化并发布**。
2. **订阅语义是"人格 + 授权 + 异步咨询"，不是"接管进程"**：订阅方获得的是卡快照（displayName / expertise / 公钥 / autoAnswer / 最低披露级），咨询经 relay 异步投递，外部 agent 始终运行在卡主机器上，不暴露端口与 API key。
3. **能力闭环现状**：
   - 发布侧：M1（发布/下架/漂移检测）✅
   - 订阅侧：M3（订阅/退订/刷新 + consult_targets 并入）✅
   - 引用侧：M4（@ 混排 + 添加内容 + 行内 chip 复制完整地址）✅，实机三入口验证通过（见 [AGENT-MARKET-M4-VERIFICATION.md](./AGENT-MARKET-M4-VERIFICATION.md)）
   - 未做：订阅更新推送（开放项 O1，SSE/轮询待 relay 演进）、市场分页/热度排序（O2）
4. **B 类会话的资产化路径**：直开会话若要进入市场体系，需先为其创建 profile（当前 UI 无"会话 → 另存为个人 Agent"入口，属潜在增强项）。

## 5. 优势 / 局限 / 适用场景

**A 类（系统内个人 Agent）**
- 优势：人格可沉淀、记忆可积累、可资产化（发布/订阅/被咨询形成跨用户协作网络）、披露授权精细。
- 局限：需维护 profile 与记忆库；应答受 autoAnswer 闸门与对方在线状态约束；live chunk 不落本地，历史主权弱。
- 适用：长期专家型角色（财报解读、读书助手等）、希望被他人订阅咨询的知识服务。

**B 类（外部 agent 直开会话）**
- 优势：零设置即用、会话级权限/工作区灵活、与 CLI 生态原生对齐（codex/claude 等自有工具链）。
- 局限：无人格沉淀、无跨会话记忆、不可进入市场体系、不可被 `@` 引用。
- 适用：临时任务型工作（编码、批处理、问答），用完即走。

## 6. 风险与建议

| # | 事项 | 建议 |
| --- | --- | --- |
| R1 | CLAUDE.md 架构图仍描述 `providers/`（rllm/openai_compatible）为内置代理层，与代码不符（已移除） | 更新架构文档，避免误导后续开发 |
| R2 | "会话 → 个人 Agent" 无转化入口，B 类用户资产化断层 | 候选增强：thread card 菜单增加"保存为人格"（生成 profile.md 草稿） |
| R3 | 前端 `AgentTypeKey` 与 Rust `ExternalRuntimeKind` 双列表维护（gemini/openclaw 仅前端） | 保持 coming-soon 标注一致性，落地产出时同步两侧 |
| R4 | 订阅快照无失效通知（卡主下架/改名后订阅方无感知，仅手动刷新可见） | 依赖 O1（事件推送）演进；短期可在快照展示 `refreshedAt` 提示陈旧度 |

## 7. 证据索引

| 事实 | 锚点 |
| --- | --- |
| 内置 runtime 已移除，全部外部化 | [runtime.rs:7-9](../app/flowix-desktop/src/commands/agent/runtime.rs#L7-L9) |
| 7 种外部 runtime 注册表 | [runtime_registry.rs:30-75](../app/flowix-desktop/src/agent_external/runtime_registry.rs#L30-L75) |
| profile → 外部 runtime 映射 | [nurture/runner.rs:184-282](../app/flowix-desktop/src/nurture/runner.rs#L184-L282) |
| 身份卡不按 agent_type 过滤 | [directory.rs:339-345](../app/flowix-desktop/src/redaction/directory.rs#L339-L345) |
| 记忆库喂养闸门（D-4） | [nurture/mod.rs:92-110](../app/flowix-desktop/src/nurture/mod.rs#L92-L110) |
| autoAnswer 闸门 | [auto_grant.rs:224-231](../app/flowix-desktop/src/redaction/auto_grant.rs#L224-L231) |
| AgentProfile 字段 | [flowix-core/agent/mod.rs:84-109](../app/flowix-core/src/agent/mod.rs#L84-L109) |
| 前端 AgentTypeKey（含 coming-soon） | [types/agent.ts:14-24](../app/flowix-web/types/agent.ts#L14-L24) |
| M4 实机验证 | [AGENT-MARKET-M4-VERIFICATION.md](./AGENT-MARKET-M4-VERIFICATION.md) |
