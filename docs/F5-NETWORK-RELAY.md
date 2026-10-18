# F5 网络中继与对端协议（FD-10）

> 状态：v1 实施中（2026-10-19 立项）。
> 上游：[AGENT-NETWORK-PLAN.md](AGENT-NETWORK-PLAN.md)（路线 B、FD-10）、
> [REDACTION-PIPELINE.md](REDACTION-PIPELINE.md)（§2 唯一出口、§5.1 信封契约）、
> [AGENT-LOCAL-CONSULT.md](AGENT-LOCAL-CONSULT.md)（F4 编排复用）。

## 1. 范围

**做（v1）**：

- 信封 wire 契约 v1（对齐 REDACTION-PIPELINE §5.1 目标形态）
- 中继参考实现（本仓库 `app/flowix-relay`，可本地跑、可独立部署）
- 客户端出站升级为 v1 wire 信封；入站收件（拉取 + 校验 + 留档 + 接受/拒绝）
- 回程信封（专家回答沿同一协议返回）
- `hopsRemaining` 对端消费契约（执法点在收件校验与转咨签发）

**不做（v1，按 PLAN §风险纪律）**：

- 目录/发现/搜索：v1 用显式地址（`agentId@relayUrl`，用户间互分享）
- 计费/经济模型、Feed、推送：单独立项
- 账号体系：v1 用设备令牌（device token）注册收件通道
- 端到端加密：v1 载荷走 TLS、中继可见明文——出设备前已过 L0-L3 脱敏与
  确认门，明文即脱敏稿；E2E 加密列 v2
- 多租户/多中继联邦：v1 单中继

## 2. 拓扑

```
A 机（发起方）                                   B 机（专家方）
咨询 → L0-L3 脱敏 → 确认门 → outbox(confirmed)
  → POST /v1/outbox（wire 信封 v1）
                     ┌─────────┐
                     │  relay  │ 校验 disclosure 完整性 → 暂存（TTL=expiresAt）
                     └─────────┘
                                       GET /v1/inbox?agentId=…（B 轮询/手动拉取）
                                       → 入站校验（过期/跳数/范围）
                                       → 接受 → F4 本机编排（专家 agent 回答）
                                       → 回答脱敏 → 确认门 → POST 回程信封
                                       （inReplyTo=原 receiptId，hops 继承-1）
  ← A 收到回答（同 inbox 通道）
```

硬约束：出设备流量**只**走确认门后的唯一出口（transport）；入站信封
**不进**任何业务存储，留档于 `.flowix/redaction/inbound/`（与 outbox 对称）。

## 3. 信封 wire 契约 v1（`flowix.consult.v1`）

对齐 REDACTION-PIPELINE §5.1；wire 层新增 `to` 必填与 `inReplyTo` 可选：

```json
{
  "envelope": "flowix.consult.v1",
  "from": { "agentId": "agt_…", "displayName": "财报验证助手" },
  "to": { "agentId": "agt_…", "displayName": "电商观察员" },
  "inReplyTo": "rct_01J…（回程信封必填，首发省略）",
  "disclosure": {
    "redactionLevel": "R2",
    "maxHops": 2,
    "hopsRemaining": 2,
    "expiresAt": 1761024000,
    "allowedScopes": ["tag:finance/财报"]
  },
  "payload": { "kind": "consultation", "text": "…" },
  "provenance": {
    "pipelineVersion": "1",
    "ruleSetVersion": "…",
    "usedFallback": false,
    "confirmedBy": "user",
    "confirmedAt": 1761020400,
    "receiptId": "rct_01J…"
  }
}
```

- 时间戳统一 Unix 秒（与现有 `RedactionEnvelope` 一致；§5.1 的 RFC3339 示例以本契约为准修正）。
- `payload.text` ≤ 64 KiB；`kind` ∈ { `consultation`, `reply` }。
- 回程信封 `payload.kind="reply"`、`inReplyTo`=原 `provenance.receiptId`、
  `disclosure.hopsRemaining` 继承请求信封的剩余值（见 §5）。

### 3.1 拒绝码（中继与收端共用）

| 码 | 触发 |
| --- | --- |
| `ENVELOPE_VERSION_UNSUPPORTED` | `envelope` ≠ v1 |
| `DISCLOSURE_INCOMPLETE` | disclosure 字段缺失/类型错（PLAN 验收：元数据完整率 100%） |
| `PAYLOAD_TOO_LARGE` | text > 64 KiB |
| `RECIPIENT_UNKNOWN` | `to.agentId` 未注册收件通道 |
| `EXPIRED` | 收端/中继投递时 `expiresAt` 已过 |
| `HOPS_EXHAUSTED` | `hopsRemaining = 0` 的载荷再被用于新咨询 |

## 4. 中继 API v1

| 端点 | 语义 |
| --- | --- |
| `POST /v1/devices` | 注册设备：`{ agentIds: [agt_…] }` → `{ deviceToken }`（收件通道绑定） |
| `POST /v1/outbox` | 投递信封；校验 §3 契约 → 按 `to.agentId` 入队 → `{ acceptedAt, queueId }` |
| `GET /v1/inbox?agentId=…` | 头 `Authorization: Bearer <deviceToken>`；返回待投信封列表（不删除） |
| `POST /v1/inbox/ack` | `{ queueIds: […] }` 确认收妥，中继出队 |
| `GET /v1/health` | 存活与版本 |

- 暂存：内存 + 定期落盘（v1 单进程，重启恢复）；TTL = `expiresAt`，过期由中继清除。
- 中继**不**解释 `payload.text`，只校验 §3 结构与 disclosure 完整性。
- 投递语义：at-least-once（ack 前可重复拉取）；收端以 `receiptId` 去重。

## 5. hopsRemaining 消费契约（对端执法）

1. **收端校验**：`hopsRemaining` 无意义于首发接收（≥0 即可收）；
   过期校验独立执行（`EXPIRED`）。
2. **回答即消耗**：专家回答的回程信封 `hopsRemaining = 请求信封值 - 1`；
   请求值 = 0 时不得回答后转咨（回答仍可回程——回程是应答不是转咨）。
3. **转咨签发**：B 若以 A 的载荷内容再咨询 C，新信封 `hopsRemaining = 收到值 - 1`；
   收到值 = 0 时禁止签发（`HOPS_EXHAUSTED`），UI 明示。
4. 发起方本机签发恒为 `maxHops = 2, hopsRemaining = 2`（现有 outbox 逻辑不变）。

## 6. 客户端行为

- **出站（已有，升级）**：`transport.rs` 在 POST 前把 `RedactionEnvelope`
  映射为 wire v1（补 `from/to/envelope/provenance`）；`to` 来自咨询目标地址
  （身份卡新增 `relayAddress` 展示，来源为对方分享的 `agentId@relayUrl`）。
- **入站（新增）**：`redaction/inbound.rs` —— 拉取（手动/定时）→ 契约校验 →
  留档 `.flowix/redaction/inbound/`（status: pending/accepted/rejected/replied）→
  UI：中列「个人 Agent」视图新增「收件」区（接受=进 F4 编排 / 拒绝=留档）。
- **回程**：专家回答出站仍走 L0-L3 + 确认门 + outbox（同一唯一出口），
  信封带 `inReplyTo` 与继承跳数。

## 7. 里程碑

| # | 内容 | 验收 |
| --- | --- | --- |
| M1 | 本协议文档 | — |
| M2 | `flowix-relay` crate：端点 + 契约校验 + 单测 | 单测覆盖 §3 全部拒绝码与 ack 语义 |
| M3 | 客户端出站 wire v1 + 入站收件模块（拉取/校验/留档/接受/拒绝） | IPC + 留档对称 outbox；收件 UI 可用 |
| M4 | 端到端：本机双笔记本经本地 relay 完成一次咨询-回答往返 | 双侧留档 byte-equal、hops 正确递减 |
