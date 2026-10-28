/**
 * 脱敏管线 IPC 类型 — 镜像后端
 * - `app/flowix-desktop/src/commands/redaction.rs`（config / l1_preview / 命令层错误码）
 * - `app/flowix-desktop/src/redaction/mod.rs`（RedactionPreviewReport）
 * - `app/flowix-desktop/src/redaction/outbox.rs`（RedactionOutboxRecord / OutboxStatus）
 * - `flowix-redaction` crate（RuleHit / RedactionReport，serde camelCase）
 *
 * 留档真源在 `<notebook>/.flowix/redaction-outbox/rct_<id>.json`，上限 200 条 FIFO；
 * 确认门状态机 `draft → confirmed / cancelled / failed`，发送唯一出口
 * `redaction_send`（非 confirmed 一律 `UNCONFIRMED_SEND_BLOCKED:*`）。
 */

/** 后端 `REDACTION_LEVELS`（r1 事实摘要 / r2 泛化摘要 / r3 领域抽象）。 */
export const REDACTION_LEVELS = ["r1", "r2", "r3"] as const;
export type RedactionLevel = (typeof REDACTION_LEVELS)[number];

export const REDACTION_LEVEL_LABELS: Record<string, string> = {
  r1: "R1 · 事实摘要",
  r2: "R2 · 泛化摘要",
  r3: "R3 · 领域抽象",
};

/** `~/.flowix/redaction/config.json`。 */
export interface RedactionConfig {
  schemaVersion: number;
  defaultLevel: string;
  levelOverrides: Record<string, string>;
  payloadLimitKb: number;
  confirmTimeoutSecs: number;
  strictLuhn: boolean;
  rewriteAbsoluteTime: boolean;
  /** FD-8：蒸馏 Agent 类型覆盖，空 = 默认 deepseek-harness。 */
  distillAgentType: string;
  /** F5：平台中继地址，空 = 未配置（transmit 返回 RELAY_NOT_CONFIGURED）。 */
  relayUrl: string;
  /** F5：本机在中继的收件通道凭据（redaction_inbox_register 写入）。 */
  deviceToken?: string | null;
  /** F5：已注册到中继的本机 agent。 */
  registeredAgents?: string[];
}

export interface RelayDeliverResult {
  receiptId: string;
  byteLength: number;
  httpStatus: number;
}

export interface RuleHit {
  ruleId: string;
  count: number;
}

/** `flowix_redaction::RedactionReport`（L1 直跑结果，camelCase）。 */
export interface RedactionReport {
  text: string;
  placeholderMap: Record<string, string>;
  hits: RuleHit[];
  ruleSetVersion: string;
}

export interface RedactionL1Preview {
  report: RedactionReport;
  wordCount: number;
}

/** 完整 preview 管线返回（未确认态，关联 outbox draft）。 */
export interface RedactionPreviewReport {
  receiptId: string;
  /** 最终载荷全文 —— 人审逐字确认的对象。 */
  finalText: string;
  /** 占位符 → 原文映射（只存本地，不出设备）。 */
  placeholderMap: Record<string, string>;
  l1Hits: RuleHit[];
  attempts: number;
  usedFallback: boolean;
  includedPaths: string[];
  droppedPaths: string[];
  truncated: boolean;
  targetLevel: string;
  ruleSetVersion: string;
}

export type OutboxStatus = "draft" | "confirmed" | "cancelled" | "failed";

/** wire v1 路由方（serde 与 flowix-relay AgentRef 同形）。 */
export interface AgentParty {
  agentId: string;
  displayName?: string;
}

export interface RedactionOutboxRecord {
  receiptId: string;
  status: OutboxStatus;
  /** Unix 毫秒。 */
  createdAt: number;
  settledAt: number | null;
  question: string;
  scopeEntryIds: string[];
  /** 当次授权范围标签（tag:/path: 前缀），进信封 disclosure.allowedScopes。旧档无此字段。 */
  allowedScopes?: string[];
  /** F5 wire v1 路由：发起方，缺省 local。旧档无此字段。 */
  fromAgent?: AgentParty;
  /** F5 wire v1 路由：收件方，缺省 manual（手动投递语义）。旧档无此字段。 */
  toAgent?: AgentParty;
  /** F5 回程信封：指向的原 receiptId（有值时 kind=reply）。 */
  inReplyTo?: string | null;
  /** F5 回程信封跳数继承。 */
  replyHopsRemaining?: number | null;
  targetLevel: string;
  ruleSetVersion: string;
  payload: string;
  placeholderMap: Record<string, string>;
  l1Hits: RuleHit[];
  includedPaths: string[];
  droppedPaths: string[];
  attempts: number;
  usedFallback: boolean;
  truncated: boolean;
}

/** F5 入站留档状态机（`.flowix/redaction-inbound/`）。 */
export type InboundStatus = "pending" | "accepted" | "rejected";

/** wire v1 disclosure 段（镜像 flowix-relay WireDisclosure）。 */
export interface WireDisclosure {
  redactionLevel: string;
  maxHops: number;
  hopsRemaining: number;
  /** Unix 秒。 */
  expiresAt: number;
  allowedScopes?: string[];
}

export interface InboundRecord {
  receiptId: string;
  queueId: string;
  fromAgentId: string;
  fromDisplayName: string;
  toAgentId: string;
  /** `consultation` | `reply`。 */
  payloadKind: string;
  payloadText: string;
  /** 入站校验拒绝码（status=rejected 时有值）。 */
  rejectCode?: string | null;
  inReplyTo?: string | null;
  disclosure: WireDisclosure;
  receivedAt: number;
  status: InboundStatus;
  /** FD-11 v2 验签结论（verified/invalid/unsigned/unverified）。 */
  signatureStatus?: string | null;
  /** FD-11 P2-9 盲转：payload 为密文（payloadText 已解密或为占位提示）。 */
  encrypted?: boolean;
}

/** redaction_inbox_poll 汇总。 */
export interface InboxPollSummary {
  stored: number;
  duplicates: number;
  rejected: number;
}

export type ConfirmGateResult =
  | { kind: "confirmed"; payload: string }
  | { kind: "cancelled" };

export interface SendPayload {
  payload: string;
  byteLength: number;
}

// ----------------------------------------------------------------------
// FD-11 v2：中继账号、目录、用量
// ----------------------------------------------------------------------

/** 中继账号（回前端视图，不含私钥）。 */
export interface RelayAccountInfo {
  username: string;
  userId: string;
  publicKey: string;
  relayUrl: string;
}

export type DirectoryCardStatus = "draft" | "confirmed" | "cancelled" | "published";

/** 目录卡发布留档（notebook `.flowix/redaction-directory/`）。 */
export interface DirectoryCardDraft {
  id: string;
  status: DirectoryCardStatus;
  agentSlug: string;
  displayName: string;
  specialtyTags: string[];
  visibility: "public" | "unlisted" | "private";
  minRedactionLevel: string;
  /** FD-12 M4 能力位：off（默认）| R3。 */
  autoAnswer?: string;
  l1Hits?: RuleHit[];
  createdAt: number;
  settledAt?: number | null;
  publishedAt?: number | null;
}

export interface DirectoryPreviewResult {
  draft: DirectoryCardDraft;
  /** L1 脱敏前原文（人审对比）。 */
  original: string;
}

/** 中继目录卡（搜索/详情返回）。 */
export interface DirectoryCard {
  address: string;
  username: string;
  agentSlug: string;
  displayName: string;
  visibility: string;
  card?: {
    publicKey?: string;
    specialtyTags?: string[];
    minRedactionLevel?: string;
    [key: string]: unknown;
  };
  updatedAt: number;
}

/** 本账号中继用量与配额。 */
export interface RelayUsage {
  windowSecs: number;
  deliverUsed: number;
  deliverLimit: number;
  pollUsed: number;
  pollLimit: number;
}

/** FD-11 P2-6：按天用量行（UTC 日期）。 */
export interface UsageDay {
  date: string;
  deliver: number;
  poll: number;
  directory: number;
}

/** FD-11 P2-6：用量报表 + 当前套餐配额档。 */
export interface UsageSummary {
  days: UsageDay[];
  plan: {
    deliverPerHour: number;
    pollPerHour: number;
    directoryPerDay: number;
  };
}

/** FD-12 M1：harness 远程咨询预授权配置（auto-grant.json）。 */
export interface AutoGrantConfig {
  schemaVersion: number;
  enabled: boolean;
  maxLevel: string;
  scopeWhitelist: string[];
  openScopes: string[];
  perSessionLimit: number;
  perDayLimit: number;
  targets: string[];
  fastMode: boolean;
  /** FD-12 M4：B 端能力位。off（默认）| R3（自动回答入站咨询并直发回程）。 */
  autoAnswer: string;
  /** 允许自动回答的来源用户白名单（* = 全部）。 */
  answerTargets: string[];
}
