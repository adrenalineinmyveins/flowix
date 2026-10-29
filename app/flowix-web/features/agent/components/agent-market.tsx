'use client';

/**
 * AGENT-MARKET M2：市场主页（中列默认视图）。
 * 浏览（空查询 = 第一页）/ 关键词 + 标签搜索 / 卡详情展开；未登录 relay
 * 时显示账号引导（复用收件箱账号区组件）。订阅动作 M3 并入。
 */
import { useCallback, useEffect, useRef, useState } from 'react';
import { BookmarkCheck, BookmarkPlus, ChevronDown, ChevronRight, Copy, Loader2, RefreshCw, Search, Store } from 'lucide-react';

import { market, redaction } from '@platform/tauri/client';
import type { SubscribedCard } from '@platform/tauri/client';
import type { DirectoryCard } from '@/lib/types/redaction';
import { invalidateMentionAgents } from '@features/editor/extensions/agent-reference/agent-mention-data';
import { RelayAccountSection } from '@features/agent/components/relay-account-section';
import { Button } from '@shared/ui/button';
import { Input } from '@shared/ui/input';
import { useI18n } from '@/lib/i18n';
import { cn } from '@/lib/utils';

export function AgentMarket({ notebookPath }: { notebookPath: string }) {
  const { t } = useI18n();
  const [accountReady, setAccountReady] = useState<boolean | null>(null);
  const [relayUrl, setRelayUrl] = useState('');
  const [query, setQuery] = useState('');
  const [tag, setTag] = useState('');
  const [cards, setCards] = useState<DirectoryCard[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [copied, setCopied] = useState<string | null>(null);
  const disposedRef = useRef(false);
  /** 默认浏览只触发一次（search 引用随 busy 变化，不作为重试信号）。 */
  const initialFetchRef = useRef(false);

  // M3（D-5）：订阅快照 + 订阅/退订/刷新动作态。
  const [subscriptions, setSubscriptions] = useState<SubscribedCard[]>([]);
  const [subBusy, setSubBusy] = useState<string | null>(null);
  const [subError, setSubError] = useState<string | null>(null);

  const reloadSubscriptions = useCallback(async () => {
    try {
      const list = await market.subscriptions();
      // AGENT-MARKET M4：订阅列表变化后失效 @ 提及缓存（两入口同源）。
      invalidateMentionAgents();
      if (!disposedRef.current) setSubscriptions(list);
    } catch {
      // 订阅列表读取失败不阻塞市场浏览。
    }
  }, []);

  useEffect(() => {
    void reloadSubscriptions();
  }, [reloadSubscriptions]);

  const subscribe = useCallback(async (address: string) => {
    if (subBusy) return;
    setSubBusy(address);
    setSubError(null);
    try {
      await market.subscribe(address, relayUrl || undefined);
      await reloadSubscriptions();
    } catch (cause) {
      if (!disposedRef.current) setSubError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      if (!disposedRef.current) setSubBusy(null);
    }
  }, [subBusy, relayUrl, reloadSubscriptions]);

  const unsubscribe = useCallback(async (address: string) => {
    if (subBusy) return;
    setSubBusy(address);
    setSubError(null);
    try {
      await market.unsubscribe(address);
      await reloadSubscriptions();
    } catch (cause) {
      if (!disposedRef.current) setSubError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      if (!disposedRef.current) setSubBusy(null);
    }
  }, [subBusy, reloadSubscriptions]);

  const refreshSubscription = useCallback(async (address: string) => {
    if (subBusy) return;
    setSubBusy(address);
    setSubError(null);
    try {
      await market.refreshSubscription(address);
      await reloadSubscriptions();
    } catch (cause) {
      if (!disposedRef.current) setSubError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      if (!disposedRef.current) setSubBusy(null);
    }
  }, [subBusy, reloadSubscriptions]);

  useEffect(() => {
    disposedRef.current = false;
    void redaction.accountStatus()
      .then((status) => {
        if (disposedRef.current) return;
        setAccountReady(status !== null);
        setRelayUrl(status?.relayUrl ?? '');
      })
      .catch(() => {
        if (!disposedRef.current) setAccountReady(false);
      });
    return () => {
      disposedRef.current = true;
    };
  }, []);

  const search = useCallback(async (q: string, tagFilter: string) => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      setCards(await redaction.directorySearch(q.trim(), tagFilter.trim() || undefined));
    } catch (cause) {
      if (!disposedRef.current) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      if (!disposedRef.current) setBusy(false);
    }
  }, [busy]);

  // 挂载后默认浏览第一页（空查询，仅一次；失败由错误行提示手动重试）。
  useEffect(() => {
    if (accountReady === true && !initialFetchRef.current) {
      initialFetchRef.current = true;
      void search('', '');
    }
  }, [accountReady, search]);

  const copyAddress = useCallback(async (address: string) => {
    try {
      await navigator.clipboard.writeText(`${address}@${relayUrl}`);
      setCopied(address);
      setTimeout(() => setCopied(null), 1500);
    } catch {
      setCopied(null);
    }
  }, [relayUrl]);

  if (accountReady === null) {
    return (
      <div className="flex h-full items-center justify-center">
        <Loader2 className="size-4 animate-spin text-[var(--muted-foreground)]" />
      </div>
    );
  }

  // 未登录：账号引导（注册/登录后即可搜索）。
  if (!accountReady) {
    return (
      <div className="flex h-full min-h-0 flex-col bg-[var(--document-bg)]">
        <div className="mx-auto w-full max-w-[560px] px-6 py-10">
          <div className="mb-4 flex items-center gap-2 text-[var(--foreground)]">
            <Store className="size-4 text-[var(--brand)]" />
            <h1 className="text-base font-semibold tracking-[-0.02em]">{t('agent.market.title')}</h1>
          </div>
          <p className="mb-6 text-sm leading-6 text-[var(--muted-foreground)]">
            {t('agent.market.signInHint')}
          </p>
          <RelayAccountSection notebookPath={notebookPath} />
        </div>
      </div>
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-col bg-[var(--document-bg)]">
      <div className="mx-auto w-full max-w-[640px] px-6 py-6">
        <div className="mb-4 flex items-center gap-2 text-[var(--foreground)]">
          <Store className="size-4 text-[var(--brand)]" />
          <h1 className="text-base font-semibold tracking-[-0.02em]">{t('agent.market.title')}</h1>
        </div>
        <div className="flex gap-2">
          <Input
            placeholder={t('agent.market.searchPlaceholder')}
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={(event) => event.key === 'Enter' && void search(query, tag)}
            className="h-8 flex-1 text-xs"
          />
          <Input
            placeholder={t('agent.market.tagPlaceholder')}
            value={tag}
            onChange={(event) => setTag(event.target.value)}
            onKeyDown={(event) => event.key === 'Enter' && void search(query, tag)}
            className="h-8 w-36 text-xs"
          />
          <Button size="sm" variant="outline" onClick={() => void search(query, tag)} disabled={busy}>
            {busy ? <Loader2 className="size-3.5 animate-spin" /> : <Search className="size-3.5" />}
          </Button>
        </div>

        {error && <p className="mt-3 text-xs text-[var(--destructive)]">{error}</p>}
        {cards && cards.length === 0 && !busy && (
          <p className="mt-3 text-xs text-[var(--muted-foreground)]">{t('agent.market.empty')}</p>
        )}
        {cards && cards.length > 0 && (
          <ul className="mt-3 space-y-1">
            {cards.map((card) => {
              const open = expanded === card.address;
              const subscribed = subscriptions.some((item) => item.address === card.address);
              return (
                <li key={card.address} className="rounded-md border border-[var(--border)]">
                  <div className="flex items-center gap-2 px-2 py-1.5 text-xs">
                    <button
                      type="button"
                      className="flex min-w-0 flex-1 items-center gap-1.5 text-left"
                      onClick={() => setExpanded(open ? null : card.address)}
                    >
                      {open
                        ? <ChevronDown className="size-3.5 shrink-0 text-[var(--muted-foreground)]" />
                        : <ChevronRight className="size-3.5 shrink-0 text-[var(--muted-foreground)]" />}
                      <span className="min-w-0">
                        <span className="block truncate font-medium text-[var(--foreground)]">
                          {card.displayName}
                        </span>
                        <span className="block truncate text-[var(--muted-foreground)]">
                          {card.address}
                          {card.card?.minRedactionLevel ? ` · ${String(card.card.minRedactionLevel)}` : ''}
                          {card.card?.autoAnswer === 'R3' ? ` · ${t('agent.autoGrant.autoAnswerR3')}` : ''}
                        </span>
                      </span>
                    </button>
                    <Button
                      size="sm"
                      variant={subscribed ? 'ghost' : 'outline'}
                      className="h-6 px-2 text-xs"
                      disabled={subBusy === card.address}
                      onClick={() => void (subscribed ? unsubscribe(card.address) : subscribe(card.address))}
                      title={subscribed ? t('agent.market.unsubscribe') : t('agent.market.subscribe')}
                    >
                      {subBusy === card.address
                        ? <Loader2 className="mr-1 size-3 animate-spin" />
                        : subscribed
                          ? <BookmarkCheck className="mr-1 size-3 text-[var(--primary)]" />
                          : <BookmarkPlus className="mr-1 size-3" />}
                      {subscribed ? t('agent.market.subscribed') : t('agent.market.subscribe')}
                    </Button>
                    <Button size="sm" variant="ghost" className="h-6 px-2 text-xs" onClick={() => void copyAddress(card.address)}>
                      <Copy className="mr-1 size-3" />
                      {copied === card.address ? t('agent.relay.copied') : t('agent.relay.copyAddress')}
                    </Button>
                  </div>
                  {open && (
                    <dl className="space-y-1 border-t border-[var(--border)] px-3 py-2 text-[11px] leading-5 text-[var(--muted-foreground)]">
                      <div className="flex gap-2">
                        <dt className="w-24 shrink-0">{t('agent.market.detailAddress')}</dt>
                        <dd className="min-w-0 break-all font-mono">{card.address}@{relayUrl}</dd>
                      </div>
                      <div className="flex gap-2">
                        <dt className="w-24 shrink-0">{t('agent.market.detailVisibility')}</dt>
                        <dd>{String(card.card?.visibility ?? card.visibility)}</dd>
                      </div>
                      <div className="flex gap-2">
                        <dt className="w-24 shrink-0">{t('agent.market.detailMinLevel')}</dt>
                        <dd>{String(card.card?.minRedactionLevel ?? '—')}</dd>
                      </div>
                      <div className="flex gap-2">
                        <dt className="w-24 shrink-0">{t('agent.market.detailAutoAnswer')}</dt>
                        <dd>{card.card?.autoAnswer === 'R3' ? t('agent.autoGrant.autoAnswerR3') : t('agent.autoGrant.autoAnswerOff')}</dd>
                      </div>
                      <div className="flex gap-2">
                        <dt className="w-24 shrink-0">{t('agent.market.detailUpdated')}</dt>
                        <dd>{card.updatedAt > 0 ? new Date(card.updatedAt * 1000).toLocaleString() : '—'}</dd>
                      </div>
                    </dl>
                  )}
                </li>
              );
            })}
          </ul>
        )}
        <p className={cn('mt-4 text-[11px] leading-5 text-[var(--muted-foreground)]')}>
          {t('agent.market.firstPageNote')}
        </p>

        {/* M3（D-5）：我的订阅分区（快照列表 + 刷新/退订；consult 可直接点名）。 */}
        <section className="mt-8 border-t border-[var(--border)] pt-4">
          <h2 className="mb-2 text-sm font-semibold text-[var(--foreground)]">
            {t('agent.market.mySubscriptions')}
          </h2>
          {subError && <p className="mb-2 text-xs text-[var(--destructive)]">{subError}</p>}
          {subscriptions.length === 0 ? (
            <p className="text-xs leading-5 text-[var(--muted-foreground)]">
              {t('agent.market.noSubscriptions')}
            </p>
          ) : (
            <ul className="space-y-1">
              {subscriptions.map((item) => (
                <li
                  key={item.address}
                  className="flex items-center justify-between gap-2 rounded-md border border-[var(--border)] px-2 py-1.5 text-xs"
                >
                  <div className="min-w-0">
                    <p className="truncate font-medium text-[var(--foreground)]">{item.displayName}</p>
                    <p className="truncate text-[var(--muted-foreground)]">
                      {item.address}@{item.relayUrl}
                    </p>
                  </div>
                  <div className="flex shrink-0 gap-1">
                    <Button
                      size="sm"
                      variant="ghost"
                      className="h-6 px-2 text-xs"
                      disabled={subBusy === item.address}
                      title={t('agent.market.refreshSubscription')}
                      onClick={() => void refreshSubscription(item.address)}
                    >
                      {subBusy === item.address
                        ? <Loader2 className="size-3 animate-spin" />
                        : <RefreshCw className="size-3" />}
                    </Button>
                    <Button
                      size="sm"
                      variant="ghost"
                      className="h-6 px-2 text-xs text-[var(--destructive)]"
                      disabled={subBusy === item.address}
                      onClick={() => void unsubscribe(item.address)}
                    >
                      {t('agent.market.unsubscribe')}
                    </Button>
                  </div>
                </li>
              ))}
            </ul>
          )}
        </section>
      </div>
    </div>
  );
}
