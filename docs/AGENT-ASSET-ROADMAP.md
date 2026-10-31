# Agent 体系资产化计划（基于系统内/外部 Agent 评估）

| 项 | 内容 |
| --- | --- |
| 输入 | [AGENT-EVALUATION-SYSTEM-VS-EXTERNAL.md](./AGENT-EVALUATION-SYSTEM-VS-EXTERNAL.md)（2026-10-29 评估，基线 `af9c1d8c`） |
| 目标 | 消除评估发现的文档失真与类型失配（R1/R3），补齐"会话 → 人格资产"断层（R2），提升订阅体验（R4/O6），并为订阅推送（O1）预留演进位 |
| 原则 | 沿用 AGENT-MARKET 里程碑制：每期独立可验收、可提交；不做无验收标准的投机性工作 |

## 0. 现状基线

- runtime 层已全外部化（7 种 `ExternalRuntimeKind`），无内置路径
- 市场 M1-M4 完成：发布 / 订阅 / consult_targets / `@` 提及与「添加内容」引用，实机三入口验证通过
- 人格资产层（nurture profile）与市场体系已闭环；**唯一断层**是普通会话（B 类）无法转化为 profile

## 1. 工作项与优先级

### P0-A 文档一致性修正（R1）

| 项 | 内容 |
| --- | --- |
| 范围 | `CLAUDE.md` 架构图删除 `providers/`（openai_compatible/rllm/tools）一节；`agent.rs` / `prompt/` 条目同步移除；补 `agent_external/`（7 runtime + runtime_registry）、`nurture/`、`redaction/`（directory/subscriptions/consult_bridge）实际结构 |
| 验收 | 架构图与 `src/` 目录逐条对得上；无已删除模块的残留引用 |
| 规模 | 纯文档，半天内 |

### P0-B 类型双列表对齐（R3）

| 项 | 内容 |
| --- | --- |
| 范围 | 前端 `AgentTypeKey`（types/agent.ts）与 Rust `ExternalRuntimeKind::parse` 的合法值集合一致性：为 `gemini` / `openclaw` 保留 `coming-soon` 标注，其余 key 建立**编译期/测试期校验**（单测断言两侧集合相等：Rust 侧导出常量，前端测试文件硬编码对照，或生成脚本同步） |
| 验收 | 新增/删除 runtime 类型时，两侧不一致会让测试红；coming-soon 项不受误报 |
| 规模 | 小，1 个测试文件 + Rust 侧导出 |

### P1-C 会话 → 个人 Agent 转化入口（R2，本计划核心增量）

| 项 | 内容 |
| --- | --- |
| 动机 | B 类会话无人格沉淀是资产化断层；评估确认用户价值（用完即走的会话中沉淀出可喂养、可发布的人格） |
| 方案 | thread card 头部「更多」菜单新增**「保存为个人 Agent」**：读取当前会话的首条用户消息/标题/模型/权限配置 → 生成 `.agents/<id>/profile.md` 草稿（display_name 预填、expertise 空、system 取会话配置摘要、agent_type 继承本会话 runtime key）→ 跳转「个人 Agent」页编辑态 |
| 数据面 | 复用 `flowix_core::agent::AgentProfile::parse/serialize` 与 nurture `.agents` 目录约定；不复制会话历史（人格与历史分离，历史仍属 thread） |
| 约束 | 同名 agent_id 冲突时自动后缀；创建后仅提示不自动发布（发布仍走 M1 链路与人审） |
| 验收 | ① codex/pi/dsh 任一会话可一键产出 profile.md 并出现在「个人 Agent」页；② 产出的 profile 可正常走 M1 发布链（market_my_cards 可见）；③ 会话本身不受影响；④ i18n zh/en |
| 测试 | Rust：profile 生成（字段映射/冲突后缀/坏输入兜底）；前端：菜单项渲染与回调接线 |

### P1-D 订阅快照陈旧度提示（R4 短期方案）

| 项 | 内容 |
| --- | --- |
| 范围 | 「我的订阅」分区与 `@` 下拉的 agent 行展示 `refreshedAt` 相对时间（如 "3 天前快照"），超过阈值（7 天）显示可点击的刷新图标（复用 M3 `market_refresh_subscription`） |
| 验收 | 快照时间渲染正确；点击刷新后 `refreshedAt` 更新且 `@` 缓存失效（`invalidateMentionAgents` 路径已有） |
| 规模 | 小 |

### P2-E 订阅列表提升为左列分组（O6）

| 项 | 内容 |
| --- | --- |
| 方案 | 左列「个人 Agent」页顶部增加「已订阅」分组（只读卡片，显示 displayName/expertise/完整地址复制/退订入口），与本地可编辑卡视觉区分（订阅角标） |
| 依赖 | P1-D（时间提示组件复用） |
| 验收 | 订阅分组与市场页订阅区数据一致；退订双向同步；视觉区分明确 |
| 暂缓条件 | 若 P2 周期紧张可延后（市场页内分区已可用） |

### P2-F 订阅更新推送（O1，外部依赖，占位）

| 项 | 内容 |
| --- | --- |
| 现状 | relay 无事件推送端点；一期不做 |
| 演进位 | relay `get_events`（SSE）就绪后：订阅端监听卡变更 → 快照失效标记 → 下次打开弹层/`@` 时惰性刷新；协议设计需与 relay 侧对齐（跨仓库） |
| 验收（届时） | 卡主改名/下架后，订阅方在事件到达后 1 次交互内看到新状态或失效提示 |

## 2. 里程碑划分

| # | 内容 | 验收 | 依赖 |
| --- | --- | --- | --- |
| N1 | P0-A 文档修正 + P0-B 类型对齐 | ✅ 2026-10-30：CLAUDE.md 架构与 src/ 逐条核对重写（删 providers/prompt/agent.rs 等失真条目，补 agent_external/nurture/redaction/commands 实况 + flowix-web entrypoints/features 结构）；`AGENT_TYPE_KEYS` 常量化 + types/agent.test.ts 三断言（去重/Rust 覆盖/仅 coming-soon 白名单差异）；tsc 通过 | 无 |
| N2 | P1-C 会话转人格入口 | ✅ 2026-10-30：复用既有 `agent_profile_create` IPC（随机 agt_ id 天然无冲突，原计划 TA3 的"同名后缀"由"独立 id 不覆盖"语义替代并加 Rust 测试）；thread card 头部「保存为个人 Agent」按钮（mobile 隐藏 K2）→ 创建 profile 草稿 → toast + 跳转个人 Agent 页；Rust 11/11 + dom-factory 接线测试 + thread-card 回归 88/88 | N1 |
| N3 | P1-D 陈旧度提示 + P2-E 已订阅分组 | ✅ 2026-10-30：市场页订阅行显示「快照更新于 {time}」（>7 天警示色）；@ 下拉 agent 行内 compact 相对时间（同阈值变色）；个人 Agent 页顶部「已订阅」只读分组（BookmarkCheck 区分本地卡，点击复制完整地址）；market 8/8 + profile-cards 8/8 回归 | N2 |
| N4 | P2-F 占位转正式（仅当 relay 演进就绪） | ⏸ 占位（relay get_events 未就绪） | relay 侧 |

## 3. 测试锚点

| # | 输入 → 预期 |
| --- | --- |
| TA1 | `AgentTypeKey` 合法集 vs `ExternalRuntimeKind::ALL` 集合断言（coming-soon 白名单除外） |
| TA2 | 会话（title=空 / 模型=inherit / 权限=确认模式）→ 保存为人格 → profile.md 字段映射正确、无历史复制 |
| TA3 | 已存在同名 agent_id → 自动 `-2` 后缀，不覆盖原 profile |
| TA4 | 产出 profile 经 `market_my_cards` 读取 → `publishState: none`（未发布态正确） |
| TA5 | 订阅快照 7 天前 → UI 显示陈旧标记 → 刷新 → 标记消失、`refreshedAt` 更新 |
| TA6 | 回归：N1/N2/N3 全量 vitest + tsc + 既有 market/picker 测试不退化 |

## 4. 风险与开放项

| # | 事项 | 处置 |
| --- | --- | --- |
| K1 | P1-C 的 system 预填策略（会话配置摘要是否足以构成可用人格） | 一期只预填骨架，弱化"自动生成完整人格"预期；编辑态引导用户补 expertise/bio |
| K2 | 会话 runtime 为 mobile（云同步只读）时入口应隐藏 | 前端按 `AgentTypeKey` 过滤菜单项 |
| K3 | O1/P2-F 跨仓库依赖不可控 | 保持占位，不承诺时间 |
| K4 | profile 数量增长后「个人 Agent」页信息密度 | 不在本计划范围，积累反馈后另立 |
