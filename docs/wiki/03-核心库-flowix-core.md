# 03 核心库 flowix-core

> 路径：`app/flowix-core/` · 定位："Flowix business core: notebook / memo storage, search, threads. **Zero Tauri dependency**."
> 桌面端、CLI、MCP Server 共享同一份存储实现与锁协议，这是本项目最重要的 crate。

## 1. 模块结构

| 源文件 | 职责 |
| --- | --- |
| `src/lib.rs` | crate 入口，公开 `memo_file` / `secret` / `service` / `text_merge` / `collection` |
| `src/service.rs` | 应用服务边界：`NoteService`（路径键）与 `MemoService`（Memo-ID 兼容）、`FlowixError`、分页游标 |
| `src/collection.rs` | 集合（多维表格/媒体库）YAML 信封 schema v1/v2：身份校验、归一化、路径 rebase |
| `src/text_merge.rs` | 保守 Markdown 三方合并（diffy 行级 patch，冲突绝不写入原文件） |
| `src/secret/mod.rs` | 凭据存储适配层：`~/.flowix/default.db` 的 `secret_kv` 表 |
| `src/memo_file/mod.rs` | 存储门面 `MemoFile` 定义与模块组织 |
| `src/memo_file/notebook_registry.rs` | 设备级注册表 `~/.flowix/index.db`：建表/迁移/进程内缓存 |
| `src/memo_file/notebook.rs` | 注册表读写、`<nb>/.flowix/notebook.json` manifest、选中笔记本状态 |
| `src/memo_file/types.rs` | 公开 DTO：`Memo` / `NotebookConfig` / `NotebookManifest` / `NotebookSetupJob` 等 |
| `src/memo_file/note_index.rs` | **路径键 Note 投影**（`notes`/`note_tags`/FTS5），可重建索引的 CRUD/分页/搜索/对账 |
| `src/memo_file/index_store/` | 旧 memo-ID 族表（memos/memo_tags/memo_todos/...）与内容 revision CAS |
| `src/memo_file/ops/` | 旧 memo CRUD、磁盘双向对账（reconcile）、标签批量移动/删除、文件名冲突解析 |
| `src/memo_file/file_io.rs` | 原子写/noclobber rename（Linux renameat2 / macOS renamex_np / Win MoveFileExW）、文件身份 (dev,ino) |
| `src/memo_file/frontmatter.rs` | YAML frontmatter 解析与就地编辑（`flowix_key` 保留键、tags 行、BOM 规范化） |
| `src/memo_file/derivation.rs` | 从 Markdown 派生 title/preview/thumbnail/tags/todos/agent threads |
| `src/memo_file/content.rs` | memo 内容读路径（全量/过滤/跨笔记本查找） |
| `src/memo_file/operation_locks.rs` | **跨进程操作锁体系**（详见第 4 节） |
| `src/memo_file/file_management.rs` | `FileManagementPolicy`：忽略规则（`.flowix`、`attachments`、`node_modules` 等） |
| `src/memo_file/media_resource.rs` | 笔记本媒体资源索引（指纹/属性/分页） |
| `src/memo_file/versions.rs` | 路径版本历史：`.flowix/archives/<id>/manifest.json`（自动版间隔 1h、上限 20） |
| `src/memo_file/migration/` | 数据迁移框架与 v001/v002 步骤 |
| `src/memo_file/onboarding.rs` | 空笔记本欢迎文档（`include_str!` 内嵌） |

## 2. 存储门面 MemoFile

`src/memo_file/mod.rs` —— 所有笔记操作的统一入口（桌面/CLI/MCP 都经它）：

```rust
pub struct MemoFile {
    registry: notebook_registry::NotebookRegistry,
    current_notebook_id: Option<String>,
    current_index_io: std::sync::Mutex<()>,   // 进程内 RMW 串行化
    index_cache: std::sync::RwLock<Option<MemoIndexFile>>,
}
```

核心方法（节选）：

| 方法 | 作用 |
| --- | --- |
| `MemoFile::new(config_dir: PathBuf)` | 以 `~/.flowix`（或覆盖值）构造 |
| `operation_locks() -> OperationLocks` | 获取锁体系句柄 |
| `with_file_write(notebook_id, path, intent, operation, write)` | 持文件写锁执行闭包（写盘统一入口） |
| `with_notebook_change(notebook_ids, operation, change)` | 持笔记本结构锁执行闭包 |
| `acquire_cross_process_write_lock()` | 旧版 `.memo-write.lock` 独占闸门（兼容旧进程） |
| `read/write_selected_notebook_id()` | 选中笔记本状态（`app_state` 表，跨进程共享） |
| `read/write_notebook_configs()` | 注册表读写（`index.db` 的 `notebooks` 表） |
| `ensure_notebook_manifest(config)` | 保证 `<nb>/.flowix/notebook.json` 存在 |
| `notebook_db_path(notebook_id)` | 笔记本 SQLite 路径（`.flowix/notebook.db`） |
| `get_default_notebook_path()` | `~/Documents/flowix` |

## 3. 服务层（service.rs）

服务层把底层存储封装为"应用用例"，是 Desktop commands 与 CLI dispatch 的直接依赖。

### 3.1 错误模型

```rust
pub enum FlowixError { InvalidInput, NotFound, Conflict, PermissionDenied,
                       Io(/*..*/), CorruptData(/*..*/), Internal }
```

### 3.2 NoteService（新路径键 API）

| 方法 | 说明 |
| --- | --- |
| `list(notebook)` / `list_page(notebook, filter, sort, tag_id, color, cursor, limit)` | 列表与游标分页（过滤/排序/标签/颜色） |
| `search_with_tag_filter(notebook, query, tag_filter, limit)` | FTS5 全文搜索（标签路径过滤含子标签） |
| `create(notebook, parent_relative_path, title, content)` | 建笔记（文件名清洗 + 冲突解析） |
| `get / save / rename / move_to / delete / delete_checked` | `save` 带 `expected_content` 乐观并发；`delete_checked` 冲突时报 `Conflict` |

```rust
pub enum NoteSaveOutcome { Saved(NoteDocument), Conflict { disk_content: String } }
```

### 3.3 MemoService（Memo-ID 兼容 API）

| 方法 | 说明 |
| --- | --- |
| `resolve_memo(id_or_filename)` | 解析顺序：绝对路径 → 各笔记本 relative_path 匹配 → memo ID → filename 兜底 |
| `edit_memo_exact(id_or_filename, old, new, dry_run)` | 精确单点替换（old 必须恰好 1 处匹配，dry_run 只预览） |
| `save_memo_with_receipt(...)` | 写后提交 content revision（SHA-256 hash + change_id + revision） |
| `create_memo_named_with_tag_in_directory(...)` | 指定目录/标题/标签创建（供 CLI `create` 与 MCP） |

## 4. 跨进程锁体系（operation_locks.rs）

桌面端、CLI、MCP、外部工具会并发触碰同一批文件。锁设计：

| 锁 | 锁文件 | 粒度 | 语义 |
| --- | --- | --- | --- |
| 文件写锁 | `~/.flowix/locks/files/<sha256>.lock` | 单文件（canonicalize 路径哈希） | 独占；同文件互斥、不同文件并行 |
| 笔记本结构锁 | `~/.flowix/locks/notebooks/<id>.lock` | 笔记本 | 共享=内容写；独占=结构变更（建/删/移动/标签批量） |
| 维护锁 | `~/.flowix/locks/maintenance/notebooks/<id>.lock` | 笔记本 | 全量对账/迁移（含非阻塞 try 变体） |
| 版本 manifest 锁 | — | 单笔记版本清单 | 写 `archives` manifest 时 |
| 旧版闸门 | `~/.flowix/.memo-write.lock` | 全局 | 旧进程独占；新操作仅共享持有 |

- 关键函数：`OperationLocks::file_write(notebook_id, notebook_root, path, intent, operation)`、`notebook_change(notebook_ids, operation)`（排序去重后逐个独占，防死锁）、`try_notebook_maintenance(...)`。
- `FileLockIntent::{Existing, Create, ExistingOrMissing}` 声明目标文件存在性预期。
- 超时 `LOCK_TIMEOUT = 5s`、`RETRY_INTERVAL = 50ms`；Windows 区分 `ERROR_LOCK_VIOLATION`。
- 集成测试：`app/flowix-cli/tests/locking.rs` 用 CLI 子进程验证真实跨进程行为。

## 5. 数据存储格式

### 5.1 设备级 `~/.flowix/`

| 路径 | 内容 |
| --- | --- |
| `index.db` | 表 `notebooks(id,name,icon,path UNIQUE,is_default,sort,...)`、`app_state(key,value)`、`notebook_setup_jobs` |
| `default.db` | `secret_kv(key,value)`：`SecretStore`，`secure_delete=ON`，unix 0600 |
| `plugin/state.json` | 插件启停 `{disabled: [...]}` |
| `templates/notebook-templates/` | 72 场景模板种子（由桌面端启动时写入） |

### 5.2 笔记本级 `<notebook>/.flowix/notebook.db`

- 新路径投影：`notes(relative_path PK, size_bytes, modified_ns, content_hash, title, preview, thumbnail, properties_json, created_at, updated_at, favorited, icon, colors_json, filename, filename_lower)`、`note_tags`、`note_todos`、`note_agents`、`note_content_revisions`、FTS5 虚表 `note_search_fts(..., tokenize='trigram')`。
- 版本常量：`SCHEMA_VERSION = 3`、`PARSER_VERSION = 9`（解析器升级触发增量重建）。
- 旧 memo 族表：`memos`、`memo_tags`、`notebook_tags`、`memo_colors`、`memo_todos`、`memo_agents`、`memo_lifecycles`、`memo_content_revisions`、`memo_index_state` 等。
- 集合投影：`collection_documents(collection_id, relative_path)`；媒体资源表 `media_resources`。

### 5.3 集合文件（collection.rs）

- 独立表格 `<名称>.table.yml`、媒体库 `*.lib.yml`、文件夹表格 `.index.yaml`。
- YAML 信封：`format: flowix.collection` + `schema_version: 1|2` + `collection` 元数据 + `payload`。
- 关键 API：`CollectionDocument::parse/validate/normalized_v2/revise/rebase_paths`；ID 规则 `col_/fld_/view_/rec_/opt_` + 32 位小写 hex（`valid_id()`）。
- v1 → v2：读取时内存升级，编辑保存才写回；目录移动后 `rebase_paths` 重写 records/source/filters 中的相对路径。

## 6. 关键纯函数

| 函数 | 位置 | 说明 |
| --- | --- | --- |
| `merge_markdown(base, local, remote) -> MergeOutcome` | `text_merge.rs` | 三方合并；`Merged/Conflict/Unsupported`，冲突标记永不落盘 |
| `reconcile_note_index(notebook_id)` | `note_index.rs` | 磁盘 ↔ 索引对账（可重建性的实现）；刷新失败留恢复标记 `.flowix/note-index-refresh-pending.d/` |
| `commit_note_content_revision_if_current(...)` | `index_store/` | 内容修订 CAS 提交（期望 hash 不匹配 → 冲突） |
| `atomic_write_bytes / atomic_create_bytes` | `file_io.rs` | 同目录临时文件 + fsync + rename；创建用内核级 no-clobber |
| `entry_name(provider, account)` | `secret/mod.rs` | 凭据键名 `provider::account` |

## 7. 测试

- 大量内嵌单元测试（`ops`/索引/并发 RMW，tests 代码约 2400 行）。
- `tests/notebook_creation_performance.rs`：revision CAS、笔记本隔离与创建基准。
- 集成冒烟由 `flowix-cli/tests/` 承担（见 [06-命令行-flowix-cli](06-命令行-flowix-cli.md)）。
