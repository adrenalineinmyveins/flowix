# harness（deepseek-harness）会话真同步方案

> 状态：**已实施并三端验证通过**（2026-10-27，S1-S4 全部落地）
> 实施中的方案修正：
> - 桌面投影必须推「DSH 历史 ⋈ 本地表」**并集**（云端整包 last-writer-wins，
>   只推 DSH 历史会把手机续聊消息从云端抹掉、来回震荡）
> - apply 回放时 dsh 线程**过滤 origin=desktop 消息**不落 thread_messages
>   （权威在 DSH host；否则堆积历史副本、UI merge 重复显示）
> - S4 读路径 merge 采用「首页追加本地尾巴 + 按 id 去重」，分页游标不动
> 范围：仅 deepseek-harness；codex / opencode 架构同构，验证后再复制
> 前置：手机端线程 id 归一已落地（b47b8eb5），手机侧无需再改动

## 1. 背景

手机打开含桌面会话卡的笔记时，deepseek-harness 会话卡恒显示「会话尚未同步到本机」。排查结论：

- harness 会话消息**不在**桌面 `thread_messages`，也**不在** `agent_external_events`（journal 仅 claude 在用）
- 消息存在 **DSH runtime 自有存储**，桌面经 host 进程 wire 协议读取（`DeepSeekHarnessManager::session_history_page`，manager.rs:1311）
- A3 同步桥（`thread_bridge.rs`）的投影与扫描都以 `thread_messages` 为准 → harness 线程对桥不可见，**从未上云**

数据链路现状：

```
桌面 DSH host 进程（消息唯一权威存储）
        ▲ wire 协议（session_history_page 翻页）
桌面 UI/HistoryAdapter(DshHistoryAdapter, provider_policy)
        ✗ A3 桥不读这条路径 ──→ 云端无实体 ──→ 手机永远「尚未同步」
```

## 2. 方案：桥读 DSH 原生历史（provider 原生投影）

不动 DSH、不动云端、不动手机；只扩桌面 `thread_bridge.rs`。

### 2.1 数据流（目标态）

```
reconcile（30s 账户轮询 / 笔记活动触发）
  1) threads_dsh ⋈ threads (updated_at > 水位)   → 活跃 harness 线程
  2) 逐线程 session_history_page 翻页读全量      → ChatMessage[]
  3) ChatMessage → V2ThreadMessage (origin=desktop) → V2ThreadPayload(th_{local})
  4) v2_content_hash(payload) vs v2_thread_states 基线 → 不 clean 才 record Put
  5) 引擎 freeze 上传（现有链路） → 云端实体 th_deepseek-harness-local-agent-inst-*
手机 pull → apply（现有 union merge）→ 会话列表/卡片可见，文字流渲染（工具/推理过滤已生效）
```

### 2.2 改动点

| # | 位置 | 内容 |
|---|---|---|
| 1 | `ThreadManager`（agent_session） | 新增 `dsh_thread_ids_since(watermark) -> Vec<(local_id, updated_at)>`：`SELECT t.thread_id, t.updated_at FROM threads t JOIN threads_dsh d ON d.thread_id = t.id WHERE t.updated_at > ?`（threads_dsh 为 product_id ↔ external session 映射表） |
| 2 | `thread_bridge.rs` reconcile | 在现有 `thread_messages` 扫描后并列跑 #1 的结果；复用同一水位 map |
| 3 | `thread_bridge.rs` 投影 | 新增 `dsh_payload_for_cloud(state, local_id) -> Option<V2ThreadPayload>`：<br>· `session_history_page(local_id, before=None, limit=50)` 起步，`has_more` 时以 `oldest_sequence` 为 before 链式翻页至尽（上限保护，如 200 页）<br>· head（title/agent_id/created_at）沿用 `read_thread_head`（threads 表行已存在）<br>· `thread_id = th_{local_id}`（cloud_thread_id 映射不变） |
| 4 | `thread_bridge.rs` snapshot | `v2_thread_snapshot` 对 dirty 线程：`thread_messages` 无消息且在 `threads_dsh` 中 → 走 #3 投影 |
| 5 | `thread_bridge.rs` 删除对账 | `all_thread_ids_with_messages` 的基准集并入 `threads_dsh.thread_id`（否则刚上传即被误判"本地已删"→ Delete 回滚，flapping） |

### 2.3 ChatMessage → V2ThreadMessage 映射

复用 `cloud_bridge.rs` 行映射的语义，字段对照：

| V2ThreadMessage | 来源（ChatMessage） | 备注 |
|---|---|---|
| id | `id` | DSH 稳定消息 id（`msg:deepseek-harness:run-N:…`），union merge 幂等键 |
| role | `role` | user/assistant/tool 原样 |
| content | `content` | |
| reasoning | `reasoning` | 手机端桌面消息不渲染（已有过滤） |
| tool_name / tool_input | `tool_name` / `tool_input` | tool_input: `serde_json::Value` → 序列化为 String |
| tool_data | `tool_data` | 沿用 `truncate_tool_data` |
| created_at | `message_timestamp_to_millis(&timestamp)` | 与现有行映射同一转换 |
| origin | 固定 `"desktop"` | |
| attachments | `Vec::new()` | DSH 附件投影暂缺，同桌面现状 |

排序：`(created_at, id)` 确定性排序后入 payload（与 `thread_payload_for_cloud` 一致），保证 hash 稳定。

### 2.4 DSH host 拉起与读取成本控制

- `session_history_page` 内部 `hosts.shared(&host_launch_spec())` 会在 host 未运行时**自动拉起 DSH 进程**
- 水位机制保证：静默期（无线程更新）零读取、零拉起；仅当某 harness 线程 `updated_at` 变化才读该线程
- 重启后水位丢失 → 首轮全量回读所有 harness 线程（一次 host 拉起 + N×翻页）。若不接受，可加降级开关：仅当 `hosts` 已有存活实例时投影，否则跳过等下轮（牺牲"重启后未打开过 harness 就不上云"的及时性）
- 决策点：默认**允许拉起**（一致性优先），观察首轮成本再定

## 3. 手机侧（零改动）

- 云端实体 id `th_deepseek-harness-local-agent-inst-*` → 手机 apply 落库、会话列表出现
- 笔记卡 threadId 为桌面本地 id（无前缀）→ **id 归一（b47b8eb5）已覆盖**，直接可看可续聊
- 桌面消息 origin=desktop → 纯文字流渲染（工具/推理过滤、textOnly 气泡已上线）

## 4. 已知边界与风险

| # | 项 | 说明 | 处置 |
|---|---|---|---|
| R1 | **桌面 UI 看不到手机续聊消息** | 手机续聊消息经 union merge 回桌面后落 `thread_messages`；但桌面打开 harness 会话走 DSH host 读路径，不 merge 本地表 | 本方案不解决；第二步改 harness 会话读路径（DSH 页 + 本地表尾巴拼接），或 UI 层拼接。数据不丢 |
| R2 | 消息 id 稳定性依赖 DSH | 若 DSH 历史 id 在重读间不稳定（重排序/重生成），union merge 会重复 | 实施前先用真实会话做两次投影 diff 验证 id 稳定性 |
| R3 | 长会话整包上传 | payload 全量序列化上传；v2 blob 按内容 hash 去重，云端存储 OK，但桌面每次变更需全量读 + 全量 hash | 与现有 thread_messages 线程同量级；超长会话（数千条）可设条数上限 + 截断标记（后续） |
| R4 | starred/archived 抹平 | `thread_payload_for_cloud` 硬编码 false，上云会抹掉手机侧收藏/归档（已知 P1b 遗留，harness 投影同病） | 实施时一并接 threads 表真实标记（若 harness 线程有该字段）或维持现状并记待办 |
| R5 | 首轮回读放大 | 水位丢失后全量回读（见 2.4） | 观察后决定是否加"host 存活才投影"降级 |
| R6 | 与 desktop default 会话的显示差异 | 手机端 harness 会话在桌面 `thread_list`（agent='default'）不可见，仅 `agent_conversation_list` 可见——既有设计，非本方案引入 | 不处理 |

## 5. 验证计划

1. **投影单测**：mock `session_history_page`（多页 has_more 链）→ 断言翻页聚合、映射、排序、hash 稳定性（同输入两次 hash 相等）
2. **reconcile 单测**：`threads_dsh` 活跃线程入队 Put；clean 判定不重复入队；删除对账不误删
3. **桌面实机**：登录自建云 → 打开/新建 harness 会话 → 30s 内云端 changes 出现 `th_deepseek-harness-*` 实体
4. **手机实机**：会话列表出现 harness 会话；`2026-10-26test.md` 两张 harness 卡从「尚未同步」变为显示文字流；续聊归一到同线程（线程数不涨）
5. **回环**：手机续聊 → 云端 revision 前进 → 桌面 pull 落 `thread_messages`（R1：桌面 UI 暂不显示，数据断言在 DB 层）

## 6. 实施拆分

| 步骤 | 内容 | 验证 |
|---|---|---|
| S1 | R2 前置实验：真实会话两次投影 diff id 稳定性 | 脚本级 |
| S2 | 改动点 #1-#5 + 单测 | cargo test |
| S3 | 桌面实机端到端（验证计划 3-5） | CDP + 云端 changes |
| S4 | （另行排期）R1 桌面读路径 merge 手机消息 | — |
