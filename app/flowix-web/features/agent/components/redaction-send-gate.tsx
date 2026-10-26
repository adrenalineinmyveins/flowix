'use client';

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { Loader2, Plus, Reply, RotateCcw, Send, ShieldCheck, X } from 'lucide-react';

import { redaction, tags } from '@platform/tauri/client';
import type {
  RedactionConfig,
  RedactionOutboxRecord,
  RelayDeliverResult,
  SendPayload,
} from '@/lib/types/redaction';
import { REDACTION_LEVELS, REDACTION_LEVEL_LABELS } from '@/lib/types/redaction';
import type { AgentAccessEntry } from '@/lib/types/agent-access';
import { useAgentAccessStore } from '@features/agent/store/agent-access-store';
import { Button } from '@shared/ui/button';
import { cn } from '@/lib/utils';
import {
  describeRedactionError,
  gateViewFromRecord,
  gateViewFromReport,
  pickLatestDraft,
  remainingConfirmSeconds,
  type RedactionGateView,
} from '@features/agent/components/redaction-gate-logic';

type GateStage =
  | { kind: 'await' }
  | { kind: 'confirmed' }
  | { kind: 'cancelled'; reason: 'user' | 'timeout' }
  | { kind: 'sent'; result: SendPayload };

type ScopeTab = 'dirs' | 'tags';

const inputClass =
  'w-full rounded-lg border border-[var(--border)] bg-[var(--background)] px-3 py-2 text-sm text-[var(--foreground)] outline-none transition-colors placeholder:text-[var(--muted-foreground)] focus:border-[var(--primary)] focus:ring-2 focus:ring-[color-mix(in_oklch,var(--primary)_16%,transparent)]';

function InlineError({ message }: { message: string }) {
  return (
    <div className="flex items-start gap-2 rounded-lg border border-[color-mix(in_oklch,var(--destructive)_24%,var(--divider))] bg-[color-mix(in_oklch,var(--destructive)_6%,transparent)] px-3 py-2 text-xs leading-5 text-[var(--destructive)]">
      {message}
    </div>
  );
}

function ScopeRow({
  label,
  detail,
  checked,
  disabled,
  onToggle,
}: {
  label: string;
  detail: string;
  checked: boolean;
  disabled?: boolean;
  onToggle: () => void;
}) {
  return (
    <label
      className={cn(
        'flex min-h-11 cursor-pointer items-center gap-3 rounded-lg border px-3 py-2 text-sm transition-colors',
        checked
          ? 'border-[var(--primary)] bg-[color-mix(in_oklch,var(--primary)_9%,var(--background))]'
          : 'border-[var(--border)] hover:bg-[var(--muted)]',
        disabled && 'cursor-not-allowed opacity-50',
      )}
    >
      <input
        type="checkbox"
        className="h-4 w-4 shrink-0 accent-[var(--primary)]"
        checked={checked}
        disabled={disabled}
        onChange={onToggle}
      />
      <span className="min-w-0 flex-1">
        <span className="block truncate font-medium text-[var(--foreground)]">{label}</span>
        <span className="block truncate text-xs text-[var(--muted-foreground)]" title={detail}>
          {detail}
        </span>
      </span>
    </label>
  );
}

export function RedactionSendGateDialog({
  notebookId,
  notebookPath,
  presetQuestion,
  presetRecipient,
  presetSender,
  replyTo,
  onClose,
}: {
  notebookId: string;
  notebookPath: string;
  /** 预填文本（如收件箱回程：专家回答草稿）。 */
  presetQuestion?: string;
  /** 预设收件方（回程 = 原咨询发起方）。 */
  presetRecipient?: { agentId: string; displayName?: string };
  /** 预设发起方（回程 = 被咨询的本机专家 agent）。 */
  presetSender?: { agentId: string; displayName?: string };
  /** 回程语义：原 receiptId + 请求信封剩余跳数（回程 = 其 - 1）。 */
  replyTo?: { receiptId: string; hopsRemaining: number };
  onClose: () => void;
}) {
  const entries = useAgentAccessStore((state) => state.config.entries);
  const addTagScope = useAgentAccessStore((state) => state.addTagScope);

  const [config, setConfig] = useState<RedactionConfig | null>(null);
  const [question, setQuestion] = useState(presetQuestion ?? '');
  const [targetLevel, setTargetLevel] = useState<string>('r2');
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [scopeTab, setScopeTab] = useState<ScopeTab>('dirs');
  const [notebookTags, setNotebookTags] = useState<{ id: string; name: string }[]>([]);
  const [tagDraft, setTagDraft] = useState('');
  /** F5 收件方：preset 锁定（回程）；否则用户可输 agentId（手动投递留空）。 */
  const [recipientAgentId, setRecipientAgentId] = useState('');
  /** F5 发起身份（发起侧）：空 = 自动取第一个注册 agent；回程由 presetSender 锁定。 */
  const [senderAgentId, setSenderAgentId] = useState('');
  /** FD-11 P2-9 盲转：payload 加密（收件方须有目录卡公钥，命名空间地址）。 */
  const [blindRelay, setBlindRelay] = useState(false);

  const [gate, setGate] = useState<RedactionGateView | null>(null);
  const [gateCreatedAt, setGateCreatedAt] = useState(0);
  const [gateStage, setGateStage] = useState<GateStage>({ kind: 'await' });
  const [remaining, setRemaining] = useState<number | null>(null);
  const [gateError, setGateError] = useState<string | null>(null);
  const [previewing, setPreviewing] = useState(false);
  const [deciding, setDeciding] = useState(false);
  const [sending, setSending] = useState(false);
  const [transmitting, setTransmitting] = useState(false);
  const [transmitResult, setTransmitResult] = useState<RelayDeliverResult | null>(null);

  const disposedRef = useRef(false);
  const decidingRef = useRef(false);

  const confirmTimeoutSecs = config?.confirmTimeoutSecs ?? 120;

  useEffect(() => {
    disposedRef.current = false;
    void (async () => {
      try {
        const [loadedConfig, outbox, tagList] = await Promise.all([
          redaction.configGet(),
          redaction.outboxList(notebookPath).catch(() => [] as RedactionOutboxRecord[]),
          notebookId ? tags.getAll(notebookId).catch(() => ({ tags: [] })) : Promise.resolve({ tags: [] }),
        ]);
        if (disposedRef.current) return;
        setConfig(loadedConfig);
        if (REDACTION_LEVELS.includes(loadedConfig.defaultLevel as (typeof REDACTION_LEVELS)[number])) {
          setTargetLevel(loadedConfig.defaultLevel);
        }
        setNotebookTags(tagList.tags);
        const draft = pickLatestDraft(outbox);
        if (draft) {
          // 恢复未决确认门：draft 留档自带报告全量字段。
          setGate(gateViewFromRecord(draft));
          setGateCreatedAt(draft.createdAt);
          setGateStage({ kind: 'await' });
          setRemaining(remainingConfirmSeconds(draft.createdAt, loadedConfig.confirmTimeoutSecs));
          setQuestion(draft.question);
          setSelectedIds(new Set(draft.scopeEntryIds));
          setTargetLevel(draft.targetLevel);
        }
      } catch (error) {
        if (!disposedRef.current) setGateError(describeRedactionError(String(error)));
      }
    })();
    return () => {
      disposedRef.current = true;
    };
  }, [notebookId, notebookPath]);

  // 确认门决定（复刻审批卡片交互：responding 期间禁用、错误原位展示）。
  const decide = useCallback(
    async (approved: boolean, reason: 'user' | 'timeout' = 'user') => {
      if (!gate || decidingRef.current) return;
      decidingRef.current = true;
      setDeciding(true);
      setGateError(null);
      try {
        await redaction.confirm(notebookPath, gate.receiptId, approved);
        if (disposedRef.current) return;
        setGateStage(approved ? { kind: 'confirmed' } : { kind: 'cancelled', reason });
      } catch (error) {
        if (!disposedRef.current) setGateError(describeRedactionError(String(error)));
      } finally {
        decidingRef.current = false;
        setDeciding(false);
      }
    },
    [gate, notebookPath],
  );
  const decideRef = useRef(decide);
  decideRef.current = decide;

  // 确认门倒计时；到 0 自动按拒绝结算（与后端 confirm_wait 超时语义一致）。
  useEffect(() => {
    if (!gate || gateStage.kind !== 'await') return;
    const timer = window.setInterval(() => {
      const next = remainingConfirmSeconds(gateCreatedAt, confirmTimeoutSecs);
      setRemaining(next);
      if (next <= 0 && !decidingRef.current) {
        void decideRef.current(false, 'timeout');
      }
    }, 1000);
    return () => window.clearInterval(timer);
    // decideRef 是稳定的 ref，无需列入依赖。
  }, [gate, gateStage.kind, gateCreatedAt, confirmTimeoutSecs]);

  const runPreview = useCallback(async () => {
    if (previewing || !question.trim() || selectedIds.size === 0) return;
    setPreviewing(true);
    setGateError(null);
    try {
      const recipient = presetRecipient
        ? {
            agentId: presetRecipient.agentId,
            displayName: presetRecipient.displayName ?? '',
          }
        : recipientAgentId.trim()
          ? { agentId: recipientAgentId.trim(), displayName: '' }
          : undefined;
      // 发起身份：回程用预设专家；发起侧（有网络收件方时）用所选或第一个注册 agent，
      // 无注册则 local（对端无法回程，等价手动投递语义）。
      const senderAgent =
        presetSender?.agentId || senderAgentId || config?.registeredAgents?.[0];
      const sender = recipient && (presetSender || senderAgent)
        ? {
            agentId: presetSender?.agentId ?? senderAgent ?? '',
            displayName: presetSender?.displayName ?? '',
          }
        : undefined;
      const report = await redaction.preview(
        notebookPath,
        question.trim(),
        [...selectedIds],
        targetLevel,
        {
          sender,
          recipient,
          inReplyTo: replyTo?.receiptId,
          replyHopsRemaining:
            replyTo !== undefined ? Math.max(0, replyTo.hopsRemaining - 1) : undefined,
          blindRelay: blindRelay && recipient !== undefined,
        },
      );
      if (disposedRef.current) return;
      const createdAt = Date.now();
      setGate(gateViewFromReport(report));
      setGateCreatedAt(createdAt);
      setGateStage({ kind: 'await' });
      setRemaining(remainingConfirmSeconds(createdAt, confirmTimeoutSecs));
    } catch (error) {
      if (!disposedRef.current) setGateError(describeRedactionError(String(error)));
    } finally {
      setPreviewing(false);
    }
  }, [
    previewing,
    question,
    selectedIds,
    targetLevel,
    notebookPath,
    confirmTimeoutSecs,
    config,
    presetSender,
    presetRecipient,
    recipientAgentId,
    senderAgentId,
    blindRelay,
    replyTo,
  ]);

  const doSend = useCallback(async () => {
    if (!gate || sending || gateStage.kind !== 'confirmed') return;
    setSending(true);
    setGateError(null);
    setTransmitResult(null);
    try {
      const result = await redaction.send(notebookPath, gate.receiptId);
      if (disposedRef.current) return;
      setGateStage({ kind: 'sent', result });
    } catch (error) {
      if (!disposedRef.current) setGateError(describeRedactionError(String(error)));
    } finally {
      setSending(false);
    }
  }, [gate, gateStage.kind, sending, notebookPath]);

  // F5 客户端传输：中继已配置时，把 confirmed 信封投递到平台（可重试）。
  const doTransmit = useCallback(async () => {
    if (!gate || transmitting || gateStage.kind !== 'sent') return;
    setTransmitting(true);
    setGateError(null);
    try {
      const result = await redaction.transmit(notebookPath, gate.receiptId);
      if (disposedRef.current) return;
      setTransmitResult(result);
    } catch (error) {
      if (!disposedRef.current) setGateError(describeRedactionError(String(error)));
    } finally {
      setTransmitting(false);
    }
  }, [gate, gateStage.kind, transmitting, notebookPath]);

  const addTagFromNotebook = useCallback(async () => {
    const tagPath = tagDraft.trim();
    if (!tagPath) return;
    setTagDraft('');
    const result = await addTagScope(tagPath);
    if (result.ok) {
      setSelectedIds((current) => new Set(current).add(result.entry.id));
    } else if (result.code === 'already-tracked') {
      const existing = useAgentAccessStore
        .getState()
        .config.entries.find((entry) => entry.kind === 'tag' && entry.tagPath === tagPath);
      if (existing) setSelectedIds((current) => new Set(current).add(existing.id));
    } else {
      setGateError('标签范围保存失败，请重试。');
    }
  }, [tagDraft, addTagScope]);

  const toggleSelected = useCallback((id: string) => {
    setSelectedIds((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, []);

  const usableEntries = useMemo(
    () => entries.filter((entry) => entry.enabled && !entry.missing),
    [entries],
  );
  const dirEntries = usableEntries.filter((entry) => entry.kind !== 'tag');
  const tagEntries = usableEntries.filter((entry) => entry.kind === 'tag');
  const existingTagPaths = new Set(
    entries.filter((entry) => entry.kind === 'tag').map((entry) => entry.tagPath ?? ''),
  );
  const addableTags = notebookTags.filter((tag) => !existingTagPaths.has(tag.name));

  const recipients = useMemo(() => {
    const ids = gate
      ? gate.scopeEntryIds.length > 0
        ? gate.scopeEntryIds
        : [...selectedIds]
      : [];
    const byId = new Map(entries.map((entry) => [entry.id, entry]));
    return ids
      .map((id) => byId.get(id))
      .filter((entry): entry is AgentAccessEntry => Boolean(entry));
  }, [gate, selectedIds, entries]);

  const requestClose = useCallback(() => onClose(), [onClose]);

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        requestClose();
      }
    };
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [requestClose]);

  const canPreview = question.trim().length > 0 && selectedIds.size > 0 && !previewing;

  return (
    <div
      className="fixed inset-0 z-[135] flex items-center justify-center bg-black/50 p-3"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) requestClose();
      }}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label="脱敏发送确认门"
        className="flex h-[min(720px,calc(100vh-80px))] w-full max-w-[860px] flex-col overflow-hidden rounded-2xl bg-[var(--background)] shadow-2xl"
      >
        <header className="flex shrink-0 items-center gap-3 border-b border-[var(--divider)] px-5 py-3">
          <ShieldCheck className="h-5 w-5 shrink-0 text-[var(--primary)]" />
          <div className="min-w-0 flex-1">
            <h1 className="truncate text-base font-semibold tracking-[-0.02em] text-[var(--foreground)]">
              脱敏发送确认门
            </h1>
            <p className="truncate text-xs text-[var(--muted-foreground)]" title={notebookPath}>
              内容出设备前经 L0 取材 → L1 规则脱敏 → L2 蒸馏 → L3 复扫，确认后才可发送。
            </p>
          </div>
          <button
            type="button"
            aria-label="关闭"
            className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg text-[var(--muted-foreground)] transition-colors hover:bg-[var(--muted)] hover:text-[var(--foreground)]"
            onClick={requestClose}
          >
            <X className="h-4 w-4" strokeWidth={1.8} />
          </button>
        </header>

        <div className="min-h-0 flex-1 overflow-y-auto px-5 py-4">
          {gateError && <div className="mb-3"><InlineError message={gateError} /></div>}

          {!gate && (
            <div className="space-y-4">
              <label className="block space-y-1.5">
                <span className="text-xs font-medium text-[var(--foreground)]">要咨询的问题</span>
                <textarea
                  className={cn(inputClass, 'min-h-24 resize-y leading-6')}
                  value={question}
                  onChange={(event) => setQuestion(event.target.value)}
                  placeholder="例如：根据财报笔记，近一季复购下滑可能和什么有关？"
                />
              </label>

              {replyTo ? (
                <div className="flex items-center gap-2 rounded-lg border border-[var(--border)] bg-[color-mix(in_oklch,var(--primary)_6%,var(--background))] px-3 py-2 text-xs text-[var(--muted-foreground)]">
                  <Reply className="h-3.5 w-3.5 shrink-0 text-[var(--primary)]" />
                  <span className="min-w-0 flex-1 truncate">
                    回程信封 → {presetRecipient?.displayName || presetRecipient?.agentId || '原发起方'}
                    （inReplyTo {replyTo.receiptId}，剩余跳数 {Math.max(0, replyTo.hopsRemaining - 1)}）
                  </span>
                </div>
              ) : (
                <div className="space-y-3">
                  <label className="block space-y-1.5">
                    <span className="text-xs font-medium text-[var(--foreground)]">
                      收件方 Agent ID（可选，F5 网络咨询必填）
                    </span>
                    <input
                      className={inputClass}
                      value={presetRecipient ? `${presetRecipient.displayName || ''} ${presetRecipient.agentId}`.trim() : recipientAgentId}
                      readOnly={Boolean(presetRecipient)}
                      onChange={(event) => setRecipientAgentId(event.target.value)}
                      placeholder="agt_…（留空 = 手动投递）"
                    />
                  </label>
                  {(config?.registeredAgents?.length ?? 0) > 0 && (() => {
                    const agents = config?.registeredAgents ?? [];
                    return (
                      <label className="block space-y-1.5">
                        <span className="text-xs font-medium text-[var(--foreground)]">
                          发起身份（以哪个注册 Agent 的名义发出）
                        </span>
                        <select
                          className={inputClass}
                          value={senderAgentId || agents[0] || ''}
                          onChange={(event) => setSenderAgentId(event.target.value)}
                        >
                          {agents.map((agent) => (
                            <option key={agent} value={agent}>{agent}</option>
                          ))}
                        </select>
                      </label>
                    );
                  })()}
                  <label className="flex items-start gap-2 rounded-lg border border-[var(--border)] px-3 py-2 text-xs leading-5 text-[var(--muted-foreground)]">
                    <input
                      type="checkbox"
                      data-blind-relay
                      className="mt-0.5"
                      checked={blindRelay}
                      onChange={(event) => setBlindRelay(event.target.checked)}
                    />
                    <span>
                      盲转加密（P2-9）：payload 端到端加密，中继只见密文。收件方须已发布
                      目录卡（含公钥）；确认通过时加密固化，披露标记与溯源仍为明文。
                    </span>
                  </label>
                </div>
              )}

              <div className="space-y-1.5">
                <span className="text-xs font-medium text-[var(--foreground)]">目标脱敏等级</span>
                <div className="grid grid-cols-3 gap-2">
                  {REDACTION_LEVELS.map((level) => (
                    <button
                      key={level}
                      type="button"
                      onClick={() => setTargetLevel(level)}
                      className={cn(
                        'min-h-10 rounded-lg border px-3 text-left text-sm transition-colors',
                        targetLevel === level
                          ? 'border-[var(--primary)] bg-[color-mix(in_oklch,var(--primary)_9%,var(--background))] text-[var(--foreground)]'
                          : 'border-[var(--border)] text-[var(--muted-foreground)] hover:bg-[var(--muted)]',
                      )}
                    >
                      {REDACTION_LEVEL_LABELS[level]}
                    </button>
                  ))}
                </div>
              </div>

              <div className="space-y-2">
                <div className="flex items-center justify-between gap-3">
                  <span className="text-xs font-medium text-[var(--foreground)]">
                    披露范围（收件方可见内容的来源）
                  </span>
                  <span className="text-xs text-[var(--muted-foreground)]">
                    已选 {selectedIds.size} 项
                  </span>
                </div>
                <div className="flex items-center gap-1" role="tablist" aria-label="披露范围类型">
                  {([
                    ['dirs', '目录'],
                    ['tags', '标签'],
                  ] as const).map(([key, label]) => (
                    <button
                      key={key}
                      type="button"
                      role="tab"
                      aria-selected={scopeTab === key}
                      onClick={() => setScopeTab(key)}
                      className={cn(
                        'h-7 rounded-lg px-2.5 text-xs outline-none transition-colors',
                        scopeTab === key
                          ? 'bg-[var(--muted)] font-medium text-[var(--foreground)]'
                          : 'text-[var(--muted-foreground)] hover:bg-[var(--muted)] hover:text-[var(--foreground)]',
                      )}
                    >
                      {label}
                    </button>
                  ))}
                </div>

                {scopeTab === 'dirs' ? (
                  <div className="space-y-2">
                    {dirEntries.length === 0 && (
                      <p className="rounded-lg border border-[var(--border)] px-3 py-4 text-center text-xs text-[var(--muted-foreground)]">
                        暂无可用的笔记本 / 文件夹授权。
                      </p>
                    )}
                    {dirEntries.map((entry) => (
                      <ScopeRow
                        key={entry.id}
                        label={entry.name}
                        detail={entry.path}
                        checked={selectedIds.has(entry.id)}
                        onToggle={() => toggleSelected(entry.id)}
                      />
                    ))}
                  </div>
                ) : (
                  <div className="space-y-2">
                    {tagEntries.map((entry) => (
                      <ScopeRow
                        key={entry.id}
                        label={entry.name}
                        detail={`${entry.tagPath ?? ''}（该标签及其子树）`}
                        checked={selectedIds.has(entry.id)}
                        onToggle={() => toggleSelected(entry.id)}
                      />
                    ))}
                    {tagEntries.length === 0 && (
                      <p className="rounded-lg border border-[var(--border)] px-3 py-3 text-center text-xs text-[var(--muted-foreground)]">
                        还没有标签级授权，从下方添加。
                      </p>
                    )}
                    <div className="flex items-center gap-2">
                      <select
                        className={cn(inputClass, 'h-9 py-0')}
                        value={tagDraft}
                        onChange={(event) => setTagDraft(event.target.value)}
                        aria-label="选择笔记本标签"
                      >
                        <option value="">选择笔记本标签…</option>
                        {addableTags.map((tag) => (
                          <option key={tag.id} value={tag.name}>
                            {tag.name}
                          </option>
                        ))}
                      </select>
                      <Button
                        type="button"
                        variant="outline"
                        className="h-9 shrink-0 gap-1 rounded-lg"
                        disabled={!tagDraft.trim()}
                        onClick={() => void addTagFromNotebook()}
                      >
                        <Plus className="h-3.5 w-3.5" />
                        添加
                      </Button>
                    </div>
                  </div>
                )}
              </div>
            </div>
          )}

          {gate && (
            <div className="space-y-4">
              <div className="flex flex-wrap items-center gap-2 text-xs">
                <span className="rounded-full bg-[color-mix(in_oklch,var(--primary)_12%,var(--card))] px-2.5 py-1 font-medium text-[var(--primary)]">
                  {REDACTION_LEVEL_LABELS[gate.targetLevel] ?? gate.targetLevel}
                </span>
                {gateStage.kind === 'await' && (
                  <span className="tabular-nums text-[var(--muted-foreground)]">
                    剩余确认时间 {Math.max(0, remaining ?? 0)} 秒，超时自动取消
                  </span>
                )}
                {gate.truncated && (
                  <span className="text-[var(--destructive)]">载荷已达上限，内容被截断</span>
                )}
                {gate.usedFallback && (
                  <span className="text-[var(--muted-foreground)]">
                    蒸馏失败，R3 纯规则兜底稿
                  </span>
                )}
                <span className="ml-auto font-mono text-[10px] text-[var(--muted-foreground)]">
                  {gate.receiptId}
                </span>
              </div>

              <section>
                <h2 className="mb-1.5 text-xs font-medium text-[var(--foreground)]">收件方（披露范围来源）</h2>
                <div className="flex flex-wrap gap-2">
                  {recipients.length === 0 && (
                    <span className="text-xs text-[var(--muted-foreground)]">（范围条目已不可解析）</span>
                  )}
                  {recipients.map((entry) => (
                    <span
                      key={entry.id}
                      className="max-w-full truncate rounded-lg border border-[var(--border)] px-2.5 py-1.5 text-xs text-[var(--foreground)]"
                      title={entry.kind === 'tag' ? entry.tagPath ?? '' : entry.path}
                    >
                      {entry.kind === 'tag' ? `# ${entry.tagPath ?? entry.name}` : entry.name}
                    </span>
                  ))}
                </div>
              </section>

              <section>
                <h2 className="mb-1.5 text-xs font-medium text-[var(--foreground)]">
                  最终载荷全文（逐字确认后才会出设备）
                </h2>
                <pre className="max-h-64 overflow-auto whitespace-pre-wrap break-words rounded-xl border border-[var(--border)] bg-[color-mix(in_oklch,var(--card)_82%,var(--background))] px-4 py-3 font-mono text-xs leading-5 text-[var(--foreground)]">
                  {gate.finalText}
                </pre>
              </section>

              <section className="grid gap-3 sm:grid-cols-2">
                <div className="rounded-xl border border-[var(--border)] px-3 py-2.5">
                  <h3 className="text-xs font-medium text-[var(--foreground)]">
                    占位符映射（{Object.keys(gate.placeholderMap).length} 项，仅存本地）
                  </h3>
                  <ul className="mt-1.5 max-h-32 space-y-1 overflow-auto text-xs text-[var(--muted-foreground)]">
                    {Object.entries(gate.placeholderMap).map(([placeholder, original]) => (
                      <li key={placeholder} className="flex gap-2">
                        <code className="text-[var(--foreground)]">{placeholder}</code>
                        <span className="truncate">← {original}</span>
                      </li>
                    ))}
                    {Object.keys(gate.placeholderMap).length === 0 && <li>无占位符</li>}
                  </ul>
                </div>
                <div className="rounded-xl border border-[var(--border)] px-3 py-2.5">
                  <h3 className="text-xs font-medium text-[var(--foreground)]">
                    规则命中（{gate.l1Hits.length} 类）
                  </h3>
                  <ul className="mt-1.5 max-h-32 space-y-1 overflow-auto text-xs text-[var(--muted-foreground)]">
                    {gate.l1Hits.map((hit) => (
                      <li key={hit.ruleId}>
                        {hit.ruleId} × {hit.count}
                      </li>
                    ))}
                    {gate.l1Hits.length === 0 && <li>无规则命中</li>}
                  </ul>
                  <p className="mt-1.5 text-xs text-[var(--muted-foreground)]">
                    纳入 {gate.includedPaths.length} 条材料 · 越界丢弃 {gate.droppedPaths.length} 条 ·
                    复扫 {gate.attempts} 次
                  </p>
                </div>
              </section>

              {gateStage.kind === 'cancelled' && (
                <div className="rounded-xl border border-[var(--border)] bg-[color-mix(in_oklch,var(--muted)_40%,var(--background))] px-4 py-3 text-sm text-[var(--muted-foreground)]">
                  {gateStage.reason === 'timeout'
                    ? '确认超时，本次发送已取消并留档。'
                    : '已取消，本次发送留档为 cancelled。'}
                </div>
              )}
              {gateStage.kind === 'sent' && (
                <div className="rounded-xl border border-[color-mix(in_oklch,var(--primary)_35%,var(--border))] bg-[color-mix(in_oklch,var(--primary)_8%,var(--background))] px-4 py-3 text-sm text-[var(--foreground)]">
                  信封已从唯一出口取出（{gateStage.result.byteLength} 字节）。网络互咨传输层（F5）接入后将从同一出口发出。
                  {transmitResult && (
                    <span className="mt-1 block text-xs text-[var(--muted-foreground)]">
                      已投递到中继（HTTP {transmitResult.httpStatus}，{transmitResult.byteLength} 字节）。
                    </span>
                  )}
                </div>
              )}
            </div>
          )}
        </div>

        <footer className="flex shrink-0 items-center justify-between gap-3 border-t border-[var(--divider)] bg-[color-mix(in_oklch,var(--background)_92%,var(--card))] px-5 py-3">
          <div className="min-w-0 truncate text-xs text-[var(--muted-foreground)]">
            {gate ? `规则版本 ${gate.ruleSetVersion}` : '生成预览会跑一次本地蒸馏会话，可能耗时数十秒'}
          </div>
          <div className="flex items-center gap-2">
            {gate && gateStage.kind === 'await' && (
              <>
                <Button type="button" variant="ghost" disabled={deciding} onClick={() => void decide(false, 'user')}>
                  取消发送
                </Button>
                <Button
                  type="button"
                  disabled={deciding}
                  onClick={() => void decide(true)}
                >
                  {deciding ? <Loader2 className="animate-spin" /> : <ShieldCheck />}
                  确认内容并解锁发送
                </Button>
              </>
            )}
            {gate && gateStage.kind === 'confirmed' && (
              <>
                <Button type="button" variant="ghost" onClick={requestClose}>
                  稍后发送
                </Button>
                <Button type="button" disabled={sending} onClick={() => void doSend()}>
                  {sending ? <Loader2 className="animate-spin" /> : <Send />}
                  发送
                </Button>
              </>
            )}
            {gate && (gateStage.kind === 'cancelled' || gateStage.kind === 'sent') && (
              <>
                {gateStage.kind === 'cancelled' && (
                  <Button
                    type="button"
                    variant="outline"
                    onClick={() => {
                      setGate(null);
                      setGateStage({ kind: 'await' });
                      setRemaining(null);
                    }}
                  >
                    <RotateCcw />
                    返回修改
                  </Button>
                )}
                {gateStage.kind === 'sent' && (config?.relayUrl ?? '').trim() !== '' && (
                  <Button
                    type="button"
                    variant="outline"
                    disabled={transmitting}
                    onClick={() => void doTransmit()}
                  >
                    {transmitting ? <Loader2 className="animate-spin" /> : <Send />}
                    {transmitResult ? '重试投递' : '投递到中继'}
                  </Button>
                )}
                <Button type="button" variant="ghost" onClick={requestClose}>
                  关闭
                </Button>
              </>
            )}
            {!gate && (
              <>
                <Button type="button" variant="ghost" onClick={requestClose}>
                  取消
                </Button>
                <Button type="button" disabled={!canPreview} onClick={() => void runPreview()}>
                  {previewing ? <Loader2 className="animate-spin" /> : <ShieldCheck />}
                  生成脱敏预览
                </Button>
              </>
            )}
          </div>
        </footer>
      </div>
    </div>
  );
}

export class RedactionSendGateDialogController {
  private container: HTMLDivElement | null = null;
  private root: Root | null = null;

  open(notebookId: string, notebookPath: string): void {
    this.close();
    const container = document.createElement('div');
    container.dataset.redactionSendGateDialog = 'true';
    document.body.append(container);
    this.container = container;
    this.root = createRoot(container);
    this.root.render(
      <RedactionSendGateDialog
        notebookId={notebookId}
        notebookPath={notebookPath}
        onClose={() => this.close()}
      />,
    );
  }

  close(): void {
    this.root?.unmount();
    this.root = null;
    this.container?.remove();
    this.container = null;
  }
}
