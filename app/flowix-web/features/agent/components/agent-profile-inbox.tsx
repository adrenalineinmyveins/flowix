'use client';

import { useCallback, useEffect, useRef, useState } from 'react';
import { Flag, Inbox, Loader2, RefreshCw, Send, ShieldCheck, Sparkles } from 'lucide-react';

import { agentProfile, consult, redaction } from '@platform/tauri/client';
import type { InboundRecord, RedactionConfig } from '@/lib/types/redaction';
import { useNoteStore } from '@features/memo/store/note-store';
import { RedactionSendGateDialog } from '@features/agent/components/redaction-send-gate';
import { RelayAccountSection } from '@features/agent/components/relay-account-section';
import { Button } from '@shared/ui/button';
import { OverlayScrollbar } from '@shared/ui/overlay-scrollbar';
import { ListSurfaceLoadingState } from '@shared/ui/list-surface';
import { useI18n, type I18nKey } from '@/lib/i18n';
import { subscribe } from '@platform/tauri/event-bus';
import { cn } from '@/lib/utils';

const STATUS_LABEL_KEYS: Record<InboundRecord['status'], I18nKey> = {
  pending: 'agent.inbox.status.pending',
  accepted: 'agent.inbox.status.accepted',
  rejected: 'agent.inbox.status.rejected',
};

function statusBadgeClass(status: InboundRecord['status']): string {
  switch (status) {
    case 'pending':
      return 'bg-[color-mix(in_oklch,var(--primary)_14%,transparent)] text-[var(--primary)]';
    case 'accepted':
      return 'bg-[color-mix(in_oklch,var(--foreground)_8%,transparent)] text-[var(--muted-foreground)]';
    case 'rejected':
      return 'bg-[color-mix(in_oklch,var(--destructive)_10%,transparent)] text-[var(--destructive)]';
  }
}

function expiresState(expiresAt: number): 'ok' | 'expired' {
  return expiresAt * 1000 > Date.now() ? 'ok' : 'expired';
}

/**
 * F5 入站收件箱（右列）：拉取中继咨询信封、查看 disclosure、接受/拒绝。
 * 接受后的 F4 编排（M3c）落地前，接受仅置状态留档。
 */
export function AgentProfileInbox({ notebookPath }: { notebookPath: string }) {
  const { t } = useI18n();
  const [records, setRecords] = useState<InboundRecord[] | null>(null);
  const [config, setConfig] = useState<RedactionConfig | null>(null);
  const [loading, setLoading] = useState(true);
  const [polling, setPolling] = useState(false);
  const [registering, setRegistering] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  /** 回程草稿（receiptId -> 回答文本），仅组件态；发送走确认门后落 outbox。 */
  const [answerDrafts, setAnswerDrafts] = useState<Record<string, string>>({});
  /** 打开回程确认门的信封（回程 = 回答该咨询）。 */
  const [replyingTo, setReplyingTo] = useState<InboundRecord | null>(null);
  const disposedRef = useRef(false);

  const reload = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [list, cfg] = await Promise.all([
        redaction.inboxList(notebookPath),
        redaction.configGet(),
      ]);
      if (disposedRef.current) return;
      setRecords(list);
      setConfig(cfg);
    } catch (cause) {
      if (!disposedRef.current) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      if (!disposedRef.current) setLoading(false);
    }
  }, [notebookPath]);

  useEffect(() => {
    disposedRef.current = false;
    void reload();
    return () => {
      disposedRef.current = true;
    };
  }, [reload]);

  const poll = useCallback(async () => {
    if (polling) return;
    setPolling(true);
    setNotice(null);
    setError(null);
    try {
      const summary = await redaction.inboxPoll(notebookPath);
      if (disposedRef.current) return;
      setNotice(t('agent.inbox.pollSummary', {
        stored: summary.stored,
        duplicates: summary.duplicates,
        rejected: summary.rejected,
      }));
      await reload();
    } catch (cause) {
      if (!disposedRef.current) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setPolling(false);
    }
  }, [notebookPath, polling, reload, t]);

  const register = useCallback(async () => {
    if (registering) return;
    setRegistering(true);
    setNotice(null);
    setError(null);
    try {
      const profiles = await agentProfile.list(notebookPath);
      const agentIds = profiles.map((profile) => profile.flowix_agent_id);
      if (agentIds.length === 0) {
        if (!disposedRef.current) setError(t('agent.inbox.noAgents'));
        return;
      }
      const next = await redaction.inboxRegister(agentIds);
      if (disposedRef.current) return;
      setConfig(next);
      setNotice(t('agent.inbox.registered', { count: agentIds.length }));
    } catch (cause) {
      if (!disposedRef.current) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setRegistering(false);
    }
  }, [notebookPath, registering, t]);

  const settle = useCallback(async (receiptId: string, accepted: boolean) => {
    setError(null);
    try {
      await redaction.inboxSetStatus(notebookPath, receiptId, accepted);
      if (disposedRef.current) return;
      await reload();
    } catch (cause) {
      if (!disposedRef.current) setError(cause instanceof Error ? cause.message : String(cause));
    }
  }, [notebookPath, reload]);

  /** FD-11 §5 一键举报：向中继人工审核队列投递（账号 token 鉴权，幂等）。 */
  const report = useCallback(async (record: InboundRecord) => {
    setError(null);
    setNotice(null);
    try {
      const accepted = await redaction.report(record.fromAgentId, record.receiptId);
      if (disposedRef.current) return;
      setNotice(accepted
        ? t('agent.inbox.reportAccepted', { from: record.fromDisplayName || record.fromAgentId })
        : t('agent.inbox.reportDuplicate'));
    } catch (cause) {
      if (!disposedRef.current) setError(cause instanceof Error ? cause.message : String(cause));
    }
  }, [t]);

  // ------------------------------------------------------------------
  // F4 编排自动回答（FD-11 v2 待办 P1-4）：把入站咨询交给本机专家生成草稿。
  // 纪律：编排自动化，出门人审——结果只填入草稿框，仍走确认门后才出设备。
  // ------------------------------------------------------------------

  /** receiptId → 自动回答运行态。 */
  const [autoBusy, setAutoBusy] = useState<Record<string, boolean>>({});
  const [autoNote, setAutoNote] = useState<Record<string, string | null>>({});

  /**
   * 触发一次自动回答：命名空间 toAgent 的 slug 段即本机专家
   * flowix_agent_id（inbox 注册时由裸 slug 补前缀而来）；from 取本机
   * 另一 agent（单 agent 时自转述）。轮询事件直到 Result / Askback / Failed。
   */
  const autoAnswer = useCallback(async (record: InboundRecord) => {
    if (autoBusy[record.receiptId]) return;
    setError(null);
    setAutoBusy((prev) => ({ ...prev, [record.receiptId]: true }));
    setAutoNote((prev) => ({ ...prev, [record.receiptId]: null }));
    try {
      const profiles = await agentProfile.list(notebookPath);
      const toAgent = record.toAgentId.split('/').pop() ?? record.toAgentId;
      const fromAgent = profiles.find((profile) => profile.flowix_agent_id !== toAgent)
        ?.flowix_agent_id ?? toAgent;
      if (!profiles.some((profile) => profile.flowix_agent_id === toAgent)) {
        throw new Error(t('agent.inbox.autoAnswerAgentMissing', { agent: toAgent }));
      }
      const meta = await consult.initiate(notebookPath, fromAgent, toAgent, record.payloadText);
      let lastSeq = 0;
      // 事件轮询（2s；编排一轮通常数十秒）。组件卸载经 disposedRef 退出。
      for (let tick = 0; tick < 150; tick += 1) {
        await new Promise((resolve) => setTimeout(resolve, 2000));
        if (disposedRef.current) return;
        const events = await consult.events(notebookPath, fromAgent, meta.consultId, lastSeq);
        let done = false;
        for (const { seq, event } of events) {
          lastSeq = Math.max(lastSeq, seq);
          if (event.kind === 'result') {
            setAnswerDrafts((prev) => ({ ...prev, [record.receiptId]: event.answer }));
            setAutoNote((prev) => ({ ...prev, [record.receiptId]: t('agent.inbox.autoAnswerDone') }));
            done = true;
          } else if (event.kind === 'askback') {
            setAutoNote((prev) => ({
              ...prev,
              [record.receiptId]: t('agent.inbox.autoAnswerAskback', { question: event.question }),
            }));
            done = true;
          } else if (event.kind === 'failed') {
            throw new Error(event.reason);
          }
        }
        if (done) return;
      }
      throw new Error(t('agent.inbox.autoAnswerTimeout'));
    } catch (cause) {
      if (!disposedRef.current) {
        setAutoNote((prev) => ({
          ...prev,
          [record.receiptId]: cause instanceof Error ? cause.message : String(cause),
        }));
      }
    } finally {
      if (!disposedRef.current) setAutoBusy((prev) => ({ ...prev, [record.receiptId]: false }));
    }
  }, [notebookPath, autoBusy, t]);

  const setTarget = useNoteStore((state) => state.setAgentProfileViewTarget);
  const selectedNotebookId = useNoteStore((state) => state.selectedNotebookId);
  const registeredCount = config?.registeredAgents?.length ?? 0;

  // FD-11 P2-8：SSE 推送——订阅中继新信封事件，命中即自动拉取（3s 去抖合并）。
  const pollRef = useRef(poll);
  pollRef.current = poll;
  useEffect(() => {
    if (!notebookPath || !config?.deviceToken) return;
    void redaction.pushStart(notebookPath).catch(() => undefined);
    let timer: number | null = null;
    const unlisten = subscribe<{ agentId: string }>('redaction-inbound', () => {
      if (timer !== null) return;
      timer = window.setTimeout(() => {
        timer = null;
        void pollRef.current();
      }, 3000);
    });
    return () => {
      if (timer !== null) window.clearTimeout(timer);
      unlisten();
      void redaction.pushStop().catch(() => undefined);
    };
  }, [notebookPath, config?.deviceToken, config?.registeredAgents?.length]);

  return (
    <div className="flex h-full min-h-0 flex-col bg-[var(--document-bg)]">
      {/* 操作行：注册收件通道 + 拉取。 */}
      <div className="flex shrink-0 flex-wrap items-center gap-2 border-b border-[var(--divider)] px-6 py-3">
        <Button
          type="button"
          variant="outline"
          className="h-8 gap-1.5 rounded-lg px-2.5 text-xs"
          disabled={registering}
          onClick={() => void register()}
          title={config?.relayUrl ? config.relayUrl : t('agent.inbox.relayMissing')}
        >
          {registering ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <ShieldCheck className="h-3.5 w-3.5" />}
          {registeredCount > 0
            ? t('agent.inbox.registered', { count: registeredCount })
            : t('agent.inbox.register')}
        </Button>
        <Button
          type="button"
          className="h-8 gap-1.5 rounded-lg px-2.5 text-xs"
          disabled={polling}
          onClick={() => void poll()}
        >
          {polling ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <RefreshCw className="h-3.5 w-3.5" />}
          {t('agent.inbox.poll')}
        </Button>
        <Button
          type="button"
          variant="ghost"
          className="ml-auto h-8 rounded-lg px-2.5 text-xs"
          onClick={() => setTarget(null)}
        >
          {t('agent.profile.close')}
        </Button>
      </div>

      <div className="relative min-h-0 flex-1">
        {loading && records === null ? (
          <ListSurfaceLoadingState label={t('agent.profile.loading')} className="h-full w-full" />
        ) : (
          <OverlayScrollbar className="h-full" scrollerClassName="h-full overflow-y-auto">
            <div className="mx-auto w-full max-w-[680px] space-y-3 px-6 py-5">
              {/* FD-11 v2：中继账号 / 目录 / 用量（发布我的 agent 卡）。 */}
              <RelayAccountSection notebookPath={notebookPath} />
              {error && (
                <div className="flex items-start gap-2 rounded-lg border border-[color-mix(in_oklch,var(--destructive)_24%,var(--divider))] bg-[color-mix(in_oklch,var(--destructive)_6%,transparent)] px-3 py-2 text-xs leading-5 text-[var(--destructive)]">
                  {error}
                </div>
              )}
              {notice && (
                <div className="rounded-lg border border-[var(--border)] px-3 py-2 text-xs leading-5 text-[var(--muted-foreground)]">
                  {notice}
                </div>
              )}

              {!loading && records?.length === 0 && (
                <div className="flex flex-col items-center gap-3 py-16 text-center">
                  <span className="flex h-12 w-12 items-center justify-center rounded-2xl bg-[color-mix(in_oklch,var(--muted)_72%,var(--background))] text-[var(--muted-foreground)]">
                    <Inbox className="h-5 w-5" />
                  </span>
                  <p className="max-w-[36ch] text-sm leading-6 text-[var(--muted-foreground)]">
                    {t('agent.inbox.empty')}
                  </p>
                </div>
              )}

              {records?.map((record) => {
                const expired = expiresState(record.disclosure.expiresAt) === 'expired';
                return (
                  <div
                    key={record.receiptId}
                    className="space-y-2 rounded-xl border border-[var(--border)] bg-[color-mix(in_oklch,var(--card)_82%,var(--background))] p-4"
                  >
                    <div className="flex flex-wrap items-center gap-2">
                      <span
                        className={cn(
                          'rounded-full px-2 py-0.5 text-[11px] font-medium',
                          statusBadgeClass(record.status),
                        )}
                      >
                        {t(STATUS_LABEL_KEYS[record.status])}
                      </span>
                      <span className="min-w-0 flex-1 truncate text-sm font-medium text-[var(--foreground)]">
                        {record.fromDisplayName || record.fromAgentId}
                      </span>
                      <span className="shrink-0 font-mono text-[11px] text-[var(--muted-foreground)]">
                        {t('agent.inbox.level')} {record.disclosure.redactionLevel.toUpperCase()}
                        {' · '}
                        {t('agent.inbox.hops')} {record.disclosure.hopsRemaining}
                        {expired && ` · ${t('agent.inbox.expired')}`}
                      </span>
                      <button
                        type="button"
                        className="shrink-0 rounded-md p-1 text-[var(--muted-foreground)] transition-colors hover:text-[var(--destructive)]"
                        title={t('agent.inbox.report')}
                        onClick={() => void report(record)}
                      >
                        <Flag className="h-3.5 w-3.5" />
                      </button>
                    </div>
                    <p className="whitespace-pre-wrap break-words text-sm leading-6 text-[var(--foreground)]">
                      {record.payloadText}
                    </p>
                    {record.encrypted && (
                      <p className="font-mono text-[11px] text-[var(--primary)]">
                        {t('agent.inbox.encrypted')}
                      </p>
                    )}
                    {record.rejectCode && (
                      <p className="font-mono text-[11px] text-[var(--destructive)]">
                        {record.rejectCode}
                      </p>
                    )}
                    {record.status === 'pending' && !expired && (
                      <div className="flex justify-end gap-2 pt-1">
                        <Button
                          type="button"
                          variant="ghost"
                          className="h-7 rounded-lg px-2.5 text-xs"
                          onClick={() => void settle(record.receiptId, false)}
                        >
                          {t('agent.inbox.reject')}
                        </Button>
                        <Button
                          type="button"
                          className="h-7 rounded-lg px-2.5 text-xs"
                          onClick={() => void settle(record.receiptId, true)}
                        >
                          {t('agent.inbox.accept')}
                        </Button>
                      </div>
                    )}
                    {record.status === 'accepted' && record.payloadKind === 'consultation' && (
                      <div className="space-y-2 border-t border-[var(--divider)] pt-3">
                        <div className="flex items-center gap-2">
                          <span className="text-xs font-medium text-[var(--foreground)]">
                            {t('agent.inbox.answerLabel')}
                          </span>
                          <Button
                            type="button"
                            variant="outline"
                            className="ml-auto h-7 gap-1.5 rounded-lg px-2.5 text-xs"
                            disabled={Boolean(autoBusy[record.receiptId])}
                            onClick={() => void autoAnswer(record)}
                          >
                            {autoBusy[record.receiptId]
                              ? <Loader2 className="h-3.5 w-3.5 animate-spin" />
                              : <Sparkles className="h-3.5 w-3.5" />}
                            {t('agent.inbox.autoAnswer')}
                          </Button>
                        </div>
                        {autoNote[record.receiptId] && (
                          <p className="text-xs leading-5 text-[var(--muted-foreground)]">
                            {autoNote[record.receiptId]}
                          </p>
                        )}
                        <textarea
                          className="w-full rounded-lg border border-[var(--border)] bg-[var(--background)] px-3 py-2 text-sm leading-6 text-[var(--foreground)] outline-none transition-colors placeholder:text-[var(--muted-foreground)] focus:border-[var(--primary)]"
                          value={answerDrafts[record.receiptId] ?? ''}
                          onChange={(event) =>
                            setAnswerDrafts((prev) => ({
                              ...prev,
                              [record.receiptId]: event.target.value,
                            }))
                          }
                          placeholder={t('agent.inbox.answerPlaceholder')}
                        />
                        <div className="flex justify-end">
                          <Button
                            type="button"
                            className="h-7 gap-1.5 rounded-lg px-2.5 text-xs"
                            disabled={!(answerDrafts[record.receiptId] ?? '').trim()}
                            onClick={() => setReplyingTo(record)}
                          >
                            <Send className="h-3.5 w-3.5" />
                            {t('agent.inbox.sendReply')}
                          </Button>
                        </div>
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
          </OverlayScrollbar>
        )}
      </div>

      {/* F5 回程确认门：回答草稿脱敏后经确认门投回中继（kind=reply，跳数继承减一）。 */}
      {replyingTo && selectedNotebookId && (
        <RedactionSendGateDialog
          notebookId={selectedNotebookId}
          notebookPath={notebookPath}
          presetQuestion={answerDrafts[replyingTo.receiptId] ?? ''}
          presetSender={{
            agentId: replyingTo.toAgentId,
            displayName: replyingTo.toAgentId,
          }}
          presetRecipient={{
            agentId: replyingTo.fromAgentId,
            displayName: replyingTo.fromDisplayName,
          }}
          replyTo={{
            receiptId: replyingTo.receiptId,
            hopsRemaining: replyingTo.disclosure.hopsRemaining,
          }}
          onClose={() => setReplyingTo(null)}
        />
      )}
    </div>
  );
}
