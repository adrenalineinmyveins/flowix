/**
 * 脱敏确认门的纯逻辑辅助 —— 不持 React 状态, 供 `redaction-send-gate.tsx`
 * 与单测共用。错误码清单对齐后端 `redaction/mod.rs::to_ipc_error` 与
 * `commands/redaction.rs` 的命令层校验。
 */

import type {
  OutboxStatus,
  RedactionOutboxRecord,
  RedactionPreviewReport,
} from '@/lib/types/redaction';

export interface RedactionGateView {
  receiptId: string;
  question: string;
  finalText: string;
  placeholderMap: Record<string, string>;
  l1Hits: { ruleId: string; count: number }[];
  targetLevel: string;
  includedPaths: string[];
  droppedPaths: string[];
  truncated: boolean;
  attempts: number;
  usedFallback: boolean;
  scopeEntryIds: string[];
  ruleSetVersion: string;
}

export function gateViewFromReport(report: RedactionPreviewReport): RedactionGateView {
  return {
    receiptId: report.receiptId,
    question: '',
    finalText: report.finalText,
    placeholderMap: report.placeholderMap,
    l1Hits: report.l1Hits,
    targetLevel: report.targetLevel,
    includedPaths: report.includedPaths,
    droppedPaths: report.droppedPaths,
    truncated: report.truncated,
    attempts: report.attempts,
    usedFallback: report.usedFallback,
    scopeEntryIds: [],
    ruleSetVersion: report.ruleSetVersion,
  };
}

export function gateViewFromRecord(record: RedactionOutboxRecord): RedactionGateView {
  return {
    receiptId: record.receiptId,
    question: record.question,
    finalText: record.payload,
    placeholderMap: record.placeholderMap,
    l1Hits: record.l1Hits,
    targetLevel: record.targetLevel,
    includedPaths: record.includedPaths,
    droppedPaths: record.droppedPaths,
    truncated: record.truncated,
    attempts: record.attempts,
    usedFallback: record.usedFallback,
    scopeEntryIds: record.scopeEntryIds,
    ruleSetVersion: record.ruleSetVersion,
  };
}

/** 确认门剩余秒数（夹到 0；负值表示已过期，调用方应只允许取消）。 */
export function remainingConfirmSeconds(
  createdAtMs: number,
  timeoutSecs: number,
  nowMs: number = Date.now(),
): number {
  const elapsed = Math.floor((nowMs - createdAtMs) / 1000);
  return Math.max(0, timeoutSecs - elapsed);
}

/** outbox 按 createdAt 升序返回，取最新一条 draft 作为可恢复的确认门。 */
export function pickLatestDraft(
  records: RedactionOutboxRecord[],
): RedactionOutboxRecord | null {
  let latest: RedactionOutboxRecord | null = null;
  for (const record of records) {
    if (record.status !== 'draft') continue;
    if (!latest || record.createdAt >= latest.createdAt) latest = record;
  }
  return latest;
}

const ERROR_SUMMARIES: Record<string, string> = {
  QUESTION_REQUIRED: '请先填写要咨询的问题。',
  INVALID_CWD: '当前笔记本路径不可用，请重新打开会话再试。',
  scope_empty: '披露范围为空：请至少勾选一个已启用的授权范围。',
  payload_too_large: '载荷超过上限',
  invalid_level: '无效的脱敏等级',
  distill_failed: '蒸馏失败',
  rule_hit_after_distill: '蒸馏稿复扫仍命中敏感规则，已停止发送',
  placeholder_broken: '占位符映射校验失败',
  confirm_timeout: '确认超时',
  RECEIPT_NOT_FOUND: '留档记录不存在，可能已被清理。',
  ALREADY_SETTLED: '该留档已结算',
  UNCONFIRMED_SEND_BLOCKED: '未经确认，发送被拦截',
  ENVELOPE_NOT_UTF8: '信封内容编码异常。',
  TEXT_REQUIRED: '请输入要检查的文本。',
  INVALID_DEFAULT_LEVEL: '无效的默认脱敏等级。',
  INVALID_LEVEL_OVERRIDE: '无效的等级覆盖配置。',
  INVALID_PAYLOAD_LIMIT: '载荷上限超出允许范围。',
  INVALID_CONFIRM_TIMEOUT: '确认超时配置超出允许范围。',
};

/** 把后端稳定错误码（可带 `:detail` 后缀）转成用户可读文案。 */
export function describeRedactionError(raw: string): string {
  const message = raw.trim();
  const colon = message.indexOf(':');
  const code = colon === -1 ? message : message.slice(0, colon);
  const detail = colon === -1 ? '' : message.slice(colon + 1);
  const summary = ERROR_SUMMARIES[code];
  if (summary) {
    return detail ? `${summary}（${detail}）` : summary;
  }
  return message || '操作失败。';
}

export const OUTBOX_STATUS_LABELS: Record<OutboxStatus, string> = {
  draft: '待确认',
  confirmed: '已确认',
  cancelled: '已取消',
  failed: '失败',
};
