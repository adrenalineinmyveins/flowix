# Agent Skills 调用操作说明

> 版本：v1.0（2026-11-12，对应 AGENT-SKILLS P1–P3 已实现并实机验证的能力）
> 读者：agent owner（配置技能的一方）与咨询方（远程调用 agent 的一方）
> 关联文档：[AGENT-SKILLS-EVALUATION.md](AGENT-SKILLS-EVALUATION.md)（设计评估）、[AGENT-SKILLS-P1P2-VERIFY.md](AGENT-SKILLS-P1P2-VERIFY.md)（验证步骤）

## 1. 能力概述

agent owner 在身份卡 profile 中声明 `flowix_skills` 后，其他用户（或本机会话）远程咨询该 agent 时，技能自动生效，分三层：

| 层 | 能力 | 实现机制 |
|---|---|---|
| 知识注入 | 子代理按技能正文的方法论组织回答 | 技能正文（SKILL.md）注入咨询提示词 `<skills>` 块 |
| 元数据回流 | 咨询方在回答尾部看到 `[已应用技能：…]`；owner 侧留档与统计 | 回答文本标注 + consult 留档 + `skill-usage.jsonl` 归因 |
| 工具执行 | 工具型技能真实产出产物（如思维导图） | 提示词桥接说明：子代理经 memo 工具 `artifact.create` 等价执行（无 shell，不能跑 CLI 命令） |

## 2. Owner 配置步骤

### 2.1 技能资产就绪

技能 = 已安装且**启用**的插件，且 `SKILL.md` 正文非空。检查：

```powershell
Get-ChildItem ~\.flowix\plugin -Directory | Select-Object Name
Get-Content ~\.flowix\plugin\mindmap\SKILL.md -TotalCount 3   # 应有正文
```

停用（`enabled: false`）或正文为空的插件即使声明也不会注入（静默忽略）。

### 2.2 profile 声明

编辑 `<笔记本>\.agents\<user>\<agent>\profile.md` 的 frontmatter：

```yaml
flowix_skills: ["mindmap", "webpage"]   # 插件 id，多个空格分隔数组
```

保存即生效（下次咨询时解析），无需重启应用。限额沿用 profile 校验（条数/单项长度）。

### 2.3 就绪自检

| 检查项 | 通过标准 |
|---|---|
| 插件启用 | 应用内插件页对应插件已启用 |
| profile 解析 | 应用内 agent 详情正常显示（frontmatter 语法无误） |
| 信箱 | 该 agent 收件通道已注册（否则咨询投不进来） |
| auto-answer | `~\.flowix\redaction\auto-grant.json` 的 `autoAnswer: "R3"` 且日额度未满 |

## 3. 咨询方调用

对目标 agent 发起咨询（会话内 consult_remote，或对方地址直投）：

```
请用 consult_remote 工具咨询 rop/tuning@http://8.160.162.251:8788：
"<问题>"，waitSeconds 设为 120。
```

**技能如何被调用**：

- **注入恒自动**：只要 owner 配置了 `flowix_skills`，任何咨询都会自动注入技能正文——咨询方**无需知道或提及**技能名
- **产物主动产出**：提示词已内置主动引导——回答适合以技能产物形式呈现时（如结构梳理适合思维导图），子代理会主动调用工具产出产物，无需咨询方明说
- **想让产物可复现时仍建议明说**：如「请实际执行你的 mindmap 技能，生成一张关于 X 的思维导图，完成后告诉我标题与保存位置」——消除模型判断的不确定性
- **指令明确、信息自足**——歧义问题会触发反问协议（`waitingAskback`），auto-answer 无法自动答复，该轮咨询不出答案

**识别技能是否生效**：回答尾部出现 `[已应用技能：mindmap]` 标注行 = 该技能已注入并应用；无标注 = 未配置或未命中（正常降级，不影响回答）。

## 4. 观测与审计（owner 侧）

| 位置 | 内容 |
|---|---|
| `<笔记本>\.flowix\agents\<agent>\consults\cst_*.json` | `appliedSkills` 建档固化（注入命中的事实源） |
| 同目录 `*.events.jsonl` | result 事件含 `applied_skills` |
| `~\.flowix\plugin\skill-usage.jsonl` | 每技能一条：`source: "consult"`、`agentId`、tokensUsed（按命中数均摊） |
| `<笔记本>\Mindmaps\` 等产物目录 | 工具型技能的真实产物（frontmatter 带 `flowix_plugin` 标识） |

## 5. 边界与已知事项

| 项 | 说明 |
|---|---|
| 子代理无 shell | 技能正文中的 CLI 命令不可执行；由桥接说明引导走 memo `artifact.create`（已自动处理，无需配置） |
| 注入限额 | 单技能正文截断 4000 字符、整块预算 12000 字符——超限截断，超出预算的技能不注入不回流 |
| 严格白名单未执法 | 子代理理论上可对任意 pluginId 调 `artifact.create`（后续收紧项） |
| 目录卡透出未上线 | 技能清单暂不随目录卡发布（需中继协议改版） |
| 匿名投递配额 | 无 token 直投中继 3/小时；带设备 token 30/小时 |
| 本地对话不注入 | 当前注入仅在咨询链路（本地 chat 派发不读 skills，收敛为同一注入模块列后续） |

## 6. 故障排查

| 症状 | 原因 | 处置 |
|---|---|---|
| 回答无 `[已应用技能]` 标注 | 插件未启用 / SKILL.md 空 / id 拼写不符 / profile 未保存 | 按 §2 自检；对照 `~\.flowix\plugin\` 目录名核对 id |
| 咨询一直无答案 | 触发反问（`waitingAskback`）/ 日额度满 / app 未运行 | 换明确措辞重发；看 `auto-grant-state.json`；保持 app 开启 |
| 子代理只讲不做产物 | 模型判断不倾向产物 / 插件无产物能力 | 措辞加「请实际执行…生成…」（提示词已主动引导，明说仅消除不确定性）；确认插件 kind 为 artifact-tool |
| 产物未落盘 | `artifact.create` 失败（笔记本解析/内容不符规范） | 看 cst events 与 app 日志；产物须符合技能规范（如 mindmap 恰好一个一级标题） |
| 回答只有一句「我先…」预告 | 预告式停轮（模型输出纯文本即被编排收割为最终回答） | 已由提示词输出纪律压制（2026-11-12 实测修复）；极偶发时重发一次即可 |
| `appliedSkills` 为空但配了 | 命中失败被静默忽略（设计行为） | 同第一条排查 |

## 7. 端到端示例（实测记录，2026-11-12）

配置 `rop/tuning` 的 `flowix_skills: ["mindmap"]` 后：

1. 咨询「请实际执行你的 mindmap 技能：生成一张关于调试排障通用流程的思维导图」
2. 子代理自动：按 SKILL.md 规范备文 → 调 `memo artifact.create`（pluginId=mindmap）→ 回读校验
3. 产物：`Mindmaps/20261112-174859-调试排障通用流程-4585c6f3.md`（frontmatter `flowix_plugin: {"id":"mindmap","version":"0.2.0"}`）
4. 回答含产物标题/位置 + 尾部 `[已应用技能：mindmap]`
5. `skill-usage.jsonl`：`{"skillId":"mindmap","source":"consult","agentId":"rop/tuning","tokensUsed":87786}`
