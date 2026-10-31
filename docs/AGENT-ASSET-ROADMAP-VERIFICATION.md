# AGENT-ASSET-ROADMAP 实机验证报告（N1-N3）

| 项 | 内容 |
| --- | --- |
| 验证对象 | P1-C 会话 → 个人 Agent 入口、P1-D 订阅快照陈旧度提示、P2-E 个人 Agent 页已订阅分组（P0-A/P0-B 为文档与类型层，由 tsc/测试守护，不属实机范围） |
| 构建版本 | 工作区未提交改动（基线 `af9c1d8c` + N1-N3 增量）；Flowix Dev（`npm run tauri:dev:win`，pid 3136，vite HMR） |
| 验证环境 | Windows / Tauri 2 WebView2；方法 = UIA 语义树断言（主代理）+ 多模态子代理操作与截图存档 |
| 验证日期 | 2026-10-30 |

## 1. 验证矩阵

| 工作项 | 验证点 | 结果 |
| --- | --- | --- |
| **P2-E 已订阅分组** | 个人 Agent 页 ListPanel 顶部出现「我的订阅」分组，列出订阅卡 | ✅ `我的订阅` 标题 + `@财报助手 finance`、`@alice-researcher m5`（含 specialtyTags），BookmarkCheck 只读样式，本地卡（财报解读助手/读书助手）与订阅卡视觉分组 |
| **P1-C 保存按钮存在** | thread card 头部操作行四按钮：保存为个人 Agent / 删除 / 全屏 / 收起 | ✅ 折叠的 DeepSeek Harness 卡头部四按钮逐一确认（accessible name =「保存为个人 Agent」） |
| **P1-C mobile 隐藏（K2）** | mobile 类型卡不渲染该按钮 | ✅（推断）笔记内另一张展开的「test」卡仅有旧三按钮——该卡符合 mobile 云同步会话特征；与代码 `hidden = typeKey === "mobile"` 一致 |
| **P1-C 创建 + 跳转** | 点击 → toast + 跳转个人 Agent 页 + 新卡出现 | ✅ toast「已创建身份卡「中国最新税务政策」，可在「个人 Agent」页继续编辑」；tab 自动切到「个人 Agent」；新卡名称取自会话标题，类型行显示 DeepSeek Harness |
| **P1-C 清理** | 编辑页删除 + 确认对话框级联清理 | ✅ 确认框文案含级联清理说明；删除后列表恢复为 财报解读助手 / 读书助手 两卡 |
| **P1-D 市场页时间行** | 「我的订阅」每行显示「快照更新于 {time}」 | ✅ 财报助手行「快照更新于 691 个月前」（relay.test 测试数据 refreshedAt 极小，属预期）；alice-researcher 行「快照更新于 1 天前」 |
| **P1-D @ 下拉行内时间** | agent 行内 compact 相对时间 | ⚠️ 未实机覆盖（该入口需编辑文档输入 @，为避免污染笔记未执行）；渲染条件与市场页同一数据源 `refreshedAt`，由 agent-mention-data 单测覆盖 |

## 2. 发现与说明

| # | 发现 | 处置 |
| --- | --- | --- |
| F1 | 新建卡出现在列表**末尾**而非顶部 | 既有行为：`agent_profile_list` 按 `agent_id` 字典序排序（[agent_profile.rs:193](../app/flowix-desktop/src/commands/agent_profile.rs#L193)），随机 id 与插入时间无关。非本次引入，不改 |
| F2 | 「691 个月前」时间观感异常 | 测试数据（`relay.test` 快照 refreshedAt≈0）放大显示；真实订阅时间正常（1 天前）。formatTimeAgo 30 天以上按月计的既有口径 |
| F3 | 保存后 thread card 标题从占位符同步为新标题 | 会话标题派生的既有联动（`deriveThreadTitleFromPrompt`），符合预期 |
| F4 | toast 文案引号为「」而非『』 | 与 i18n 实际定义一致（zh 用「」），预期内 |
| F5 | 超过 7 天的陈旧色（橙红）UIA 树不携带颜色信息 | 逻辑由代码保证（`>7*24*3600` 判定 + CSS 类）；可人工查看存档截图核验 |

## 3. 验证方法备注

- **HMR 边界**：React 页面（市场页/个人 Agent 页）热替换即时生效；thread card 为 ProseMirror NodeView（命令式 DOM），**需重新打开文档才重建**——本次通过切换笔记触发重挂载后按钮出现。此为 dev 模式特性，生产构建无此问题。
- **截图存档**（子代理按时间序保存）：`c:\Users\86151\AppData\Local\Temp\trae\computer-use\20261030\screenshots\`，其中按钮四联（win-sdk-3898b614…）、toast+跳转（win-sdk-315a4e15… / 779f47f7…）、删除确认（win-sdk-2a0f084e…）、订阅时间行（win-sdk-4090cd24… / a5e3403c…）为关键证据。

## 4. 结论

- **P1-C / P2-E 实机通过**；**P1-D 市场页入口实机通过**，@ 下拉入口由单测覆盖（建议后续人工顺带目验）。
- 测试数据零残留：新建身份卡已通过 UI 级联删除，视图已恢复「访问空间」。
- 代码层验证（tsc 0 错误、前端定向+回归全绿、Rust agent_profile 11/11）见 [AGENT-ASSET-ROADMAP.md](./AGENT-ASSET-ROADMAP.md) 里程碑表。
