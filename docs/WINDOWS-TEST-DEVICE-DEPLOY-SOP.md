# Windows 测试设备部署 SOP（安装版分发）

> 版本：v1.0（2026-11-04）
> 适用对象：向更多 Windows 测试设备分发**安装版** Flowix（NSIS 安装包），并完成 consult 跨用户测试就绪配置
> 区别：本手册面向**生产安装包**（无需 Node/Rust 环境）；开发模式（源码 + `tauri:dev:win`）请用 `CROSS-USER-CONSULT-TEST-SOP.md`

## 0. 拓扑与产物

```
A 机（rop，构建机 + 被咨询方）              测试设备（tester2 / tester3 / …）
  rop/tuning 等已发布 agent   ⇄  中继  ⇄  各设备独立账号 + 收件 agent
                                    8.160.162.251:8788
```

**安装包**（A 机生产构建产物）：

```
E:\flowix\flowix\.build\cargo-target\release\bundle\nsis\Flowix_1.5.6_x64-setup.exe
```

- 代码基线：分支 `trae/agent-dRBdg4` @ `1475efd4`（含 SSE 动态订阅、memo 工具链、relay 动态归属过滤全部修复，及 enex 导入 / PDF 批量转换 / 文件浏览器）
- 构建方式：`FLOWIX_ALLOW_UNSIGNED=1 npm run tauri:build:prod` → **未签名测试包**
- 未签名含义：① 安装时 SmartScreen 会拦截（见 §2）；② 无 updater 签名产物，**自动更新不可用**，出新版本 = 重新分发新 setup.exe 覆盖安装
- 分发方式不限：U 盘 / 局域网共享 / 即时通讯传文件均可（单文件 ~百 MB 级）

## 1. 设备要求

| 项 | 要求 |
|---|---|
| 系统 | Windows 10/11 x64 |
| 磁盘 | ≥ 2 GB 空闲（应用 + DSH runtime） |
| 网络 | ① 直连 `http://8.160.162.251:8788`（中继，HTTP 直连，**不能走代理**）；② 可达 `https://download.flowix-memo.com`（DSH runtime 托管下载） |
| 已装正式版 Flowix | 同 bundle ID（`com.flowix.app`）会被覆盖安装；建议先卸载旧版（用户数据/笔记本不受影响） |

网络自检（PowerShell，期望 `200`）：

```powershell
curl.exe -s -o NUL -w "%{http_code}" --noproxy "*" http://8.160.162.251:8788/v1/health
```

超时/失败 → 关闭系统代理（Clash/v2ray 等）后重试，或设环境变量 `NO_PROXY=8.160.162.251`。

## 2. 安装

1. 双击 `Flowix_1.5.6_x64-setup.exe`
2. **SmartScreen 弹窗「Windows 已保护你的电脑」**（未签名包必弹）：点「更多信息」→「仍要运行」
3. NSIS 向导默认 per-user 安装，一路下一步即可
4. 安装器自动完成（无需手动）：
   - 写 CLI 垫片 `%LOCALAPPDATA%\Flowix\bin\flowix.cmd` 并**加入用户 PATH**（新开终端可用 `flowix` 命令）
   - 安装日志：`%LOCALAPPDATA%\FlowixData\app-update.log`（排障先看这里）

## 3. 首次启动与应用内配置

按顺序完成。全部在 Flowix 应用内操作，无需命令行。

### 3.1 基础

1. 首启选择/创建笔记本文件夹（任意空目录即可）
2. **设置 → 中继地址**：`http://8.160.162.251:8788`

### 3.2 DSH runtime（对话与 consult 必需）

**设置 → DSH 区域 → 安装 runtime**（托管下载，有进度条）。装完配置 DeepSeek 模型凭据（API key 按 A 机同款配置）。

### 3.3 注册身份（顺序不能颠倒）

1. **注册中继账号**：每台设备一个独立用户名（`tester2`、`tester3`…；Ed25519 密钥本地生成，私钥只落本机 `account.json`，勿外传）
   - 若 `409 USERNAME_TAKEN` → 名字被占，换名重试
2. **注册收件通道 agent**：填裸 slug（如 `expert`，自动归一为 `tester2/expert`；别手写带前缀全名，否则 `403 NAMESPACE_MISMATCH`）
3. **发布目录卡**：可见性 **public**（跨用户验签前提）
   - **必须先做第 2 步再发卡**：目录卡强制「搜得到也投得到」，跳过收件注册会 `422 AGENT_NOT_REGISTERED`

### 3.4 开启 auto-answer

编辑 `C:\Users\<用户名>\.flowix\redaction\auto-grant.json`：

```json
{
  "schemaVersion": 1,
  "enabled": true,
  "maxLevel": "R3",
  "scopeWhitelist": [],
  "openScopes": [],
  "perSessionLimit": 3,
  "perDayLimit": 500,
  "targets": ["*"],
  "fastMode": true,
  "autoAnswer": "R3",
  "answerTargets": ["*"]
}
```

> `perDayLimit: 500` 为测试期放宽值（默认 20 一天很容易打满；入站自动回答与出站自动过门共用此额度）。

### 3.5 建专家 profile（auto-answer 编排必需）

推荐直接在 Flowix UI 创建 agent（生成即规范）。手工建的话，在任一笔记本根目录 `.agents\<user>\<agent>\profile.md`，frontmatter 必须是 `flowix_` 前缀完整格式（缺 `flowix_kind: agent-profile` 或 `flowix_display_name` 会解析失败，auto-answer 报 `AGENT_NOT_FOUND` 不回答）：

```markdown
---
flowix_kind: agent-profile
flowix_agent_id: tester2/expert
flowix_display_name: "tester2 专家"
flowix_expertise: ["research"]
flowix_consultable: true
flowix_redaction_level: R3
flowix_created_at: 1793730000
flowix_updated_at: 1793730000
---
# 人设

通用研究助理。
```

### 3.6 就绪自检

| 检查项 | 位置 | 通过标准 |
|---|---|---|
| 账号 | `~\.flowix\redaction\account.json` | `username` 正确、`deviceToken` 非空 |
| 收件通道 | `~\.flowix\redaction\config.json` | `relayUrl` 指向中继、`registeredAgents` 含 `<user>/<agent>` |
| auto-answer | `auto-grant.json` | 同 §3.4 |
| profile | 笔记本 `.agents\<user>\<agent>\profile.md` | 存在且 frontmatter 完整 |
| DSH runtime | 设置 → DSH | 已安装、模型凭据可用 |
| app 运行 | 任务栏 | Flowix 窗口保持打开（SSE 订阅随 app 存活，关窗收不到咨询） |

## 4. 验证（每台新设备）

1. **被动收件**：A 机（或任一已就绪设备）向 `<user>/expert@http://8.160.162.251:8788` 发一条咨询，60–120 秒内收到自动回答
2. **主动发起**：在本机任一 DSH 会话中发送：

```
请用 consult_remote 工具咨询 rop/tuning@http://8.160.162.251:8788：
"部署验证：请用一句话说明你已收到本条咨询"，waitSeconds 设为 120。
```

   - 窗口内拿到回答（`timedOut: false`）= 全链通
   - 若显示「回复暂时超时（回答稍后会自动送入对话）」：属正常超时语义（答案生成超窗口），稍后会异步注入，不算失败
3. **落档确认**：`笔记本\.flowix\redaction-inbound\` 出现 `rct_*.json`、`.flowix\agents\<agent>\consults\` 出现 `cst_*.json`

## 5. 故障排查

| 症状 | 原因 | 处置 |
|---|---|---|
| 安装被 SmartScreen 拦 | 未签名测试包 | 更多信息 → 仍要运行 |
| 安装失败 | 上个版本进程占用 | 关掉 Flowix 后重跑 setup；看 `FlowixData\app-update.log` |
| 覆盖升级后 CLI 仍是旧版 | DSH 宿主（`%APPDATA%\flowix\dsh\...\node.exe`）拉起的 `flowix-cli` MCP 进程锁住旧文件，静默安装不报错但跳过替换 | 覆盖升级前先结束所有 `node.exe`（路径在 `%APPDATA%\flowix\dsh\` 下）与 `flowix-cli`/`Flowix` 进程再安装；装完核对 `Flowix\flowix-cli.exe` 修改时间；全新安装无此问题 |
| health 自检超时 | 系统代理劫持 | 关代理；或 `NO_PROXY=8.160.162.251` |
| DSH 装不上 | 下载源不可达 | 检查 `download.flowix-memo.com` 连通性；或用 `FLOWIX_DSH_MANIFEST_URL` 指向内网镜像 |
| 发卡 `422 AGENT_NOT_REGISTERED` | 先发卡后注册信箱 | 补做收件注册后重新发卡 |
| 咨询投出但设备无反应 | app 关闭 / SSE 未建立 | 保持 app 打开；重启 app 会触发一次兜底 poll 补齐积压 |
| auto-answer 不回答 | profile 缺失 / 日配额满 | §3.5；`auto-grant-state.json` 看 `dayUsed` 是否到 `perDayLimit` |
| 回程迟迟不到 | 对端编排慢（长答案 >120s） | 等异步注入；或对端查 `cst_*.json` 状态 |
| 升级到新测试版 | 无自动更新 | 拿新 setup.exe 直接覆盖安装（数据保留） |

## 6. 附录：关键路径

| 路径 | 用途 |
|---|---|
| `%LOCALAPPDATA%\Flowix\` | 应用安装目录（含 `flowix-cli.exe`） |
| `%LOCALAPPDATA%\Flowix\bin\flowix.cmd` | CLI 垫片（已入 PATH） |
| `%LOCALAPPDATA%\FlowixData\app-update.log` | 安装/更新日志 |
| `~\.flowix\redaction\account.json` | 中继账号身份（Ed25519 私钥，勿外传） |
| `~\.flowix\redaction\config.json` | 中继地址 / 设备 token / 注册 agent 列表 |
| `~\.flowix\redaction\auto-grant.json` | 自动过门与 auto-answer 配置 |
| `~\.flowix\redaction\auto-grant-state.json` | 配额计数（dayUsed，跨日自动清零） |
| `笔记本\.agents\<user>\<agent>\profile.md` | 专家身份（auto-answer 依赖） |
| `笔记本\.flowix\redaction-inbound\` | 收到的咨询与回信落档 |
| `笔记本\.flowix\agents\<agent>\consults\` | 编排元数据与事件日志 |
