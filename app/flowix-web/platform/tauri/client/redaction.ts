import { invoke } from '@tauri-apps/api/core';

import type {
  ConfirmGateResult,
  RedactionConfig,
  RedactionL1Preview,
  RedactionOutboxRecord,
  RedactionPreviewReport,
  RelayDeliverResult,
  SendPayload,
} from '@/lib/types/redaction';

// F6 脱敏管线 IPC（后端 commands/redaction.rs + redaction/{mod,outbox}.rs）。
// preview 会跑完整 L0→L1→L2（蒸馏）→L3 管线，可能耗时一个 Agent 会话；
// confirm 是确认门唯一决定入口，send 是发送唯一出口。
export const redaction = {
  configGet: () => invoke<RedactionConfig>('redaction_config_get'),

  configSet: (config: RedactionConfig) =>
    invoke<RedactionConfig>('redaction_config_set', { config }),

  /** M1 诊断切片：文本直跑 L1 规则（不进 L0/L2 管线）。 */
  l1Preview: (text: string) =>
    invoke<RedactionL1Preview>('redaction_l1_preview', { text }),

  /** 完整 preview：L0 scope 组装 → L1 规则 → L2 蒸馏 → L3 复扫。返回 draft 态报告。 */
  preview: (cwd: string, question: string, scopeEntryIds: string[], targetLevel: string) =>
    invoke<RedactionPreviewReport>('redaction_preview', {
      cwd,
      question,
      scopeEntryIds,
      targetLevel,
    }),

  /** 确认门决定：approved → confirmed；否则 cancelled（超时语义同拒绝）。 */
  confirm: (cwd: string, receiptId: string, approved: boolean) =>
    invoke<RedactionOutboxRecord>('redaction_confirm', { cwd, receiptId, approved }),

  /** 确认门挂起等待（agent 自动发起路径），超时由后端结算 cancelled。 */
  confirmWait: (cwd: string, receiptId: string, timeoutSecs?: number) =>
    invoke<ConfirmGateResult>('redaction_confirm_wait', { cwd, receiptId, timeoutSecs }),

  /** 唯一发送出口：仅 confirmed 留档可取信封字节（与留档 byte-equal）。 */
  send: (cwd: string, receiptId: string) =>
    invoke<SendPayload>('redaction_send', { cwd, receiptId }),

  /** F5 客户端传输：从唯一出口取字节投递到配置的平台中继（可重试）。 */
  transmit: (cwd: string, receiptId: string) =>
    invoke<RelayDeliverResult>('redaction_transmit', { cwd, receiptId }),

  /** 出站留档列表（createdAt 升序），确认门 draft 恢复与审计共用。 */
  outboxList: (cwd: string) =>
    invoke<RedactionOutboxRecord[]>('redaction_outbox_list', { cwd }),
};
