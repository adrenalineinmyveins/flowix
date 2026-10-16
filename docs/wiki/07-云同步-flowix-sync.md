# 07 云同步 flowix-sync

> 路径：`app/flowix-sync/` · "Flowix Cloud authentication and notebook synchronization engine"。
> 负责 Cloud HTTP 契约（注册/登录/OAuth/会员/结账）与 v2 同步协议；Access Token 只驻内存，Refresh Token 经 trait 由桌面层持久化。

- 默认 API 端点：`https://cloud.flowix.cc`（`lib.rs::DEFAULT_CLOUD_API_BASE`）。
- 协议 epoch：`PROTOCOL_EPOCH = 2`（服务端为 last-commit-wins 的 revision 链 + 全局 sync_seq 变更流）。
- 同步键：`notebook_id + relative_path`（路径即云端 ID），与本地 memo ID / frontmatter 无关。

## 1. 模块结构

| 源文件 | 职责 |
| --- | --- |
| `src/client.rs` | `CloudClient`：全部 Cloud HTTP 端点（auth/apple/google/refresh/me/entitlements/products/checkout + v2 sync status/changes/bootstrap/history/push/blobs） |
| `src/models.rs` | 账户/会话/会员 DTO；`RefreshTokenStore` trait |
| `src/manager.rs` | `SyncManager` 聚合根（client + store + auth 状态 + membership + 通知器），拆分 auth/catalog/lifecycle/state/v2_engine 子模块 |
| `src/manager/v2_engine/sync_pipeline.rs` | `sync_v2_account_once` 主编排 |
| `src/manager/v2_engine/freeze.rs` | dirty → inflight 冻结（含 blob 预约/上传） |
| `src/manager/v2_engine/push.rs` | inflight 分批 push / ACK / 冲突处理 |
| `src/manager/v2_engine/inbound.rs` | pull changes / bootstrap / blob 下载 |
| `src/store.rs` + `store/v2.rs` | `SyncStore`（独立 SQLite，WAL，busy_timeout 5s）与 v2 全部表 |
| `src/v2.rs` | v2 协议纯函数与类型（content hash / note_id / push 操作枚举 / 附件收集） |
| `src/error.rs` | `SyncError`（Http/Database/Api/NotAuthenticated/NotebookDisabled/InvalidState/RevisionConflict/MoveConflict） |
| `src/text_merge.rs` | 直接 re-export `flowix_core::text_merge`（云端与编辑器共享同一合并算法） |

## 2. 认证体系（manager/auth.rs + lifecycle.rs）

| 方法 | 说明 |
| --- | --- |
| `login(email, password)` / `register` | 邮箱密码 |
| `sign_in_with_apple` | 配合桌面端 `apple_sign_in.rs` 的原生质询（`AppleAuthChallenge` 类型） |
| `sign_in_with_google_desktop` | 配合 `flowix://auth/google/callback` 深链回调 |
| `logout` | 清除凭据与同步状态 |
| `run_session_service()` | 后台会话恢复：保存的 refresh token + 指数退避（2s→60s）；仅 `401 INVALID_REFRESH_TOKEN` 才清凭据 |

- **auth generation 机制**：每次登录递增 generation，防旧异步请求覆盖新登录状态。
- 会员/商品/结账：`cloud_refresh_membership` / `cloud_list_products` / `cloud_create_checkout`（manager/catalog.rs）。

## 3. v2 同步协议

### 3.1 单轮账户同步 sync_v2_account_once

```
1. 确定启用集合（v2_synced_notebooks.enabled，可限定单本）
2. push_v2_pending_moves        # note.move 操作补推
3. reconcile_v2_snapshot        # 磁盘内容 hash vs 已确认基线 → 标 dirty
4. freeze（冻结）：
   dirty（跳过同 generation 已 inflight）→ 生成 op_<uuidv7> + base_revision
   note.put → v2_reserve_blob → v2_upload_blob（PUT 直传 S3 或代理；直传不带 Bearer，
              header 白名单 content-type/x-amz-checksum-sha256/x-amz-meta-*）
   操作 JSON 原子写入 v2_inflight_operations（generation 变化则放弃）
5. push：到期操作按 100/批 POST /v2/sync/push
   成功 → acknowledge_v2_operation（服务端 revision/hash/syncSeq + ACK 原子写基线；记 self_sync_seqs 防回声）
   409 REVISION_CONFLICT → 立即上抛（不先拉云端 head——防下次扫描把本地字节当新编辑静默覆盖对端）
   其他失败 → defer（attempts/next_retry_at 指数重试）
6. pull：
   cursor 过期（410 CURSOR_EXPIRED）或新启用笔记本 → bootstrap 分页快照（limit=10，逐 blob 下载校验）
   常规 → GET /v2/sync/changes?cursor&limit=100（校验 cursor 前进；同实体按 sync_seq 去重；
          跳过 self_sync_seqs 回声；为 note 变更下载并 SHA-256 验证正文与附件）
   → V2RemoteApply 列表交桌面层落盘，complete_v2_sync_with_apply 提交 cursor
```

### 3.2 冲突处理

- push 被拒（旧 revision）→ 桌面端用 `merge_markdown`（diffy 三方合并）合并本地与云端 head；
- 无法合并 → 未提交内容记入本地版本历史（`.flowix/archives`），采用云端 head；
- 素材 API：`v2_conflict_material(...)`、`v2_rebase_after_conflict(...)`、`discard_v2_move_after_conflict(...)`（move 冲突）。

### 3.3 协议类型（v2.rs）

| 类型/函数 | 说明 |
| --- | --- |
| `v2_content_hash(bytes) -> String` | SHA-256 → base64url 无填充 |
| `v2_path_note_id(notebook_id, relative_path) -> String` | `hash(nb_id \0 rel_path)`：路径即云端 ID |
| `v2_local_content_diverged(local, acknowledged, has_pending)` | 本地是否有未上传编辑（dirty generation 权威，磁盘 hash 弥合 watcher settle 窗口） |
| `V2PushOperation` | `NotebookPut/NotebookDelete/NotePut/NoteDelete/NoteMove`（均带 operation_id + base_revision） |
| `collect_v2_attachments(dir, markdown)` | 识别 `asset://localhost/`、相对 `attachments/`、Obsidian `![[...]]` 嵌入 |

## 4. 同步状态库（SyncStore）

独立 SQLite（桌面层指定路径 `sync.db`），核心表：

| 表 | 内容 |
| --- | --- |
| `v2_cloud_account` | 单例 + `protocol_epoch=2` 约束 |
| `v2_sync_state` / `v2_notebook_sync_state` | 全局 / 每笔记本 cursor |
| `v2_synced_notebooks` | enabled / bootstrap_required |
| `v2_note_states` / `v2_notebook_states` | revision / content_hash / last_seq 基线 |
| `v2_dirty_entities` | generation + fingerprint |
| `v2_inflight_operations` | UNIQUE(entity,id,generation) |
| `v2_retry_state` / `v2_pending_moves` / `session_restore_policy` | 重试 / 移动 / 会话恢复 |

换账号时整批清空（`store/v2.rs`）。

## 5. 关键 API 速查

| 符号 | 位置 |
| --- | --- |
| `SyncManager`（login/sync_v2_account/v2_conflict_material/record_v2_local_change/run_session_service） | [manager.rs](../../app/flowix-sync/src/manager.rs) 及子模块 |
| `CloudClient`（v2_push/v2_changes/v2_bootstrap_page/v2_history/v2_reserve_blob/v2_upload_blob/v2_download_blob） | [client.rs](../../app/flowix-sync/src/client.rs) |
| `SyncStore`（freeze_v2_operation/acknowledge_v2_operation/defer_v2_operation/v2_dirty_entities/v2_inflight_due） | [store/v2.rs](../../app/flowix-sync/src/store/v2.rs) |
| 协议纯函数 | [v2.rs](../../app/flowix-sync/src/v2.rs) |
| 边界约定 README | [README.md](../../app/flowix-sync/README.md) |

## 6. 桌面侧集成

- `flowix-desktop/src/app/bootstrap.rs:274-293` 构造 `SyncManager` 并后台跑 `run_session_service`；
- watcher 与 cloud 命令调用 `record_v2_local_change` / `record_v2_local_move` 喂 dirty 队列；
- 冲突 UI：`cloud_list_conflicts` / `cloud_resolve_markdown_conflict` / `cloud_resolve_attachment_conflict`；
- 验收状态见 [docs/CLOUD_SYNC_ACCEPTANCE.md](../CLOUD_SYNC_ACCEPTANCE.md)（"原生双实例发布门禁" 3 项 Pending）。
