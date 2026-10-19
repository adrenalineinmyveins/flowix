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
| F1 Profile | 数据模型 + 接线 + 卡片 UI | ✅ | `flowix-core/src/agent/`、`.agents/` 双数据面、AgentState 记账（含 token，2026-10-18 接线）；卡片 UI（2026-10-17）：`agent_profile_update` 扩展隐私边界/model 可选参数，前端 `agent-profile-cards.tsx` 列表+详情+隐私三字段编辑，入口在会话标题栏「个人 Agent…」；（2026-10-18）面板并入中列视图切换组第四 tab，与访问空间/卡片/对话并列，并对齐中列统一格式（--list-bg、避让标题行、OverlayScrollbar、i18n、Radix 删除确认、@[440px] 容器查询），全视图标题避让 pl-[100px]→pl-[126px] |
| F2 喂养 | N1-N3 + 多 agent 映射 | ✅ | `nurture/{mod,runner}.rs`、disclosure_scope 门控、水位增量 |
| F3 技能 | manifest v3 + knowledgeScope + P2 导入导出 + P4 执法 | ✅ | `flowix-plugin-runtime/src/manifest.rs`、skill_registry；P2（2026-10-18）：`plugin_export/plugin_import`（zip 往返 byte-equal + zip-slip 防护）；P4：`ToolPolicy` 类别级黑名单执行器（越权 tool_call 终止 run 留痕）+ `effective_target_level` 与 preview 联动 |
| F4 本机互咨 | mailbox + BudgetGuard + AskBackGate + DSH 编排 + 咨询记账 | ✅ | `consult/{mod,orchestrator}.rs`、`commands/consult.rs`；被咨询次数/lastConsultedAt 落 AgentState |
| F6 脱敏 | M1 规则地基 | ✅ 验收过 | `flowix-redaction` crate：对抗样本命中率 100%、占位符防毒化 |
| F6 脱敏 | M2 蒸馏 + L3 复扫 | ✅ 验收过 | `redaction/mod.rs`：注入样本拦截 100%、回炉上限 2 |
| F6 脱敏 | M3 确认门 + outbox | ✅ 验收过 | `redaction/outbox.rs`：UNCONFIRMED_SEND_BLOCKED 兜底、byte-equal、FIFO 200 |
| F5 网络互咨 | FD-10 全里程碑 M1–M4b | ✅ | M1 协议 v1 + M2 `flowix-relay` 参考实现（14 测试）+ M3a 入站收件 + M3b 出站 wire v1（byte-equal）+ M3 前端收件 UI + M3c 回程信封（kind=reply）+ M4a 真机收件闭环（注册/拉取/接受/ack）+ M4b 网络往返测试（`tests/network_roundtrip.rs`：内嵌 relay 全链路 A 发起→B 收件→回程→A 收 reply + 跳数递减；顺带修复 wire redactionLevel 小写被 422 拒收、发起侧 sender 自动取注册 agent）。遗留（v2）：F4 编排自动回答、发起身份选择器 UI、E2E 加密 |

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

- **现状**：客户端传输切片已落地（2026-10-18）——`redaction_transmit` IPC →
  `redaction/transport.rs`：从唯一出口 `take_confirmed_payload` 取字节 POST 到
  `redaction/config.json` 的 `relayUrl`；draft 沿 `UNCONFIRMED_SEND_BLOCKED` 兜底，
  投递失败留档保持 confirmed（幂等可重试）。确认门 UI 在 relay 配置后提供
  「投递到中继 / 重试投递」。
- **待做**：对端协议与云端中继建设（AGENT-NETWORK-PLAN §5 / FD-10 立项）；
  `disclosure.hopsRemaining` 的消费契约（见 §4 未决 3）；FD-7 approval 类别。

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

新增 disclosure 类别 vs 复用 permissions 类别——影响 dsh-appserver 改动面，
实施 F5 对端协议时定。

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
