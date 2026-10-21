# Flowix 安卓版 UI 设计方案

> 版本：v1.0（2026-10-19）｜参考基线：桌面端 1.5.6 设计体系（`app/flowix-web/lib/theme/` + `styles/theme/*.css`）｜现状基线：移动端 M1（`app/flowix-mobile/frontend/`）

## 1. 设计目标与原则

1. **同源桌面，一眼 Flowix**：色板、字体层级、圆角、动效曲线全部与桌面端共用同一套 token 语义（OKLCH 定义），用户在桌面与手机间切换无认知成本。
2. **近白中性面 + 单色锚**（桌面端既定哲学）：全部界面 chrome 保持中性近白/近黑，品牌色只出现在按钮、焦点环、链接、选中指示、标签五个焦点位置；层次靠 ΔL 微亮度差表达，不靠阴影堆叠。
3. **移动端做减法**：桌面四栏（导航抽屉/列表/工作列/浏览列）→ 手机单列栈式导航；砍掉的功能明确放弃，不塞入"移动妥协版"。
4. **云端优先**：移动端定位为"消费云端笔记本 + 随手编辑回推"（M1 既定），同步状态始终可感知、不打扰。

## 2. 设计令牌（Design Tokens）

### 2.1 主题策略

| 项 | 决策 |
|---|---|
| 默认 | **跟随系统**（Android 用户预期），`prefers-color-scheme` 调度 light/dark |
| 可选 | 浅色 / 深色 / 岩灰（桌面默认，品牌识别）/ 余烬，共 4 套 + 跟随系统，与桌面端选项完全一致 |
| 机制 | 与桌面同构：`<html data-theme>` + CSS 变量；桌面 4 个 `styles/theme/*.css` **直接拷贝复用**（token 命名完全一致），移动端只需补 `system` 调度逻辑 |
| 首帧 | localStorage `flowix-theme`（与桌面同 key）防深色闪白 |

### 2.2 色彩（核心 token 表，OKLCH 原值同桌面）

浅色（light，冷蓝灰 + 紫锚）与深色（dark，冷蓝灰暗面 + 陶土锚）为 P0；岩灰/余烬拷贝即得：

| Token | light | dark | 用途（移动端） |
|---|---|---|---|
| `--background` | `oklch(0.988 0.006 255)` | `oklch(0.173 0.009 265)` | 页面底 |
| `--card` | `#ffffff` | `oklch(0.240 0.013 265)` | 笔记卡片/输入框/工具栏 |
| `--foreground` | `oklch(0.300 0.009 255)` | `oklch(0.680 0.013 265)` | 主文字 |
| `--muted-foreground` | `oklch(0.620 0.009 255)` | `oklch(0.716 0.015 265 / 0.58)` | 摘要/时间戳 |
| `--primary` | `oklch(0.552 0.185 273)` | `oklch(0.550 0.120 22)` | 主按钮/焦点环 |
| `--primary-foreground` | `#fff` | `oklch(0.970 0.007 265)` | 主按钮文字 |
| `--brand` | `oklch(0.552 0.185 273)` | `oklch(0.550 0.120 22)` | 选中竖条/链接/进行中指示 |
| `--accent` | `oklch(0.950 0.006 255)` | `oklch(0.310 0.013 265)` | 按压/选中底 |
| `--border` | `oklch(0.946 0.006 255)` | `oklch(0.300 0.014 265)` | 卡片描边 |
| `--divider` | `oklch(0.932 0.006 255)` | `oklch(0.250 0.014 265)` | 分隔线 |
| `--destructive` | `oklch(0.620 0.165 22)` | 同 | 退出登录/删除 |
| `--success` | `oklch(0.740 0.115 145)` | 同 | 同步完成 |
| `--document-bg` | `#ffffff` | `oklch(0.160 0.013 265)` | 编辑器纸面 |

### 2.3 字体与字号（移动端在桌面基础上 +1~2px 补偿视距）

| 层级 | 桌面 | 移动 | 说明 |
|---|---|---|---|
| 字体族 | Inter → 系统中文栈 | **同栈**：`Inter, system-ui, -apple-system, 'PingFang SC', 'Microsoft YaHei'` | 打包 Inter woff2 拷贝复用 |
| 屏幕大标题 | — | 22px / 600 | 登录 logo、设置分组头 |
| 顶栏标题 | 14px | **17px / 600** | 笔记本名、屏幕名 |
| 卡片标题 | 14px/20px | **15px / 20px**，两行截断 | 笔记卡片 |
| 正文/编辑器 | 15px × 1.8 | **16px × 1.8** | 手机阅读基线 |
| 摘要 | — | 13px / 1.5，两行截断 | 卡片 preview |
| 元信息 | 12px | **12px**，`tabular-nums` | 时间戳/字数 |
| 辅助 | 11px | 11px | 版本号/状态条 |
| 编辑器标题 | h1 1.9rem… | h1 24px / h2 20px / h3 17px | 行高 1.3 |
| 代码 | 14px/1.8 | 13px / ui-monospace 栈 | |

### 2.4 形状 / 阴影 / 动效

| Token | 值（同桌面） | 移动端应用 |
|---|---|---|
| 圆角-卡片 | 12px | 笔记卡片、设置卡片 |
| 圆角-控件 | 10px | 按钮、输入框、底部工具栏容器 |
| 圆角-chip | 6px | 标签 chip |
| 圆角-弹层 | 16px（底部 sheet 上圆角，Material 3 惯例） | 笔记本切换 sheet、菜单 |
| 浮层阴影 | `0 4px 24px -3px rgb(0 0 0 / 24%)` | sheet/气泡菜单（唯一阴影） |
| 焦点环 | `0 0 0 2px var(--ring)` | 外接键盘场景 |
| 缓动 | `cubic-bezier(0.16, 1, 0.3, 1)`（expo-out） | 全部转场 |
| 时长 | sheet 进出 240ms；卡片按压 150ms；同步图标旋转 1s linear | |
| 按压反馈 | 卡片/按钮 `scale(0.98)` + `--accent` 底（替代桌面 hover） | 触控核心反馈 |

### 2.5 触控与安全区

- 最小触控目标 **44×44px**（图标按钮视觉 24px + 命中区撑满）。
- 安全区：顶栏 `padding-top: env(safe-area-inset-top)`；FAB/底部工具栏 `env(safe-area-inset-bottom)`；横屏补左右 inset。
- Android 12+ edge-to-edge：WebView 沉浸 + 顶栏底衬渐变 `linear-gradient(to bottom, var(--background) 60%, transparent)`（同桌面标题栏 overlay 手法）。

## 3. 信息架构与导航

### 3.1 屏幕地图

```
启动 Boot ──未登录──> 登录 Login ──┐
   │已恢复                          │
   ▼                                │
笔记列表 Notes（主屏）◄──────────────┘
   ├──> 编辑器 Editor（全屏，返回回列表）
   ├──> 搜索 Search（全屏覆盖层）
   └──> 设置 Settings（全屏，返回回列表）
```

- **无底部 Tab Bar**：功能面只有"笔记 + 设置"，单主屏 + 栈式导航，避免空 tab。
- **返回键契约**（Android 核心缺口，P0 修复）：系统返回 = 栈回退；列表屏再按返回 = `moveTaskToBack`（不杀进程，保同步轮询）。技术：`history.pushState` + `popstate` 拦截（WebView 内建，无需原生改动）。
- 转场：编辑器/设置从右侧滑入 240ms expo-out（模拟栈推进），返回反向；列表↔搜索淡入淡出。

### 3.2 主屏结构（对齐桌面四栏的单列投影）

| 桌面元素 | 移动端映射 |
|---|---|
| 导航抽屉（笔记本树/标签） | 顶栏笔记本下拉 sheet（M1 现状保留升级） |
| 列表列（卡片/文件夹/会话） | 主屏卡片列表（仅 cards 视图；会话/Agent 不下沉） |
| 工作列（文档） | 编辑器全屏 |
| 状态栏 26px | 顶栏内同步指示 + 列表顶浮动状态条 |
| 全局搜索命令面板 | 搜索屏（顶栏放大镜进入） |

## 4. 各屏详细设计

### 4.1 启动 Boot

- 居中：产品 Logo（`assets/product-logo.svg`，56×56，深色模式单色处理同桌面）+ "Flowix" 22px/600。
- 副文案 12px muted："正在恢复会话…"（省略号风格同桌面文案规范）。
- 背景 `--background`；禁止纯白/纯黑闪屏（首帧 token 已注入）。

### 4.2 登录 Login

```
┌──────────────────────────────┐
│         (安全区)              │
│                              │
│          [Logo 56px]         │
│           Flowix             │
│    云端笔记本 · 移动端          │
│                              │
│  ┌────────────────────────┐  │
│  │ 邮箱                   │  │  ← 输入框 h=48px，圆角10，--card 底 + --border 描边
│  └────────────────────────┘  │
│  ┌────────────────────────┐  │
│  │ 密码              (小眼睛)│  │  ← 密码可见性切换（P0 补齐）
│  └────────────────────────┘  │
│  ┌────────────────────────┐  │
│  │        登  录           │  │  ← 主按钮 h=48px，--primary 实底，busy 时"登录中…"
│  └────────────────────────┘  │
│      没有账号？注册一个         │  ← 13px，--primary 字色
│                              │
│  (错误条：--destructive 10%底) │
└──────────────────────────────┘
```

- 键盘弹出：表单区上移（`interactive-widget=resizes-content`），登录按钮不被遮挡。
- 错误文案规范化：后端 message 原样展示，映射表（`INVALID_CREDENTIALS → 邮箱或密码不正确`）。

### 4.3 笔记列表 Notes（主屏）

```
┌──────────────────────────────┐
│ ▤ 我的笔记本 ▾      ⟳   ⚙   │ ← 顶栏 h=52px：笔记本切换 / 同步 / 设置
├──────────────────────────────┤
│ ┌──────────────────────────┐ │
│ │ 🔍 搜索全部笔记…          │ │ ← 搜索入口行 h=44px（点击进搜索屏，非输入）
│ └──────────────────────────┘ │
│ ┌──────────────────────────┐ │
│ │▎周会纪要 10-18            │ │ ← 卡片：标题15px两行 / 时间戳12px tabular-nums
│ │▎讨论了移动端排期与…        │ │   摘要13px两行 muted
│ │▎#工作  #排期       3 分钟前│ │   标签chip 6px圆角中性 + 底部右侧相对时间
│ └──────────────────────────┘ │
│ ┌──────────────────────────┐ │
│ │▎读书笔记 · 设计的心理…     │ │ ← 长按卡片 = 选中态（左侧4px --brand 竖条
│ │▎第三章讲认知负载…          │ │   + --accent 底，桌面同款视觉签名）
│ └──────────────────────────┘ │
│           (空态：暂无笔记…)   │
│                          ⊕  │ ← FAB 56px 圆形 --primary，右下 16px+安全区
├──────────────────────────────┤
│  ⬆ 已同步 · 3 分钟前          │ ← 状态条 h=28px 11px，同步中转 spinner，
└──────────────────────────────┘    有错误显 --destructive 字色，点击展开详情
```

规格与交互：

- **笔记本切换 sheet**：点顶栏笔记本名 → 底部 sheet（上圆角 16px + 浮层阴影，240ms expo-out 上滑）。列表项 = 笔记本图标（复用桌面 `notebook-icons/` 70+ 彩色图标）+ 名称 + 云同步小圆点（绿=已同步/灰=本地）。底部"刷新云端列表"文字按钮。
- **同步指示**：顶栏 ⟳ 图标（lucide `refresh-cw`）；同步中旋转 + `--brand` 色；完成静止；失败红点角标。点击 = 立即 `cloud_sync_now`。
- **新建**：FAB 点击 → 底部居中弹出**新建输入条**（标题输入 + 确认），替代 M1 常驻底栏（省一屏高度）；Enter 快捷确认。
- **下拉刷新**：Material 风格圆环 spinner + 触发 `cloud_sync_now`（`overscroll-behavior: contain`）。
- **长按菜单**（P1，依赖后端 `note_delete`）：删除（destructive 10% 底）＋ 复制标题。
- **列表项按需增强**（P1）：置顶图钉、待办图标（数据源 note_list 需补字段）。

### 4.4 编辑器 Editor

```
┌──────────────────────────────┐
│ ←   周会纪要.md        ✓已存  │ ← 顶栏 h=52px：返回/文件名12px/保存状态
├──────────────────────────────┤
│                              │
│   正文 16px × 1.8             │ ← 纸面 --document-bg，左右 padding 20px，
│   h1 24 / h2 20 / h3 17      │   阅读宽度 min(100%, 720px) 居中
│   blockquote 左 3px --brand   │
│   代码块 --editor-block-bg    │
│                              │
├──────────────────────────────┤
│ B  I  S  H  H₂O ·  ⊕  ⌘     │ ← 格式工具栏 h=48px + 安全区：
│                              │   粗体/斜体/删除线/标题/高亮·列表/任务/
└──────────────────────────────┘   插入图片(P2)/更多（撤销重做·查找）
```

关键决策：

- **底部格式工具栏**（移动端与桌面的最大分野）：桌面靠选区气泡 + 斜杠菜单（鼠标精确），手机上改为常驻底部工具栏，键盘弹出时悬浮于键盘上方（`visualViewport` 跟随）。工具项对 Tiptap `Editor.commands` 直调。
- **选区气泡菜单保留但降级为辅助**：选中文本时气泡仅含 B/I/S/高亮 四项（26px 高，桌面同款 7px 圆角样式），完整格式走底栏。
- **保存模型**：P0 保持手动保存但升级为**自动保存**——停止输入 800ms 防抖 `getMarkdown()` → `note_save`（后端已有 3s 同步防抖，两级防抖不冲突）；顶栏状态 `已保存`/`保存中…`/`未保存 ●`；返回时若有未保存先落盘再退。
- **frontmatter**：沿用 M1 拆装方案，编辑器只装正文。
- **Markdown 快捷语法**（StarterKit 输入规则）保留，与工具栏并存。
- 图片/附件/表格/Mermaid（P2）：依赖后端 blob 上传 command，UI 预留 ⊕ 入口位。

### 4.5 搜索 Search（P0 从列表内嵌升级为全屏）

```
┌──────────────────────────────┐
│ ←  ┌──────────────────────┐  │
│    │ 🔍 搜索全部笔记       │  │ ← 自动聚焦输入框，实时防抖 300ms
│    └──────────────────────┘  │
├──────────────────────────────┤
│ 周会纪要                      │ ← 结果卡片：标题 + 命中摘要（关键词
│ …讨论了「排期」的后续…        │   --brand 高亮）+ 所属笔记本 12px
│ 我的笔记本                    │
├──────────────────────────────┤
│ 读书笔记                      │
│ …「排期」相关章节…            │
│ 我的笔记本                    │
└──────────────────────────────┘
```

- 空态：'未找到笔记'（桌面文案规范）；历史/热门搜索 P2。
- 命中项点击 → 打开编辑器并滚动到命中段（P1，需 note_read 后定位）。

### 4.6 设置 Settings

| 分组 | 内容 |
|---|---|
| 账号 | 头像占位圆 + 昵称 + 邮箱；`退出登录` destructive 按钮（确认 sheet：'退出后本地缓存保留，重新登录即可恢复'） |
| 同步 | 已缓存笔记本 N 本；`立即同步` 按钮；上次同步时间 |
| 外观（新增） | 主题五选：跟随系统/浅色/深色/岩灰/余烬，选中项 `--brand` 竖条（卡片选中视觉签名复用）；正文大小：小/标准/大（15/16/18px，写 `--app-font-size`） |
| 关于 | Flowix Mobile vX.Y.Z；开源地址；`检查更新`（P2） |

## 5. 组件规范

| 组件 | 规格 | 状态 |
|---|---|---|
| 笔记卡片 | 12px 圆角 `--card` 底 1px `--border` 描边，padding 12px；按压 `scale(0.98)`+`--accent`；选中（长按）左 4px `--brand` 竖条 | 默认/按压/选中/同步中(角标) |
| 主按钮 | h=48px 圆角 10px `--primary` 实底白字 15px/600；busy 禁用+spinner；active 下沉 1px | 同桌面 button default |
| 次按钮 | ghost：`--card` 底 + `--border`，字 `--foreground` | 刷新云端列表等 |
| 输入框 | h=48px 圆角 10px `--card` 底 `--border` 描边，聚焦描边 `--primary`（无阴影光晕，移动端减噪） | 默认/聚焦/错误 |
| 标签 chip | 6px 圆角、1px `--border`、px-6px、12px muted 字、`#` 前缀图标；**刻意中性不染品牌色**（桌面原则） | — |
| 底部 Sheet | 上圆角 16px、浮层阴影、`--popover` 底、把手条 36×4px `--divider` | 进出 240ms expo-out |
| Toast | 底部居中胶囊，深底浅字（`--inverse-*`），8px 圆角，2.5s 自动消失 | 成功/错误 |
| 空态 | 图标（lucide 线性 1.5px，40px muted）+ 一句文案：'暂无笔记' '未找到笔记' '暂无已同步笔记本' | 桌面"暂无XX"句式 |
| 图标 | **lucide-react**（根依赖已有，与桌面同源）：`refresh-cw / settings / search / plus / chevron-down / arrow-left / check / eye / pin / trash-2` | 24px 触控区 44px |

## 6. 桌面端 → 移动端映射总表

| 桌面 | 移动端 | 说明 |
|---|---|---|
| 5 主题 OKLCH 色板 | **直接复用**（拷贝 theme css） | system 调度逻辑新写 |
| memo-card（竖条选中/两行摘要/chip/时间戳） | **继承全部视觉签名** | hover→按压，单击选中→长按选中 |
| 4px 品牌竖条选中态 | 列表长按选中、设置主题选中 | 核心识别符 |
| 浮层阴影 24% | sheet/气泡菜单 | 唯一阴影 |
| expo-out 动效曲线 | 全部转场 | 同值 |
| 选区气泡 + 斜杠菜单 | 底部格式工具栏为主 + 精简气泡 | 触控适配 |
| 状态栏 26px | 顶栏同步图标 + 底部状态条 | 信息不减，占位更小 |
| 笔记本 70+ 彩色图标 | sheet 列表复用 | assets 拷贝 |
| Inter 字体 + 中文栈 | 同源打包 | woff2 复用 |
| 导航抽屉/浏览列/AI 会话/DSH/插件 | **不下沉** | 明确放弃，保持移动纯粹 |
| 标签树/拖拽排序 | 仅展示 chip（P1 后端补标签数据） | — |

## 7. 实施路线

### P0 · 视觉与基建重构（纯前端，无后端依赖）
1. 引入 token 层：`styles/theme/*.css` 拷贝 + `mobile-tokens.css`（触控/间距/字号补偿）+ `data-theme` 调度 + localStorage。
2. 图标库接入 lucide-react，替换全部 unicode 图标。
3. 四屏按本方案重构（列表卡片化、FAB、sheet、状态条、登录视觉、设置分组 + 外观组）。
4. 返回键栈处理（history/popstate）+ 转场动效。
5. 编辑器：底部格式工具栏 + 800ms 自动保存 + 保存状态指示。
6. 搜索升级全屏 + 防抖 + 关键词高亮。

### P1 · 体验增强（需少量后端 command）
- `note_delete`（长按删除）、note_list 补时间戳/标签/置顶字段（卡片信息补全）、主题偏好持久化（settings command 或本地存储）。
- 下拉刷新、同步错误详情 sheet、冲突提示（监听 `cache-notes-changed` 附冲突文件名）。

### P2 · 能力扩展（依赖云协议/新 command）
- 图片/附件（blob 上传 command + 编辑器图片扩展）、表格/高亮/任务列表扩展、字数统计、检查更新、历史版本。

## 8. 技术落地要点

- **样式基建**：保持单仓无独立 package.json 现状；新建 `frontend/src/styles/` 目录分层（`tokens.css` / `theme/*.css` / `base.css` / `screens.css` / `editor.css`），vite 配置无需改（CSS import 即可）。不引 Tailwind（M1 单 CSS 文件迁移成本 > 收益，token 变量层已够）。
- **主题调度**：`apply.ts` 简化版（无 sanitize 全量校验）：读 localStorage → matchMedia 变化监听 → 写 `data-theme` + `color-scheme`。
- **键盘避让**：viewport meta 加 `interactive-widget=resizes-content`；编辑器工具栏用 `visualViewport.addEventListener('resize')` 跟随。
- **返回键**：每次进栈 `history.pushState({screen})`；`popstate` 消费栈；列表栈空时通过 `__TAURI__.core.invoke` 调新增的 `minimize_to_back` command（一行 Rust：`moveTaskToBack`）。
- **性能**：列表 > 200 条时上 `content-visibility: auto`（虚拟化 P2 再议）；Inter 字体 `font-display: swap`。

---

*本方案与桌面端 token 同名同值，实施时以 `app/flowix-web/styles/theme/*.css` 为单一真源拷贝，勿手抄色值。*

## 实施记录（P0，2026-10-19 完成并实测通过）

- 落地：`frontend/src/styles/`（theme×4 拷贝 + tokens/base/screens/editor）+ `theme.ts` 调度 + 首帧防闪脚本（index.html）+ `interactive-widget=resizes-content`
- 四屏重构 + 新增 SearchScreen 全部按方案实施；lucide-react 图标全面替换 unicode
- 实测（雷电模拟器，公网云账号）：卡片列表/相对时间/FAB/新建 sheet/笔记本 sheet（选中竖条）✓；编辑器 12 项底部工具栏 + 保存状态机 ✓；**800ms 自动保存实测落盘**（文件尾部出现输入内容）✓；**系统返回键编辑器→列表回退**（WryActivity 自带 canGoBack→goBack 与 popstate 栈配合；软键盘开启时首次 back 收起键盘属 Android 标准行为）✓；设置外观组主题五选/字号三选切换无崩溃 ✓
- 构建：`npm run build:mobile` 通过（CSS 23KB 含四主题，JS 651KB）；APK `gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk`
- 遗留：返回键在**列表屏**再按会走 Activity 默认行为（退出）——`minimize_to_back`（moveTaskToBack）留待 P1 原生补齐；下拉刷新（P1）；长按选中/删除（P1，依赖 `note_delete`）

## 实施记录（P1，2026-10-19 完成并实测通过）

- **note_delete**（`commands.rs` 新命令，note_save 同款两段式：排他锁删文件 → 锁外清索引 → `LocalChangeKind::Delete` 入脏队列 → 3s 防抖同步）+ UI 长按链路（`onContextMenu` 选中竖条 → 底部操作 sheet 删除/取消）
- **列表屏返回退后台**：`MainActivity.kt` 覆写 `onBackPressed` → `moveTaskToBack(true)`（子屏仍走 WryActivity 的 goBack 栈）
- **下拉刷新**：列表顶部 touch 手势（64px 阈值，指示器随手指）触发 `cloud_sync_now`
- 实测（模拟器 + 公网云）：长按 sheet 弹出 ✓；Loop 笔记本删除 loop-note-2 → 本地文件消失 + **云端 changes 出现 delete（syncSeq 25）** ✓；列表屏返回 → 退桌面但进程存活（同步轮询保留）✓
- 观察确认：跨账号缓存的笔记本（未启用同步的）删除为纯本地操作，不推云——账号切换边界设计内行为

## 实施记录（P2-A，2026-10-19 完成并实测通过）

- **编辑器扩展**：TaskList/TaskItem（nested）+ Placeholder（空文档占位"记录自己的想法"，桌面同文案）+ 工具栏新增任务列表按钮（14 按钮）
- **字数统计**：顶栏保存状态旁 "N 字"（`getText().length`，tabular-nums）
- **明确不做**：高亮（`====`）与表格——官方 `@tiptap/markdown` 不支持其序列化，加了会造成保存丢格式；需自定义 serializer 后再开放（根 node_modules 已有对应扩展包）
- 实测：loop-note-1 显示"136 字" ✓；工具栏插入任务列表 → 自动保存 → 文件出现 `- [ ] task-item-via-mobile`（Markdown 序列化往返正确）✓

## 实施记录（P2 收尾，2026-10-19 完成并实测通过）

- **图片附件**：工具栏 ImagePlus 按钮 → `<input type="file">`（wry 已实现 Android 文件选择器）→ `attachment_save` 落盘 `attachments/` → 自定义 `MobileImage` 扩展（官方 `@tiptap/markdown` node 级 tokenizer/parse/render 三件套）插入图片节点 → NodeView 经 `attachment_read` 转 dataURL 渲染；markdown 序列化为桌面同款 `![alt](asset://localhost/<encoded 绝对路径>)`，跨设备按文件名回落。实测：选图 → 渲染 64×64 → 自动保存 → `attachments/18.png` 408B + 笔记引用双实体上云 → 重启后往返仍渲染 ✓
- **历史版本**：编辑器顶栏 Clock3 → 底部 sheet（时间/字节/最新标记）→ 只读预览 → 恢复此版本（复用自动保存链路推云）。实测 5 版本全链路追溯 + 恢复落盘 ✓
- **检查更新**：设置页"关于"卡（当前版本 + 检查更新 + 发现新版时"去下载"）；后端 `GET /v2/app/latest`（systemd env 配置，已部署）。实测"已是最新"/"发现新版本 1.6.0"两路径 ✓
- **附带修复**：自动保存卸载保护 effect 的 `saveState` deps bug（单次 setContent 的保存 timer 被 cleanup 清掉，恢复/插图路径永不落盘）——saveState 改走 ref
- P2 全部完成。后续增强项：桌面打开移动端图片的"按文件名回落渲染"（桌面跨设备绝对路径既有短板）、高亮/表格自定义 serializer

## 实施记录（跨端增强轮，2026-10-19 第三轮）

- **桌面图片按文件名回落（已实现，待桌面端实测）**：`view-image.ts` 图片 onerror 时按 storageKey 的 basename 调新命令 `resolve_attachment_filename`（media.rs，遍历所有笔记本 attachments/ 子树）→ 命中则 convertFileSrc 重载，只回落一次防循环。验证方式：桌面端打开云端的 `m1-10-merge.md`（含移动端插入的 `attachments/18.png` 引用）
- **高亮/表格（实测通过）**：`@tiptap/extension-highlight` 与 `@tiptap/extension-table` 官方包 **v3.23+ 已内建完整 markdown 序列化**（此前"官方不支持"结论过时）——移动端直接装配零自定义。工具栏新增高亮（Highlighter）与表格按钮（插入 2×2 / 光标在表格内时删除表格）；CSS：mark 用 brand 28% 透明底、表格横向滚动。实测：peer 写入 `==important highlight==` + GFM 表格 → APP 拉取渲染（mark + 3 行表格）→ 编辑保存 → **云端高亮与表格结构完整保留** ✓。边界：桌面 `flowix:text` 彩色高亮注释在移动端降级为默认高亮（注释丢失）；表格 import 注意 ESM 无 default export（`import { Table }`）
- **新发现 bug（高优待办）：logout/login 循环引发链式冲突副本风暴**。现象：app 内退出登录→重新登录后，所有"本地内容 ≠ 云端 head"的文件（含正常本地领先编辑）被批量 409 → base_content 缺失 → 全部存冲突副本 → 副本上云 → 下轮继续，副本嵌套翻倍增长。止血 = pm clear 清状态重登（干净会话无风暴）。疑点：logout 后 v2 note states 被清/重登 bootstrap 把 base 置空 → push base=null 撞已有实体 409（note 无 create-or-adopt 宽容）。需排查 store 的 logout 清理与 login 后 base 恢复路径
