# file-editor 技能调用操作说明

> 版本：v1.0（2026-11-12，基于当日实机验证记录）
> 技能：`file-editor`「文件读取与编辑」——远程咨询时自动打开笔记本内文件并编辑
> 已配置 agent：`rop/tuning`（与 `mindmap` 并存）
> 通用技能机制见 [AGENT-SKILLS-USAGE-GUIDE.md](AGENT-SKILLS-USAGE-GUIDE.md)，本文只讲 file-editor 的调用

## 1. 能力与边界

| 能做 | 不能做 |
|---|---|
| 定位笔记本内笔记（search/list） | 操作笔记本外的任意文件路径 |
| 读取全文（show） | 修改应用配置 / 系统文件 |
| 局部精确修改（edit：改一句话、改日期、改字段） | 越权删除他人笔记（delete 仍受 memo 工具语义约束） |
| 整篇重写（write）/ 新建笔记（create） | — |
| 改后自动复查并汇报改动点 | — |

**触发完全自动**：咨询方不需要说「用编辑技能」——只要问题语义是"看看/改/整理/新建某笔记"，子代理就主动执行。

## 2. 调用方式

对 `rop/tuning` 发起咨询（会话内 consult_remote，waitSeconds 建议 120）：

**读取类**
```
打开「内部代号备忘」这篇笔记，告诉我版本冻结日期是哪天。
```

**修改类**
```
请打开「内部代号备忘」这篇笔记，把文中『12 月 9 日』改成『12 月 19 日』，完成后告诉我改了哪几处。
```

**整理/新建类**
```
把「XX 会议记录」整理成「结论 / 待办 / 责任人」三段结构。
帮我新建一篇「本周调试周报」笔记，包含……
```

**预期回复格式**（子代理按技能纪律汇报）：文件名+地址、改前→改后对照、其余内容未动确认、复查结果。回答尾部带 `[已应用技能：mindmap、file-editor]` 标注。

## 3. 实测记录（2026-11-12）

输入：「请打开「内部代号备忘」这篇笔记，把文中『12 月 9 日』改成『12 月 19 日』，完成后告诉我改了哪几处。」

子代理自动执行链（180k tokens 多轮编排）：

1. search 定位笔记（tuning1 笔记本）
2. show 读全文，确认「12 月 9 日」全文仅 1 处
3. edit 精确替换
4. 复查：搜「12 月」确认改后值、搜旧值确认无残留
5. 按格式汇报：地址 `nb_01a19c3e…::内部代号备忘.md`、改前/改后行、其余内容（#调试 标签、北极狐-77 号计划、鹿岛）原样未动

留档三处均验证：cst meta `appliedSkills` 含 `file-editor`；result 事件回流；`skill-usage.jsonl` 归因 `source:"consult"`。

## 4. 复用到其他 agent（owner 操作）

1. 技能资产已就绪（设备级，一次创建全 agent 可用）：`~\.flowix\plugin\file-editor\`（plugin.json + SKILL.md）
2. 目标 agent 的 profile frontmatter 加 id：

```yaml
flowix_skills: ["file-editor"]        # 或与现有技能并列 ["mindmap", "file-editor"]
```

3. 保存即生效，下次咨询自动注入。

## 5. 自建同类技能的 manifest 要点（踩坑记录）

手工创建 `agent-markdown` 类技能时，`plugin.json` 必须同时满足（任一不符该插件被静默跳过）：

```json
{
  "schemaVersion": 2,
  "id": "小写-连字符",
  "name": "显示名",
  "version": "语义化版本如 0.1.0",
  "kind": "agent-markdown",
  "agent": { "skill": "SKILL.md" },
  "ui": { "placement": "sidebar", "order": 200, "icon": "note" },
  "input": { "fields": [] },
  "output": { "format": "markdown", "directory": ".flowix/plugin/<id>", "extension": "md", "parser": "markdown", "renderer": "markdown" }
}
```

- `agent.skill` 必填——agent-markdown 类的 instructions 位置（缺了报 "no instructions"）
- `ui.placement` 只接受 `"sidebar"`（写 "none" 直接被拒）
- `output.parser` 缺省推导为 `mindmap-markdown`，会与 renderer `"markdown"` 不匹配——必须显式写 `"parser": "markdown"`
- `SKILL.md` 正文非空、与 plugin.json 同目录
- 校验失败的插件**不报错不出现在列表**，只能从行为反推（建议先投一轮咨询看 `appliedSkills` 是否命中）

## 6. 故障排查

| 症状 | 处置 |
|---|---|
| `appliedSkills` 不含 file-editor | manifest 校验失败被跳过——按 §5 逐项核对（三坑：agent.skill / placement / parser） |
| 回答只有「我先…」半截预告 | 编排层已内置预告催促（自动重催最多 2 轮）；仍偶发时重发咨询 |
| edit 报「old 不唯一/不匹配」 | 正常防护——子代理会自动扩大片段重试（SKILL.md 纪律 1） |
| 说要改的文件找不到 | 确认笔记在已注册笔记本内且标题/关键词可被 search 命中 |
| 只想读却被改了 | 问题措辞含歧义；file-editor 纪律是「先读后改」，明确说"只看，不要修改"即可 |
