import { invoke } from '@tauri-apps/api/core';

import type {
  AgentParty,
  ConfirmGateResult,
  DirectoryCard,
  DirectoryCardDraft,
  DirectoryPreviewResult,
  InboundRecord,
  InboxPollSummary,
  RedactionConfig,
  RedactionL1Preview,
  RedactionOutboxRecord,
  RedactionPreviewReport,
  RelayAccountInfo,
  RelayUsage,
  RelayDeliverResult,
  UsageSummary,
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
  preview: (
    cwd: string,
    question: string,
    scopeEntryIds: string[],
    targetLevel: string,
    options?: {
      minLevels?: string[];
      sender?: AgentParty;
      recipient?: AgentParty;
      /** 回程信封：指向的原 receiptId（kind=reply）。 */
      inReplyTo?: string;
      /** 回程信封跳数继承（请求信封 hopsRemaining - 1）。 */
      replyHopsRemaining?: number;
      /** FD-11 P2-9 盲转：payload 加密（收件方须有目录卡公钥）。 */
      blindRelay?: boolean;
    },
  ) =>
    invoke<RedactionPreviewReport>('redaction_preview', {
      cwd,
      question,
      scopeEntryIds,
      targetLevel,
      minLevels: options?.minLevels,
      senderAgentId: options?.sender?.agentId,
      senderDisplayName: options?.sender?.displayName,
      recipientAgentId: options?.recipient?.agentId,
      recipientDisplayName: options?.recipient?.displayName,
      inReplyTo: options?.inReplyTo,
      replyHopsRemaining: options?.replyHopsRemaining,
      blindRelay: options?.blindRelay,
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

  /** F5 入站：向中继注册本机 agent 收件通道（凭据落 config）。 */
  inboxRegister: (agentIds: string[]) =>
    invoke<RedactionConfig>('redaction_inbox_register', { agentIds }),

  /** F5 入站：拉取全部已注册 agent 的收件队列（校验 → 留档 → ack）。 */
  inboxPoll: (cwd: string) =>
    invoke<InboxPollSummary>('redaction_inbox_poll', { cwd }),

  /** F5 入站：留档列表（receivedAt 降序）。 */
  inboxList: (cwd: string) =>
    invoke<InboundRecord[]>('redaction_inbox_list', { cwd }),

  /** F5 入站：接受 / 拒绝一条 pending 咨询。 */
  inboxSetStatus: (cwd: string, receiptId: string, accepted: boolean) =>
    invoke<void>('redaction_inbox_set_status', { cwd, receiptId, accepted }),

  // FD-11 v2：中继账号、目录、用量。

  accountStatus: () =>
    invoke<RelayAccountInfo | null>('redaction_account_status'),

  /** 注册或登录（密钥即身份）；成功后主中继对齐账号中继。 */
  accountRegister: (username: string, relayUrl: string) =>
    invoke<RelayAccountInfo>('redaction_account_register', { username, relayUrl }),

  accountForget: () => invoke<void>('redaction_account_forget'),

  /** 目录卡发布预览（L1 过闸 → draft 留档待人审）。 */
  directoryPreview: (
    cwd: string,
    agentSlug: string,
    displayName: string,
    specialtyTags: string[],
    visibility: string,
    minRedactionLevel: string,
  ) =>
    invoke<DirectoryPreviewResult>('redaction_directory_preview', {
      cwd, agentSlug, displayName, specialtyTags, visibility, minRedactionLevel,
    }),

  directoryConfirm: (cwd: string, draftId: string, approved: boolean) =>
    invoke<DirectoryCardDraft>('redaction_directory_confirm', { cwd, draftId, approved }),

  directoryPublish: (cwd: string, draftId: string) =>
    invoke<DirectoryCardDraft>('redaction_directory_publish', { cwd, draftId }),

  directorySearch: (q: string, tag?: string, relayUrl?: string) =>
    invoke<DirectoryCard[]>('redaction_directory_search', { q, tag, relayUrl }),

  directoryCard: (user: string, agentSlug: string, relayUrl?: string) =>
    invoke<DirectoryCard>('redaction_directory_card', { user, agentSlug, relayUrl }),

  relayUsage: () => invoke<RelayUsage>('redaction_relay_usage'),

  /** FD-11 §5 收端举报（返回 true=已受理，false=已举报过）。 */
  report: (targetAgent: string, receiptId: string) =>
    invoke<boolean>('redaction_report', { targetAgent, receiptId }),

  /** FD-11 P2-8：SSE 推送订阅（新信封事件 → `redaction-inbound`）。 */
  pushStart: (cwd: string) => invoke<void>('redaction_push_start', { cwd }),
  pushStop: () => invoke<void>('redaction_push_stop'),

  /** FD-11 P2-6：近 N 天用量报表 + 套餐配额档。 */
  usageSummary: (days: number) =>
    invoke<UsageSummary>('redaction_usage_summary', { days }),
};
