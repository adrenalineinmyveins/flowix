'use client';

import { useCallback, useEffect, useRef, useState } from 'react';
import { Inbox, Loader2, RefreshCw, ShieldCheck } from 'lucide-react';

import { agentProfile, redaction } from '@platform/tauri/client';
import type { InboundRecord, RedactionConfig } from '@/lib/types/redaction';
import { useNoteStore } from '@features/memo/store/note-store';
import { Button } from '@shared/ui/button';
import { OverlayScrollbar } from '@shared/ui/overlay-scrollbar';
import { ListSurfaceLoadingState } from '@shared/ui/list-surface';
import { useI18n, type I18nKey } from '@/lib/i18n';
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

  const setTarget = useNoteStore((state) => state.setAgentProfileViewTarget);
  const registeredCount = config?.registeredAgents?.length ?? 0;

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
                    </div>
                    <p className="whitespace-pre-wrap break-words text-sm leading-6 text-[var(--foreground)]">
                      {record.payloadText}
                    </p>
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
                  </div>
                );
              })}
            </div>
          </OverlayScrollbar>
        )}
      </div>
    </div>
  );
}
