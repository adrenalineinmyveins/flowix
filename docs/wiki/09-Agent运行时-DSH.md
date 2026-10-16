# 09 Agent 运行时：DSH（dsh-appserver + dsh-flowix-memory）

本文覆盖 Node.js/TypeScript 侧的三个目录：

| 目录 | 角色 |
| --- | --- |
| `dsh-appserver/` | Flowix 自研 **Agent 应用服务器**（Cordis 插件），在 DSH 原生服务之上暴露"Codex app-server 形态"的 Thread/Turn/Item JSON-RPC 协议 |
| `dsh-flowix-memory/` | 极小的 DSH bundle 插件：把本地 `flowix-cli` MCP server 注册进任意 Harness 实例（Agent 获得 `mcp__flowix__memo` 工具） |
| `dsh/` | 仅 `upstream.lock.json`（锁定上游 `github.com/deepseek-ai/deepseek-harness` 的 commit）；运行时以自包含 Node 24 包独立分发（见 `docs/DSH-RELEASE.md`） |

## 1. dsh-appserver 分层架构

```
src/
├── cordis-plugin.js          # Cordis 插件入口：inject 原生服务 → 装配 DshAppServer → 启动 stdio/HTTP
├── app-server/
│   ├── server.js             # DshAppServer：握手/校验/调度/错误映射/连接订阅管理
│   ├── transports/           # stdio.js（JSONL）、http.js（POST /rpc + SSE + 移动认证）
│   ├── protocol/             # json-rpc.js、foundation.js（协议版本）、method-schema.js（声明式方法目录）、domain-errors.js
│   ├── methods/              # 13 个资源方法模块（thread/turn/session/model/credential/mcp/...）
│   ├── approvals/            # ApprovalManager + approval-presenter（JSON-RPC server request 审批）
│   ├── services/             # 18 个应用服务（业务全部在这里）
│   ├── runtime/              # NativeDshAdapter、AgentRuntimeRegistry、RequestScheduler、HistoryGuard、ProjectionWorkerPool
│   └── projections/          # 事件归一化 / Turn 投影 / Transcript 投影 / 通知投影 / 缓存与索引
├── index.ts / types.ts       # TS 协议类型 + 演进方向的窄端口（ThreadPort/TurnPort/ModelAdminPort...）
└── mock-adapter.ts           # InMemoryHarnessAdapter（协议烟测）
```

设计约束（`DESIGN.md`）：

- **DSH Session event log 是唯一持久事实源**，App Server 不建第二套业务状态；
- 同一 Thread 变更串行、不同 Thread 并行（RequestScheduler 按资源键串行）；
- Adapter 不解析 JSON-RPC，Transport 不实现领域行为；
- capabilities 必须反映当前 ctx 真实可用的服务；
- 超阈值的历史投影进 Worker Pool（worker_threads），实时事件留主线程。

### 与上游 DSH 的协作

`NativeDshAdapter`（`runtime/native-dsh-runtime.js`）**不 spawn 二进制**，而是持有宿主注入的 Cordis ctx，以编程 API 调用原生服务：`ctx.agents.create/resume/get`、`ctx.sessionPersistence.open('read')`、`ctx.commands.execute`、`ctx.llm.listModels`、`ctx.credentials.set`、`ctx.get('subagents')`、`ctx.get('sessionQuery').observeSession` 等；事件经 `ctx.on('session/event' | 'agent/assistant-stream' | 'approval/request' | ...)` 订阅。每个服务都有能力探测降级（如 persistence 缺 `readEvents` 时回退 `open('read')` 快照）。

## 2. 协议设计

- **JSON-RPC 2.0**：标准错误码 + 扩展 `notInitialized(-32002)` / `alreadyInitialized(-32003)` / `server_overloaded(-32001)`。
- **版本**：`APP_SERVER_PROTOCOL_VERSION = 1`（协商）；`APP_SERVER_API_VERSION = 'v2'`（Codex 形态资源面）。
- **领域错误**：稳定 `error.data.kind`（客户端不得解析英文消息）：`capability_unavailable(-32011)`、`request_cancelled(-32012)`、`revision_conflict(-32013)`、`session_not_found(-32020)`、`history_too_large(-32030)`、`result_too_large(-32031)`。
- **声明式方法目录**：`protocol/method-schema.js::METHOD_SCHEMAS` 同时用于参数校验（`validateMethodParams`）与调度元数据（`methodSchedule`：thread/connection/read/config/model-config/mcp/credential），`npm run docs:protocol` 从它生成协议文档。

### 方法清单（56 个，按域）

| 域 | 方法 |
| --- | --- |
| 生命周期 | `initialize` `shutdown` `serverRequest/respond` |
| Thread | `thread/start`（服务端铸造 `session-<uuid>` 持久 ID）`thread/resume` `thread/read` `thread/list` `thread/fork`（按 boundarySeq 切事件日志）`thread/turns/list` `thread/events/list` `thread/close` `thread/archive` |
| Turn | `turn/start` `turn/steer`（注入活动 turn 的 next-step inbox，不新建 turn）`turn/interrupt` |
| Session | `session/flush` `session/ensure` `session/prompt` `session/history`（以 turn 为分页单位）`session/dispose` `run/cancel` |
| 命令/技能 | `thread/command`（`/export` 触发会话导出）`thread/skills` `skills/list` `skills/config/write` `skills/extraRoots/set` |
| 配置 | `config/read` `config/value/write` `config/batchWrite` `configRequirements/read`（写路径白名单；秘密走 credential/*） |
| MCP | `mcpServerStatus/list` `mcpServer/refresh` `config/mcpServer/reload` `mcpServer/oauth/login` `mcpServer/tool/call` `mcpServer/resource/read` `mcp/resource/read` |
| 子代理 | `thread/subagent/spawn|list|send|resume|interrupt|wait|close` |
| Goal | `thread/goal/get|set|clear`（乐观版本 expectedVersion，编译为 `/goal` 命令） |
| 审批 | `thread/approvalPolicy/read|write`（ask\|never） |
| 模型 | `model/catalog` `model/discover` `model/config/read|upsert|remove` |
| 凭证 | `credential/read|set|unset`（reference 必须是合法环境变量名） |
| Runtime | `runtime/capabilities` `runtime/status` |
| Flowix 扩展 | `flowix/jobs/list` `flowix/session/usage`（token 用量）`flowix/plugins/list`（host/preset/profile 三层）`flowix/runtime/profile` |

### 服务端通知（节选）

`thread/started`、`thread/status/changed`、`turn/started|completed`、`item/started|completed`、`item/agentMessage/delta`、`goal/changed`、`config/changed`、四类审批 server request（commandExecution/fileChange/permissions/tool）、`subagent/*`；带 `sourceSeq` 的事件可重放去重。

## 3. 事件投影（核心机制）

概念映射：**DSH Session = 协议 Thread**；Turn = 事件流中 `turn/start`/`turn/end` 对（稳定 ID `<threadId>-turn-<n>`）；Item = userMessage/agentMessage/toolCall/toolResult/...（状态机 inProgress→completed|failed|cancelled，带 revision + sourceSeq）。

三层投影：

1. **实时投影**（`native-dsh-runtime.js::projectEvent`）：订阅 `session/event` → 失效缓存 → 喂增量索引 → 产生 `turn/*`、`item/*` 通知；流式 assistant chunk 与持久快照合一（防"流式一条+快照一条"重复）；`turnEndError` 归类配额耗尽/限流等结构化失败（含嵌套 provider JSON 解包）。
2. **历史投影**（`projections/event-projector.js::projectHistoryMessages`）：把 append-only 事件日志折叠为消息数组——tool/call+result 合并一条；命令聚合；过滤模型向隐藏消息（plan-steer 重复、system-reminder 等，以持久 `source` 标记为准）；压缩检查点显示为 `context-compaction` 行（`preserveCompactedHistory: true` 保留追加式时间线）；分页以 turn 为原子单位。
3. **通知重放投影**（`notification-projector.js::projectNotifications`）：把任意 `afterSeq` 之后的事件重放为与实时通知同形态的序列，供 SSE 重连/移动端 `/sync` 补发。

支撑组件：`ItemLifecycle`（每 thread 2048 item/4096 event 上界，started→delta→completed 单调去重）、`ProjectionCache`（LRU+TTL+64MB 字节上限，版本键 `events.length:lastSeq`）、`SessionProjectionIndex`（进程内增量索引，正确性始终回退事件日志）、`ProjectionWorkerPool`（事件 ≥5000 时卸载到 worker_threads，5s 超时 + AbortSignal）。

### 审批机制

`ApprovalManager`：DSH `approval/request` → JSON-RPC **server request** 回传客户端；Thread 租约（thread/start|resume|turn/start 时 `acquire(threadId, connectionId)`）决定发给哪个连接，防同机进程伪造所有权；pending 审批为进程内状态，SSE 重连补发；120s 超时/断连优雅期后 settle 为 `unavailable`。

## 4. 传输层

| 传输 | 要点 |
| --- | --- |
| stdio（`transports/stdio.js`） | JSONL 逐行；有 id 无 method 的输入 = 对 server request 的响应；单连接 `'default'` |
| HTTP/SSE（`transports/http.js`） | `POST /rpc`（1MB 上限）；`GET /events` 可恢复 SSE（先缓冲实时 → 按 afterSeq/Last-Event-ID 重放 ≤5000 → 高水位去重切实时；心跳 15s；背压队列化）；`GET /sync`；`GET /exports/:id`；`/readyz` `/healthz` |

安全边界：默认只绑 `127.0.0.1`；非回环监听必须配置 authToken/pairingSecret；浏览器 Origin 默认拒绝；移动配对 `/auth/pair`（防爆破 5 次/60s）+ refresh/revoke + `x-device-id` 绑定；可选 `secureClientIdentity`（首连签发连接身份防伪造）。

## 5. 18 个应用服务速览

| 服务 | 一句话职责 |
| --- | --- |
| ThreadLifecycleService | thread start/resume/close/archive；resume 检测 provider/model 路由变化重建 Agent |
| TurnService | followup 开回合 / steer 插话 / cancel 中断；预注册稳定 turnId |
| SessionRepository | live/持久会话只读访问（二分分页 / readEvents / inspect 回退） |
| SessionQueryService | flush/ensure/history/usage；history 经 HistoryGuard + Worker 卸载 |
| AttachmentGateway | 回合输入附件准入（仅编码 image） |
| CommandService | `ctx.commands.execute`；`/export` 生成会话导出（ExportStore） |
| SkillCatalogService | 技能目录（scope 判定 + 有界读穿缓存 + revision） |
| ConfigService | DSH settings 读写（layer 归一、秘密脱敏、路径白名单、revision 冲突映射） |
| McpService | MCP 服务器状态/刷新/OAuth/工具调用 |
| CredentialAdminService | 凭证 describe/set/unset |
| ModelSettingsService / ModelCatalogService | `llm-pi-ai` providers 路由 revision 化读写 / 模型目录合并 |
| RuntimeCapabilityService | 按实际 ctx 服务动态生成 capabilities |
| GoalService | goal 投影 + `/goal` 命令编译 + 乐观版本 |
| ItemLifecycle（service） | 实时/重放条目生命周期门 |
| SubagentService | DSH 原生 continuable subagents（限 16 并发/4 深度） |
| ExportStore | 导出文件 ≤1MB 内联，否则 tmpdir 受控目录 + 15min TTL |
| MobileAuthStore | 移动配对 token（SHA-256 哈希存储、timingSafeEqual、0600、设备上限 64） |

## 6. dsh-flowix-memory 插件

```
dsh-flowix-memory/
├── cordis.patch.yml      # 向 DSH 配置注入 @deepseek-ai/dsh-mcp-client：
│                        #   serverName: flowix, transport: stdio,
│                        #   command: process.execPath（宿主 Node）,
│                        #   args: [DSH_PROFILE_DIR/node_modules/dsh-flowix-memory/launcher.mjs]
├── launcher.mjs          # ~160 行零依赖启动器
├── memo-tool-schema.json # memo 工具 JSON Schema（flowix-cli 编译期内嵌同一份）
└── package.json
```

launcher 两条路径：

1. **找到 CLI**（优先 `FLOWIX_DSH_MCP_CLI`/`FLOWIX_CLI_PATH` → macOS/Windows 应用内路径 → PATH）：`spawn(cli, ['mcp'])` 全双工透明代理 stdin/stdout/stderr（转发退出码）；
2. **CLI 缺失**：运行最小 MCP server（仍注册 `memo` 工具），调用时返回 `isError:true` 的可操作安装指引——保证工具注册状态稳定。

安装：`dsh plugin --profile flowix add ./dsh-flowix-memory`；Flowix Desktop 把同一 bundle 装进 `flowix` profile 并设置 `FLOWIX_DSH_MCP_CLI`。spawn 前剥离环境中类凭证变量与所有 `DSH_*` 变量。

## 7. 测试布局

| 层 | 内容 |
| --- | --- |
| Vitest（`npm test`） | `protocol.test.ts`（用 InMemoryHarnessAdapter 端到端协议）、`projector.test.ts`（投影/错误归类）、approval/item-lifecycle/thread-lifecycle/config/mcp/skills/subagent/native-dsh-adapter 各服务、`long-session.test.ts`（生产守护：HistoryGuard/缓存有界/万级事件） |
| Node smoke | `http-transport` / `models` / `mobile-http`（移动配对+SSE）/ `smoke:projection-worker`；Windows 专属 `.ps1` 两个 |
| 真实 Runtime smoke | `runtime-stdio/http/llm.smoke.mjs`，宿主装配器 `test/runtime/host.mjs` 解析已安装 DSH（`~/Library/Application Support/Flowix/dsh/current.json`）并拉起真实 Cordis 宿主 |

类型检查：`npm run typecheck`（`tsconfig.json` 严格查 TS）+ `tsconfig.js.json`（allowJs 模块解析检查核心 JS）。
