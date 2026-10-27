# 方案 A：Thread 云实体 —— 移动端 AI 对话与桌面端同步详设

> 状态：设计稿（未实施）。前置评估见对话记录与 `docs/android-app-execution-plan.md`。
> 关联：方案 B（对话文件化）为降级备选；P0（移动端独立引擎）与本方案 A4 合并。

## 1. 目标

对话线程（threads + thread_messages）成为云同步一等实体，实现桌面 ↔ 移动端：
- 全量历史双向可见
- 任一端可续聊（消息重放语义，见 §9.4）
- 双端并发产生消息时零丢失（消息级合并）

## 2. 现状约束（决定方案形态的硬事实）

| # | 事实 | 影响 |
|---|---|---|
| C1 | `v2_dirty_entities` / `v2_inflight_operations` 两表 `CHECK(entity_type IN ('notebook','note'))` 硬编码 | 加实体必须**重建表迁移**（SQLite CHECK 不可 ALTER），全部客户端 sync.db 都要迁 |
| C2 | `pull_v2_changes` 遇未知 `entity_type` 直接 `Err(InvalidState)` **炸整趟同步**（inbound.rs:131） | 旧客户端遇到 thread change 会**笔记同步也全挂**——前向兼容是首要工程 |
| C3 | `V2PushOperation` 强类型枚举（5 变体）贯穿客户端 freeze/push 与服务端 apply | wire 三端（sync 引擎 / cloud / 桌面桥）同步扩变体 |
| C4 | blob 基建（内容寻址 / 预约上传 / 下载校验）实体无关 | **复用**：thread 内容走 blob，服务端只存 hash |
| C5 | 服务端 `changes` 流的 `entity_type` 是 TEXT 泛型列，`V2Change` 字段（name/icon/filename/content_hash/size）足够容纳 thread 元数据 | 服务端 changes 通道**零改动**即可承载 thread |
| C6 | 桌面对话引擎 = 外部 CLI 运行时，`thread_external_sessions.external_session_id` 跨设备无效 | 跨端续聊只能消息重放，工具密集对话降级（不可消除，只能声明） |
| C7 | thread.db 含 runtime 专属表（codex/dsh/opencode/hermes/claude/pi）与 `agent_external_events` | 这些**不上云**；只同步 threads/thread_messages 两个逻辑表的投影 |

## 3. 数据模型：整 Thread 单实体（JSON blob）

**不设 message 独立实体**。一个 thread = 一个实体，内容为消息数组 JSON，经 C4 blob 通道存储。

```
ThreadPayload（blob 内容，版本化 JSON）:
{
  "version": 1,
  "threadId": "th_...",
  "agentId": "builtin.chat" | "pi.xxx" | ...,
  "title": "...",
  "createdAt": 1792400000000,
  "messages": [
    {
      "id": "msg_...",            // 全局唯一，双端生成（uuid v7）
      "role": "user" | "assistant" | "tool" | "system",
      "content": "...",
      "reasoning": null,
      "toolName": null, "toolInput": null, "toolData": null,  // 工具块只读快照
      "createdAt": 1792400000000,
      "origin": "desktop:pi" | "mobile"       // 溯源
    }
  ]
}
```

**选型理由**（vs message 独立实体）：
- 复用 note 的"内容即 blob"全链路：配额、哈希校验、历史版本（`/v2/sync/history` 免费获得对话历史）、bootstrap 分页全部直接可用
- 消息不可变 + append-mostly，使"整包替换 + 客户端合并"正确性易证；独立 message 实体则要设计 per-message 冲突/排序/删除语义，复杂度数量级上升
- 代价见 §9.3（全量重传）

## 4. Wire 协议

```rust
// flowix-sync v2.rs
pub enum V2EntityType { Notebook, Note, Thread }          // +as_str/parse

pub enum V2PushOperation {
    ...
    ThreadPut {                                            // thread.put
        operation_id: String,
        base_revision: Option<String>,
        thread: V2ThreadPut {                              // camelCase
            id, agent_id, title, content_hash, size_bytes,
        },
    },
    ThreadDelete { operation_id, base_revision, thread_id },
}
```

`V2Change` / `V2BootstrapNote` 泛化：复用现有字段（`name`=title、`filename`=`agent_id` 挪用 **不采用**——新增 `Option` 字段 `thread_agent_id` 更清晰，serde `#[serde(default)]` 对旧数据向后兼容；C5 的 changes 表加列 `agent_id TEXT`）。

服务端 `bootstrap` 返回：`threads: Vec<V2BootstrapThread>` 顶层新数组（`#[serde(default)]`，旧客户端反序列化忽略未知字段 ✓——但旧客户端会走进 C2 的炸点，见 R1）。

## 5. 服务端（flowix-cloud）

```sql
CREATE TABLE threads (            -- 照 notes 表模式
  user_id TEXT, id TEXT,
  agent_id TEXT NOT NULL,
  title TEXT NOT NULL,
  revision TEXT NOT NULL,
  content_hash TEXT,              -- → blobs 表（复用）
  size_bytes INTEGER,
  deleted INTEGER NOT NULL DEFAULT 0,
  sync_seq INTEGER NOT NULL,
  created_at INTEGER, updated_at INTEGER,
  PRIMARY KEY(user_id, id)
);
CREATE TABLE thread_revisions (...);  -- 照 note_revisions（对话历史免费）
```

- `apply_thread_put/delete`：照 `apply_note_put` 的 base 严格检查 + blob 存在性检查 + record_change(`entity_type='thread'`)
- 端点**零新增**：push/bootstrap/changes/history 全部复用（history 按 noteId 参数已天然支持传 thread id——需验证按 entity 查询路由）
- quota：`user_usage` 计入 blob 大小（现有逻辑按 blob 记账，自动覆盖）

## 6. 客户端引擎（flowix-sync）

1. **迁移**（C1）：两表重建（新 CHECK 含 'thread'），事务内 `INSERT INTO new SELECT * FROM old`
2. `v2_thread_states` 状态表（照 v2_note_states）
3. freeze：ThreadPut → blob 预约/上传 → payload_json 冻结（照 NotePut）
4. pull：`entity_type == "thread"` → 下载 blob → `V2RemoteApply::Thread` 变体 → 由适配层落库
5. **冲突消解（核心算法）**：push 409 → 拉远端 payload → **消息级合并**：
   - 双方消息按 `id` 求并集（id 冲突取任一——内容同源生成概率≈0）
   - 排序：`(createdAt, id)` 稳定序，**不使用桌面 sequence**（跨端无全局序）
   - title 冲突：取 `updated_at` 新者
   - 合并结果：本地写库 + 重推（与 note 冲突消解同款两段式位置）
   - 合并是**纯函数**，进 `flowix-sync` 单元测试（fixture：双端交错/工具块并发/千条消息）
6. **C2 前向容错**：`pull_v2_changes` 未知 entity_type → `tracing::warn + continue`（**独立小 PR 先行**，见 A0）

## 7. 桌面桥（flowix-desktop）

- **写路径**：ThreadManager 消息落库点（append/工具块完成/标题更新）→ 桥接 hook 生成 ThreadPayload 快照 → `record_v2_local_change(Thread, put, hash)`，**500ms 防抖**（流式期间每 chunk 落库不能每条都推）
- **读路径**：`V2RemoteApply::Thread` → upsert thread.db（消息按 id 幂等插入；本地已有同 id 跳过）→ emit `threads-changed` 事件刷 UI
- **回环抑制**：自己推的 echo（self_sync_seqs）现有机制覆盖
- **排除面**（C7）：runtime 专属表、external_sessions、agent_external_events 不参与；桌面被 external runtime 拉起的会话在移动端显示为"只读续聊"（replay 时给 runtime 一份冻结的消息历史）

## 8. 移动端（flowix-mobile，与 P0 合并为 A4）

- Rust：OpenAI 兼容 SSE 直连引擎（新 `agent/` 模块：provider 配置存 Android Keystore 或私有文件，**key 绝不上云**）
- 消息写入：本机精简 thread 存储（直接复用 ThreadPayload JSON 文件或单表 SQLite）→ 同一 `record_v2_local_change` 接入同步
- UI：会话列表屏 + 聊天屏（agent-chunk 同款事件流、工具块只读渲染）

## 9. 风险评估

### 致命/高（必须先解）

| ID | 风险 | 概率 | 影响 | 缓解 |
|---|---|---|---|---|
| R1 | **旧客户端炸同步**（C2）：thread change 到达旧桌面版 → 整个账户同步失败（笔记也停摆） | 高（上线即触发） | 致命 | A0 容错 PR **必须先行铺版**；自建云可控（升级自己的桌面），cloud.flowix.cc 公有用户需强制升级窗口 |
| R2 | sync.db 迁移失败/中断（C1 表重建） | 低 | 高（同步瘫痪） | 迁移事务化 + 失败回滚标志位 + 单测覆盖"含 inflight 数据的迁移" |
| R3 | 双端并发回合合并错乱（同刻双发、工具块乱序） | 中 | 高（消息丢失/乱序） | 合并纯函数充分测试；id 并集保证不丢；排序用 (createdAt,id) 不依赖端内 sequence |

### 中

| ID | 风险 | 缓解 |
|---|---|---|
| R4 | **全量重传放大**（R3 选型代价）：千条消息 + 工具数据的对话每回合推整包（MB 级） | 单 thread blob 上限（如 2MB 触发归档拆分策略：旧消息折叠为 summary part 存附件）；同步节流；短期接受 |
| R5 | 续聊语义降级（C6）：外部 runtime 会话不可迁移，工具密集对话跨端续聊体验差 | 明确产品语义："移动端续聊 = 消息重放开新回合"；UI 标注 origin |
| R6 | 桌面双写一致性：thread.db 写入点分散（流式 chunk / 工具 / 标题） | 桥接收敛在 ThreadManager 单点 + 防抖快照；回环测试 |
| R7 | 隐私边界：对话全量上云（含工具数据快照） | 自建云可接受；文档明示；toolData 大对象裁剪策略（>64KB 截断标记） |
| R8 | 服务端 history 端点按 noteId 查询需兼容 thread id（路由歧义：note 与 thread id 空间可能碰撞） | thread id 强制 `th_` 前缀（notebook `nb_`/note 是 hash）；history SQL 按 id 前缀或双表 UNION |

### 低

| ID | 风险 | 缓解 |
|---|---|---|
| R9 | wire 枚举漏改一处（C3 三端变体） | 编译期强类型自然暴露；集成测试补 thread 用例 |
| R10 | 配额耗尽（对话 blob 刷量） | quota 现有记账 + 客户端超限提示 |

## 10. 实施计划（PR 粒度，串行依赖）

| 阶段 | 内容 | 交付物 | 验收标准 |
|---|---|---|---|
| **A0** | 前向容错：未知 entity_type 跳过（sync 引擎）+ 单测 | 小 PR，可独立发版 | ✅ **已完成（2026-10-19）**：`inbound.rs` 未知实体 warn+continue；回归测试 `pull_v2_changes_skips_unknown_entity_types`（mock HTTP changes 流注入 thread 实体 → 跳过不炸、其余 change 正常物化）+ `bootstrap_ignores_unknown_top_level_fields`（bootstrap 含 threads 字段可反序列化）；flowix-sync 57/57 零回归 |
| **A1** | 服务端：threads/thread_revisions 表 + `thread.put/delete` + bootstrap/changes 输出 + 集成测试 | flowix-cloud 升级（服务器重部署） | ✅ **已完成（2026-10-19）**：wire（`V2PushOperation::ThreadPut/Delete`、`V2BootstrapThread`、`V2Change.agentId`、`V2Bootstrap.threads`）；`db.rs` threads/thread_revisions 表 + changes.agent_id 幂等迁移；`sync.rs` apply_thread_put（严格 base + BLOB_MISSING 校验）/apply_thread_delete/bootstrap 全量输出 threads/changes 携带 agentId/history `th_` 前缀路由（title 复用 relativePath）；引擎 5 处 match 穷尽占位（A2 接管）；集成测试 `thread_entity_roundtrip`（blob→put→BLOB_MISSING→409→rebase→changes→bootstrap→history→delete）；flowix-sync 57/57 + cloud 3/3；已重部署 8.160.162.251 并公网 smoke 8 步全过 |
| **A2** | 引擎：迁移 + ThreadPut freeze/push/pull + **消息级合并** + 单测/集成测试 | flowix-sync 升级 | ✅ **已完成（2026-10-19）**：sync.db 迁移（两表 CHECK 重建搬数据，FK 事务外关闭，幂等 + 含 inflight 数据回归测试）；`v2_thread_states` 表 + `threads_bootstrap_required` 标志（两段式 commit 清除）；`V2ThreadPayload/V2ThreadMessage` + `v2_merge_thread_payloads` 纯函数（id 并集 + (createdAt,id) 排序 + title 取更新方，千条性能测试）；`record_v2_thread_change` 入队 API；freeze ThreadPut（blob reserve/upload）实装；acknowledge/commit thread state 落库；pull/bootstrap 物化 `V2RemoteApply::Thread`（thread 只走账户级同步，scope 模式跳过）；push 409 两段式消解（拉远端 payload → 丢 inflight 保 dirty → 桥 upsert 合并 → 下轮以新 base 重推；远端已删场景：delete 结算 / put 复活）；sync 签名扩展 threads 参数（桌面/移动调用点已适配）；测试：sync 68/68（merge 7 + store 4 + A0 测试随行为更新）+ cloud 4/4（新增 `thread_concurrent_merge_zero_loss`：双客户端并发 → 409 → bootstrap+merge → rebase 重推 → 三消息并集零丢失）+ core 354/354 + desktop 763/763；服务端零改动无需重部署 |
| **A3** | 桌面桥：ThreadManager→dirty 快照（防抖）+ pull upsert + `threads-changed` | flowix-desktop | ✅ **代码完成（2026-10-19，端到端验收并入 A5）**：`agent_session/store/cloud_bridge.rs`（threads/thread_messages ↔ V2ThreadPayload 双向投影；消息 (createdAt,id) 排序保证序列化确定性 → hash 可作 fingerprint；tool_data >64KB 截断（R7）；timestamp ISO/毫秒串双格式解析；upsert 按消息 id 幂等 + sequence 重分配 + title 同步）；`commands/cloud/local_adapter/thread_bridge.rs`（**pass 前对账**代替写入点插桩——threads.updated_at 内存水位增量扫描 + payload hash 与云端基线比对防回环 + 云有本地无 → Delete 传播；30s Poll 兜底）；coordinator 账户级 pass 注入 dirty 线程快照（`v2_dirty_thread_ids` 新查询）；`complete_v2_sync_with_apply_async`（apply 异步版，guard 即释防 !Send）；remote_apply Thread 分支 upsert/删除 + `threads-changed` 事件；前端 `threads-cloud-sync.ts` 订阅刷新产品线程列表（DSH + pi/opencode/gemini/openclaw）。id 映射：cloud = `th_` + 本地 thread_id（thread.db 零迁移）。C7 排除面：外部 runtime 消息（agent_external_events）不桥接。测试：cloud_bridge 4/4（投影 round-trip 确定性、upsert 幂等并集、截断、时间戳）+ desktop 767/767 + sync 68/68 + tsc 零错 |
| **A4** | 移动端：LLM SSE 引擎 + provider 设置 + 会话/聊天 UI + 同步接入 | flowix-mobile | ✅ **代码完成（2026-10-19，端到端验收并入 A5）**：新 `agent/` 模块——`mod.rs`（AgentState：threads.db 单表整包 JSON 存储 + provider 配置平文件 `agent_provider.json`（**API Key 不上云**）；`local_thread_snapshot`/`apply_remote_thread` 同步桥；本地与云端同用 `th_m_` 前缀 id 无映射层）；`chat.rs`（OpenAI 兼容 SSE 直连：reqwest stream + 手写 data:/[DONE] 帧解析、content/reasoning_content 双通道、120s 无 chunk 超时、停止/中断保留部分回答、**wire 协议对齐桌面 `agent-chunk`**（kind tag + snake_case 字段）；回合结束落库 → `record_v2_thread_change`（fingerprint 幂等）→ 3s 防抖同步）；`commands.rs`（provider get/set、thread list/messages/delete、chat_send 异步事件流、chat_stop 原子标志）；sync.rs 快照注入（dirty thread → V2LocalThread）+ cache.rs Thread apply（幂等并集 merge + `threads-changed` 事件）；前端 ChatListScreen/ChatScreen（SSE 流渲染 + 思考过程折叠 + 键盘跟随 visualViewport 方案 + 停止生成）、SettingsScreen AI 助手卡、NotesScreen 对话入口；Cargo +futures +reqwest/stream +rusqlite +uuid。验证：cargo check 全绿 + vite build 通过 + **Android x86_64 debug APK 构建成功**（NDK 交叉编译：rusqlite bundled/futures/reqwest stream 全过）。30s 轮询 + 3s 防抖覆盖「桌面 30s 内收到」验收窗口 |
| **A5** | 端到端验收 | 验收记录 | ✅ **已完成（2026-10-19/20，模拟器实测）**：环境 x86_64 模拟器（新 APK）+ 自建服务器 8.160.162.251 + mock LLM（OpenAI 兼容 SSE，宿主机 18900/10.0.2.2）+ 干净账号 a5@test.local（loop 账号被副本风暴污染已隔离）+ CDP 自动化。**场景 1 移动端→云**：真实 UI/SSE 回合（user+assistant 流式落库）→ 3s 防抖 → 云端 bootstrap.threads 可见、blob 双消息（origin=mobile）✅；**场景 2 双端并发**：对端脚本以桌面 wire push 并发回合 → 移动端旧 base 推送 409 → 引擎两段式消解（拉远端 apply 并集 → re-dirty）→ 30s 内重推 → 云端 **6 条并集零丢失**（mobile 4 + desktop 2），对端 title 更新经 merge 保留 ✅；**场景 3 断网重连**：iptables 运行中断网（authenticated 保持）→ 离线回合落库 + dirty 入队 + 同步退避 → 解除后 sync `uploaded=1` 补推 → 云端 **12 条**（三轮离线全上，整线程快照语义自愈早前 dirty 丢失）✅；**场景 4 历史回滚**：`th_` history 4 版本链（674B→960B→1.6KB→3.1KB，对应 2→4→6→12 条），历史 blob 可下载回读（seq=4 初始 2 条版本）✅。**验收发现**（记录）：① cloud_api_base 假地址法在 restore 重试耗尽后清凭据（M1 已知行为的再现），带重启的断网模拟须用 iptables；② 会话单例轮换：对端脚本频繁 password login 会作废移动端 refresh token（既有设计 last-one-wins，双端测试期间应避免脚本 login）；③ loop 账号副本风暴现场再次确认（35 笔记 15+ 链式冲突副本，与 A5 隔离，根因仍为独立待办） |

依赖链：A0 → A1 → A2 → {A3, A4 可并行} → A5。

## 11. 降级路径

若 A2 合并算法或 A3 桥接受阻，可中途切换方案 B（对话文件化到专用笔记本）：A0/A1 成果保留（容错 + 服务端表可用于未来），A4 的移动端引擎与 UI **完全复用**——两条方案仅"传输载体"不同。

## 12. 附：logout/login 副本风暴根因与修复（2026-10-20）

**现象**：每次 logout→login 后云端爆发一批 "(Flowix conflict op_xxx)" 链式嵌套副本（loop 账号现场：35 条笔记 15+ 副本，文件名嵌套多层），且 `loop-note-2`（云端已删）的 put 冲突以 `CLOUD_CONFLICT_KIND_NOT_SUPPORTED_M1` 卡死整个移动端 sync pass（thread 推送也被阻）。

**根因链**（四环相扣）：
1. `logout` → `clear_v2_account()` 清空 v2_note_states/dirty/cursor，但**本地缓存文件保留**（桌面为用户数据不能删，移动端缓存目录也未清）；
2. login 后首轮 pass 顺序缺陷：**reconcile/freeze/push 先于 bootstrap**——遗留文件全量组装为 `V2LocalNote`，states 已空 → freeze `base_revision=None`；
3. 服务端 `apply_note_put` 对 note 走严格 base 检查：base=None + 云端已有 → **每条笔记 409**；
4. `v2_conflict_material` 的 `base_content` 源自**客户端本地 states**（[v2_engine.rs](../app/flowix-sync/src/manager/v2_engine.rs) `v2_conflict_material`）——states 空则 base=None → `merge_markdown` 无从三方合并 → 移动端消解走"采用远端 + **本地全存冲突副本**"分支 → 副本成为新笔记被上传；下次 login 重复 2-4 → **链式放大**。

**修复**（三处，2026-10-20）：
- **A（引擎根因，双端生效）**：[sync_pipeline.rs](../app/flowix-sync/src/manager/v2_engine/sync_pipeline.rs) `sync_v2_account_once`——bootstrap_required（登录后/首次启用/CURSOR_EXPIRED）时本轮**纯拉取**，reconcile/freeze/push 全部跳过；dirty 保留，states 基线建立后下轮正常推送。回归测试 `bootstrap_required_pass_skips_push_entirely`（单连接 mock：若 push 先行会解析失败）。
- **B（移动端对齐桌面）**：[sync.rs](../app/flowix-mobile/src/sync.rs) `resolve_put_conflict` 的 `remote.deleted` 分支从"Err 卡死"改为桌面同款"接受云端删除"（删本地 + 索引 prune + rebase）。
- **C（移动端防静默分叉）**：`sync_pass_once` 增加内容对账（桌面 `reconcile_v2_snapshot` 的移动版）：本地文件哈希 ≠ 云端基线 → 补记 Put——覆盖修复 A 后"bootstrap 轮之后的真实离线编辑"补推路径。

**验证**（2026-10-20，模拟器实测）：a5 干净账号建 2 条笔记上云 → **二次** logout/login + 手动同步 → 云端笔记数保持 2（**零副本**，修复前每轮 login 副本翻倍且文件名链式嵌套）；登录后首轮 bootstrap 纯拉取、次轮推遗留 dirty（uploaded 收敛）、无 `CLOUD_CONFLICT_KIND_NOT_SUPPORTED_M1` 卡死。回归：flowix-sync 69/69（新增 `bootstrap_required_pass_skips_push_entirely`）+ desktop 767/767 + cloud 4/4。

**残留清理**（2026-10-20，用户确认后执行）：loop 账号 29 个冲突副本走协议删除（`note.delete` push 批量 29/29 成功，产生 changes 流事件）——桌面端下次同步自动删除本地副本文件；保留 5 条正本（loop-note-1 / m1-10-merge / m1-10-conflict / m1-10-format / attachments/18.png），loop-note-2 维持墓碑。

## 13. 附：桌面 dev 真机双端联测记录（2026-10-20）

**环境**：Windows 桌面 `npm run tauri:dev:win`（`FLOWIX_CLOUD_API_BASE=http://8.160.162.251:8787`）+ x86_64 模拟器（com.flowix.app.mobile，CDP 自动化）+ loop 账号。桌面登录走 SecretStore 注入（`~/.flowix/default.db` 的 `secret_kv` 表明文存 `flowix_cloud_refresh::default`，python 直写 + 重启 dev → restore 自动认证），绕开 UI 登录入口。

**结果**：

| 方向 | 路径 | 验证 |
|---|---|---|
| 云→桌面 | 桌面 restore 认证 → bootstrap 纯拉取 | ✅ `threads_bootstrap_required` 清零；移动端线程 `th_m_01a154f3790f`（agent_id=mobile）+ 2 条消息落 thread.db（threads/thread_messages，title/sequence 正确）；遗留 note dirty 清空 |
| 桌面→云 | thread.db 写入（python 注入 user 消息 `deskmsg_001`）→ 30s Poll pass 前对账 → push | ✅ 对账水印扫描扫到 `updated_at` 变化 → hash ≠ 基线 → record Put → push 成功 → 云端新 revision `rev_01a1577ca3d5` → 本地 acknowledge 后**基线 hash 与本地投影重算值逐字节一致**（base64url(sha256) 验证） |
| 云→移动端 | token 注入 restore 失败（见下）→ CDP `cloud_login` → auto_sync 30s pass | ✅ `th_m_01a154f3790f` 拉取落库（3 条消息并集）→ ChatListScreen 显示「3 条消息」→ ChatScreen 完整渲染桌面写入的第 3 条「桌面续聊消息」 |

双端并发并集（原 d4）不再单独执行：A5 场景 2（并发 409 → 消解 → 6 条零丢失）+ 本轮同线程双端写入并集已覆盖。

**新发现**（待办）：

1. ~~**桌面无原生 UI 云消息路径**~~ **已修复（2026-10-20）：桌面 builtin chat 落地**。桌面全部既有 Agent 入口（DSH/Codex/Claude/Pi/OpenCode/Hermes）为外部 runtime，消息落 `agent_external_events`（C7 排除面，设计如此）；为此把移动端 A4 SSE 引擎移植到桌面作为第 7 个 `ExternalCliRuntime`（`Kind::Mobile`）：[mobile.rs](../app/flowix-desktop/src/agent_external/mobile.rs) `MobileChatManager`（无子进程，runs 跟踪 + stop 标志；手写 `data:`/`[DONE]` 帧解析、120s chunk 间隔超时、停止保留部分回答加 `[已手动停止]` 后缀）；provider 配置平文件 `<config_dir>/mobile-agent-provider.json`（baseUrl/apiKey/model，API key 不上云），设置入口「偏好设置 → AI → 移动端对话」（`mobile_provider_get/set` 命令）；user/assistant 消息直接落 `thread_messages`（云同步源），回复经 `emit_chunk_with_run_id` 注入 run_id 键控；上云走 A3 桥 pass 前对账（无需手动 dirty）。**三方向实测闭环**（mock LLM SSE）：桌面 mobile 会话发消息 → UI 流式渲染 + `thread_messages` 落库（user/assistant 各 1 条无重复）→ 12s 内 A3 桥自动对账 push（云端 blob 3+2=5 条消息级并集）→ 移动端前台唤起拉到完整 5 条。回归：desktop `agent_external` 297/297（含 `registry_contains_every_external_runtime_once` 加 mobile 后断言）+ conversations 1/1 + 前端 agent-types 7/7 + tsc 零错。**实机验证（真实 DeepSeek API，同日）**：provider 配 `api.deepseek.com` / `deepseek-chat`（key 复用 `~/.dsh/.credentials.yaml` refs 的 `FLOWIX_DSH_DEEPSEEK_API_KEY`）→ 桌面发「一句话介绍 DeepSeek 公司」→ 真实模型流式回复 1.1s 落库 → 51s 内 A3 桥对账 push（`rev_01a158b2b20a`，7 条消息并集）→ 移动端前台唤起拉到完整 7 条。**排障坑（已修）**：PS5.1 `Set-Content -Encoding UTF8` 写 UTF-8 BOM → `serde_json::from_str` 解析失败 → `chat_stream` 返回 `MOBILE_PROVIDER_CONFIG_INVALID` 且消息不落库（手工编辑配置文件同理，记事本 UTF-8 也带 BOM）；[mobile.rs](../app/flowix-desktop/src/agent_external/mobile.rs) `provider_config()` 读取后 `trim_start_matches('\u{feff}')` 剥 BOM。另注：tauri-dev watch 会在 Rust 文件变更时杀进程重启，发送操作与重编译撞车会静默丢消息（草稿保留）——实测工程注意。
2. ~~**refresh token 文件注入 restore 无效**（根因未明，待查）~~ **已定位（2026-10-20 复测）：多端竞态，非 bug**。停掉桌面 dev 后干净环境注入全新 login token → force-stop 重启 → restore **完全正常**（认证成功、refresh 轮换新 token 写回文件、cursor/bootstrap 正常）——顺带首次真实验证了「login → 重启 → restore 恢复」全链。此前失败机制：桌面 dev 进程的 access 过期自动 refresh 与注入 token 竞争，云端 `issue_session` 单会话 last-one-wins（每次轮换作废该用户全部旧 token），注入 token 在移动端消费前即被作废；移动端 restore 401 后按设计删凭据（[auth.rs](../app/flowix-sync/src/manager/auth.rs) `is_invalid_refresh_token` 分支）。测试工程启示：做 token 注入/会话类实验前先停其它端进程。
3. ~~**单会话轮换对双端共存的影响**~~ **已实施多设备会话（2026-10-20）**：[auth.rs](../app/flowix-cloud/src/auth.rs) 改造——`issue_session` 不再作废该用户全部旧 token；`refresh` 只删被消费单行再发新行（单次使用语义保留）；`logout` 本就按单行删（单设备下线）。回归测试 `multi_device_sessions_coexist`（双 login 独立 session、A 轮换后 B 仍可 refresh、被消费 token 401、logout 只下线自己），cloud 5/5。已部署 8.160.162.251 并公网冒烟通过（A refresh 后 B refresh 200，旧语义下必 401）；真机双端并存验证：移动端全程在线未被桌面 login/refresh 打断，桌面独立 session restore 成功，双 `v2_cloud_account` 并存。遗留语义变化：**改密码/「登出所有设备」不再天然全端失效**（当前无此入口，未来加设备管理时需按 user_id 清行）。
4. **`session_restore_policy.blocked` 恢复熔断**（联测排障新知）：restore 失败链（401 → 删凭据 → 前端 logout → `clear_v2_account`）会把 sync.db 的 `session_restore_policy.blocked` 置 1，此后**进程重启时 restore 直接 return（不读 token 不发请求）**——注入有效 token 也不会恢复，必须 `UPDATE session_restore_policy SET blocked = 0` 或走 UI 重新登录（成功认证时 `set_v2_account` 自动清零）。排查手法（可复用）：本地 fake-cloud HTTP 监听 + `FLOWIX_CLOUD_API_BASE` 指过去，直接观测客户端发出的 refresh body——本轮以此捕获桌面实际 token 一锤定音。另注：logout 会清 `v2_synced_notebooks`，重新登录后需在 UI 重新启用笔记本同步，pass 才会恢复推送。
5. ~~**桌面无原生 UI 云消息路径（只读展示部分）~~ **已修复（2026-10-20）：移动端会话桌面可见可读**。此前云同步 apply 落库的移动端线程（agent='mobile'）在桌面「对话」列表不可见，两处断点：① [conversations.rs](../app/flowix-desktop/src/agent_session/store/conversations.rs) 列表/分页/计数三处 notebook 圈定过滤（`i.notebook_id = ?`）把无笔记本归属的跨端线程排除——加 `OR i.agent = 'mobile'` 豁免（跨端对话全局可见）；② 前端无 'mobile' AgentTypeKey——补类型、AGENT_TYPES 条目（`icon-mobile.svg` 主题自适应图标 + i18n「移动端对话（只读）」）、runtime spec（空设置/空权限，恒不可新建）、`AgentRuntimeStatus.mobile` 前端恒补 `available:false`、列表组件两处 notebook 豁免；标题 fallback `agent.mobileSession.title`。消息读取零改动：mobile 不在 history adapters 注册表 → fallback local adapter 恰走 `thread_get_page`（thread_messages）。**实测**：筛选组出现「移动端对话（共 N 个）」、线程点开 3 条消息（含桌面续聊消息）完整渲染。回归：agent_session 44/44 + 前端 22/22 + tsc 零错。~~遗留：mobile 会话输入框仍渲染但发送无 runtime~~ → 发送能力已由待办 #1 builtin chat 落地（见上）。另附联测恢复操作记录：笔记本云同步启用走「笔记本编辑对话框 → 云同步开关」（偏好设置无独立云账户区——用户找不到登录入口的根源），引擎侧等效 SQL 为 `set_v2_notebook` 的 upsert。
6. ~~**云同步入口藏得过深（experimental 门控）**~~ **已修复（2026-10-20）：解禁为常规功能**。此前 `boot.json experimental=false` 时用户**完全无云同步入口**——偏好设置侧栏隐藏「账号」tab、笔记本编辑对话框隐藏云同步开关（联测期间只能 SQL 直写 `set_v2_notebook` 启用），这是用户多次找不到登录入口的根源。方案 A 联测全链闭环（notes/threads 双向 + 多设备 + builtin chat）后解禁：① [preferences-view.tsx](../app/flowix-web/features/preferences/preferences-view.tsx) 4 处（tab 显隐过滤列表 / 深链豁免 / 防回退 effect / CloudSyncSection 渲染）移除 cloudSync 的 experimental 门控（connections/tools/history 保持门控）；② [notebook-list.tsx](../app/flowix-web/features/memo/components/notebook-list.tsx) 移除云同步状态监听/断网重连补同步的 5 处 `!experimental` 守卫（experimental 为纯 UI 开关，引擎侧 30s Poll 由 `v2_synced_notebooks` 驱动不感知它，解禁零引擎行为变化）；③ [notebook-dialogs.tsx](../app/flowix-web/features/memo/components/notebook-dialogs.tsx) 笔记本编辑对话框云同步开关去门控（开关内部 `available` prop 已有未登录引导）。**实测**（experimental=false 的 dev 机）：偏好设置侧栏「功能」组出现「账号」tab，点开完整渲染 CloudSyncSection（loop@selfhost.local 已登录态 + 会员存储 + 退出登录）；笔记本列表云同步状态监听恢复。回归：tsc 零错 + use-cloud-account-state 9/9 + notebook-folder-view 2/2。入口路径变更记录：登录 = 偏好设置 → 账号；启用笔记本同步 = 笔记本编辑对话框 → 云同步开关（不再需要开 experimental）。**后续解耦（2026-10-24，commit `931d9a8f`）**：生产包实测发现 threads 推送随笔记本 pass——`v2_synced_notebooks` 空（如 logout 后未重新启用）时账户级 pass 不被调度（poll/retry gate 要求 enabled 非空），桌面线程不上云。已放开 [coordinator.rs](../app/flowix-desktop/src/commands/cloud/coordinator.rs) 两处 gate 仅要求 authenticated（引擎 inbound 本就按 enabled 过滤笔记、threads 不过滤，空 notebook 集的账户 pass 是安全的 threads-only pass）；手动 sync_now 原本不受限。同轮实测另确认两条已知行为：同机双实例（dev/生产）共享 SecretStore 同一 token 行会 refresh 互踢（恢复法：服务端自签独立 session + 清 blocked）；threads-only 场景由本解耦修复。

## 14. 附：跨端延迟优化与双向完整实测（2026-10-26）

**背景**：实测「桌面发消息 → app 停会话页」端到端 30-60s，用户反馈过慢。优化（commit `184f4aae`）：桌面 `CLOUD_POLL_INTERVAL` 30s→10s + 移动端 `spawn_auto_sync` 轮询 30s→10s + 移动端 `visibilitychange` 唤起 0 等待补拉 `syncNow`（后台 webview 冻结时事件不触发，回前台即拉）。移动端会话页 UI 刷新缺口同轮修复（commit `10d140cf`：ChatScreen 补 `threads-changed` 监听）。SSE/推送通道改造明确延后。

**双向延迟实测**（桌面 dev 10s poll + 模拟器 APK 10s pull，mock LLM 回合 ~1s；桌面→app 终轮 5s：04:02:29 落库 → app 会话页 5s 内显示「64+64=? → 128」）：

| 链路段 | 实测 | 分解 |
|---|---|---|
| app→云（成功回合） | ~4s（04:24:10 发 → rev@04:24:14） | 回合 ~1s + 3s 防抖 push |
| 云→桌面 thread.db | ~12s（04:24:14 → 04:24:26 apply） | 10s poll 相位 |
| app→桌面端到端（DB 口径） | 16.1s | 上两行相加 |
| app→桌面端到端（UI 自动显示口径） | **5.6s**（相位命中轮，见缺口二修复后实测） | — |

模型结论：平均 ~10s、最坏 ~22s（双侧 10s 相位全错开 + 3s 防抖）；唤起/相位命中时 5s 级。桌面→app 同模型。

**新发现缺口一：移动端失败回合消息不上云（已修）**。反向验证时 mock 意外离线暴露：app 回合失败（`AGENT_REQUEST_FAILED`）后 user 消息落本地库（payload 28 条）但 **8 分钟零推送**（云端 revision 停在上一轮）。根因：[chat.rs](../app/flowix-mobile/src/agent/chat.rs) `enqueue_thread_change_and_sync` 只挂在**成功回合收尾**（`!assistant_content.is_empty()` 分支），user 消息虽在 `chat_send` 落库但请求失败/流错误路径直接 return 不入队 → dirty 队列永远没有该线程，直到同线程下一个成功回合才连带补推。桌面桥（pass 前对账，按 updated_at 水位 + hash 比对）无此问题。修复：user 消息落库即 enqueue（fingerprint 覆盖整 payload，回合收尾再次 enqueue 幂等更新，fingerprint 不同才涨 generation）。**实机验证**（新 APK，mock 停）：失败回合 user 消息 **3.4s** 上云（rev seq31@04:46:47，发送 04:46:43.6），桌面 10s poll 拉到并（经缺口二修复）UI 自动显示——修复前同场景 8 分钟零推送。

**新发现缺口二：桌面会话页不刷新远端线程消息（已修）**。app 消息已 apply 进桌面 thread.db（python 查证数据在），但桌面停在会话页 60s+ 不显示；聚焦窗口也不触发（focus reconciliation 只刷 `activeThreadIds`/interested 集合，恢复路径打开的会话 surface 不写这些）。根因两层：① 前端 `threads-changed` 订阅（[threads-cloud-sync.ts](../app/flowix-web/features/agent/store/threads-cloud-sync.ts)）只刷新线程**列表**不刷新打开会话的**消息**；② 云同步移动端会话的 AgentTypeKey 是 **`'mobile'`**（[agent-types.ts](../app/flowix-web/lib/agent-types.ts)「云同步下来的移动端会话」，非 deepseek-harness——首个补丁版本守卫写错类型导致验证失败，排查记录备查）。修复：[agent-conversation-detail.tsx](../app/flowix-web/features/agent/components/agent-conversation-detail.tsx) 会话组件对 `instance.agentType === "mobile"` 订阅 `threads-changed` → `loadMessages`（stale-while-revalidate，幂等合并不打断本地流式；本地 runtime 类型有自己的流式/事件链，不在刷新范围）。**实测**：修复后 app 发消息（成功回合），桌面会话页零交互 **5.6s** 自动显示。

**顺带观察（预存行为，未改）**：每轮移动端成功回合产生**两条**云端 revision——app 推送（rev N）+ ~30s 后桌面 echo 重推（rev N+1，体积差 3-9B）。后者为桌面桥 reconcile 的投影归一化差异（桌面本地投影 hash ≠ 云端基线 → 误判 dirty → 重推自己投影；acknowledge 后基线对齐收敛、无回环放大）。正确性无影响，仅每轮多一次小 blob 写入；待投影序列化完全对齐后自然消失，或后续在 hash 比对前做投影规范化。失败回合补丁后移动端每轮成功回合会多一次 user-only 中间推送（同样小 blob，幂等收敛）。

**本轮改动**：[chat.rs](../app/flowix-mobile/src/agent/chat.rs)（enqueue 前移 + 模块注释）、[agent-conversation-detail.tsx](../app/flowix-web/features/agent/components/agent-conversation-detail.tsx)（threads-changed 订阅）、本文档。验证：tsc 零错 + cargo check 零新告警 + 模拟器新 APK 实测（上）。

## 15. 附：笔记「桌面→云→手机」同步评估与待办（2026-10-26）

**评估结论：功能可用**。Loop 笔记本（桌面已启用云同步）三路径实测：产品内新建（编辑器/CLI）**~15s** 端到端（创建→云端→手机列表可见、正文完整）；编辑已有笔记（含外部工具直改文件）**2s**（写入→云端 revision 更新）→ 手机拉取渲染完整（202 字含新增行）；附件同步 A5 已验证。

**待办：外部工具新建文件的 memo 注册滞后，同步挂起时间不可控**。实测外部进程直接在笔记本目录新建 md：watcher 秒级捕获并记 dirty ✅、搜索索引（notebook.db `notes` 表）秒级收录 ✅，但 **memo 注册（`memos`/`memo_lifecycles`，列表实体 + 云同步组装依赖）不发生** → dirty 挂起、pass 无法组装上传，实测 **4 分 21 秒**无推送，直到 CLI create 触发注册对账才连带补上（补注册与 CLI 新笔记注册时间戳相差 120ms 佐证同批）；若无后续产品内操作，可能挂到重启。根因：[processor.rs](../app/flowix-desktop/src/watcher/processor.rs) Create/Modify 路径只调 `refresh_note_path`（搜索索引层），无 memo 注册步骤；memo 注册对账（含 memo_id 分配）只由产品内创建类操作或启动流程触发。**修改已有文件不受影响**（memo 已注册，modify→dirty→push 全链秒级）。影响面：第三方编辑器（Typora/Obsidian 等）直接往笔记本目录新建文件的协作场景。修复方向（待做）：watcher Create 事件路径补 memo 注册，或云同步 pass 组装时发现 dirty note 无 memo 行即触发注册对账——涉及 memo_id 分配核心逻辑，需独立改动并配测试。

## 16. 附：笔记本锁死循环——外部文档会话导致云同步停摆 + 本地编辑被覆盖（2026-10-26，高优先级待办）

**用户实测报告**：桌面编辑 `Untitled.md` 保存多次，手机端始终未收到，最终本地内容回到编辑前原文（**编辑丢失**）。

**根因链**（实测取证，两次全新进程 100% 复现）：
1. **桌面云同步 pass 全灭**：pass 的 memo index 阶段（`refresh_note_index_path`）申请 notebook **shared** 文锁 5s 超时（`locks/notebooks/<id>.lock`），说明进程内某任务持有该笔记本 **Exclusive**（`notebook_change`）不释放。与 **workspace 恢复打开的 Untitled.md 外部文档会话**强相关：重启 dev 后 11 秒内（首轮 pass）即复发；无其他 flowix 进程在场（已排查残留 CLI）。
2. **失败 pass 的 apply 阶段每轮回写远端版本到本地磁盘**：external-watch 观测 Untitled.md 每 ~5.7s（与 pass 超时周期锁步）被 modified 一次、偶发 deleted+重建；因 pass 失败不 acknowledge，下轮再 apply 再回写——**把用户本地编辑覆盖回云端旧版**（本地 14:18 起内容回到原文、size 179 稳定）。
3. **手机端编辑器自动保存回写云端**（打开笔记即「已保存」回写 put，云端 changes seq 165-172 连续 5 次 Untitled put）→ 双端回声循环，桌面确认失败期间云端版本被手机持续刷新。
4. 用户桌面 dirty 挂起（generation 2→9），`last_success_at` 停在 2026-10-26 06:04:52Z，之后所有 pass 失败；`blob_reservations` 无新行（push 从未到达 blob 阶段）。

**影响**：任一「外部文档打开 + 云端存在该笔记新版本」的组合下，该笔记本云同步完全停摆、本地编辑被静默覆盖丢失。严重级。

**修复方向（待做，需专项）**：① 定位 Exclusive 持有路径——外部文档会话生命周期（`external_document_watch` / document-session / workspace restore 链）中拿 `notebook_change` 后不释放或与 apply 回写形成锁序死锁；建议先给 `operation_locks.rs` 的 acquire/release 加 tracing（operation 名落日志），复现时直接读出持有者；② 复查 pass 失败路径：apply 回写后 memo index 失败时应**回滚或不回写**（当前「apply 已落盘 + pass 失败不 ack」组合正是覆盖用户编辑的机制）；③ 手机端编辑器「打开即自动保存回写」应加内容比对（无变化不 put），消除回声源。

**临时规避（待验证）**：关闭桌面对应外部文档标签（恢复会话不再打开它）后观察 pass 是否恢复；本记录时点桌面 dev 仍处于死锁状态（第二轮复现中）。

**修复（2026-10-26，同日闭环）**：先给 [operation_locks.rs](../app/flowix-core/src/memo_file/operation_locks.rs) 加进程内独占锁持有登记（超时错误点名持有者 + tracing warn）复现取证——持有者为 **`resolve_cloud_file_put_conflict`**（409 消解器）。根因即上文②的自死锁变体：消解器持 notebook **Exclusive** 后调 `register_existing_file_for_notebook_id` → `sync_index_on_write` → `refresh_note_path` 内部 `notebook_read`（**Shared**）→ **同进程同线程新句柄自等自死锁 5s 超时**（fs2 不同句柄不识别同线程）→ 消解失败 → rebase 未执行 → dirty 保留 → 下轮 pass 再撞 409 → 无限循环（每轮消解写盘一次 = 5.7s 周期回写，本地编辑被反复覆盖）。修复（core 层统一，桌面/移动端 all-in）：`notebook_read` 增加同线程重入——本线程已持该笔记本 Exclusive 时直接放行（Exclusive 语义覆盖 Shared，无需真实文件锁），`NotebookChangeGuard` 在 thread-local 登记持有 key、drop 移除。测试：新增 `notebook_read_is_reentrant_while_holding_change_on_same_thread`（修复前该场景 5s 超时），锁模块 8/8 + flowix-core 355/355 零回归。**实机验证**：修复版 dev 重启后 90s 零锁超时 WARN；挂起 3 小时的 5 个 inflight op 与 dirty 全部清零；`last_success_at` 恢复实时推进；云端与本地内容一致收敛；「立即同步」按钮由常亮禁用恢复可点且手动 pass 无错。

**修复③落地（2026-10-26 下午，移动端 EditorScreen）**：消除「打开笔记即自动保存回写」回声源——① 加载流程 `setContent` 必触发 `onUpdate`，加 `loadingContentRef` 抑制（加载回写不算编辑，不标 dirty 不排保存）；② `savedContentRef` 已保存基线 = 「原始 frontmatter + 加载后序列化 roundtrip 产物」（吸收 parse→serialize 非字节级差异），`onUpdate` 与基线 `sameAsSaved`（两侧 CRLF→LF 规范化后比较）等价则直接 saved——撤销回原点也不再触发保存；③ `performSave` 成功后推进基线。历史恢复走正常编辑链路（与基线不同即 dirty 保存，等价恢复则正确地不保存）。**实机验证（模拟器 CDP，全绿）**：CRLF 内容笔记（桌面写盘形态）打开 → saveState 恒「已保存」、关闭后 sync 差分 uploaded=0（修复前该场景序列化 LF ≠ 磁盘 CRLF 必 put）；编辑追加文字 → autosave 正常、uploaded=1、内容落盘含追加文本。

## 17. 移动端笔记内嵌 AI 对话卡片（thread card，2026-10-26）

**需求**：桌面端可在笔记中插入 agent 对话卡片（HTML 注释存储，随笔记云同步）；移动端此前遇到该注释会**静默丢弃**（`@tiptap/markdown` 对 html token 走 `generateJSON`，DOM 解析丢注释）。本轮让移动端互认该格式：能显示、能新建、能对话。

**存储格式与桌面完全一致**：`<!-- flowix:agent-thread-card {"version":1,"instanceId":...,"threadId":...,"agentType":...,"collapsed":...} -->`（marker/正则/parse/serialize 均从桌面 [agent-thread-card-markdown.ts](../app/flowix-web/features/agent/thread-card/agent-thread-card-markdown.ts) 移植为 [agent-thread-card-md.ts](../app/flowix-mobile/frontend/src/extensions/agent-thread-card-md.ts) 零依赖纯函数模块）。旧版 `::agent-thread-card{...}` 语法同样认领并升级为新格式。

**实现**（纯前端，零 Rust 改动）：
- [agent-thread-card.tsx](../app/flowix-mobile/frontend/src/extensions/agent-thread-card.tsx)：`agentThreadCard` Atom 节点 + `markdownTokenizer`（block 级 tokenizer 在 marked html 规则前认领注释）+ `parseMarkdown`/`renderMarkdown`（与桌面、与移动端 MobileImage 用同一套 `@tiptap/markdown` v3.23.4 扩展点）；卡片视图用 ReactNodeViewRenderer。
- 会话逻辑 `useThreadChat`（与 ChatScreen 同源：`agent-chunk` 流式 + `threads-changed` 刷新 + `api.agentChatSend`）；threadId 初始可为 null，首条消息后回填 node attrs 持久化。竞态兜底：`chat_send` 立即返回但 chunk 并发到达，chunk 早于 IPC 返回时直接采纳 `chunk.thread_id`。
- **交互分级**：`typeKey === 'mobile'`（移动端自建或云端移动端会话）可完整对话（输入框/停止/流式）；桌面 runtime 类型卡（deepseek-harness 等）只读——消息可显示（线程已云同步时），无输入框。
- [EditorScreen.tsx](../app/flowix-mobile/frontend/src/EditorScreen.tsx) 工具栏新增「插入 AI 对话」按钮（Bot 图标）；ChatScreen 的 MessageBubble 导出复用。

**往返验证**（[atc-roundtrip.test.mts](../../m1-10/atc-roundtrip.test.mts)，node 24 类型剥离直跑移动端纯函数 + marked v17 同款 tokenizer 注册，11 项断言全过）：桌面注释 parse→render 逐字节一致、二次往返幂等、移动端新建卡身份保留、旧版语法升级、无关注释不误认领、threadId 回填序列化正确。已查明 marked v17 会把注释后的单个换行并入前一 token 的 raw（space 合并），不影响 metadata 解析与序列化。

**待实机验证**：模拟器 APK 安装后（插入卡片→发送→流式回复→threadId 回填→保存→桌面打开该笔记卡片互认）。

**实机验证（2026-10-26，模拟器 x86_64 debug 签名 APK）**：
- **插卡与对话**：工具栏 Bot 按钮 → 卡片插入 → 发送 → mock 引擎流式回复完整渲染（user + assistant 气泡）。
- **持久化往返**：返回列表再进笔记，卡片与消息完整恢复（注释 parse→serialize 往返 + threadId 回填后按线程重载）。
- **上云**：在 Loop 本（云同步本）loop-note-1.md 文末插卡对话后，云端 blob 出现顶格注释 `<!-- flowix:agent-thread-card {"version":1,"instanceId":"58d06974-…","threadId":"th_m_01a177b1fa11…","agentType":"mobile",…} -->`（seq 202/203/205 三连 put），线程 th_m_01a177b1fa11… 同步上云。格式与桌面字节级一致（桌面读侧走同款 marker/解析，为其既有能力）。
- **修复一处流式 bug**：`stream_end` 后 liveText/liveReasoning 未清空导致落库消息与流式气泡重复显示（卡片 hook 与 ChatScreen 同源同病，两处同步修复）。

**遗留待办（非本功能引入）**：① 移动端**新建笔记首推不生效**——note_create/note_save 记 dirty 后 pass 上传 0（LoopCardTest.md 十余分钟未上云），而编辑已有云笔记（loop-note-1）dirty 推送正常；新 note 无 baseline 时内容对账也跳过，需查 dirty 队列消费路径。**→ 已复核不复现（见 §18 遗留②）**。② 桌面生产包（旧版）云同步轮询停摆：18:03 后未再拉取云端 put（dev 版未登录云）；重启生产包 + dev 均未恢复，与本轮 thread card 无关，归入 §15/§16 既有挂起问题族。③ 桌面打开移动端卡的视觉确认随 ② 恢复后补做。
- **修复验证**：新版 APK 重装后续发一轮（before 2 → after 4 条气泡，user/assistant 各一条），重复显示消除；旧对话重开加载亦干净。

## 18. 桌面云同步停摆排查与修复 + thread card 跨端双卡互认（2026-10-26）

**现象**：桌面生产/dev 进程 18:03 拉取一次后再不拉取云端新 put（移动端 18:11-18:12 三连 put 未达桌面），重启进程也不恢复。

**根因链（两层）**：
1. **登录态丢失（触发）**：服务端 refresh token **单次使用**（[auth.rs](../app/flowix-cloud/src/auth.rs) refresh 即 DELETE 旧行签新行）。旧 dev 进程持登录态运行中 access 过期自动 refresh 轮换；进程被 `Stop-Process -Force` 强杀落在「服务端已消费旧 token、客户端 secrets 未写回新 token」窗口 → secrets 里残留已消费 token → 新进程启动 restore → 服务端 INVALID_REFRESH_TOKEN → 客户端 [auth.rs L123-131](../app/flowix-sync/src/manager/auth.rs) 自动清账号（clear_v2_account：sync.db states/cursor/synced_notebooks 全清 + 删 secrets token）→ 后续所有进程读到的都是未登录态，云同步静默不跑。
2. **重新登录后 bootstrap 冲突死循环（暴露的第二层）**：恢复登录（API 登录注入 refreshToken + 重建 synced_notebooks 行）后 pass 报 `CLOUD_LOCAL_EDIT_CONFLICT: local file preserved at <file>` 循环不收敛。根因：[v2_local_content_diverged](../app/flowix-sync/src/v2.rs) 的 `(本地存在, 无基线)` 判据——logout 清空 states 后 bootstrap 从 cursor 0 重放全量历史 changes，本地内容（最终态）与任何一条**历史版本**都不相等，且分页拉取使批内去重也无法覆盖跨页历史 → 每条历史 put 都报冲突 → cursor 永不推进。

**修复**（[remote_apply.rs](../app/flowix-desktop/src/commands/cloud/local_adapter/remote_apply.rs) 三处）：
- P0-2 回声抑制去掉 old_path 条件：disk hash == expected 即跳过写盘（登出重登后无基线时本地与远端一致的场景不再误入 P1-3）；
- 同批 changes 按 filename 去重只 apply 最后一条（中间版本只可能来自 cursor 重放，逐条落盘会把本地反复覆盖回旧版）；
- put 路径的 P1-3 判据去掉 `(Some, None)`（无基线≠分叉；本地领先的兜底改由 has_pending dirty 队列承担），并在无基线覆盖前把本地字节**首次归档**一份 conflict 副本（副本路径按 note_id 固定、已存在则跳过，重放不覆盖真本地版）——任何场景本地内容不丢。删除路径的 (Some, None) 保护语义未动。

**验证**：修复后 dev 热重编，cursor 0→225 一轮收敛、threads bootstrap 完成、零 WARN；本地与云端 8 文件全量一致、无冲突副本残留（对齐场景走 P0-2 skip）；外部直写 2026-10-26test.md（双卡版 562B）30s 内经 watcher→dirty→push 上云（seq 226）。cloud 相关 35 单测零回归（含 delete 路径无基线保护用例）。

**thread card 跨端双卡互认（实机闭环）**：桌面卡（deepseek-harness，threadId 为本地 runtime 线程）+ 移动端卡（mobile，threadId 空）写入同一笔记上云后，移动端打开渲染 **2 张卡**：mobile 卡可对话（composer 在）、桌面卡只读（「桌面端 AI 会话 · 只读」，线程未上云时提示「会话尚未同步到本机」）；移动端自动保存回写后云端 blob 双 marker 完整保留（48=2×24 字符）。桌面→云→手机、手机→云→（桌面文件层）双向均通。

**操作备查**：dev 连自建云必须带 `FLOWIX_CLOUD_API_BASE=http://8.160.162.251:8787`（默认 cloud.flowix.cc，登录会 INVALID_CREDENTIALS）；桌面 dev WebView 调试用 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9223` 后 CDP 直连（可 invoke cloud_login/cloud_sync_now）。账号 loop@selfhost.local。

**遗留**：① ~~强杀落 refresh 轮换窗口丢登录态的窗口期风险~~ **已加固并部署（2026-10-27）**：flowix-cloud [auth.rs](../app/flowix-cloud/src/auth.rs) 加 `RefreshGrace`——refresh token 消费后 60s 宽限窗口内幂等重放（返回与首次轮换**完全相同**的新 token 对，客户端写回后与主表一致）；验活条件 = 新 refresh 行仍在主表，logout 或新 token 被再次轮换消费后宽限即失效（不复活会话）。覆盖「服务端已消费、客户端未写回 secrets」的强杀窗口（原场景 restore INVALID → clear_v2_account 登出）。内存缓存（单实例自建云），重启丢失可接受（60s 窗口撞重启概率忽略）。客户端零改动透明受益。集成测试更新为宽限语义（重放幂等/验活失效/logout 失效/伪造拒绝），cloud 5/5 全过；本地端到端复现事故现场验证。**已部署自建云**：Docker rust:alpine 交叉编译 musl static-pie（顺带修 flowix-core `rename_file_noclobber` 的 musl 分支——libc crate 无 renameat2 绑定，改 syscall 直调；gnu/android 路径不变），scp → systemd 替换重启（旧版备份 flowix-cloud.bak-），**线上验证全绿**：register/rotate 200、事故现场（丢弃新 token 持旧恢复）200 同 token、继续轮换 200、旧 token 消费后 401；② ~~移动端新建笔记首推不生效~~ **已复核不复现（2026-10-26 下午）**：Loop 本 note_create 后手动 `cloud_sync_now`（uploaded=1）与自动路径（等 40s 防抖/轮询后差分 uploaded=0）均正常上云；初判 §17 当时现象为本节 bootstrap 冲突死循环同现场症状（pass 在冲突循环中失败，非独立 bug）。

## 19. 移动端对话窗 P0 渲染层 + P1a 上下文注入（2026-10-26，已交付）

评估文档（mobile-chat-parity-eval.md 第二版）双端分工模型的第一、二步落地：**P0 消息渲染对齐** + **P1a 笔记上下文注入**。P1b（会话管理）/P2（图片输入）待后续。

### P0：消息渲染对齐（纯前端）

- **chat-render.ts**（新）：桌面渲染管线移植——marked v17 实例（gfm + breaks），html renderer 全转义只放行 `<a>`（`sanitizeHref` http/https/相对路径白名单，template 解码后再过白名单防 `javascript&#x3A;` 绕过）、image href 白名单、**CJK 加粗边界修正**（`agentCjkStrong` 扩展）、code renderer 输出 `pre.chat-code[data-lang]`。Shiki/KaTeX 未移植（P2b 按需）。
- **ChatScreen.tsx**：`MessageBubble` 富渲染——user 纯文本+时间戳、tool 消息 → `ToolCallRow`（Terminal 图标+名字+摘要+展开 toolInput/toolData）、assistant（reasoning details + markdown + 流式光标 + CopyButton）、system markdown；`renderChatMarkdown` + `dangerouslySetInnerHTML`；流式 liveText 阶段同样按 markdown 渲染。
- **事件委托复制**：`pre.chat-code` 右上角点击复制（CSS `::after` 角标 + 命中区）；代码块语言标签 `::before content:attr(data-lang)`。
- **CSS**：`.chat-md` 排版（p/h/list/blockquote/a/img/hr/table/code）、`pre.chat-code` 横向滚动、工具行/操作行样式。
- `package.json` 显式声明 `marked` 依赖（原为传递依赖，防断供）。

### P1a：笔记上下文注入（移动端对话窗核心能力）

- **chat.rs**：`ChatNoteContext { notebook_id, relative_path }` + `build_note_context_suffix`（get_notebook_config_by_id → 读文件 → 前 500 字 → `<## CONTEXT PROMPT ##>` 块，对齐桌面 context-block.ts 格式；读失败静默降级）。**仅首条消息注入**（`payload.messages.is_empty()` 时拼接），落库 content 恒为用户原文（llmContent 只进请求）——与桌面 content/llmContent 分离语义一致。
- **commands.rs**：`agent_chat_send` 加 `note_context: Option<AgentNoteContext>` 参数；`AgentNoteContext` 必须 `#[serde(rename_all = "camelCase")]`（**Tauri 2 命令参数顶层自动 camel→snake，嵌套结构体字段不转**，否则报 missing field `notebook_id`）。
- **前端接线**：扩展 `addOptions/addStorage` 携带 `noteContext`（EditorScreen configure 传入当前笔记），组件内读 `editor.storage.agentThreadCard.noteContext`（**不能用包装组件传参**——`(props) => Component(props, extra)` 破坏 ReactNodeViewRenderer 组件身份，新卡发送后气泡不渲染）；`useThreadChat` 第三参 + send 时随 `agentChatSend` 上送。

### 验证（模拟器 CDP 实机）

mock 引擎（mock-llm.cjs，PORT=18900）回复 markdown 富内容并回显最后一条 user 消息尾部 700 字。verify-p0-p1a.mjs 全绿：

- **P0**：strong/em/blockquote/ol/rust 代码块/text 回显块/语言角标×2/复制按钮/时间戳×2 全部命中；
- **P1a**：回显含 `CONTEXT PROMPT` marker + 笔记内容（`loop note one`）；user 气泡落库干净（无 CONTEXT 块）。

**验证脚本坑**：多卡笔记里 `document.querySelector('.agent-thread-card-m .atc-m-input')` 命中**文档序第一张卡**（旧卡），消息发错卡——必须先取 `cards[last]` 再在其内部 querySelector。

### 交付文件

chat-render.ts（新）/ ChatScreen.tsx / extensions/agent-thread-card.tsx / agent/chat.rs / agent/commands.rs / api.ts / EditorScreen.tsx / styles/screens.css / package.json。

## 20. 移动端对话窗 P1b 会话与交互补齐（2026-10-26，已交付）

评估文档第三步落地：会话列表增强 + 草稿持久化 + 卡片全屏跳转。未含：超长消息折叠、顶部历史分页、连续工具分组、桌面卡特殊消息徽章（按需后续）。

### 会话列表（ChatListScreen 重写）

- **筛选 chips**：全部 / 收藏 / 已归档（归档在「全部」隐藏，只在「已归档」出现）。
- **日期分组**：今天 / 昨天 / 7 天内 / 更早（threads 按 updatedAt 倒序，组序自然降序）。
- **未读点**：`thread-read.ts` localStorage map（threadId→已读时间戳），`updatedAt > 已读` 且有消息判未读——纯本地视图不上云；ChatScreen 打开/刷新时 markThreadRead。
- **长按菜单**（480ms pointer 计时 + 抑制后续 click）：m-sheet 动作面板——重命名（内联输入）/ 收藏⇄ / 归档⇄ / 删除。pointermove 取消（滚动不误触）。

### schema：starred/archived（收藏/归档）

- `V2ThreadPayload` 加 `starred`/`archived`（`#[serde(default)]`，旧端反序列化忽略、重写时丢弃——v1 JSON 版本化向后兼容，无迁移成本）。
- **merge 语义：取 local**（v2_merge_thread_payloads）——旧端（未升级的桌面）重写 blob 会丢字段，local 优先保证本端标记不被远端旧版本覆盖；桌面投影（cloud_bridge.rs）暂写 false。单测 `thread_merge_keeps_local_starred_archived_flags` 锁定。
- 新命令 `agent_thread_update(thread_id, title?, starred?, archived?)`：update_thread_meta 落库 → record_v2_thread_change(Put) → threads-changed → 防抖同步。重命名与 merge 的「title 取最后消息更新一方」规则共存：消息静止时重命名生效，远端再来新消息时 title 会被消息方覆盖（已知限制）。

### ChatScreen

- **标题**：agentThreadList 查当前会话 title（chat.rs 首条消息自动取名 30 字），threads-changed 跟随远端改名。
- **草稿持久化**：localStorage `flowix.chat.draft.<threadId>`（新对话统一 'new' 键），onChange 即存、发送/失败路径清理。
- **滚动跟随/脱离**：仅当用户在底部 120px 内时自动滚底，向上翻阅脱离跟随（pinnedRef + onScroll）。

### 卡片（agent-thread-card）

- **全屏跳转**：有 threadId 的卡 header 显示 Maximize2 按钮 → `onOpenThread`（扩展 storage，与 noteContext 同机制）→ App 的 openChat 直通 ChatScreen。
- **草稿持久化**：blur/折叠时写 `attrs.inputDraft`（随笔记自动保存，注释 JSON 与桌面同格式跨端互认）；初始态从 attrs 恢复；发送成功清空。每键不写 attrs（避免笔记 dirty 抖动）。

### 验证（模拟器 CDP 实机，三脚本全绿）

- **列表**（verify-p1b-list.mjs）：chips×3 / 分组（今天+7 天内）/ 未读点→打开后消除 / 长按重命名→列表更新 / 收藏→收藏筛选可见 / 归档→全部隐藏+已归档可见+取消归档还原。
- **卡片跳转**（verify-p1b-card.mjs）：Maximize2 → ChatScreen 8 气泡 + 标题正确。
- **卡片草稿**（verify-p1b-draft.mjs）：输入→blur→保存→重开笔记恢复。
- cargo check（flowix-sync + flowix-mobile）零错误；flowix-sync 70 单测全过。⚠️ flowix-desktop 被 flowix-relay 工作区未完成改动（routes.rs SSE 闭包 E0308，非本任务）卡住无法整编，cloud_bridge.rs 仅 3 处字面量补字段（与 mobile 同构已验证模式）。

**验证脚本坑（CDP）**：① 长按模拟必须补 `pointerup`+`click` 完整序列，否则 suppressClick 标志不消费、吞掉后续点击；② React onBlur 监听原生 `focusout`（blur 不冒泡不委托），`dispatchEvent(FocusEvent('blur'))` 不触发。

### 交付文件

flowix-sync/v2.rs / flowix-desktop cloud_bridge.rs / flowix-mobile agent（mod.rs/commands.rs/chat.rs/lib.rs）/ 前端（thread-read.ts 新、ChatListScreen 重写、ChatScreen、agent-thread-card、EditorScreen、App、api.ts、screens.css）。

## 21. 移动端对话窗 P2a 图片输入（2026-10-26，已交付）

评估文档第四步落地。P2b（KaTeX/生成图片条/turn 时长）未做，按需后续。

### schema：V2ThreadMessage.attachments

- `V2MessageAttachment { file_name, storage_key }`（fileName 跨端按同名文件回落、storageKey 本机绝对路径）——与笔记图片附件同语义。
- `#[serde(default, skip_serializing_if = "Vec::is_empty")]`：**无附件消息序列化不含该键**，旧消息字节稳定（apply 幂等/云端 hash 不漂移）。roundtrip 单测覆盖 camelCase 与省略行为。
- 消息按 id 并集合并，attachments 随消息走，merge 无需额外逻辑。

### 发送链路（chat.rs）

- `agent_chat_send` 加 `attachments` 参数（嵌套结构体 serde camelCase）；空文本 + 空附件报 `AGENT_CHAT_EMPTY`；纯图消息标题占位 `[图片]`。
- 带附件的 user 消息投影为 **vision 数组 content**：`[{type:text},{type:image_url,image_url:{url:data:image/png;base64,...}}]`；`attachment_data_url` 读 storageKey → base64（mime 按扩展推断），读失败静默跳过降级纯文本。历史重发中他端绝对路径失效时同样降级（本机生成的附件在本机重发有效）。

### 前端

- **ChatScreen**：选图按钮（file input → FileReader → `attachment_save`）→ 待发缩略图条（可移除）→ 随消息上送；附件锚定笔记本 = 卡片全屏进入带的笔记笔记本 ?? 第一个笔记本（App `chatNotebookId`）；乐观消息带 attachments。
- **渲染**：`AttachmentThumbs`（attachment_read 读回 dataURL，失败文件名占位）挂在 user 气泡内，卡片与全屏共用（`attachmentNotebookId` prop：卡片传 noteContext.notebookId）。
- 卡片输入不带图（跳全屏发，附件渲染两边同源）。

### 验证（模拟器 CDP 实机全绿）

verify-p2-image.mjs：1×1 PNG 经 attachment_save 落 Loop/attachments/ → 新线程带附件发送 → mock 回复含「收到 1 张图片」（vision 投影 ✓）→ 落库消息 attachments=["p2-test.png"]（✓）→ ChatScreen user 气泡缩略图渲染（✓）→ 选图按钮存在（✓）。CDP 无法驱动原生文件选择器，UI 选图路径以 invoke 等价验证 + DOM 断言覆盖。

### 交付文件

flowix-sync（v2.rs/lib.rs 导出）/ flowix-desktop cloud_bridge.rs（构造点补字段）/ flowix-mobile agent（chat.rs/commands.rs）/ 前端（ChatScreen、agent-thread-card、App、api.ts、screens.css）。
