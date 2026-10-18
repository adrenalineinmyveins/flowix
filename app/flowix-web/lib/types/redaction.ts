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

export type ConfirmGateResult =
  | { kind: "confirmed"; payload: string }
  | { kind: "cancelled" };

export interface SendPayload {
  payload: string;
  byteLength: number;
}
