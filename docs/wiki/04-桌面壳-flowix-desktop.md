# 04 桌面壳 flowix-desktop

> 路径：`app/flowix-desktop/` · Tauri v2 桌面应用（Rust），整个系统的**最顶层组装者**：窗口、~260 个 IPC 命令、六种外部 Agent 运行时宿主、DSH 分发、文件监听、插件/模板系统、双通道自动更新。

## 1. 源码地图

```
src/
├── main.rs / lib.rs            # 入口（run() 委托 app::run）
├── app/
│   ├── bootstrap.rs            # ★ 应用构建：插件链、状态装配、260 命令注册、启动对账、事件路由
│   ├── state.rs                # AppState（20+ 托管字段：memo_file/cloud/runtimes/watcher/...）
│   └── watchdog.rs             # 空闲 Agent run 看门狗（5s 扫描，默认 30min 无输出回收）
├── commands/                   # ★ 全部 #[tauri::command]（按域分文件）
│   ├── memo/{reads,creates,deletes,versions}.rs   # 笔记 CRUD/版本
│   ├── agent/{chat,external_config,model_catalog,codex_catalog,notebook_agents,runtime,terminal,image_cache}.rs
│   ├── thread.rs               # 40+ 线程/会话历史命令
│   ├── cloud.rs / dsh.rs / plugin.rs / settings.rs / notebook.rs / tag.rs
│   ├── file.rs / media.rs / collection.rs / document_list.rs / export.rs / dialog/
│   ├── window.rs / cli.rs / boot.rs / kv.rs / product.rs / web.rs / font.rs
│   ├── document_shutdown.rs / document_operations.rs / recovery.rs / external_document*.rs
├── agent_external/             # ★ 六种外部 Agent CLI 运行时（~60 文件）
│   ├── runtime_registry.rs     # ExternalRuntimeRegistry + dispatch_chat 准入协议
│   ├── codex/ claude/ hermes/ opencode/ pi/ deepseek_harness/
│   ├── shared/{process_io,lifecycle}.rs  # 进程 IO 公共层
│   └── node.rs                 # Node 二进制裁剪/发现
├── watcher/                    # ★ 文件监听（manager→event→filter→processor→runtime）
├── dsh.rs + dsh/               # ★ DSH 运行时分发（下载/验签/健康检查/原子发布）
├── agent_wire.rs               # ★ AgentChunk 等 IPC wire 协议（与前端 types/agent.ts 镜像）
├── agent_lifecycle.rs / agent_history.rs / agent_types.rs
├── document_io.rs / document_mutation.rs / memo_events.rs / document_derived.rs
├── plugin.rs + plugin/         # 插件系统（安装/校验/运行/产物）
├── template_store.rs           # 编译期内置模板种子化
├── app_update.rs / app_update_state.rs / update_security.rs
├── cli_link.rs                 # CLI sidecar 链接（symlink / .cmd + 注册表 PATH）
├── apple_sign_in.rs / device_registration.rs / runtime_state.rs
├── events.rs                   # TauriDispatcher（pub-sub 抽象，可测试替换）
├── window_chrome.rs / process_window.rs / frame_scrollbar.rs
└── config/（含 security_bookmark.rs）
```

## 2. 启动流程（app/bootstrap.rs）

1. **构建 Tauri app**：插件链（process/updater/opener/deep-link 等）→ 装配 `AppState`（`MemoFile`、`SyncManager`、六个 Agent manager、watcher、plugin store...）→ 注册 ~260 个命令（`bootstrap.rs:430-764`）。
2. **`setup` 阶段**：CLI sidecar symlink、模板种子化（`template_store::initialize`）、macOS 安全书签恢复、`listen_for_shutdown`（Windows 安装器协作）、DSH 状态探测、设备登记（启动 10s 后 fire-and-forget）。
3. **后台启动对账** `run_startup_reconciliation`（`bootstrap.rs:877-1008`，`StartupCoordinator` 门控前端 `wait_for_startup_ready`）：
   选定笔记本 → 设备级数据迁移 → 当前笔记本内部迁移 → `reconcile_note_index` 索引对账 → watcher `rebind_all` 绑定当前笔记本（其余笔记本首帧后补挂）。
4. **运行事件路由** `handle_run_event`：拖放 → `document_access.grant`；窗口销毁 → 撤销授权；`ExitRequested` → 文档保存握手（20s 超时）→ 停止全部 Agent 子进程（5s 总超时）→ checkpoint。

## 3. Tauri Commands 总览（按域）

> 完整注册表：`src/app/bootstrap.rs:430-764`。命令名为 snake_case，前端经 `platform/tauri/client/` 域客户端调用。

| 域 | 代表命令（非全量） | 文件 |
| --- | --- | --- |
| 笔记读 | `list_notes_by_path` `get_path_notes` `read_document` `search_path_notes` `write_document` `get_used_memo_tag_ids` | `commands/memo/reads.rs` |
| 笔记写 | `add_path_document` `rename_memo_title` `move_memo_to_directory` `import_external_document_by_path` `create_path_from_template` | `commands/memo/creates.rs` |
| 版本/删除 | `list_path_versions` `create_path_version` `restore_path_version` `delete_memo` `prune_missing_memo` | `memo/versions.rs`、`memo/deletes.rs` |
| 笔记本 | `get_notebooks` `create_notebook` `update_notebook` `delete_notebook` `set_current_notebook` `start_notebook_import` | `commands/notebook.rs` |
| 文件树 | `get_file_tree` `read_file` `read_docx_file` `write_file` `move_file` `create_folder` `import_file`... | `commands/file.rs` |
| 标签/集合/媒体 | `get_all_tags` `move_memo_tag`；collection 5 个；`list_media_resources_page` 等 | `tag.rs`、`collection.rs`、`media.rs` |
| Agent 对话 | `chat_with_agent_stream` `steer_agent_stream` `stop_agent_stream` `agent_running_threads`；Codex 追问队列/approval；Pi 追问提升；`execute_deepseek_harness_command` | `commands/agent/chat.rs` |
| 线程历史 | `thread_*`、`agent_conversation_*`、各 CLI 专属 `*_thread_*` 共 40+ | `commands/thread.rs` |
| 云同步 | `cloud_get_state` `cloud_login` `cloud_sign_in_with_apple` `cloud_sync_now` `cloud_list_conflicts` `cloud_resolve_markdown_conflict` | `commands/cloud.rs` |
| DSH | `dsh_status` `dsh_install_runtime` `dsh_update` `dsh_ensure_runtime` `dsh_manage_profile_plugin` | `commands/dsh.rs` |
| 插件 | `plugin_list/install/uninstall/run/run_stop` `plugin_prepare_prompt` `artifact_resolve` | `commands/plugin.rs`、`artifact.rs` |
| 更新/启动 | `check_app_update` `install_app_update` `get_boot_features` `wait_for_startup_ready` | `app_update.rs`、`commands/boot.rs` |
| 窗口/杂项 | `show_main_window` `open_preferences_window` `apply_window_theme` `parse_web_page` `export_pdf` `cli_link_status` `install_cli_path` | `window.rs` 等 |

## 4. Agent 运行时管理（agent_external/）

### 4.1 六种运行时与统一抽象

`runtime_registry.rs`：

```rust
pub enum ExternalRuntimeKind { Codex, Claude, Hermes, OpenCode, Pi, DeepSeekHarness }

pub trait ExternalCliRuntime {
    async fn chat_stream(...); async fn steer_chat(...); async fn stop_chat(...);
    async fn running_threads(...); async fn stop_all(...); async fn reap_inactive_runs(...);
}
```

各 CLI 的子进程协议形态：

| 运行时 | 协议 | 说明 |
| --- | --- | --- |
| Codex | 长驻 app-server，JSON-RPC over JSONL stdio | `initialize → thread/start → turn/start`；turn 通知按 codex_thread_id/turn_id 绑定（`codex/app_server.rs` 的 `Connection`/`ActiveTurn`） |
| DeepSeekHarness | JSON-RPC stdio（`DSH_PROTOCOL_VERSION=1`） | 进程为 Flowix 托管的 DSH Node 运行时（见第 5 节） |
| Claude | 一次性子进程 `claude -p --output-format stream-json --verbose` | stdout 逐行 JSONL → AgentChunk（`claude/command.rs:35-78` 组装参数，支持 `--resume`/permission-mode/`--add-dir`） |
| Hermes / OpenCode | ACP（Agent Client Protocol）JSON-RPC | — |
| Pi | RPC manager（JSONL） | 每会话独立 Pi session 目录 + 可复用 RPC 进程，详见 `docs/PI-INTEGRATION.md` |

### 4.2 幂等准入协议（dispatch_chat，runtime_registry.rs:233-430）

1. 生成/复用 `run_id` + 消息 SHA-256 指纹；
2. `AdmissionState` 维护 requests（≤4096，30min TTL）/ occupied（thread→run）/ protected（归档删除保护）；
3. 同 `(thread_id, run_id)` 且同指纹的重复请求 → 订阅既有 `watch::channel`，**不重复启动子进程**；
4. 已有活跃 run → `AlreadyRunning`；终态由三层证据对账：`StreamEnd` 事件 → `agent-run-terminal` 事件 → 运行时快照（`reconcile_admissions`）；
5. 通过后 `tokio::spawn` supervisor（`catch_unwind` 包裹），结果回填 watch 通道。

### 4.3 进程 IO 公共层（shared/process_io.rs）

| 设施 | 说明 |
| --- | --- |
| `read_capped_line` | 单行 512 KiB 硬上限（防巨帧）；stderr 保留 256 KiB |
| `StreamingEmitBuffer` | 200ms / 64KB 合帧（"每 token 一次 IPC"→"每 flush 一次"），保证 Text→ToolCall→ToolResult 顺序 |
| `redact_sensitive_text` | 跨进程诊断前正则脱敏 API key/Bearer |
| `classify_agent_error` | 厂商错误归一（quota_exhausted/rate_limited/authentication/network，含中文配额词识别） |
| `kill_child_tree` | Windows `taskkill /T /F`；Unix SIGTERM→SIGKILL 整个进程组 |
| `resolve_node_binary` / `ensure_node_on_path`（node.rs） | env → PATH → nvm/fnm/volta/asdf/n 发现 Node 并注入子进程 PATH |

### 4.4 wire 协议（agent_wire.rs）

- `AgentChunk`：13 个变体（UserMessage/Text/Reasoning/ContextCompaction/ToolCall/ToolResult/DshCommand/CodexCommand/Error/StreamStart/StreamEnd/Usage/SessionResolved），每个都带 `thread_id`；**wire 用 snake_case**（不经 Tauri 自动 camelCase 转换），与前端 `types/agent.ts` 1:1 镜像。
- `StreamEnd` 经 `emit_stream_end_once`（AtomicBool CAS）保证只发一次——stop/流尾/看门狗三方竞争的唯一收口。
- `RuntimePathConfig.permission_mode`：read-only / workspace-write / danger-full-access / inherit，下发给各 CLI。
- `RunInfo`：活跃 run 元信息（区分本地 `pending_thread_id` 与厂商 `session_id`）。

### 4.5 生命周期配套

- **cwd 冻结**（`shared/lifecycle.rs::resolve_and_freeze_runtime_cwd`）：首条消息解析 cwd 后落库 `agent_conversation_instances.frozen_cwd`，后续 turn 不漂移；无 cwd 响亮失败。
- **看门狗**（`app/watchdog.rs`）：30 分钟无 stdout 事件的 run 发 Error+StreamEnd 并杀进程组。
- **归档/删除**（`agent_lifecycle.rs`）：`AgentLifecycleService` 按 runtime adapter 分发（无 adapter 的 runtime 退化为 Flowix 本地语义）。

## 5. DSH 运行时分发（dsh.rs）

DSH 独立于 Tauri 应用更新，安装流水线 `install_runtime_inner`（dsh.rs:486-583）：

```
manifest 校验（schema/product/protocol/min_flowix_version）
→ 断点续传下载（RANGE + .part，限 512 MiB）
→ SHA-256 + Minisign 双验签（update_security.rs，与 App updater 同一把公钥）
→ staging 解压（限 2 GiB/10 万条目，safe_archive_path 防穿越）
→ 元数据校验（dsh-runtime.json）
→ 健康检查（真实 spawn 一次 app-server：initialize→thread/start→thread/close→shutdown，30s 超时）
→ before_publish（停掉所有 dsh-host）→ 原子发布（旧版本 rename 备份，失败回滚）→ 清理
```

- 布局：`<app data>/flowix/dsh/{versions/<ver>, current.json, downloads/}`；node-bundle 型启动 = `<node> <entrypoint>`。
- dev 模式：`FLOWIX_DSH_DEV_ROOT` 指向仓库 `.build/dsh-runtime-dev/`。
- `run_profile_plugin` 把 profile 插件管理委托给内嵌官方 `dsh` CLI（白名单校验，`dsh-flowix-memory` 禁删）。
- 事件：`dsh-download-progress`；全局操作锁防并发安装/卸载。

## 6. 文档读写与文件监听

### 6.1 写路径纪律

- `document_io.rs`：所有文档命令的阻塞 IO 走 `Semaphore(4)` + `spawn_blocking`，不持应用锁跨 await。
- `document_mutation.rs::DocumentMutationCoordinator`：应用写入与 watcher 稳定观察在事件发布前统一 `commit()`，以 `(notebook_id, relative_path)` 为键做 CAS 内容修订，产出 `DocumentCommit{content_hash, revision, change_id}`。
- `memo_events.rs`：统一事件总线 `memo-event`（Created/Updated/Deleted/TagsRenamed/TagsDeleted），`MemoChangeSource` 7 种来源（cloud_sync 源不回推，防同步环）。

### 6.2 watcher/ 五段式

```
manager（notify 生命周期）→ event（跨平台抽象）→ filter（whitelist/自写抑制/debounce）
→ processor（索引分流）→ runtime（Tauri state 窄接口）
```

- `MemoWatcher`（manager.rs）：notify 共享回调线程只做廉价过滤+入队 → mpsc → **单 worker 线程串行处理**（保 FIFO；`catch_unwind` 隔离 panic）。
- `RenameTracker` 用文件系统身份（dev,ino）关联分裂的 rename 事件；`RemoveCoalescer` 450ms 延迟墓碑。
- processor：Markdown 路径 → `refresh_note_path` → emit `flowix:path-note-changed` → `record_v2_local_change` 触发云同步。
- 自写抑制：`write_note_atomic`（watcher/runtime.rs:44-53）写前后内容哈希双标记。

## 7. 插件与模板系统

- **插件**（`plugin.rs` + `plugin/`）：声明式（详见 [08-插件运行时与安装助手](08-插件运行时与安装助手.md)）。桌面侧负责：安装（staging→校验→备份→原子换名，禁降级）、`plugin_prepare_prompt`（SKILL 指令 + 输出契约 + 上下文拼装 → 交给所选 Agent 执行）、`plugin_run`（`PluginRunCoordinator` 状态机）、产物解析写入（`plugin/artifact.rs` 四种 parser）。
- **模板**（`template_store.rs` + `build.rs`）：编译期把 `.flowix/templates/notebook-templates/` 全部文件 `include_bytes!` 内嵌；启动时种子化到 `~/.flowix/templates/`（`write_if_missing` 不覆盖用户定制），并处理旧版 8 模板集合与旧目录迁移。

## 8. 自动更新与 CLI 链接

- **App 更新**（`app_update.rs`）：端点 `https://download.flowix.cc/updater/{platform}/latest.json`，Minisign 验签；`AppUpdateState` 相位机防并发/取消竞态；Windows 静默安装结果经 `%LOCALAPPDATA%\FlowixData\app-update-result.txt` 回收校验。
- **CLI 链接**（`cli_link.rs`）：Unix 上 `~/.local/bin/flowix` symlink 指向 sidecar；Windows 写 `.cmd` shim + `HKCU\Environment` PATH 并广播 `WM_SETTINGCHANGE`。

## 9. 与其他 crate 的调用关系

| 被调方 | 用途 |
| --- | --- |
| `flowix-core` | `MemoFile`（注册表/索引/锁/原子写）、`NoteService`/`MemoService`、`SecretStore`（约 49 处 import） |
| `flowix-sync` | `SyncManager`（bootstrap.rs:274-293 构造，后台 `run_session_service`）、v2 变更记录、Apple 登录类型 |
| `flowix-plugin-runtime` | 内置插件 manifest/SKILL 常量、产物文档序列化、ID 校验 |
| `flowix-installer-helper` | 仅 Windows：`listen_for_shutdown` 监听安装器退出请求 |

## 10. 关键文件速查

| 符号 | 位置 |
| --- | --- |
| `AppState` | [state.rs](../../app/flowix-desktop/src/app/state.rs) |
| `run()` / `handle_run_event()` / `run_startup_reconciliation()` | [bootstrap.rs](../../app/flowix-desktop/src/app/bootstrap.rs) |
| `ExternalRuntimeRegistry` / `dispatch_chat()` | [runtime_registry.rs](../../app/flowix-desktop/src/agent_external/runtime_registry.rs) |
| `AgentChunk` / `RunInfo` / `RuntimePathConfig` | [agent_wire.rs](../../app/flowix-desktop/src/agent_wire.rs) |
| `StreamingEmitBuffer` / `kill_child_tree` | [process_io.rs](../../app/flowix-desktop/src/agent_external/shared/process_io.rs) |
| `install_runtime_inner()` / `health_check()` | [dsh.rs](../../app/flowix-desktop/src/dsh.rs) |
| `MemoWatcher` / `PathNoteEventProcessor` | [manager.rs](../../app/flowix-desktop/src/watcher/manager.rs) / [processor.rs](../../app/flowix-desktop/src/watcher/processor.rs) |
| `DocumentMutationCoordinator` | [document_mutation.rs](../../app/flowix-desktop/src/document_mutation.rs) |
| `MemoEvent` / `MemoChangeSource` | [memo_events.rs](../../app/flowix-desktop/src/memo_events.rs) |
| `install_app_update()` / `verify_signature()` | [app_update.rs](../../app/flowix-desktop/src/app_update.rs) / [update_security.rs](../../app/flowix-desktop/src/update_security.rs) |
| `PluginDescriptor` / `write_output()` | [plugin.rs](../../app/flowix-desktop/src/plugin.rs) |
| `ensure_cli_symlink()` | [cli_link.rs](../../app/flowix-desktop/src/cli_link.rs) |
