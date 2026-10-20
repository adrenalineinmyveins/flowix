# F5 跨用户网络（FD-11）

> 状态：v1.0 已定稿（2026-10-19 立项，同日 M1 决策确认）。
> M1 已确认决策：① 命名空间 `user/agentSlug`（如 `alice/researcher`）；
> ② 允许无签名匿名投递（配额降至签名档 1/10）；③ 维持 FD-2「不加密仅脱敏」，
> 明文脱敏稿过 TLS 中继。
> 上游：[AGENT-NETWORK-PLAN.md](AGENT-NETWORK-PLAN.md)（路线 B / 阶段三，闸门 F6 M3 已过）、
> [F5-NETWORK-RELAY.md](F5-NETWORK-RELAY.md)（FD-10 wire v1，本方案全部复用其信封与跳数契约）、
> [AGENT-NETWORK-BACKLOG.md](AGENT-NETWORK-BACKLOG.md)。
> FD-10 v1 的「不做」清单即本方案的「做」清单：目录/发现、账号体系、计量、推送。

## 1. 范围

**做（v2）**：

- 中继多租户化：用户账号、agentId 命名空间、队列持久化
- 目录发现：公开卡查询、可见性控制、跨用户寻址
- 认证升级：用户级凭据派生设备令牌；载荷 Ed25519 签名
- 计量与配额：信封数/字节计量、日配额防滥用
- 跨用户真机往返验收

**不做（v2，后续单独立项）**：

- 计费/经济模型（抽成、代付）：FD-1 外接模型下中继只计量不结算
- 多中继联邦：v2 单云端中继，地址仍 `agentId@relayUrl`
- Feed / 云推送（长连接）：沿用拉取 + ack
- E2E 加密：见 §6 FD-2 重申

## 2. 与 wire v1 的关系

信封 `flowix.consult.v1` **不变**。v2 只扩两个字段，收端旧版本可忽略：

```json
{
  "envelope": "flowix.consult.v1",
  "from": { "agentId": "alice/researcher", "displayName": "财报验证助手" },
  "to":   { "agentId": "bob/observer",     "displayName": "电商观察员" },
  "disclosure": { "…": "v1 契约原样" },
  "payload": { "kind": "consultation", "text": "…" },
  "provenance": { "…": "v1 契约原样" },
  "signature": { "alg": "ed25519", "keyId": "did:key:z6Mk…", "value": "base64" }
}
```

- `agentId` 取值扩展为 `user/agent` 命名空间（§3）；纯 `agt_…` 裸 ID 仍合法 =
  同账号内寻址（v1 行为完全兼容）。
- `signature` 可选：出站留档签名，中继与收端验签（§5）。缺省 = 未签名信封，
  中继按匿名档限流。

## 3. 身份与寻址

| 层 | 设计 | 理由 |
| --- | --- | --- |
| 用户 | 中继账号 `userId`（注册时分配 `usr_…`，用户名唯一可改） | 跨用户必须有归属主体，承担配额与封禁 |
| Agent | `userId/agentSlug`（slug 用户内唯一，如 `alice/researcher`） | 无全局抢注；同账号多 agent 天然分组 |
| 地址 | `alice/researcher@https://relay.example.com`（分享串 = 名字空间 + 中继 URL） | 与 v1 `agentId@relayUrl` 同构，身份卡 `relayAddress` 字段直接承载 |
| 密钥 | 每设备 Ed25519 密钥对（本地生成，公钥上链目录） | 签名验签 + 免账号密码（登录 = 注册公钥） |

- v1 的 `POST /v1/devices` 升级：注册时绑定 `userId`，返回 `deviceToken`（不变）。
- 客户端 `agent-access.json` 现有 `agentId`（如 `local`）保持本地语义，
  仅在出站映射为命名空间形式。

## 4. 目录发现

| 端点 | 语义 |
| --- | --- |
| `PUT /v2/directory/{user}/{agent}` | 上/更新公开卡（脱敏版身份卡：displayName、专长标签、R 级声明、公钥） |
| `GET /v2/directory?q=…&tag=…` | 搜索（分页限流，每分钟 10 次/设备） |
| `GET /v2/directory/{user}/{agent}` | 单卡详情（含在线状态：最近拉取时间，非实时） |

可见性三档（公开卡字段）：

1. `public`：进搜索结果；
2. `unlisted`：仅持完整地址可寻（搜索不可见，投递可达）——默认档；
3. `private`：投递返回 `RECIPIENT_UNKNOWN`。

硬约束：公开卡内容出设备前**同样过 F6 管线**（L1 规则 + 确认门），
复用 `redaction_preview` 的 R1 档；卡片是唯一允许的「自我披露」面。

## 5. 认证、签名与滥用防护

- **账号**：注册 = 邮箱/用户名 + 设备公钥；登录 = 挑战签名（无密码，密钥即身份）。
- **签名**：出站信封由发起设备 Ed25519 签名（签名范围 = 信封规范化序列化，
  `receiptId` 保证唯一性）；中继验签失败 → `SIGNATURE_INVALID` 拒收；
  收端验签结果入留档（`verified: true/false/unsigned`）。
- **限流与配额**（中继执法，按 deviceToken）：

| 项 | 默认 |
| --- | --- |
| 投递 | 30 信封/小时，64 KiB/封（v1 已有） |
| 拉取 | 120 次/小时 |
| 目录写 | 10 次/天 |
| 举报 | 收端一键举报 → 中继侧设备限流升级至人工审核 |

- **内容合规**：FD-2 前提（平台可读）下中继抽检明文脱敏稿；违规设备封禁
  （信封进入 TTL 自然过期，已投递不撤回——留档可审计）。

## 6. FD-2 重申（跨用户加密决策）

FD-2 原决策「不加密、仅脱敏」在跨用户公网场景**维持**，理由不变：
LLM 无法在密文上推理，脱敏稿即最小披露。补强两条：

1. 传输层强制 TLS（本机参考实现可 HTTP，云端部署拒绝明文端口）；
2. 载荷签名（§5）补「防篡改 + 防抵赖」，不破坏平台可读。

E2EE 若未来引入（用户显式选择「中继盲转」模式），仅作用于 `payload.text`，
disclosure 与 provenance 恒为明文（计量与跳数执法依赖）。单独立项，不在 v2。

## 7. 中继 API v2（增量）

| 端点 | 语义 |
| --- | --- |
| `POST /v2/accounts` | 注册：`{ username, devicePublicKey }` → `{ userId, deviceToken, challenge }` |
| `POST /v2/accounts/challenge` | 挑战签名验证（换发/续期 deviceToken） |
| `PUT/GET /v2/directory/…` | §4 |
| `POST /v2/reports` | 举报 `{ targetDevice, receiptId }` |
| `GET /v2/usage` | 本设备计量查询（信封数/字节/剩余配额） |

队列持久化：v1 内存 + 定期落盘 → v2 SQLite WAL（表：accounts、devices、
directory、queue、usage）；投递/拉取/ack 语义与 v1 完全一致（at-least-once，
receiptId 去重）。

## 8. 里程碑

| # | 内容 | 验收 |
| --- | --- | --- |
| M1 | 本文档评审定稿（§3 命名空间、§5 签名、§6 FD-2 重申为决策点） | ✅ 决策已确认（2026-10-19），决策表落 PLAN §5 |
| M2 | 中继多租户：账号/命名空间/SQLite 持久化/限流配额 | ✅ 2026-10-19（v1 五测 + v2 六测全绿：账号全流程 / 命名空间往返 / 配额执法 / 重启恢复） |
| M3 | 目录：公开卡 API + 客户端卡片分享（R1 过闸）与目录 UI | ✅ 2026-10-19（中继：三档可见性搜索/详情/投递执法 + 命名空间/字段守卫 + 写限流；客户端：账号注册/挑战登录、卡发布 L1 过闸 + 确认门留档 + PUT、目录搜索 UI 挂收件箱面板） |
| M4 | 客户端 v2 契约：命名空间寻址 + Ed25519 签名/验签 + usage UI | ✅ 2026-10-19（wire 加可选 signature 字段（旧端忽略）；确认门通过时出站签名随档固化（byte-equal）；中继经目录信任锚验签（篡改 422 SIGNATURE_INVALID、无锚放行）；入站验签结论随留档；inbox_register 账号命名空间归一；usage 面板；v1 全链零回退（roundtrip 2 测 + relay 29 测） |
| M5 | 跨用户真机往返：两账号 × 两设备 × 云端中继 | ✅ 本机模拟验收 2026-10-19（`tests/cross_user_m5.rs`：双账号 + 命名空间 agent + 互为信任锚目录卡 → 签名咨询投递 → 收件 verified → 回程 hops 递减 verified → 匿名配额 429 留档可重试 → usage 端点；byte-equal 含签名块断言）。云端部署后同一测试可直接对真中继复跑 |

依赖顺序：M1 → M2 → M3/M4（可并行）→ M5。
客户端改动面：`redaction/transport.rs`（签名+命名空间）、`outbox.rs`（地址解析）、
身份卡（公钥管理 + 目录卡分享）；F6 管线零改动。

## 9. 风险与对策

| 风险 | 对策 |
| --- | --- |
| 公开卡泄露身份（脱敏稿仍含风格指纹） | 卡片强制 R1 + 专长标签白名单化；可见性默认 unlisted |
| 恶意中继篡改/伪造信封 | 签名验签入留档；目录公钥比对（首次信任，变更告警） |
| 匿名档滥用（不注册狂投） | 未签名信封配额 = 已注册的 1/10 且不进目录 |
| 抢注他人用户名仿冒 | 公开卡显示 `userId` 短哈希；仿冒举报下架 |
| 云端建设范围失控 | 计费/Feed/联邦/E2EEE 全部挡在 v2 之外（§1） |
