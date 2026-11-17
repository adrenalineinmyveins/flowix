# Agent Skills 跨用户应用能力评估

> 版本：v1.0（2026-11-10）
> 评估对象：在 agent（身份卡 profile）中配置 skills，使其他用户经远程 consult 调用此 agent 时应用 skill 能力的可行性与实施路径
> 结论：**可行，基础设施已备，缺运行时消费层**。推荐方案 A（提示词注入知识型技能）先行打通闭环

---

## 1. 背景与目标

Flowix 身份卡体系已支持跨用户咨询：A 机用户经 `consult_remote` → 中继 → B 机 auto-answer 编排（DSH 子代理）→ 回程签名。当前被咨询 agent 的人格（system）、专长（expertise）、长期记忆目录（memory）均已注入咨询提示词，但 **skills 未接入**。

目标：agent owner 在 profile 中声明的 skills，在其他用户咨询该 agent 时真实生效——知识型技能指导回答质量，工具型技能提供执行能力。

## 2. 现状盘点（证据）

| 层 | 现状 | 证据 |
|---|---|---|
| 数据层 | `flowix_skills` 字段已存在，含项数/长度校验、frontmatter 序列化往返 | `flowix-core/src/agent/mod.rs:110-111`（字段）、`:158`（限额校验）、`:290-291`（序列化） |
| CRUD | MCP 工具 `agent_profile` 与 UI 读写全通（trim/去空） | `flowix-desktop/src/commands/agent_profile.rs:255,296` |
| 资产层 | 技能注册表聚合插件技能（source/kind/发布状态/启用态）+ 调用统计（`skill-usage.jsonl`）；DSH 侧技能聚合预留 `list_dsh_skills` 注入口（当前为空） | `flowix-desktop/src/skill_registry.rs:26-51` |
| 发现层 | 目录卡结构已预留 `skills` 字段位（当前恒空） | `flowix-desktop/src/redaction/directory.rs:565` |
| **运行时** | **零消费**：全 desktop 代码中 `.skills` 的读取点仅存在于 CRUD 与单测——本地 chat 派发不读，consult 编排不读 | 全仓 grep 交叉验证（2026-11-10） |
| DSH 侧 | DSH 会话有自身 skills 注入机制（AGENTS.md/skills 全上下文注入），但是**设备/会话级**装配，非 per-agent | `redaction.rs:1310` 注释（token 用量观测） |

**结论**：`flowix_skills` 目前是纯声明性字段——"配了但不生效"。

## 3. 远程链路断点分析

其他用户咨询此 agent 的完整链路与 skills 断点：

```
咨询方 A 机                        被咨询方 B 机（生效现场）
consult_remote → relay → run_auto_answer → build_consult_prompt → thread/subagent/spawn → DSH 子代理 → reply
                                       ✅ system/expertise/memory 已注入
                                       ❌ skills 无通道（spawn 仅传 parentThreadId + prompt）
```

- `consult/orchestrator.rs:316-317`：`thread/subagent/spawn` 参数仅 `parentThreadId` + `prompt`
- `consult/mod.rs build_consult_prompt`：注入点已存在，扩展面清晰
- 子代理按 DSH 设备级默认环境装配，owner 的 skills 声明完全不参与

## 4. 方案分层

### 方案 A：提示词注入（推荐先行）

在 `build_consult_prompt` 中按 `to_profile.skills` 读取技能正文（经 skill_registry / 插件目录解析为 markdown），以 `<skill>` 块注入提示词。

- **生效范围**：agent-markdown 类知识型技能（流程指导、方法论、检查清单）——正文注入即真实生效
- **改动面**：`consult/mod.rs` 一个函数 + 技能正文解析辅助 + 单测；估算最小
- **限制**：artifact-tool 类技能无执行能力（仅"知道"而不能"做"）
- **超长控制**：复用 profile 现有 skills 限额（条数/单项长度），注入总量截断保护

### 方案 B：MCP 工具按 agent 白名单开放（二期）

DSH 子代理的 MCP 工具集（memo / consult_remote / 插件产物工具）目前设备全局挂载。按被咨询 agent 的 skills 白名单在编排会话内开放对应工具（`dsh_plugin_settings_path` 已有挂接点，`manager.rs:1323`）。

- **生效范围**：artifact-tool 类技能（生成思维导图、读集合数据等）
- **改动面**：DSH 会话工具装配链 + 编排参数；中等
- **前置**：需明确编排子会话的工具注入机制边界（spawn 协议是否支持 per-thread 工具集）

### 方案 C：DSH 原生 skills 通道（待验证）

若 DSH `thread/subagent/spawn` 协议原生支持 skills 参数，编排透传即可。本机无 DSH 源码（`.build/upstream` 未拉取，runtime 为打包态），**暂不可证**；可作为向上游的验证项/PR。

### 对比

| 维度 | A 提示词注入 | B 工具白名单 | C 原生通道 |
|---|---|---|---|
| 知识型技能 | ✅ 完整生效 | — | 取决于上游 |
| 工具型技能 | ❌ 仅声明 | ✅ | 取决于上游 |
| 改动量 | 小 | 中 | 未知 |
| 上游依赖 | 无 | 弱 | 强 |
| 风险 | 提示词膨胀（可控） | 工具面暴露收敛 | 不可控 |

## 5. 配套设计点（不可省略）

1. **披露语义**：skills 是 owner 的能力声明，对外暴露纳入既有 redaction 体系：
   - 技能名 + 描述 → 目录卡（发现层，字段位已留，随发卡透出）
   - 技能正文是否随咨询披露 → 按 R1–R3 等级裁剪（R3 抽象态默认只透技能名）
2. **咨询方感知**：回答回流"本次应用了哪些技能"元数据（`ConsultEvent::Result` 扩展或 `artifact_suggestion` 通道先例），否则调用方无法验证 skill 被真实使用
3. **本地一致性**：本地对话（chat 派发）与远程咨询（consult 编排）对 skills 的消费应收敛为同一注入模块，避免两套语义
4. **用量归因**：技能被远程咨询应用时计入 `skill-usage.jsonl`（skill_registry 统计口径扩展 remote 来源）

## 6. 实施分期

| 期 | 内容 | 验收 |
|---|---|---|
| P1 | 方案 A：consult 提示词注入知识型技能 + 目录卡 skills 透出 | 黑盒验证：被咨询 agent 按技能方法论组织回答，虚构技能内容可被命中；目录卡可见技能清单 |
| P2 | 技能应用元数据回流 + skill-usage 远程归因 | 咨询方收到"已应用技能"标注 |
| P3 | 方案 B：artifact-tool 工具按 agent 白名单开放 | 远程咨询可产出插件产物（如思维导图） |

## 7. 风险与未决事项

- **提示词膨胀**：多技能注入挤占上下文——需总量上限与截断策略（DSH 每 step 全上下文重发，成本敏感）
- **技能正文泄露面**：注入 prompt 即进入 B 机会话日志（zstd 落盘）与潜在回程——披露等级裁剪必须与注入同步设计
- **DSH 上游**：方案 C 可行性未验证；若上游支持，A/C 可合并为透传
- **`list_dsh_skills` 空实现**：DSH 侧技能聚合注入口待上层运行时接通（skill_registry 已预留）
