# 移动端个人 Agent 界面——评估分析文档

> 状态：评估稿（2026-10-28，基于当前代码与实测）
> 前置：`docs/mobile-chat-parity-eval.md`（2026-10-26，P0-P2 功能对齐已全部落地并验证）；本文在其基础上纳入此后三天的架构演进（桌面会话卡解除只读、线程 id 归一、provider runtime 会话上云、双向续聊回环）后的**新现状与新缺口**。
> 关联：`docs/mobile-agent-plan.md`（方案 A 主文档）、`docs/harness-thread-sync-plan.md`（provider runtime 同步）

---

## 0. 结论摘要

**总体判断：移动端 Agent 界面的「查看 + 轻聊」主线已完成且经三端实测；剩余缺口集中在四类——1 个数据层缺陷（收藏/归档被桌面抹平）、1 个上下文断层（手机续聊不进桌面 runtime 会话上下文）、性能面（长会话全量渲染）、体验面（错误引导/徽章/搜索）。**

| 维度 | 完成度 | 说明 |
|---|---|---|
| 会话列表（ChatListScreen） | ★★★★☆ | 筛选/分组/未读/长按菜单齐备；**收藏/归档存在跨端抹平缺陷（D1）** |
| 全屏对话（ChatScreen） | ★★★★☆ | 流式/附件/工具行/markdown/滚动跟随齐备；**长会话全量渲染无分页（D3）** |
| 笔记内会话卡（agent-thread-card） | ★★★★☆ | 双端卡互认、手机续聊、文字流过滤齐备；空卡 hint 小 bug（D8） |
| AI 配置（SettingsScreen） | ★★★☆☆ | base/key/model 可配；**错误引导弱（D6）** |
| 跨端数据面 | ★★★★☆ | 双向闭环 + provider runtime（harness/codex/opencode）上云；claude/hermes/pi 未覆盖（D5） |

---

## 1. 评估范围与证据基线

界面清单：Login（服务器配置）/ Notes / Editor（含会话卡）/ ChatList / Chat / Settings。
数据面：`threads.db`（V2ThreadPayload 整包）、云 v2 线程实体（union merge）、`agent_*` IPC 命令面（api.ts 19 个 agent 命令）。
证据：本会话三端实测记录（雷电模拟器 + 桌面 dev CDP + 自建云 8.160.162.251:8787），关键断言：6+1 个 harness 会话上云、108 条长会话完整同步、手机续聊归一同实体（108→110）、桌面 apply 过滤 + 读路径 merge、无 push 震荡。

## 2. 已完成能力（截至 2026-10-28）

**会话流（三端闭环）**
- 手机发起/续聊会话；桌面会话（含 deepseek-harness / codex / opencode 全系 runtime）实时上云，手机可见可续聊；桌面 UI 经首页 merge 显示手机续聊消息
- 桌面会话卡（笔记内）手机端可看可续聊（只读限制已解除，commit 6939b451）；文字流渲染过滤（工具/推理不渲染，数据完整）
- 线程 id 归一（b47b8eb5）：桌面本地 id / 云端 id 双形态互认，未同步报 `AGENT_THREAD_NOT_SYNCED` 不再静默分叉

**界面功能**（parity-eval P0-P2 落地项）
- ChatScreen：markdown/代码块/工具行展开/复制/时间戳/reasoning 折叠/流式光标/停止/图片附件（vision）/草稿持久化/滚动跟随
- ChatListScreen：日期分组/未读点/收藏/归档筛选/长按菜单（重命名/收藏/归档/删除）/agentId 标注
- 卡片：折叠/草稿跨端互认（inputDraft 随笔记同步）/全屏打开/删除；上下文注入（首条消息带笔记前 N 字）

## 3. 缺口清单（按严重度排序）

### D1【数据缺陷·P0】收藏/归档被桌面投影抹平 —— ✅ 已修复（e6c24f94）
桌面出向投影 `starred/archived` 曾硬编码 false 且无本地镜像。修复：threads 表加镜像列
（migrations v14），apply 落列（云端真源）、投影读列回推。实测：手机收藏 → 桌面落列
starred=1 → 投影 clean 收敛（云端保持 true）→ 手机视图正常。

### D2【产品断层·P1】手机续聊不进桌面 runtime 上下文
手机在 harness/codex/opencode 会话续聊：消息进云端与桌面 `thread_messages`（桌面 UI 能显示），但 **runtime session（DSH 等）不含这些消息**——桌面端继续对话时，runtime 以自己的 session 历史为上下文，**看不到手机说过什么**，出现语义断层。
**修复方向**（两选一）：a) 桌面发送前把云端新增消息（origin≠desktop）注入 runtime 上下文（prompt 拼接）；b) 手机端对 provider runtime 会话标记「轻跟进」并禁续聊输入（保守）。涉及桌面 runtime 桥，建议单独立项评估。

### D3【性能·P1】长会话全量拉取渲染
`agent_thread_messages`（Rust）一次返回全部消息，ChatScreen/卡片全量渲染。provider runtime 长会话上云后已成真实场景（实测 108 条；桌面侧无上限）。
**修复方向**：Rust 命令加 `before/limit` 分页 + 前端顶部历史加载（桌面 `thread_get_page` 同款游标语义）。

### D4【体验·P1】超长消息无折叠预算
单条超长回复（工具结果/markdown 长文）全高渲染，长会话滚动成本高。桌面有折叠预算 + 展开全文机制，移动端未做（parity-eval P1 遗留）。

### D5【数据面·P2】journal runtime 与 pi 会话不可见
claude / hermes（消息在 `agent_external_events`）与 pi 的会话未纳入同步（provider runtime 泛化时明确排除）。手机端看不到这类桌面会话。
**修复方向**：journal 投影路径（events → messages → payload），与 provider runtime 桥同构，待有真实使用需求再做。

### D6【体验·P1】Provider 未配置的错误引导弱
未配 AI 时发送返回 `AGENT_PROVIDER_NOT_CONFIGURED`，界面直接显示错误码字符串；空列表虽有「在设置中配置 AI 服务」提示，但错误路径无**跳转设置**动作。本次排查实测触发过（模拟器重建后配置丢失）。

### D7【体验·P2】会话列表无搜索、runtime 标识生硬
- 列表无搜索框（桌面全局搜索有会话维度）
- agentId 以原始字符串展示（`deepseek-harness` 等），无中文友好名/图标徽章；桌面卡的特殊系统消息（compaction 等）无 messageType 徽章（parity-eval P1 遗留）

### D8【小 bug·P2】空 threadId 新卡 hint 不显示
`useThreadChat` 中 `messages` 初始为 null 且 threadId 为 null 时 reload 早退——「发送第一条消息开始对话」hint 永不渲染（`agent-thread-card.tsx:235-237` 条件永假）。功能不受影响（composer 可用）。

### D9【健壮性·P2】同步层遗留（非界面，影响体验）
- 幽灵分叉死锁无自愈（磁盘≠基线且无 dirty 时 apply 永远报错；本次事故即其变体：**有 dirty + 无基线 + bootstrap 纯拉取**组合死锁）
- 桌面 dev 依赖 `FLOWIX_CLOUD_API_BASE` 环境变量，漏设静默回退官方云（本次事故根因之一，建议默认值或显式警告）
- 首页 merge 时序局限：桌面在手机消息之后产生更新消息时，手机消息显示位置略偏（仅显示层，已注释说明）

## 4. 优先级建议

| 批次 | 项 | 理由 |
|---|---|---|
| **P0（数据正确性）** | D1 收藏/归档持久化 | 已有 UI 功能事实上不可靠，跨端丢数据 |
| **P1（主线补强）** | D3 历史分页 → D4 折叠预算 → D6 错误引导 | 长会话已成真实场景；引导是新手第一触点 |
| **P1（产品决策）** | D2 上下文断层 | 需先做产品决策（注入 vs 禁续聊），再排期 |
| **P2（按需）** | D5 journal 同步、D7 搜索/徽章、D8 hint、D9 健壮性 | 有使用信号再排 |

## 5. 验证方式建议（实施后）

- D1：手机收藏 → 云端 payload `starred=true` → 桌面推送一轮 → 云端仍 true → 另一端可见
- D2：手机续聊后桌面再发消息，断言 runtime 收到的上下文含手机消息（DSH 侧日志或回复内容引用）
- D3：108+ 条会话首屏渲染 < 1s、上滑加载无卡顿
- D6：清空 provider 配置 → 发送 → 出现引导而非错误码
