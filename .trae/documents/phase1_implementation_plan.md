# 阶段一（单机底座）实施计划

> 范围：F1 Profile 模型+卡片、F2 喂养管道（阈值触发+标签级 scope）、F3 P1/P2（技能点选、发布、导入导出、统计）
> 验收标准（来自 AGENT-NETWORK-PLAN §6）：
> 1. 新增 10 条同领域笔记后，身份卡专长标签无人工干预更新
> 2. 未授权 scope 内容在喂养输入中不可达（fuzz 验证）
> 3. 技能导出→删除→导入内容一致

## 仓库调研结论

- flowix-core：frontmatter 解析体系成熟（`extract_document_metadata`/`serialize_frontmatter`），note_index 投影走 `MemoIndexEntry.properties`，`normalize_tag_path` 可校验标签路径。
- flowix-desktop：`AgentAccessEntry` 当前 kind 仅 `Notebook|Folder`；`memo_events::emit` 只发前端 Tauri 事件，无内部订阅；`document_derived.rs` 提供 coalescing worker 模式可复用；插件体系（catalog/run/import/export/integrity）已完整，F3 P1/P2 主要是聚合层与统计层。
- DSH：subagent/goal/approval 运行时就绪（阶段二用）。
- 构建：`cargo check -p flowix-core` 与 `cargo check -p flowix-desktop` 为主要验证手段；前端需 `pnpm` 构建（环境未确认，优先保证 Rust 编译）。

## 文件与模块

| 文件 | 变更 |
| --- | --- |
| `app/flowix-core/src/agent/mod.rs`（新建） | AgentProfile 结构、frontmatter 解析/序列化、校验、agentId 生成 |
| `app/flowix-core/src/agent/profile.rs`（新建） | 解析实现 |
| `app/flowix-core/src/lib.rs` | 导出 `agent` 模块 |
| `app/flowix-desktop/src/config/access.rs` | `AgentAccessEntry.kind` 新增 `Tag` 变体 + 迁移兼容 |
| `app/flowix-desktop/src/commands/agent_profile.rs`（新建） | `agent_profile_get/create/update/delete` IPC |
| `app/flowix-desktop/src/commands/mod.rs` | 注册新 IPC |
| `app/flowix-desktop/src/memo_events.rs` | `emit` 内增加内部 broadcast 分发 |
| `app/flowix-desktop/src/nurture/mod.rs`（新建） | trigger + scheduler 骨架（N1） |
| `app/flowix-desktop/src/agent_profile/`（新建） | profile 投影与 NoteService 写回辅助 |
| `app/flowix-desktop/src/skill_registry.rs`（新建） | 技能注册表（plugin + DSH 聚合）、publish state、usage 统计 |
| `app/flowix-desktop/src/commands/skill.rs`（新建） | `skill_*` IPC |
| `app/flowix-desktop/src/plugin.rs` | 扩展 publishState 字段 + usage 事件埋点 |

## 实施步骤（依赖序）

### M1：F1 Profile 数据模型（flowix-core）
1. 新建 `flowix-core/src/agent/` 模块：`AgentProfile` 结构（对应 AGENT-PROFILE-MODEL §3 frontmatter 字段，`flowix_` 前缀）、`parse_profile(content) -> Result<AgentProfile>`、`serialize_profile(profile, body) -> String`、`generate_agent_id() -> String`（8 字节 hex，`agt_` 前缀）、校验（expertise 每项过 `normalize_tag_path`，bio ≤ 500 字符，redactionLevel ∈ R1|R2|R3）。
2. `lib.rs` 导出模块。
3. 单元测试：解析合法 profile、非法 expertise 拒收、round-trip 保持正文。

### M2：AgentAccessEntry 扩展 Tag 粒度（F6 scope 前置）
1. `access.rs`：`AgentAccessKind` 加 `Tag`，`AgentAccessEntry` 加 `tag_path: Option<String>`。
2. 迁移：读取旧 JSON（无 kind 时默认 Notebook/Folder 按 path 是否为根判断），向后兼容。
3. 单元测试：旧配置可加载、Tag 条目存储正确。

### M3：F1 Profile IPC 与投影（flowix-desktop）
1. 新建 `agent_profile/`：从 `.agents/<id>/profile.md` 读写，走 NoteService 笔记锁。
2. 新建 `commands/agent_profile.rs`：`agent_profile_list(notebookId)`、`agent_profile_get`、`agent_profile_create`、`agent_profile_update`、`agent_profile_delete`。
3. `commands/mod.rs` 注册。
4. note_index 侧：profile.md 的 `flowix_kind: agent-profile` 已随现有 frontmatter 投影进入 properties，无需改表。
5. 验证：`cargo check -p flowix-desktop`。

### M4：F2 N1 — 触发器 + 调度器骨架
1. `memo_events.rs`：`emit` 内加 `tokio::sync::broadcast`（容量 256），前端事件不变。
2. 新建 `nurture/mod.rs`：`NurtureTrigger` 订阅 broadcast，实现双过滤（`.agents/`/`.flowix/` 路径丢弃 + scope 过滤）+ 阈值计数 + 入队；`NurtureScheduler` coalescing worker（复用 `document_derived` 模式）+ skip 判断 + 留痕到 `.flowix/agents/<id>/nurture-log.jsonl`。
3. 留痕与状态文件 `nurture.json` 读写。
4. 单元测试：路径过滤、阈值触发、合并去重、skip 留痕。

### M5：F3 P1/P2 — 技能注册表 + 导入导出 + 统计
1. 新建 `skill_registry.rs`：聚合 plugin catalog（现有 `PluginCatalogSnapshot`）与 DSH skills（经 `skills/list`）为 `SkillDescriptor` 列表；publishState（private/shared）存 `~/.flowix/plugin/state.json`。
2. 新建 `commands/skill.rs`：`skill_registry_list`、`skill_set_publish_state`、`skill_usage_summary`。
3. `plugin.rs`：`plugin-run` 事件埋点写 `~/.flowix/plugin/skill-usage.jsonl`；复用现有 import/export（已存在，仅加 integrity 校验断言）。
4. 验证：`cargo check -p flowix-desktop`。

### M6：端到端冒烟验证
1. 构建 flowix-core 与 flowix-desktop：`cargo check`（或 build）。
2. 跑新增单元测试。
3. 手工脚本验证验收标准 1/2/3 的后端路径（profile 创建→scope 过滤→技能 import/export 完整性）。

## 依赖与考虑

- M1 是 M3/M4 的前置（profile 结构 + scope Tag）。
- M4 依赖 M2（scope 过滤需要 Tag 变体）。
- DSH 侧技能聚合依赖 appserver 运行时方法可用；若构建环境无 Node，M5 的 DSH 聚合仅定义接口，运行时接线留作后续。
- 前端 UI（卡片、点选器）不在本计划 Rust 实现范围内；IPC 契约已定义，前端可对接。
- 不改动 flowix-core 的 note_index 表结构（取材新查询留待 F2 N2）。

## 验证

- `cargo check -p flowix-core`、`cargo check -p flowix-desktop` 通过。
- 新增单元测试通过（profile 解析、scope 迁移、trigger 过滤、skill 聚合）。
- 验收标准 2（scope 不可达）：fuzz 测试用 100 条 scope 外路径事件，断言 0 条计入计数。
- 验收标准 3（技能导入导出一致性）：导出包 → 删除 → 导入，比对 integrity 哈希一致。

## 风险

- **编译面大**：flowix-desktop 依赖 Tauri，构建可能慢或环境缺依赖。对策：优先 `cargo check`，必要时只 check 改动 crate。
- **DSH 聚合在纯 Rust 侧不可直连**：M5 的 DSH 部分通过 IPC 方法调用，定义接口即可，不阻塞 Rust 编译。
- **前端未实现**：验收标准 1（专长自动更新）的完整闭环需 F2 N2（蒸馏）+ UI，本计划只做到后端骨架；蒸馏会话留作 F2 N2 后续。已在步骤中明示。
- **scope 迁移破坏现有配置**：M2 采用向后兼容读取，旧配置缺 kind 时按 path 推断，不破坏现有 access。
