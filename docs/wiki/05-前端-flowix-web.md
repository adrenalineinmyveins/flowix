# 05 前端 flowix-web

> 路径：`app/flowix-web/` · React 19 + TypeScript + Vite 7 + Zustand 5 + Tailwind/shadcn，Tauri v2 的 Webview 前端。
> 构建根配置在仓库根（`vite.config.ts` / `tsconfig.json` / `tailwind.config.js`），产物输出 `.build/web-dist`。

## 1. 目录结构

```
app/flowix-web/
├── index.html                 # 入口 HTML（先加载 /theme-boot.js 防主题闪烁，再挂 #app-loading spinner）
├── entrypoints/
│   ├── main.ts                # 仅 import "@flowix-target-entry"（构建期别名 → desktop.tsx）
│   └── desktop.tsx            # 设置 dataset.platform/os、initTauriClient()、渲染 <App/>
├── app/                       # ★ 协调层组合根（只做编排，不做业务真源）
│   ├── app.tsx                # hash 路由分流：主窗口 vs #preferences[/tab]
│   ├── main-window-effects.tsx / agent-window-effects.tsx
│   ├── main-window-startup.ts / main-window-memo-event-handler.ts
│   ├── user-config-sync.ts / use-agent-access-window-sync.ts / after-first-paint.ts
│   └── main-window/{main-window,use-main-window-business-controller,use-main-window-system-controller}.tsx
├── features/                  # ★ 17 个业务模块（memo/document/agent/workspace/preferences/plugin/...）
│   └── <feature>/public/      #   对外只暴露 public/ API（CI 强制）
├── lib/                       # 基础层：memo-dispatcher / event-dispatcher / agent-types / i18n / shortcuts / theme
├── shared/                    # UI 原子组件与通用 hooks
├── platform/                  # ★ Tauri 适配层：client/（域客户端）、event-bus、core
├── types/                     # 中性类型层：agent.ts / memo.ts / note-item.ts / memo-item.ts
├── styles/                    # index.css 汇总 + theme/{light,dark,rock,ember}.css + 编辑器全家桶
├── mock/                      # actionable-notice mock 页
└── public/theme-boot.js       # CSS paint 前写 data-theme（因 CSP 'self' 不能 inline）
```

路径别名（`tsconfig.json` / `vite.config.ts` 一致）：`@` → flowix-web 根、`@app`、`@features`、`@platform`、`@shared`、`@flowix-target-entry` → `entrypoints/desktop.tsx`。

## 2. 入口链与窗口模型

```
index.html → theme-boot.js（防闪白）→ entrypoints/main.ts（别名转发）
→ desktop.tsx（platform 标记 + initTauriClient → window.__tauriRpc）
→ app.tsx：
   hash 无 #preferences → 懒加载 <MainWindow/> + MainWindowEffects + AgentWindowEffects → windows.showMain()
   hash #preferences[/tab] → 只挂 <PreferencesView/>（不挂两套 effects）
```

- **多 Webview = 同一 SPA 的多个实例**：主窗口与偏好窗口各持独立 JS context，因此每个窗口都要独立同步 user-config / agent-access。
- `AgentWindowEffects` 是"所有内容型 Webview 共享的 Agent 基础设施"（事件订阅 + memo 事件桥），不是独立窗口专属。

### app/ 协调层职责表

| 文件 | 职责 |
| --- | --- |
| `app.tsx` | Provider 栈（ErrorBoundary/I18n/Theme/Tooltip/Shortcuts/NativeEditMenuBridge）；macOS WebKit 方向键 U+001D 拦截 |
| `main-window-effects.tsx` | 主窗口副作用：memo 事件注册、启动时序、窗口标题、`flowix:path-note-changed` 路径移动应用 |
| `main-window-startup.ts` | `initializeMainWindowStartup()`：boot 门控 → 笔记本身份 → 六类 workspace target 恢复（external/media/collection/table/document-list/agent-conversation）→ 首帧确认 |
| `main-window-memo-event-handler.ts` | 单条 `MemoEvent` → 意图级 action 纯函数（tags 事件局部 patch 防闪烁；external_tool 新建自动开列） |
| `user-config-sync.ts` | 后端配置变更 → 本窗口 store 失效（property_presets/preference/dsh_config/ai_config） |
| `use-agent-access-window-sync.ts` | 每窗口 `useAgentAccessStore` 重载 |
| `after-first-paint.ts` | rAF + requestIdleCallback（1.5s 兜底）调度非关键任务 |
| `use-main-window-business-controller` | 业务组合根：笔记本增删、插件打开协调、文件夹集合类型切换 |
| `use-main-window-system-controller` | 系统组合根：应用更新、DSH 下载进度、Onboarding |

## 3. 状态管理与 IPC 模式

### 3.1 Zustand per-feature store

- store 分散在 `features/<x>/store/` 与 `lib/store/`；`create` + `persist`，localStorage 键集中在 `lib/constants.ts` 的 `STORAGE_KEYS`。
- 状态所有权契约（`docs/FRONTEND-MODULE-BOUNDARIES.md`）：每类状态唯一 owner；跨 feature 只走对方 `public/` API，公开 API 只暴露业务意图与稳定快照，禁止泄漏整个可变 store。
- 导航事务五步：begin → document flush 旧会话 → 拥有方 feature 加载 → 仍当前才 commit target → 失败回滚。

### 3.2 Tauri invoke 三条路径（platform/）

1. **域客户端（主流）**：`platform/tauri/client.ts` 聚合 `client/` 下按域拆分模块（agent/cloud/desktop/plugin/general/properties/notes/memos/updater/recovery），方法直调 `invoke<T>('snake_case_command', { camelCaseArgs })`。
2. **Hook**：`use-tauri-rpc.ts` 返回 `{ request(method, params) }`。
3. **原语重导出**：`core.ts` 重导出 `invoke` / `convertFileSrc`（附件上传/asset URL 等少数点）。
- 变异类 IPC 统一走 `document-mutation.ts::invokeDocumentMutation`（带 `expectedContent` 乐观锁，返回 saved/conflict/missing/error 判别结果）。

### 3.3 事件总线（platform/tauri/event-bus.ts)

- 同名 Tauri 事件只挂 1 个底层 listener，`Map<event, Set<handler>>` 共享；注册失败指数退避（1s→30s）。
- 已收口通道：`memo-event`、`agent-chunk`、`user-config-changed`、`agent-access-changed`、`dsh-download-progress`、`flowix:path-note-changed`、`flowix:open-target` 等。

## 4. 事件分发与去重（lib/）

### 4.1 EventDispatcher（lib/event-dispatcher.ts）

通用事件分发器类：`subscribe(handler, filter?)` 声明式订阅 + `dispatch()` 同步派发（handler 异常隔离）+ `use(middleware)` koa 式洋葱中间件。与 event-bus 职责正交：event-bus 是 Tauri listen 适配层，本模块是应用语义层。

### 4.2 memoDispatcher（lib/memo-dispatcher.ts + memo-dispatcher-dedup.ts）

- 模块级单例 `memoDispatcher`；`acquireMemoEventBridge()` 引用计数式订阅 `memo-event`（StrictMode/HMR 安全）。
- **50ms dedup 中间件**：按 `(notebookId, relativePath)` 键合并 `updated` 事件（last-write-wins，`derivedChanged` OR 合并）；`created/deleted` 立即冲掉 pending；`revision` 单调性丢弃乱序（LRU 1 万条）。
- 动机：FSEvents 双触发 + 编辑器 debounce 保存期间避免 N 次 rerender/N 次 IPC。

### 4.3 其余 lib 模块

| 模块 | 职责 |
| --- | --- |
| `agent-types.ts` | `AGENT_TYPES` 8 种 agent 目录（图标/名称/能力/releaseStatus）；能力查询函数 `supportsTextStreaming` 等 |
| `collection-display-registry.ts` / `file-display-registry.ts` | 集合/文件展示身份注册表（displayId 生命周期、pin 引用计数、reconciliation） |
| `export.ts` / `export-utils.ts` | Markdown→HTML→Word 兼容 .doc；文件名清洗（120 字符上限） |
| `constants.ts` | `STORAGE_KEYS`、`BINARY_EXTENSIONS`/`isTextFile` |
| `i18n/` | zh-CN / en-US 消息与检测 |
| `shortcuts/` | 快捷键体系（parser/matcher/registry/handler-registry/platform/provider） |
| `theme/` | ThemeId/palette/sanitize/apply；色板单一真源在 CSS |
| `tag-path.ts` | 标签路径语法（与 flowix-core `normalize_tag_path` 对齐） |

## 5. 类型契约层（types/）

> 与 Rust 侧的**硬镜像契约**——字段改名即破坏 IPC，必须双端同步。

| 文件 | 内容 |
| --- | --- |
| `memo.ts` | `MemoEvent` 判别联合（kind 必须 snake_case，镜像 `memo_events.rs`）、`MemoChangeSource`（7 来源）、`MemoContentCommit`（contentHash/revision/changeId）、`MemoDerivedChanged` |
| `note-item.ts` | 新一代 `NoteListItem`（`kind:'path-note'`，身份 `notebookId+relativePath`）、`noteListItemKey()` → `path:{notebookId}:{relativePath}`、`NoteColor` 7 色 |
| `memo-item.ts` | `MemoItem`（@deprecated legacy DTO）+ re-export note-item 保旧 import 路径 |
| `agent.ts` | `AgentChunk` wire 联合（snake_case，与 Rust `AgentChunk` 1:1）、`RuntimeConfig`/`RuntimeConfigPatch` 三态（undefined 跳过/null 清空/有值锁定，镜像 `threads.rs`）、`ChatMessage`、`AgentEvent`（camelCase 归一事件层）、`RunInfo` |

注意两套命名并存：wire（snake_case，serde 直出）与 IPC 返回值（Tauri 自动 camelCase），如 `started_at` vs `startedAt`。

## 6. 样式体系

- Tailwind color token 全部引用运行时 CSS 变量（`var(--background)` 等）；主题 = 切换 `html[data-theme]`。
- `styles/index.css` 按域 `@import`：fonts、editor 全家桶（含 bubble-menus/code-block/mermaid/link-tooltip/agent-thread-card...）、multidimensional-table、overlay-scrollbar、pdf-print、plugin-markmap。
- 平台差异：`documentElement.dataset.platform`（mac/non-mac）驱动选择器；存在成对组件（如 `memo-list-titlebar-mac/win.tsx`）。
- Vite 定制插件（根 `vite.config.ts`）：`modernFontWoff2Only`（字体 ~1MB→~250KB）、`removePackagedCrossorigin`（修 WKWebView asset 协议 module 加载）；`manualChunks` 锁 shiki-core/vendor-mermaid/vendor-katex 重型块。

## 7. 关键文件速查

| 符号 | 位置 |
| --- | --- |
| `App`（hash 分流根） | [app.tsx](../../app/flowix-web/app/app.tsx) |
| `initializeMainWindowStartup()` | [main-window-startup.ts](../../app/flowix-web/app/main-window-startup.ts) |
| `handleMainWindowMemoEvent()` | [main-window-memo-event-handler.ts](../../app/flowix-web/app/main-window-memo-event-handler.ts) |
| `EventDispatcher<T>` | [event-dispatcher.ts](../../app/flowix-web/lib/event-dispatcher.ts) |
| `memoDispatcher` / dedup | [memo-dispatcher.ts](../../app/flowix-web/lib/memo-dispatcher.ts) / [memo-dispatcher-dedup.ts](../../app/flowix-web/lib/memo-dispatcher-dedup.ts) |
| `subscribe()`（事件总线） | [event-bus.ts](../../app/flowix-web/platform/tauri/event-bus.ts) |
| `AGENT_TYPES` | [agent-types.ts](../../app/flowix-web/lib/agent-types.ts) |
| `noteListItemKey()` | [note-item.ts](../../app/flowix-web/types/note-item.ts) |
