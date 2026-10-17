# Agent 网络遗留事项（Backlog）

> 状态快照：2026-10-17。F6 脱敏管线三个里程碑（M1 规则地基 / M2 蒸馏复扫 /
> M3 确认门留档）Rust 侧全部完成并入库，本文记录跨域完成度与未竟事项。
> 关联设计文档：[AGENT-NETWORK-PLAN.md](AGENT-NETWORK-PLAN.md)（总规划）、
> [REDACTION-PIPELINE.md](REDACTION-PIPELINE.md)（F6）、
> [AGENT-PROFILE-MODEL.md](AGENT-PROFILE-MODEL.md)（F1）、
> [AGENT-NURTURE-PIPELINE.md](AGENT-NURTURE-PIPELINE.md)（F2）、
> [AGENT-SKILL-SYSTEM.md](AGENT-SKILL-SYSTEM.md)（F3）、
> [AGENT-LOCAL-CONSULT.md](AGENT-LOCAL-CONSULT.md)（F4）。

## 1. 完成度矩阵

| 域 | 里程碑 | 状态 | 关键落点 |
| --- | --- | --- | --- |
| F1 Profile | 数据模型 + 接线 | ✅ | `flowix-core/src/agent/`、`.agents/` 双数据面、AgentState 记账（token 除外） |
| F2 喂养 | N1-N3 + 多 agent 映射 | ✅ | `nurture/{mod,runner}.rs`、disclosure_scope 门控、水位增量 |
| F3 技能 | manifest v3 + knowledgeScope | ✅（运行时消费见 P1-2） | `flowix-plugin-runtime/src/manifest.rs`、skill_registry |
| F4 本机互咨 | mailbox + BudgetGuard + AskBackGate + DSH 编排 | ✅ | `consult/{mod,orchestrator}.rs`、`commands/consult.rs` |
| F6 脱敏 | M1 规则地基 | ✅ 验收过 | `flowix-redaction` crate：对抗样本命中率 100%、占位符防毒化 |
| F6 脱敏 | M2 蒸馏 + L3 复扫 | ✅ 验收过 | `redaction/mod.rs`：注入样本拦截 100%、回炉上限 2 |
| F6 脱敏 | M3 确认门 + outbox | ✅ 验收过 | `redaction/outbox.rs`：UNCONFIRMED_SEND_BLOCKED 兜底、byte-equal、FIFO 200 |
| F5 网络互咨 | 协议 + 传输 | ⛔ 未启动 | 解锁条件已满足（原门槛「F6 M1-M3 验收」已过） |

## 2. P0 —— 端到端闭环缺口

### 2.1 前端确认门卡片与 scope 选择器

- **现状**：IPC 全部就绪——`redaction_preview`（question/scope/level 全管线）、
  `redaction_confirm`、`redaction_confirm_wait`（gate 挂起）、`redaction_send`
  （唯一出口）、`redaction_outbox_list`、`redaction_config_get/set`、
  `redaction_l1_preview`（诊断）。
- **待做**：确认门卡片（复用审批卡片交互：最终载荷全文 + 占位符/规则命中清单 +
  目标等级 + 收件方卡片）；scope 选择器扩展标签档（`AgentAccessKind::Tag`）。
- **完成判据**：真机上 preview → 人审 → confirm → send 全程可点，取消/超时
  卡片状态正确回落。

### 2.2 F5 网络传输层

- **现状**：发送唯一出口已定——`redaction_send` → `take_confirmed_payload`
  （`UNCONFIRMED_SEND_BLOCKED` 兜底、字节与留档 byte-equal）。信封结构
  `RedactionEnvelope`（receiptId/payload/targetLevel/ruleSetVersion/createdAt）
  已确定性序列化。
- **待做**：按 AGENT-NETWORK-PLAN §5 实现对端协议与传输；发送路径只允许接在
  上述出口；`disclosure.hopsRemaining` 的消费契约（见 §4 未决 3）。

### 2.3 dsh-appserver 构建与模型凭证

- **现状**：L2 蒸馏接线完成（复用 `run_distill_once`，title=`redaction:distill`），
  但沙箱内 `dsh-appserver` 仅有源码未构建、无模型 API 凭证。
- **完成判据**：真机跑通一次真实 L2 蒸馏（任意等级），验证蒸馏稿过 L3 复扫。

## 3. P1 —— 规格内未完成项

### 3.1 token 用量 → AgentState 记账

`AgentState.record_tokens` 已就位，等待 `agent-chunk` usage 事件接线后灌入
（当前 runs/consultations/recentThreadIds 已记账，tokensIn/tokensOut 恒 0）。

### 3.2 manifest v3 运行时消费

`toolGrants` / `minRedactionLevel` 已在 manifest 校验层落地（R1-R3 校验、
工具白名单、shell+confirm 需 allowlist），运行时尚未消费：
- `minRedactionLevel` 应与 `redaction_preview` 的 `target_level` 联动取 max；
- `toolGrants` 应在插件工具调用处做执法。

### 3.3 DSH approval 的 disclosure 审批类别（规格 §9 未决 1）

新增 disclosure 类别 vs 复用 permissions 类别——影响 dsh-appserver 改动面，
实施 F5 时定。

### 3.4 蒸馏 Agent 类型选择（规格 §9 未决 4）

当前硬编码 `deepseek-harness`。建议：跟随用户当前默认 Agent，
`redaction/config.json` 可覆盖（可加 `distillAgentType` 字段）。

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
