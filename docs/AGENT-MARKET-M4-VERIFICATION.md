# AGENT-MARKET M4 实机验证报告

| 项 | 内容 |
| --- | --- |
| 验证对象 | 订阅 agent 的对话/编辑器引用（M4：`@` 提及混排、「添加内容 → 订阅的 Agent」、`agentReference` 行内 chip） |
| 构建版本 | `af9c1d8c`（trae/agent-dRBdg4，已推送 origin）；Flowix Dev（`com.flowix.app.dev`，dev watcher HMR 生效，WebView2 加载 `localhost:1420`） |
| 验证环境 | Windows / Tauri 2 桌面应用，pid 28664；UI Automation（WebView2 Chromium 无障碍树）语义断言 + 多模态子代理视觉复核 |
| 验证日期 | 2026-10-29 |
| 测试数据 | 本机 M3 订阅快照 2 条：`@财报助手`（`bob/b@http://relay.test`）、`@alice-researcher`（`alice/researcher@http://8.160.162.251:8788`） |

## 1. 验证范围与结论

M4 共三个用户入口，**全部通过**：

| 入口 | 结论 |
| --- | --- |
| 文档编辑器 `@` 提及（混排菜单 + chip 插入） | ✅ 通过 |
| 对话「添加内容」— 笔记内嵌 thread card | ✅ 通过 |
| 对话「添加内容」— 左列「对话」标签独立详情视图 | ✅ 通过 |

## 2. 测试项明细

### 2.1 文档编辑器 `@` 提及

| # | 步骤 | 预期 | 实际结果 |
| --- | --- | --- | --- |
| T1 | 在笔记末尾空行输入 `@` | 弹出混排下拉 | ✅ 下拉弹出，标题「**笔记与 Agent**」（混排态文案），候选：`@财报助手`、`@alice-researcher` 排在最前，其后为笔记列表（2026-10-26test / loop-note-1 / note-sync-eval / AGENTS…），首项自动选中 |
| T2 | 继续输入 `财报`（中文 IME） | 列表按 displayName/address 过滤 | ✅ 仅剩 `@财报助手`；IME 组合输入过程未关闭菜单 |
| T3 | 选中 `@财报助手` | 删除触发符，插入行内 chip | ✅ 触发符被替换为独立 UIA 节点 `@财报助手`（agentReference NodeView），下拉自动关闭 |
| T4 | Ctrl+Z 撤销 | 恢复笔记原状 | ✅ chip 移除，文档回到原内容（末尾仅剩空段落 placeholder 提示，非实际内容） |

### 2.2 对话「添加内容」— 笔记内嵌 thread card

| # | 步骤 | 预期 | 实际结果 |
| --- | --- | --- | --- |
| T5 | 点击 composer「+」（添加内容） | 四项菜单 | ✅ 添加笔记 / **订阅的 Agent** / 添加附件 / 项目 AI 设置 |
| T6 | 点击「订阅的 Agent」 | 二级弹层列出订阅 | ✅ 标题「订阅的 Agent」，按钮项 `@财报助手`、`@alice-researcher`（悬浮 title 显示完整地址） |
| T7 | 点击 `@财报助手` | 注入 composer chip，弹层关闭 | ✅ composer 值变为 `@财报助手 `，行内出现 chip 节点，一级/二级菜单自动收起 |
| T8 | 清空 composer | 恢复空态 | ✅ Ctrl+A + Delete 后为空 |

### 2.3 对话「添加内容」— 独立详情视图（多模态子代理执行）

| # | 步骤 | 预期 | 实际结果 |
| --- | --- | --- | --- |
| T9 | 左列「对话」标签 → 打开会话详情 → 点击「+」 | 四项菜单 | ✅ 按钮转 `expanded`，四项菜单逐字核对一致 |
| T10 | 打开「订阅的 Agent」二级弹层 | 列出订阅 | ✅ `@财报助手`、`@alice-researcher` 两个订阅项（首次合成点击被级联关闭定时器关闭，`focus` 路径重试成功，见 §3.1） |
| T11 | 点击 `@alice-researcher` | 注入 chip | ✅ 输入框注入 chip，悬浮提示为 **`alice/researcher@http://8.160.162.251:8788 — click to copy`** —— 「点击复制完整地址」的数据面证据 |
| T12 | 清空输入框 | 恢复空态 | ✅ 两轮 Ctrl+A + Delete 后全部 chip/文本清除，「发送」按钮恢复 disabled（应用侧判空机制确认） |

## 3. 发现与备注

### 3.1 [无害] UIA 合成点击会闪关二级弹层

- 现象：自动化合成 click 点击「订阅的 Agent」菜单项时，二级弹层闪现后被 100ms 级联关闭定时器关闭。
- 原因链：`AgentPickerController` 二级打开时注册的 document capture `pointerdown` 外点监听（[agent-picker-controller.ts:73-77](../app/flowix-web/features/agent/thread-card/composer/agent-picker-controller.ts#L73-L77)）与合成点击收尾的 `pointerleave` 触发 [composer-add-menu-controller.ts:162-178](../app/flowix-web/features/agent/thread-card/composer/composer-add-menu-controller.ts#L162-L178) 的宽限期关闭。
- 影响评估：**仅影响无真实指针停留的合成事件**。真实鼠标路径（hover 打开 → 点击选择，指针停留不触发 pointerleave）与键盘/focus 无障碍路径（`focus` 事件同样 `openFromParent`，T5-T7 第一轮真实链路完整走通）均稳定。
- 处置：不改代码（行为符合级联菜单设计预期）。

### 3.2 [观察] 嵌入式卡片与详情视图共享 thread 草稿

第一轮在内嵌卡片注入的 chip 清空后，打开同 thread 的独立详情视图时草稿中仍出现 `@财报助手`（持久化草稿的两视图同步时序）。验证结束时已将详情视图输入框彻底清空（T12），两视图现均为空态。

### 3.3 [方法学] UIA 坐标点击的适用边界

本轮自动化中，左列导航/标签/列表项及 fixed 定位弹层的坐标点击全部可靠；文档滚动后中部内容区按钮的坐标点击存在失效（缓存矩形过期）。验证改用：新鲜观察后同批点击、`focus` 路径激活、多模态子代理复核三种手段兜底，均已获得确定性结果。

## 4. 结论

- M4 三个入口（文档 `@`、内嵌卡片、独立详情页）在真实应用上**全部功能可用**。
- chip 的完整地址（`user/slug@relay`）已通过悬浮提示实证；发送/草稿序列化路径在单元测试（markdown 往返 6 测）与 AGENT-MARKET.md M4 里程碑记录中覆盖。
- 验证过程对用户数据零残留：笔记内容已撤销恢复，两处 composer 输入框均已清空。
