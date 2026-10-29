'use client';

import { useCallback, useEffect, useRef, useState } from 'react';
import { BadgeCheck, Globe, Loader2, LogOut, Search, Upload } from 'lucide-react';

import { redaction } from '@platform/tauri/client';
import type {
  RelayAccountInfo,
  RelayUsage,
  UsageSummary,
} from '@/lib/types/redaction';
import { Button } from '@shared/ui/button';
import { Input } from '@shared/ui/input';
import { useI18n } from '@/lib/i18n';
import { PublishCardDialog } from '@features/agent/components/publish-card-dialog';

type DirectoryCard = import('@/lib/types/redaction').DirectoryCard;

/** 中继账号 + 目录：注册/用量、目录搜索、发布我的 agent 卡（L1 过闸 + 确认门）。 */
export function RelayAccountSection({ notebookPath }: { notebookPath: string }) {
  const { t } = useI18n();
  const [account, setAccount] = useState<RelayAccountInfo | null>(null);
  const [usage, setUsage] = useState<RelayUsage | null>(null);
  /** FD-11 P2-6：近 7 天用量报表（展开态）。 */
  const [summary, setSummary] = useState<UsageSummary | null>(null);
  const [summaryOpen, setSummaryOpen] = useState(false);
  const [username, setUsername] = useState('');
  const [relayUrl, setRelayUrl] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [publishOpen, setPublishOpen] = useState(false);
  const disposedRef = useRef(false);

  const reload = useCallback(async () => {
    try {
      const status = await redaction.accountStatus();
      if (disposedRef.current) return;
      setAccount(status);
      setRelayUrl((current) => current || status?.relayUrl || '');
      setUsage(status ? await redaction.relayUsage().catch(() => null) : null);
    } catch (cause) {
      if (!disposedRef.current) setError(cause instanceof Error ? cause.message : String(cause));
    }
  }, []);

  useEffect(() => {
    disposedRef.current = false;
    void reload();
    return () => {
      disposedRef.current = true;
    };
  }, [reload]);

  const register = useCallback(async () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      const info = await redaction.accountRegister(username.trim(), relayUrl.trim());
      if (disposedRef.current) return;
      setAccount(info);
      setNotice(t('agent.relay.registered', { username: info.username }));
      setUsage(await redaction.relayUsage().catch(() => null));
    } catch (cause) {
      if (!disposedRef.current) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  }, [busy, username, relayUrl, t]);

  const forget = useCallback(async () => {
    if (busy) return;
    setBusy(true);
    try {
      await redaction.accountForget();
      if (disposedRef.current) return;
      setAccount(null);
      setUsage(null);
      setSummary(null);
      setSummaryOpen(false);
    } catch (cause) {
      if (!disposedRef.current) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  }, [busy]);

  /** P2-6：展开/收起近 7 天报表（懒加载）。 */
  const toggleSummary = useCallback(async () => {
    if (!summaryOpen && !summary) {
      try {
        setSummary(await redaction.usageSummary(7));
      } catch (cause) {
        setError(cause instanceof Error ? cause.message : String(cause));
        return;
      }
    }
    setSummaryOpen((open) => !open);
  }, [summaryOpen, summary]);

  return (
    <section className="rounded-lg border border-[var(--border)] bg-[var(--card)] p-4">
      <header className="mb-3 flex items-center gap-2">
        <Globe className="size-4 text-[var(--muted-foreground)]" />
        <h2 className="text-sm font-medium">{t('agent.relay.title')}</h2>
      </header>

      {error && <p className="mb-2 text-xs text-[var(--destructive)]">{error}</p>}
      {notice && <p className="mb-2 text-xs text-[var(--muted-foreground)]">{notice}</p>}

      {account ? (
        <div className="space-y-2 text-xs text-[var(--muted-foreground)]">
          <p className="flex items-center gap-1 text-[var(--foreground)]">
            <BadgeCheck className="size-3.5 text-[var(--primary)]" />
            {account.username}
            <span className="text-[var(--muted-foreground)]">@{account.relayUrl}</span>
          </p>
          {usage && (
            <p>
              {t('agent.relay.usage', {
                deliver: `${usage.deliverUsed}/${usage.deliverLimit}`,
                poll: `${usage.pollUsed}/${usage.pollLimit}`,
              })}
            </p>
          )}
          <button
            type="button"
            className="text-xs text-[var(--primary)] hover:underline"
            onClick={() => void toggleSummary()}
          >
            {summaryOpen ? t('agent.relay.reportHide') : t('agent.relay.reportShow')}
          </button>
          {summaryOpen && summary && (
            <div className="space-y-1 rounded-md border border-[var(--border)] p-2">
              {summary.days.length === 0 && (
                <p className="text-xs text-[var(--muted-foreground)]">{t('agent.relay.reportEmpty')}</p>
              )}
              {summary.days.map((day) => (
                <p key={day.date} className="flex justify-between gap-2 font-mono text-[11px]">
                  <span>{day.date}</span>
                  <span>
                    {t('agent.relay.reportDeliver')}: {day.deliver} · {t('agent.relay.reportPoll')}: {day.poll}
                    {day.directory > 0 && ` · ${t('agent.relay.reportDirectory')}: ${day.directory}`}
                  </span>
                </p>
              ))}
              <p className="text-[11px] text-[var(--muted-foreground)]">
                {t('agent.relay.reportPlan', {
                  deliver: summary.plan.deliverPerHour,
                  poll: summary.plan.pollPerHour,
                  directory: summary.plan.directoryPerDay,
                })}
              </p>
            </div>
          )}
          <div className="flex gap-2 pt-1">
            <Button size="sm" variant="outline" onClick={() => setPublishOpen(true)}>
              <Upload className="mr-1 size-3.5" />
              {t('agent.relay.publish')}
            </Button>
            <Button size="sm" variant="ghost" onClick={forget} disabled={busy}>
              <LogOut className="mr-1 size-3.5" />
              {t('agent.relay.forget')}
            </Button>
          </div>
        </div>
      ) : (
        <div className="space-y-2">
          <Input
            placeholder={t('agent.relay.usernamePlaceholder')}
            value={username}
            onChange={(event) => setUsername(event.target.value)}
            className="h-8 text-xs"
          />
          <Input
            placeholder={t('agent.relay.relayPlaceholder')}
            value={relayUrl}
            onChange={(event) => setRelayUrl(event.target.value)}
            className="h-8 text-xs"
          />
          <Button size="sm" onClick={register} disabled={busy || !username.trim() || !relayUrl.trim()}>
            {busy ? <Loader2 className="mr-1 size-3.5 animate-spin" /> : null}
            {t('agent.relay.register')}
          </Button>
        </div>
      )}

      {account && <DirectorySearchBox relayUrl={account.relayUrl} />}

      {publishOpen && (
        <PublishCardDialog
          notebookPath={notebookPath}
          username={account?.username ?? ''}
          onClose={() => setPublishOpen(false)}
        />
      )}
    </section>
  );
}

function DirectorySearchBox({ relayUrl }: { relayUrl: string }) {
  const { t } = useI18n();
  const [query, setQuery] = useState('');
  const [cards, setCards] = useState<DirectoryCard[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState<string | null>(null);

  const search = useCallback(async () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      setCards(await redaction.directorySearch(query.trim(), undefined, relayUrl));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  }, [busy, query, relayUrl]);

  const copyAddress = useCallback(async (address: string) => {
    try {
      await navigator.clipboard.writeText(`${address}@${relayUrl}`);
      setCopied(address);
      setTimeout(() => setCopied(null), 1500);
    } catch {
      setCopied(null);
    }
  }, [relayUrl]);

  return (
    <div className="mt-4 border-t border-[var(--border)] pt-3">
      <div className="flex gap-2">
        <Input
          placeholder={t('agent.relay.searchPlaceholder')}
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          onKeyDown={(event) => event.key === 'Enter' && void search()}
          className="h-8 text-xs"
        />
        <Button size="sm" variant="outline" onClick={search} disabled={busy}>
          {busy ? <Loader2 className="size-3.5 animate-spin" /> : <Search className="size-3.5" />}
        </Button>
      </div>
      {error && <p className="mt-2 text-xs text-[var(--destructive)]">{error}</p>}
      {cards && cards.length === 0 && (
        <p className="mt-2 text-xs text-[var(--muted-foreground)]">{t('agent.relay.empty')}</p>
      )}
      {cards && cards.length > 0 && (
        <ul className="mt-2 space-y-1">
          {cards.map((card) => (
            <li
              key={card.address}
              className="flex items-center justify-between gap-2 rounded-md border border-[var(--border)] px-2 py-1.5 text-xs"
            >
              <div className="min-w-0">
                <p className="truncate font-medium text-[var(--foreground)]">{card.displayName}</p>
                <p className="truncate text-[var(--muted-foreground)]">
                  {card.address}
                  {card.card?.minRedactionLevel ? ` · ${String(card.card.minRedactionLevel)}` : ''}
                  {card.card?.autoAnswer === 'R3' ? ` · ${t('agent.autoGrant.autoAnswerR3')}` : ''}
                </p>
              </div>
              <Button size="sm" variant="ghost" onClick={() => void copyAddress(card.address)}>
                {copied === card.address ? t('agent.relay.copied') : t('agent.relay.copyAddress')}
              </Button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
