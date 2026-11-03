# 跨用户 Agent 咨询通信测试 SOP（B 机操作手册）

> 版本：v1.0（2026-11-02）
> 适用对象：B 机操作员（被咨询方，用户 `beta`）
> 前提：A 机（用户 `rop`）已就绪；中继 `http://8.160.162.251:8788` 已部署加固版（归属先到先得 + 命名空间注册约束）

## 0. 测试拓扑

```
A 机（rop，发起方）                    B 机（beta，被咨询方/本手册）
  rop/agt_a83d9cf0f4a530af  ⇄  中继  ⇄  beta/expert
                                    8.160.162.251:8788
```

测试目标：验证两台真实设备间 consult 全链（投递→取件→auto-answer 编排→回程→验签→反向）。

**注意：测试期间咨询目标一律使用 `beta/expert`，不要使用 `alice/researcher`（A 机代答身份，会污染跨设备语义）。**

---

## 1. 前置条件检查

| 项 | 检查方法 | 通过标准 |
|---|---|---|
| 开发环境 | `node -v`、`cargo -V` | Node ≥ 20，Rust stable |
| 中继可达 | 见 §2 网络自检 | HTTP 200 |
| 磁盘空间 | 剩余 ≥ 20 GB | 构建产物 + node_modules 需要 |

---

## 2. 阶段一：取代码并启动

```powershell
git clone -b trae/agent-dRBdg4 https://github.com/adrenalineinmyveins/flowix.git
cd flowix
npm install
npm run tauri:dev:win
```

- 首次全量构建约 10–15 分钟（Rust workspace + 前端）
- 分支必须是 `trae/agent-dRBdg4`——包含 consult 全链与 relay 加固全部修复；`main` 上没有
- 若报 `Port 1420 is already in use`：

```powershell
Get-NetTCPConnection -LocalPort 1420 -State Listen |
  ForEach-Object { Stop-Process -Id $_.OwningProcess -Force }
```

- 窗口标题应为 **Flowix Dev**（dev bundle），出现主界面即成功

## 3. 阶段二：网络自检

```powershell
curl.exe -s -o NUL -w "%{http_code}" --noproxy "*" http://8.160.162.251:8788/v1/health
```

- 期望输出 `200`
- **超时/失败** → 关闭系统代理（Clash/v2ray 等）后重试。中继走直连，代理会导致后续所有请求超时（A 机实测踩坑）

## 4. 阶段三：注册身份（Flowix 设置界面内）

按顺序完成四步：

1. **中继地址**：设置中填 `http://8.160.162.251:8788`
2. **注册中继账号**：用户名 **`beta`**（Ed25519 密钥本地生成，即账号身份；私钥只落本机 `account.json`）
3. **注册收件通道 agent**：**`expert`**（自动归一为 `beta/expert`）
4. **发布目录卡**：可见性 **public**（T3 验签的前提——A 机要拿你的目录卡公钥验回信签名）。**注意顺序**：发卡前该 agent 必须已注册收件通道（步骤 3），否则中继返回 `422 AGENT_NOT_REGISTERED`——目录卡强制要求「搜得到也投得到」

## 5. 阶段四：开启 auto-answer

编辑 `C:\Users\<用户名>\.flowix\redaction\auto-grant.json`：

```json
{
  "schemaVersion": 1,
  "enabled": true,
  "maxLevel": "R3",
  "scopeWhitelist": [],
  "openScopes": [],
  "perSessionLimit": 3,
  "perDayLimit": 20,
  "targets": ["*"],
  "fastMode": true,
  "autoAnswer": "R3",
  "answerTargets": ["*"]
}
```

## 6. 阶段五：建专家 profile（auto-answer 编排必需）

在 B 机任一笔记本根目录建 `.agents\beta\expert\profile.md`（目录按 agent_id 斜杠分层）：

```markdown
---
flowix_kind: agent-profile
flowix_agent_id: beta/expert
flowix_display_name: "beta 专家"
flowix_expertise: ["research"]
flowix_consultable: true
flowix_redaction_level: R3
flowix_created_at: 1793643000
flowix_updated_at: 1793643000
---
# 人设

通用研究助理。
```

**字段必须是 `flowix_` 前缀完整格式**：缺 `flowix_kind: agent-profile` 或 `flowix_display_name` 会导致解析失败，auto-answer 报 `AGENT_NOT_FOUND` 不回答（推荐直接在 Flowix UI 创建 agent，生成即规范）。

## 7. 阶段六：就绪自检

| 检查项 | 位置 | 通过标准 |
|---|---|---|
| 账号 | `~\.flowix\redaction\account.json` | `username: "beta"`、`deviceToken` 非空 |
| 收件通道 | `~\.flowix\redaction\config.json` | `relayUrl` 指向中继、`deviceToken` 非空、`registeredAgents` 含 `beta/expert` |
| auto-answer | `auto-grant.json` | 同 §5 |
| profile | 笔记本 `.agents\expert\profile.md` | 存在且 frontmatter 正确 |
| app 运行 | 任务栏 | Flowix Dev 窗口保持打开（SSE 订阅随 app 存活） |

---

## 8. 测试配合（B 机侧动作）

### T1–T3（A 机发起，B 机被动）

**无需任何操作**，保持 app 运行即可。A 机侧执行与观测。B 机侧可自行确认：

- 收到咨询：`笔记本\.flowix\redaction-inbound\` 出现新 `rct_*.json`
- 编排运行：`笔记本\.flowix\agents\expert\consults\cst_*.json` 状态 `running → completed`（正常 10–60 秒）
- 回信出门：同目录 `cst_*.events.jsonl` 出现 `result` 事件

### T4（反向：B 机发起）

B 机在**任一 AI 会话**（DSH 会话）中发送：

```
请用 consult_remote 工具咨询 rop/agt_a83d9cf0f4a530af@http://8.160.162.251:8788：
"跨用户反向测试：请用一句话说明双向通信已打通"，waitSeconds 设为 60。
```

通过标准：会话在 60 秒内收到 A 机的回答（`timedOut: false`）。

### T5（加固回归，可选，PowerShell 直接执行）

```powershell
# ① 匿名注册命名空间 id → 期望 403 NAMESPACE_REQUIRED
curl.exe -sS -X POST "http://8.160.162.251:8788/v1/devices" -H "Content-Type: application/json" -d '{\"agentIds\":[\"beta2/x\"]}' --noproxy "*"

# ② 抢注已占 agent → 期望 409 AGENT_TAKEN（A 机侧执行，B 机只需知悉预期）
```

---

## 9. 故障排查

| 症状 | 可能原因 | 处置 |
|---|---|---|
| health 自检超时 | 系统代理劫持 | 关代理；或给终端设 `NO_PROXY=8.160.162.251` |
| 启动报 1420 占用 | 残留 vite 进程 | 见 §2 清理命令 |
| 注册账号 409 USERNAME_TAKEN | 用户名已被用 | 换用户名（如 `beta2`），并同步告知 A 机新地址 |
| 注册 agent 403 NAMESPACE_MISMATCH | Bearer 账号与前缀不符 | agent 用裸 slug `expert`，别手写 `beta/expert` |
| 咨询投出但 B 机收不到 | SSE 订阅未建立 / config 未生效 | 重启 app；核对 §7 自检表 |
| auto-answer 不回答 | profile 缺失 / 日配额满 / app 未运行 | 见 §6；`auto-grant-state.json` 看 `dayUsed` 是否到 `perDayLimit`；保持 app 打开 |
| 回程一直不到（A 机 60s 超时） | B 机编排失败 | 看 `cst_*.json` 状态与 app 日志；`AGENT_NOT_FOUND`=profile，`budget_exceeded`=调大 `perDayLimit` |
| 目录卡未发布导致验签 unsigned | 跳过了 §4 第 4 步 | 补发目录卡（public） |

---

## 10. 附录：关键路径

| 路径 | 用途 |
|---|---|
| `~\.flowix\redaction\account.json` | 中继账号身份（Ed25519 私钥，勿外传） |
| `~\.flowix\redaction\config.json` | 中继地址 / 设备 token / 注册 agent 列表 |
| `~\.flowix\redaction\auto-grant.json` | 自动过门与 auto-answer 配置 |
| `~\.flowix\redaction\auto-grant-state.json` | 配额计数（dayUsed） |
| `笔记本\.agents\expert\profile.md` | 专家身份（auto-answer 依赖） |
| `笔记本\.flowix\redaction-inbound\` | 收到的咨询与回信落档 |
| `笔记本\.flowix\agents\expert\consults\` | 编排元数据与事件日志 |

## 11. 测试完成后清理（可选）

- 若 B 机不再作为测试节点：在设置中注销收件通道；`beta/expert` 归属将保留在中继（先到先得），后续同账号设备可随时改绑恢复
- 日志与落档不影响功能，可保留作审计
