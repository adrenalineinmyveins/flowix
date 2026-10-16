# 06 命令行 flowix-cli

> 路径：`app/flowix-cli/` · 非交互 CLI + MCP stdio Server，"共享同一份 memo_file 存储，跟桌面端 watcher 双向同步"。
> 桌面端把它作为 Tauri sidecar（`binaries/flowix-cli-<target>`）打包，并 symlink 到 `~/.local/bin/flowix`。

## 1. 模块结构

| 源文件 | 职责 |
| --- | --- |
| `src/main.rs` | 二进制入口：Windows UTF-8 console codepage、`--json` 错误输出、退出码 |
| `src/cli.rs` | clap 命令树（`enum Cli`）与 argv 解析、`cli_command()`（含 shell 补全生成） |
| `src/dispatch.rs` | `run_cli` 命令调度 |
| `src/store.rs` | `MemoFile` 打开、笔记本解析/选中态、计数 |
| `src/path_store.rs` | 路径寻址操作（list/show/create/edit/write/delete/search）与地址解析 |
| `src/operation.rs` | 传输中立的 `FlowixOperation` 枚举 + `execute()`（**CLI 与 MCP 共享**） |
| `src/mcp.rs` | MCP stdio JSON-RPC server（单工具 `memo`） |
| `src/collection_store.rs` | 只读集合数据集访问（collectionId 投影查询 → 全盘扫描兜底） |
| `src/plugin.rs` | 插件产物工具 CLI 适配（list/describe/create，stdin 为内容源） |
| `src/errors.rs` | `CliError`（退出码 + 稳定机器错误码） |
| `src/paths.rs` | `FLOWIX_HOME` / `FLOWIX_DATA` 覆盖与默认 `~/.flowix` 解析 |
| `src/fmt.rs` / `output.rs` | 人读/JSON 输出助手 |
| `tests/` | `cli_smoke.rs`（冒烟）、`locking.rs`（跨进程锁）、`mcp_e2e.rs`（MCP 子进程端到端） |

## 2. 子命令清单

全局选项 `-j/--json`。地址格式：`<笔记本ID>::<笔记本内相对路径>` 或笔记绝对路径。

| 命令 | 说明 |
| --- | --- |
| `flowix notebooks` | 列出笔记本（JSON 含 selected） |
| `flowix list [notebook]` | 列笔记（先 `reconcile_note_index_blocking` 对账） |
| `flowix tags [notebook]` | 列标签 |
| `flowix show <地址>` | 读笔记（address/body/元数据） |
| `flowix create [notebook] --file\|--stdin` | 建笔记（`--file` 为推荐的 UTF-8 传内容方式，防 PowerShell 5.1 编码损坏） |
| `flowix delete <地址>` | 删除（checked：磁盘已变则 Conflict，退出码 4） |
| `flowix search <query> [-b nb] [-t tag-path] [-l N]` | FTS5 全文搜索（跨笔记本或单本） |
| `flowix edit <地址> --old <text> (--new \| --new-stdin \| --new-file) [--dry-run]` | 精确单点替换（必须恰好 1 处匹配） |
| `flowix write <地址> --file\|--stdin` | 覆盖整篇（expected_content 乐观并发） |
| `flowix collection read --notebook <nb> (--id <col_id> \| --path <rel>)` | 读表格/媒体库数据集；ID 冲突报 `COLLECTION_IDENTITY_CONFLICT` |
| `flowix plugin list / describe <id> / create <id> [-b nb] [--source-note addr]` | 内置 + 第三方 artifact 工具（内容走 stdin） |
| `flowix completion <bash\|zsh\|fish>` | shell 补全 |
| `flowix mcp` | 启动 MCP stdio server |

### 退出码与错误契约

| 退出码 | 含义 |
| --- | --- |
| 0 | 成功 |
| 1 | 其他错误 |
| 2 | 用法错误 |
| 3 | 未找到 |
| 4 | 冲突 |
| 5 | IO 错误 |

`--json` 时错误统一输出 `{ok:false,error:{code,message}}`（稳定机器码，如 `NOT_FOUND` / `CONFLICT` / `COLLECTION_IDENTITY_CONFLICT`）。

### 地址解析（path_store.rs::address）

`notebook::relative/path` → 绝对路径（canonicalize 后找最深包含它的笔记本）→ 裸相对路径（当前选中/默认笔记本）；拒绝 `..` 与空段。

## 3. MCP Server（mcp.rs）

- **传输**：stdio、行分隔 JSON-RPC 2.0；方法 `initialize` / `ping` / `tools/list` / `tools/call`。
- **协议版本**：支持 `2024-11-05` … `2025-11-25`；serverInfo `flowix-memo`。
- **唯一工具 `memo`**（annotations：非只读、可破坏、非幂等），入参两种互斥形态：

```jsonc
// 1) 结构化 action（推荐，走 FlowixOperation）
{ "action": "search", "query": "部署", "limit": 10 }
{ "action": "edit", "address": "nb::doc.md", "old": "A", "content": "B", "dryRun": false }
{ "action": "artifact.create", "pluginId": "mindmap", "content": "# Root\n## B1" }

// 2) legacy command（受限 CLI 语法字符串）
{ "command": "list my-notebook", "stdin": null }
```

结构化 action 全集：`notebooks / list / tags / show / search / create / edit / write / delete / collection.read / artifact.list / artifact.describe / artifact.create`。

- **legacy command 安全约束**：`shell_words` 切词；拒绝 shell 元字符 `| ; && > < \` $( ${`；拒绝前导 `flowix`；拒绝 help/version/mcp/completion；**MCP 禁止 `--file`/`--new-file`**（不能读 MCP 客户端本地路径），内容只能走 `stdin` 参数。
- 工具 schema 编译期内嵌自 `dsh-flowix-memory/memo-tool-schema.json`（单一契约源）。
- 响应：`content:[{type:'text'}] + structuredContent + isError`。

## 4. 关键类型

```rust
// cli.rs
pub enum Cli { Version, Notebooks, List, Tags, Show, Create, Delete, CollectionRead,
               Search, Edit, Write, PluginList, PluginDescribe, PluginCreate,
               Completion, Mcp }

// operation.rs — CLI 与 MCP 的共享执行层
pub enum FlowixOperation { /* 与上表命令一一对应的中立表示 */ }
pub(crate) fn execute(operation: FlowixOperation) -> Result<Value, CliError>

// errors.rs
pub enum CliError { Usage, NotFound, Io, Conflict, Other }
impl CliError { pub fn exit_code(&self) -> i32; pub fn code(&self) -> &'static str }
```

## 5. 测试

| 文件 | 覆盖 |
| --- | --- |
| `tests/cli_smoke.rs` | help/version/plugin describe/退出码/JSON 错误形状/Windows stdin 限制 |
| `tests/locking.rs` | 真实 CLI 子进程并行写不同文件、结构锁等待、锁持有者异常退出 |
| `tests/mcp_e2e.rs` | MCP 子进程端到端：initialize/tools/list/tools/call（结构化 + command 双路径） |

## 6. 典型调用方

1. 桌面端 sidecar（`cli_link.rs` symlink）；
2. `dsh-flowix-memory` launcher（spawn `flowix mcp` 做全双工 stdio 代理）；
3. 用户在任意终端 / CI / 其他 AI 工具中直接调用。
