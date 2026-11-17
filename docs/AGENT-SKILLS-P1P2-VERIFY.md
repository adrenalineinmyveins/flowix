# Agent Skills P1+P2 实机验证步骤

> 版本：v1.0（2026-11-12）
> 验证目标：P1（skills 注入咨询提示词）+ P2（应用元数据回流 + skill-usage 归因）
> 环境：A 机（本机，被咨询方 `rop/tuning`）；验证以单机回环为主，双机变体可选
> 前提：P1+P2 改动在工作区未提交，需 dev app 编译生效

## 0. 前置检查（已完成项）

- ✅ mindmap 插件就绪：`~\.flowix\plugin\mindmap\SKILL.md`（649 字节，非空）+ `plugin.json`
- ✅ 中继可达、`rop/tuning` 信箱已注册（前期 T1-T5 已验证）

## 1. 启动含新代码的 dev app

```powershell
# ① 关闭生产版 Flowix（避免与 dev 版双实例双 SSE 订阅抢收件）
Get-Process Flowix,flowix-cli -ErrorAction SilentlyContinue | Stop-Process -Force

# ② 编译并启动 dev app（含 P1+P2 改动；窗口标题 Flowix Dev）
cd e:\flowix\flowix
npm run tauri:dev:win
```

首次增量编译约数分钟；窗口出现即就绪。**保持 dev app 开着**（auto-answer 依赖 SSE 订阅）。

## 2. 给 tuning 配置技能

编辑 `E:\笔记导出\weixinobU7VjgjFTTManiUT7MaGehxAQIY_2026-11-02_1793590102722\.agents\rop\tuning\profile.md`，frontmatter 增加：

```yaml
flowix_skills: ["mindmap"]
```

（已有该行则直接改值；保存即可，无需重启。）

## 3. 发起咨询（单机回环）

在 dev app 内任一 DSH 会话发送：

```
请用 consult_remote 工具咨询 rop/tuning@http://8.160.162.251:8788：
"技能注入验证：请简述你解答问题时可依据的方法论或工具"，waitSeconds 设为 120。
```

预期 30–120 秒收到回答（长答案可能超窗口，异步注入亦有效）。

## 4. 检查点（按序）

| # | 验证点 | 位置 / 命令 | 通过标准 | 对应 |
|---|---|---|---|---|
| A | 咨询方文本回流 | 发起会话的回答 | 回答尾部出现 `[已应用技能：mindmap]` | P1 生效 + P2 文本回流 |
| B | 建档固化 | 会话执行后看检查点 C/D 同目录 | 见下 | P2 |
| C | consult 留档 | `$root = "E:\笔记导出\weixinobU7VjgjFTTManiUT7MaGehxAQIY_2026-11-02_1793590102722"; Get-ChildItem "$root\.flowix\agents\rop\tuning\consults\cst_*.json" | Sort-Object LastWriteTime -Descending | Select-Object -First 1 \| Get-Content` | 最新 `cst_*.json` 含 `"appliedSkills":["mindmap"]` | P1 命中固化 |
| D | 事件回流 | 同目录最新 `*.events.jsonl` 尾部 | `result` 事件含 `"appliedSkills":["mindmap"]` | P2 |
| E | 用量归因 | `Get-Content ~\.flowix\plugin\skill-usage.jsonl -Tail 3` | 出现 `"skillId":"mindmap","source":"consult","agentId":"rop/tuning"` | P2 归因 |

检查点 E 的 `tokensUsed` 为该咨询 token 按命中技能数均摊值。

### 可选深挖：子代理提示词实证（检查点 F）

确认子代理实际收到 `<skills>` 块（区分「注入了」与「agent 用了」）：

```powershell
# 找最新子代理会话日志并解码（多帧 zstd）
$f = Get-ChildItem ~\.dsh\sessions -Recurse -Filter session.v4.jsonl.zstd |
  Sort-Object LastWriteTime -Descending | Select-Object -First 1
python -c "import compression.zstd as z,sys; sys.stdout.buffer.write(z.read(bytes(open(r'$($f.FullName)','rb').read().split(b'\x28\xb5\x2f\xfd')[-1])))" 2>$null
# 输出中搜 <skills> 块与 <skill id="mindmap"
```

（解码命令失败时可跳过——A/C 已足够证明注入命中。）

## 5. 反例（降级路径，建议补测）

把 profile 的 `flowix_skills` 改为 `["no-such-skill"]`，重复 §3：

- 回答尾部**无**标注行
- 最新 `cst_*.json` 的 `appliedSkills` 为空数组
- skill-usage 无新 consult 记录

（证明「配了但无资产 → 静默降级，不阻塞咨询」。）

## 6. 双机变体（可选）

B 机（beta/expert）对 `rop/tuning@http://8.160.162.251:8788` 发起同款咨询——B 机**无需更新代码**（发起侧无改动），回程注入与文本标注在 B 机会话可见；A/B/E 检查点仍在 A 机执行。

## 7. 已知边界（预期内，非失败）

- mindmap 属 artifact-tool 类技能：注入的是「如何生成思维导图」的工具用法，子代理在咨询会话内没有插件执行工具，回答只会**引用**该方法论而非产出导图——标注行/appliedSkills/skill-usage 三点不受影响
- `perDayLimit` 当日额度（现为 500）足够；若遇 `day_limit` 拒绝见 `auto-grant-state.json`
- 验证期间不要运行任何 setup.exe / 安装器（维护停机信号会终止 dev app）
