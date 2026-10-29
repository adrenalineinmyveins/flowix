'use client';

import { useCallback, useEffect, useRef, useState } from 'react';
import { Bot, Fingerprint, Inbox, Loader2, Plus, Save, Store, Trash2, X } from 'lucide-react';

import { agentProfile, market, redaction, tags as tagsClient } from '@platform/tauri/client';
import type { MarketMyCard } from '@platform/tauri/client';
import type { RelayAccountInfo } from '@/lib/types/redaction';
import { PublishCardDialog } from '@features/agent/components/publish-card-dialog';
import { AgentMarket } from '@features/agent/components/agent-market';
import type { AgentProfile, AgentState } from '@/lib/types/agent-profile';
import { PROFILE_REDACTION_LEVELS } from '@/lib/types/agent-profile';
import { AGENT_TYPES } from '@/lib/agent-types';
import { useNoteStore } from '@features/memo/store/note-store';
import { AgentProfileInbox } from '@features/agent/components/agent-profile-inbox';
import { MEMO_LIST_VIEW_HEADER_INSET_CLASS } from '@features/memo/components/memo-list-view-tabs';
import { WorkColumnTitlebarShell } from '@features/shell/components/work-column-titlebar-shell';
import { SidebarToggleIcon } from '@shared/icons/sidebar-toggle-icon';
import { DOCUMENT_TITLEBAR_ICON_BUTTON_MAC, DOCUMENT_TITLEBAR_ICON_BUTTON_WIN } from '@features/document/components/document-titlebar-shared';
import { isWindowsPlatform } from '@features/shortcuts';
import { Button } from '@shared/ui/button';
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription } from '@shared/ui/dialog';
import { OverlayScrollbar } from '@shared/ui/overlay-scrollbar';
import { ListSurfaceLoadingState } from '@shared/ui/list-surface';
import { useI18n, type I18nKey } from '@/lib/i18n';
import { cn } from '@/lib/utils';

/** 披露等级下拉文案走 i18n。 */
const LEVEL_LABEL_KEYS: Record<string, I18nKey> = {
  '': 'agent.profile.level.unset',
  R1: 'agent.profile.level.r1',
  R2: 'agent.profile.level.r2',
  R3: 'agent.profile.level.r3',
};

/** AGENT-MARKET M1：发布态徽章文案键。 */
const PUBLISH_STATE_KEYS: Record<string, I18nKey> = {
  draft: 'agent.market.state.draft',
  confirmed: 'agent.market.state.confirmed',
  published: 'agent.market.state.published',
};

const inputClass =
  'h-9 w-full rounded-lg border border-[var(--border)] bg-[var(--background)] px-3 text-sm text-[var(--foreground)] outline-none transition-colors placeholder:text-[var(--muted-foreground)] focus:border-[var(--primary)] focus:ring-2 focus:ring-[color-mix(in_oklch,var(--primary)_16%,transparent)]';
const textareaClass =
  'min-h-20 w-full resize-y rounded-lg border border-[var(--border)] bg-[var(--background)] px-3 py-2.5 text-sm leading-6 text-[var(--foreground)] outline-none transition-colors placeholder:text-[var(--muted-foreground)] focus:border-[var(--primary)] focus:ring-2 focus:ring-[color-mix(in_oklch,var(--primary)_16%,transparent)]';

interface ProfileForm {
  displayName: string;
  bio: string;
  agentType: string;
  expertise: string;
  model: string;
  consultable: boolean;
  redactionLevel: string;
  /** AGENT-PROFILE-REDESIGN P3：扩展字段（memory/tools/skills 为条目数组）。 */
  system: string;
  tools: string[];
  skills: string[];
  metadata: string;
  memory: string[];
}

function formFromProfile(profile: AgentProfile): ProfileForm {
  return {
    displayName: profile.flowix_display_name,
    bio: profile.flowix_bio,
    agentType: profile.flowix_agent_type,
    expertise: profile.flowix_expertise.join('\n'),
    model: profile.flowix_model ?? '',
    consultable: profile.flowix_consultable,
    redactionLevel: profile.flowix_redaction_level ?? '',
    system: profile.flowix_system ?? '',
    tools: [...(profile.flowix_tools ?? [])],
    skills: [...(profile.flowix_skills ?? [])],
    metadata: Object.entries(profile.flowix_metadata ?? {})
      .map(([key, value]) => `${key}=${value}`)
      .join('\n'),
    memory: [...(profile.flowix_memory ?? [])],
  };
}

/** metadata 行编辑：`key=value` 每行一对。
 * 值含 `=` 时以首个为界；缺 `=` 的行丢弃。 */
function parseMetadata(raw: string): Record<string, string> {
  const map: Record<string, string> = {};
  for (const line of parseLines(raw)) {
    const eq = line.indexOf('=');
    if (eq <= 0) continue;
    map[line.slice(0, eq).trim()] = line.slice(eq + 1);
  }
  return map;
}

function parseLines(raw: string): string[] {
  return raw
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);
}

/** 追加条目（去空白、去重），返回新数组。 */
function appendItem(items: string[], raw: string): string[] {
  const value = raw.trim();
  if (!value || items.includes(value)) return items;
  return [...items, value];
}

/** chips 容器 + 单个 chip 的删除按钮（memory/tools/skills 共用）。 */
function ChipRow({ items, onRemove }: { items: string[]; onRemove: (item: string) => void }) {
  if (items.length === 0) return null;
  return (
    <div className="flex flex-wrap gap-1.5">
      {items.map((item) => (
        <span
          key={item}
          className="inline-flex items-center gap-1 rounded-full border border-[var(--border)] bg-[var(--muted)] px-2.5 py-0.5 font-mono text-xs text-[var(--foreground)]"
        >
          {item}
          <button
            type="button"
            className="text-[var(--muted-foreground)] transition-colors hover:text-[var(--destructive)]"
            onClick={() => onRemove(item)}
            aria-label={`remove ${item}`}
          >
            <X className="h-3 w-3" />
          </button>
        </span>
      ))}
    </div>
  );
}

/** 自由 chips 编辑器（tools/skills）：输入回车追加，chip 可删。 */
function ChipsEditor({ items, onChange, placeholder }: { items: string[]; onChange: (items: string[]) => void; placeholder?: string }) {
  const [draft, setDraft] = useState('');
  const commit = () => {
    if (!draft.trim()) return;
    onChange(appendItem(items, draft));
    setDraft('');
  };
  return (
    <div className="space-y-2">
      <ChipRow items={items} onRemove={(item) => onChange(items.filter((value) => value !== item))} />
      <input
        className={inputClass}
        value={draft}
        placeholder={placeholder}
        onChange={(event) => setDraft(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === 'Enter') {
            event.preventDefault();
            commit();
          }
        }}
        onBlur={commit}
      />
    </div>
  );
}

/**
 * 记忆库编辑器（AGENT-PROFILE-REDESIGN P3 迭代）：
 * tag 项从笔记本标签下拉选取（`tag:<path>`）；note:/collection: 与清单外
 * tag 走手输。已选项 chips 可删。UI 全权——扩缩均放行（UI 即人审）。
 */
function MemoryEditor({
  items,
  onChange,
  notebookTags,
  pickLabel,
  manualPlaceholder,
  emptyLabel,
}: {
  items: string[];
  onChange: (items: string[]) => void;
  notebookTags: { id: string; name: string }[];
  pickLabel: string;
  manualPlaceholder?: string;
  emptyLabel?: string;
}) {
  const [draft, setDraft] = useState('');
  const commit = () => {
    if (!draft.trim()) return;
    onChange(appendItem(items, draft));
    setDraft('');
  };
  const selectedTagPaths = new Set(items.filter((item) => item.startsWith('tag:')).map((item) => item.slice('tag:'.length)));
  const addableTags = notebookTags.filter((tag) => !selectedTagPaths.has(tag.name));
  return (
    <div className="space-y-2">
      <ChipRow items={items} onRemove={(item) => onChange(items.filter((value) => value !== item))} />
      <div className="flex gap-2">
        <select
          className={cn(inputClass, 'py-0')}
          value=""
          onChange={(event) => {
            const name = event.target.value;
            if (name) onChange(appendItem(items, `tag:${name}`));
          }}
          disabled={addableTags.length === 0}
        >
          <option value="">{addableTags.length === 0 ? (emptyLabel ?? pickLabel) : pickLabel}</option>
          {addableTags.map((tag) => (
            <option key={tag.id} value={tag.name}>
              {tag.name}
            </option>
          ))}
        </select>
        <input
          className={inputClass}
          value={draft}
          placeholder={manualPlaceholder}
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter') {
              event.preventDefault();
              commit();
            }
          }}
          onBlur={commit}
        />
      </div>
    </div>
  );
}

function Field({ label, hint, children }: { label: string; hint?: string; children: React.ReactNode }) {
  return (
    <label className="block space-y-1.5">
      <span className="flex items-baseline justify-between gap-3 text-xs font-medium text-[var(--foreground)]">
        <span>{label}</span>
        {hint && <span className="font-normal text-[var(--muted-foreground)]">{hint}</span>}
      </span>
      {children}
    </label>
  );
}

function Toggle({ checked, onChange, label }: { checked: boolean; onChange: (value: boolean) => void; label: string }) {
  return (
    <label className="inline-flex cursor-pointer items-center gap-2 text-sm text-[var(--muted-foreground)]">
      <input
        type="checkbox"
        checked={checked}
        onChange={(event) => onChange(event.target.checked)}
        aria-label={label}
        className="peer sr-only"
      />
      <span className="relative h-5 w-9 rounded-full bg-[var(--muted)] transition-colors peer-checked:bg-[var(--primary)] peer-focus-visible:ring-2 peer-focus-visible:ring-[color-mix(in_oklch,var(--primary)_28%,transparent)] after:absolute after:left-0.5 after:top-0.5 after:h-4 after:w-4 after:rounded-full after:bg-[var(--card)] after:shadow-sm after:transition-transform peer-checked:after:translate-x-4" aria-hidden="true" />
      <span>{label}</span>
    </label>
  );
}

function InlineError({ message }: { message: string }) {
  return (
    <div className="flex items-start gap-2 rounded-lg border border-[color-mix(in_oklch,var(--destructive)_24%,var(--divider))] bg-[color-mix(in_oklch,var(--destructive)_6%,transparent)] px-3 py-2 text-xs leading-5 text-[var(--destructive)]">
      {message}
    </div>
  );
}

function StateChip({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-lg border border-[var(--border)] px-3 py-2">
      <div className="text-[11px] text-[var(--muted-foreground)]">{label}</div>
      <div className="mt-0.5 font-mono text-sm text-[var(--foreground)]">{value}</div>
    </div>
  );
}

/**
 * 中列「个人 Agent」列表：标题行 + 新建入口 + 身份卡列表。
 * 点击卡片在右列打开编辑页（store: agentProfileViewTarget）。
 */
export function AgentProfileListPanel({ notebookPath }: { notebookPath: string }) {
  const { t } = useI18n();
  const [profiles, setProfiles] = useState<AgentProfile[] | null>(null);
  /** AGENT-MARKET M1：发布态徽章数据（agentId → 状态）。 */
  const [myCards, setMyCards] = useState<Record<string, MarketMyCard>>({});
  const [loading, setLoading] = useState(true);
  const target = useNoteStore((state) => state.agentProfileViewTarget);
  const version = useNoteStore((state) => state.agentProfilesVersion);
  const disposedRef = useRef(false);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [list, cards] = await Promise.all([
        agentProfile.list(notebookPath),
        market.myCards(notebookPath).catch(() => [] as MarketMyCard[]),
      ]);
      if (disposedRef.current) return;
      setProfiles(list);
      setMyCards(Object.fromEntries(cards.map((card) => [card.agentId, card])));
    } catch {
      if (!disposedRef.current) setProfiles([]);
    } finally {
      if (!disposedRef.current) setLoading(false);
    }
  }, [notebookPath]);

  useEffect(() => {
    disposedRef.current = false;
    void load();
    return () => {
      disposedRef.current = true;
    };
  }, [load, version]);

  const setTarget = useNoteStore((state) => state.setAgentProfileViewTarget);
  const activeAgentId = target && 'agentId' in target ? target.agentId : null;
  /** AGENT-MARKET D-1：市场入口默认选中（target 为 null 或 market）。 */
  const marketActive = !target || 'market' in target;

  return (
    <div className="flex h-full min-h-0 flex-col bg-[var(--list-bg)]">
      {/* 中列统一头部行：标题避让左上角悬浮的视图切换器。 */}
      <div className={`flex shrink-0 items-center justify-between gap-2 pb-2 pr-3 ${MEMO_LIST_VIEW_HEADER_INSET_CLASS}`}>
        <span className="min-w-0 truncate text-[15px] font-medium text-[var(--foreground)]">
          {t('agent.profile.title')}
        </span>
        <Button
          type="button"
          variant="outline"
          className="h-7 shrink-0 gap-1 rounded-lg px-2 text-xs"
          onClick={() => setTarget({ creating: true })}
        >
          <Plus className="h-3.5 w-3.5" />
          {t('agent.profile.newCard')}
        </Button>
        <Button
          type="button"
          variant="ghost"
          className="h-7 shrink-0 gap-1 rounded-lg px-2 text-xs"
          onClick={() => setTarget({ inbox: true })}
        >
          <Inbox className="h-3.5 w-3.5" />
          {t('agent.inbox.title')}
        </Button>
      </div>

      <div className="relative min-h-0 flex-1">
        {loading && !profiles ? (
          <ListSurfaceLoadingState label={t('agent.profile.loading')} className="h-full w-full" />
        ) : (
          <OverlayScrollbar className="h-full" scrollerClassName="h-full overflow-y-auto px-1.5 py-1">
            {!loading && profiles?.length === 0 && (
              <p className="px-3 py-6 text-center text-xs leading-5 text-[var(--muted-foreground)]">
                {t('agent.profile.empty')}
              </p>
            )}
            <div className="space-y-1">
              {/* AGENT-MARKET D-1：市场入口（默认选中，中列显示市场主页）。 */}
              <button
                type="button"
                onClick={() => setTarget({ market: true })}
                className={cn(
                  'flex w-full items-center gap-1.5 rounded-lg px-2.5 py-2 text-left outline-none transition-colors',
                  marketActive
                    ? 'bg-[var(--muted)]'
                    : 'hover:bg-[color-mix(in_oklch,var(--muted)_50%,transparent)]',
                )}
              >
                <Store className="h-3.5 w-3.5 shrink-0 text-[var(--brand)]" />
                <span className="min-w-0 flex-1 truncate text-sm font-medium text-[var(--foreground)]">
                  {t('agent.market.entry')}
                </span>
              </button>
              {profiles?.map((profile) => {
                const active = profile.flowix_agent_id === activeAgentId;
                return (
                  <button
                    key={profile.flowix_agent_id}
                    type="button"
                    onClick={() => setTarget({ agentId: profile.flowix_agent_id })}
                    className={cn(
                      'flex w-full flex-col gap-1 rounded-lg px-2.5 py-2 text-left outline-none transition-colors',
                      active
                        ? 'bg-[var(--muted)]'
                        : 'hover:bg-[color-mix(in_oklch,var(--muted)_50%,transparent)]',
                    )}
                  >
                    <span className="flex w-full items-center gap-1.5">
                      <Bot className="h-3.5 w-3.5 shrink-0 text-[var(--muted-foreground)]" />
                      <span className="min-w-0 flex-1 truncate text-sm font-medium text-[var(--foreground)]">
                        {profile.flowix_display_name}
                      </span>
                      <span
                        className={cn(
                          'h-1.5 w-1.5 shrink-0 rounded-full',
                          profile.flowix_consultable ? 'bg-[var(--primary)]' : 'bg-[var(--muted-foreground)]',
                        )}
                        title={profile.flowix_consultable ? t('agent.profile.consultableOn') : t('agent.profile.consultableOff')}
                        aria-hidden="true"
                      />
                    </span>
                    <span className="truncate text-[11px] text-[var(--muted-foreground)]">
                      {AGENT_TYPES.find((type) => type.key === profile.flowix_agent_type)?.name ?? profile.flowix_agent_type}
                      {profile.flowix_expertise.length > 0 && ` · ${profile.flowix_expertise.join(' / ')}`}
                    </span>
                    {(() => {
                      // AGENT-MARKET M1：发布态徽章（none 不占位）。
                      const card = myCards[profile.flowix_agent_id];
                      if (!card || card.publishState === 'none') return null;
                      const unpublished = card.publishState === 'published' && card.visibility === 'private';
                      const label = unpublished
                        ? t('agent.market.stateUnpublished')
                        : card.drifted
                          ? t('agent.market.stateDrifted')
                          : t(PUBLISH_STATE_KEYS[card.publishState] ?? 'agent.market.state.published');
                      return (
                        <span
                          className={cn(
                            'w-fit rounded px-1 py-px text-[10px] leading-4',
                            card.drifted || card.publishState === 'draft' || card.publishState === 'confirmed'
                              ? 'bg-[color-mix(in_oklch,var(--primary)_14%,transparent)] text-[var(--primary)]'
                              : 'bg-[var(--muted)] text-[var(--muted-foreground)]',
                          )}
                        >
                          {label}
                        </span>
                      );
                    })()}
                  </button>
                );
              })}
            </div>
          </OverlayScrollbar>
        )}
      </div>
    </div>
  );
}

/**
 * 右列「个人 Agent」标题栏：图标 + 视图标题 + 关闭。
 */
export function AgentProfileTitlebar({
  reserveWindowsControls,
  isSidebarCollapsed,
  onExpandSidebar,
}: {
  reserveWindowsControls: boolean;
  isSidebarCollapsed: boolean;
  onExpandSidebar: () => void;
}) {
  const { t } = useI18n();
  const isWindows = isWindowsPlatform();
  const target = useNoteStore((state) => state.agentProfileViewTarget);
  const setTarget = useNoteStore((state) => state.setAgentProfileViewTarget);
  const iconButtonClass = isWindows ? DOCUMENT_TITLEBAR_ICON_BUTTON_WIN : DOCUMENT_TITLEBAR_ICON_BUTTON_MAC;

  return (
    <WorkColumnTitlebarShell isWindows={isWindows} reserveWindowsControls={reserveWindowsControls}>
      {isSidebarCollapsed && (
        <button
          type="button"
          onClick={onExpandSidebar}
          aria-label={t('document.titlebar.showSidebar')}
          title={t('document.titlebar.showSidebarTooltip')}
          className={cn(iconButtonClass, 'mr-1')}
        >
          <SidebarToggleIcon className="h-4 w-4" />
        </button>
      )}
      <Fingerprint className="ml-1 h-4 w-4 shrink-0 text-[var(--brand)]" />
      <span className="ml-2 min-w-0 truncate text-sm font-medium text-[var(--foreground)]">
        {target && 'creating' in target
          ? t('agent.profile.createTitle')
          : target && 'inbox' in target
            ? t('agent.inbox.title')
            : target && 'market' in target
              ? t('agent.market.title')
              : t('agent.profile.title')}
      </span>
      <button
        type="button"
        onClick={() => setTarget(null)}
        aria-label={t('agent.profile.close')}
        title={t('agent.profile.close')}
        className={cn(iconButtonClass, 'ml-auto')}
      >
        <X className="h-4 w-4" />
      </button>
    </WorkColumnTitlebarShell>
  );
}

/**
 * 右列「个人 Agent」编辑页：创建表单 / 身份卡编辑（基本信息、隐私边界、运行状态）。
 * 目标来自 note-store（agentProfileViewTarget）；列表数据版本由 agentProfilesVersion 联动。
 */
export function AgentProfileEditor({ notebookPath }: { notebookPath: string }) {
  const { t } = useI18n();
  const target = useNoteStore((state) => state.agentProfileViewTarget);
  const setTarget = useNoteStore((state) => state.setAgentProfileViewTarget);
  const bumpProfiles = useNoteStore((state) => state.bumpAgentProfiles);

  const [profile, setProfile] = useState<AgentProfile | null>(null);
  const [form, setForm] = useState<ProfileForm | null>(null);
  const [agentState, setAgentState] = useState<AgentState | null>(null);
  const [createForm, setCreateForm] = useState({ displayName: '', agentType: 'codex' });
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<{ id: string; name: string } | null>(null);
  const disposedRef = useRef(false);

  // AGENT-MARKET M1：发布管理（状态 + 账号 + 下架中）。
  const [myCard, setMyCard] = useState<MarketMyCard | null>(null);
  const [account, setAccount] = useState<RelayAccountInfo | null>(null);
  const [publishOpen, setPublishOpen] = useState(false);
  const [unpublishing, setUnpublishing] = useState(false);
  const [unpublishConfirm, setUnpublishConfirm] = useState(false);

  const agentId = target && 'agentId' in target ? target.agentId : null;

  // 发布态 + 中继账号（未登录时发布按钮给出引导文案）。
  const reloadMyCard = useCallback(async () => {
    try {
      const cards = await market.myCards(notebookPath);
      if (disposedRef.current) return;
      setMyCard(agentId ? cards.find((card) => card.agentId === agentId) ?? null : null);
    } catch {
      if (!disposedRef.current) setMyCard(null);
    }
  }, [notebookPath, agentId]);

  useEffect(() => {
    void reloadMyCard();
    void redaction.accountStatus()
      .then((status) => {
        if (!disposedRef.current) setAccount(status);
      })
      .catch(() => undefined);
  }, [reloadMyCard]);

  const unpublish = useCallback(async () => {
    if (!agentId || unpublishing) return;
    setUnpublishing(true);
    setError(null);
    try {
      await market.unpublish(notebookPath, agentId);
      setUnpublishConfirm(false);
      await reloadMyCard();
      bumpProfiles();
    } catch (cause) {
      if (!disposedRef.current) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setUnpublishing(false);
    }
  }, [agentId, unpublishing, notebookPath, reloadMyCard, bumpProfiles]);

  // 记忆库下拉数据源：当前笔记本标签（path → id 匹配，取不到则为空）。
  const notebooks = useNoteStore((state) => state.notebooks);
  const [notebookTags, setNotebookTags] = useState<{ id: string; name: string }[]>([]);
  useEffect(() => {
    let cancelled = false;
    const notebookId = notebooks.find((notebook) => notebook.path === notebookPath)?.id;
    if (!notebookId) {
      setNotebookTags([]);
      return undefined;
    }
    void tagsClient.getAll(notebookId)
      .then((result) => {
        if (!cancelled) setNotebookTags(result.tags);
      })
      .catch(() => {
        if (!cancelled) setNotebookTags([]);
      });
    return () => {
      cancelled = true;
    };
  }, [notebooks, notebookPath]);

  // 加载选中身份卡（含本地运行状态）。
  useEffect(() => {
    disposedRef.current = false;
    setError(null);
    if (!agentId) {
      setProfile(null);
      setForm(null);
      setAgentState(null);
      return;
    }
    let cancelled = false;
    setLoading(true);
    void agentProfile.get(notebookPath, agentId)
      .then((loaded) => {
        if (cancelled) return;
        setProfile(loaded);
        setForm(formFromProfile(loaded));
        setAgentState(null);
        return agentProfile.getState(notebookPath, agentId)
          .then((state) => {
            if (!cancelled) setAgentState(state);
          })
          .catch(() => {
            // STATE_NOT_FOUND = 新 agent 尚未运行，保留 null。
          });
      })
      .catch((cause) => {
        if (!cancelled) setError(cause instanceof Error ? cause.message : String(cause));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
      disposedRef.current = true;
    };
  }, [notebookPath, agentId]);

  const patchForm = useCallback((patch: Partial<ProfileForm>) => {
    setForm((current) => (current ? { ...current, ...patch } : current));
  }, []);

  const save = useCallback(async () => {
    if (!form || !agentId || saving) return;
    setSaving(true);
    setError(null);
    try {
      const updated = await agentProfile.update(notebookPath, agentId, {
        displayName: form.displayName.trim(),
        bio: form.bio.trim(),
        agentType: form.agentType,
        expertise: parseLines(form.expertise),
        model: form.model.trim(),
        consultable: form.consultable,
        redactionLevel: form.redactionLevel,
        // P3 扩展字段：UI 全权（memory 扩缩均放行——UI 即人审）。
        system: form.system,
        tools: form.tools,
        skills: form.skills,
        metadata: parseMetadata(form.metadata),
        memory: form.memory,
      });
      if (disposedRef.current) return;
      setProfile(updated);
      setForm(formFromProfile(updated));
      bumpProfiles();
    } catch (cause) {
      if (!disposedRef.current) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setSaving(false);
    }
  }, [form, agentId, saving, notebookPath, bumpProfiles]);

  const create = useCallback(async () => {
    const displayName = createForm.displayName.trim();
    if (!displayName || saving) return;
    setSaving(true);
    setError(null);
    try {
      const created = await agentProfile.create(notebookPath, {
        displayName,
        bio: '',
        expertise: [],
        agentType: createForm.agentType,
      });
      if (disposedRef.current) return;
      setCreateForm({ displayName: '', agentType: 'codex' });
      bumpProfiles();
      setTarget({ agentId: created.flowix_agent_id });
    } catch (cause) {
      if (!disposedRef.current) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setSaving(false);
    }
  }, [createForm, saving, notebookPath, bumpProfiles, setTarget]);

  const remove = useCallback(async (id: string) => {
    if (saving) return;
    setSaving(true);
    setError(null);
    try {
      await agentProfile.delete(notebookPath, id);
      if (disposedRef.current) return;
      setDeleteTarget(null);
      setTarget(null);
      bumpProfiles();
    } catch (cause) {
      if (!disposedRef.current) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setSaving(false);
    }
  }, [saving, notebookPath, setTarget, bumpProfiles]);

  // AGENT-MARKET D-1：市场为默认视图（未选目标或显式选中市场）。
  if (!target || 'market' in target) {
    return <AgentMarket notebookPath={notebookPath} />;
  }

  // 收件箱：入站咨询信封的拉取 / 接受 / 拒绝。
  if ('inbox' in target) {
    return <AgentProfileInbox notebookPath={notebookPath} />;
  }

  const selectableTypes = AGENT_TYPES;

  return (
    <div className="flex h-full min-h-0 flex-col bg-[var(--document-bg)]">
      <OverlayScrollbar className="min-h-0 flex-1" scrollerClassName="h-full overflow-y-auto">
        <div className="mx-auto w-full max-w-[640px] px-6 py-6">
          {error && <div className="mb-4"><InlineError message={error} /></div>}

          {'creating' in target ? (
            <div className="space-y-4">
              <h1 className="text-base font-semibold tracking-[-0.02em] text-[var(--foreground)]">
                {t('agent.profile.createTitle')}
              </h1>
              <Field label={t('agent.profile.name')}>
                <input
                  className={inputClass}
                  value={createForm.displayName}
                  onChange={(event) => setCreateForm((current) => ({ ...current, displayName: event.target.value }))}
                  placeholder={t('agent.profile.namePlaceholder')}
                  autoFocus
                />
              </Field>
              <Field label={t('agent.profile.agentType')}>
                <select
                  className={cn(inputClass, 'py-0')}
                  value={createForm.agentType}
                  onChange={(event) => setCreateForm((current) => ({ ...current, agentType: event.target.value }))}
                >
                  {selectableTypes.map((type) => (
                    <option key={type.key} value={type.key}>{type.name}</option>
                  ))}
                </select>
              </Field>
              <div className="flex justify-end gap-2 pt-2">
                <Button type="button" variant="ghost" onClick={() => setTarget(null)}>{t('dialog.cancel')}</Button>
                <Button type="button" disabled={!createForm.displayName.trim() || saving} onClick={() => void create()}>
                  {saving ? <Loader2 className="animate-spin" /> : <Plus />}
                  {t('agent.profile.create')}
                </Button>
              </div>
            </div>
          ) : loading ? (
            <ListSurfaceLoadingState label={t('agent.profile.loading')} className="h-64 w-full" />
          ) : form && agentId ? (
            <div className="space-y-6">
              <section className="space-y-4">
                <h1 className="text-base font-semibold tracking-[-0.02em] text-[var(--foreground)]">
                  {t('agent.profile.basicInfo')}
                </h1>
                <div className="grid gap-4 sm:grid-cols-2">
                  <Field label={t('agent.profile.name')}>
                    <input
                      className={inputClass}
                      value={form.displayName}
                      onChange={(event) => patchForm({ displayName: event.target.value })}
                    />
                  </Field>
                  <Field label={t('agent.profile.agentType')}>
                    <select
                      className={cn(inputClass, 'py-0')}
                      value={form.agentType}
                      onChange={(event) => patchForm({ agentType: event.target.value })}
                    >
                      {selectableTypes.map((type) => (
                        <option key={type.key} value={type.key}>{type.name}</option>
                      ))}
                    </select>
                  </Field>
                </div>
                <Field label={t('agent.profile.bio')} hint={t('agent.profile.bioHint')}>
                  <textarea
                    className={textareaClass}
                    value={form.bio}
                    onChange={(event) => patchForm({ bio: event.target.value })}
                    placeholder={t('agent.profile.bioPlaceholder')}
                  />
                </Field>
                <div className="grid gap-4 sm:grid-cols-2">
                  <Field label={t('agent.profile.expertise')} hint={t('agent.profile.expertiseHint')}>
                    <textarea
                      className={cn(textareaClass, 'min-h-16 font-mono text-xs leading-5')}
                      value={form.expertise}
                      onChange={(event) => patchForm({ expertise: event.target.value })}
                      placeholder={t('agent.profile.expertisePlaceholder')}
                    />
                  </Field>
                  <Field label={t('agent.profile.model')} hint={t('agent.profile.modelHint')}>
                    <input
                      className={inputClass}
                      value={form.model}
                      onChange={(event) => patchForm({ model: event.target.value })}
                      placeholder={t('agent.profile.modelPlaceholder')}
                    />
                  </Field>
                </div>
              </section>

              <section className="space-y-3 rounded-xl border border-[var(--border)] bg-[color-mix(in_oklch,var(--card)_82%,var(--background))] p-4">
                <h2 className="text-sm font-semibold text-[var(--foreground)]">{t('agent.profile.privacy')}</h2>
                <Toggle
                  checked={form.consultable}
                  onChange={(consultable) => patchForm({ consultable })}
                  label={t('agent.profile.consultable')}
                />
                <Field label={t('agent.profile.redactionLevel')}>
                  <select
                    className={cn(inputClass, 'py-0')}
                    value={form.redactionLevel}
                    onChange={(event) => patchForm({ redactionLevel: event.target.value })}
                  >
                    {PROFILE_REDACTION_LEVELS.map((level) => (
                      <option key={level} value={level}>
                        {t(LEVEL_LABEL_KEYS[level])}
                      </option>
                    ))}
                  </select>
                </Field>
                <p className="text-xs leading-5 text-[var(--muted-foreground)]">
                  {t('agent.profile.privacyHint')}
                </p>
              </section>

              {/* AGENT-MARKET M1：市场发布管理（发布/更新/下架）。 */}
              <section className="space-y-2">
                <h2 className="text-sm font-semibold text-[var(--foreground)]">{t('agent.market.publishSection')}</h2>
                {myCard && myCard.publishState !== 'none' ? (
                  <p className="text-xs leading-5 text-[var(--muted-foreground)]">
                    {myCard.publishState === 'published' && myCard.visibility === 'private'
                      ? t('agent.market.unpublishedHint')
                      : t('agent.market.publishedHint', {
                          address: `${account?.username ?? '…'}/${myCard.publishedSlug ?? ''}`,
                          visibility: myCard.visibility ?? '',
                        })}
                    {myCard.drifted && ` ${t('agent.market.driftHint')}`}
                  </p>
                ) : (
                  <p className="text-xs leading-5 text-[var(--muted-foreground)]">
                    {t('agent.market.noneHint')}
                  </p>
                )}
                {!account && (
                  <p className="text-xs leading-5 text-[var(--destructive)]">
                    {t('agent.market.noAccount')}
                  </p>
                )}
                <div className="flex gap-2">
                  <Button
                    type="button"
                    variant="outline"
                    className="h-7 gap-1 text-xs"
                    disabled={!account || unpublishing}
                    onClick={() => setPublishOpen(true)}
                  >
                    {myCard && myCard.publishState !== 'none' && myCard.visibility !== 'private'
                      ? t('agent.market.update')
                      : myCard?.visibility === 'private'
                        ? t('agent.market.republish')
                        : t('agent.market.publish')}
                  </Button>
                  {myCard?.publishState === 'published' && myCard.visibility !== 'private' && (
                    <Button
                      type="button"
                      variant="ghost"
                      className="h-7 gap-1 text-xs text-[var(--destructive)]"
                      disabled={unpublishing}
                      onClick={() => setUnpublishConfirm(true)}
                    >
                      {unpublishing ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : null}
                      {t('agent.market.unpublish')}
                    </Button>
                  )}
                </div>
              </section>

              {/* AGENT-PROFILE-REDESIGN P3：对话系统提示 + 记忆库（即披露授权）。 */}
              <section className="space-y-3">
                <h2 className="text-sm font-semibold text-[var(--foreground)]">{t('agent.profile.advanced')}</h2>
                <Field label={t('agent.profile.system')} hint={t('agent.profile.systemHint')}>
                  <textarea
                    className={textareaClass}
                    value={form.system}
                    onChange={(event) => patchForm({ system: event.target.value })}
                    placeholder={t('agent.profile.systemPlaceholder')}
                  />
                </Field>
                <Field label={t('agent.profile.memory')} hint={t('agent.profile.memoryHint')}>
                  <MemoryEditor
                    items={form.memory}
                    onChange={(memory) => patchForm({ memory })}
                    notebookTags={notebookTags}
                    pickLabel={t('agent.profile.memoryPickTag')}
                    manualPlaceholder={t('agent.profile.memoryManualPlaceholder')}
                    emptyLabel={t('agent.profile.memoryNoTags')}
                  />
                </Field>
                <p className="text-xs leading-5 text-[var(--muted-foreground)]">
                  {t('agent.profile.memoryDisclosureNote')}
                </p>
                <div className="grid grid-cols-2 gap-3">
                  <Field label={t('agent.profile.tools')} hint={t('agent.profile.toolsHint')}>
                    <ChipsEditor
                      items={form.tools}
                      onChange={(tools) => patchForm({ tools })}
                      placeholder={t('agent.profile.listPlaceholder')}
                    />
                  </Field>
                  <Field label={t('agent.profile.skills')} hint={t('agent.profile.skillsHint')}>
                    <ChipsEditor
                      items={form.skills}
                      onChange={(skills) => patchForm({ skills })}
                      placeholder={t('agent.profile.listPlaceholder')}
                    />
                  </Field>
                </div>
                <Field label={t('agent.profile.metadata')} hint={t('agent.profile.metadataHint')}>
                  <textarea
                    className={cn(textareaClass, 'min-h-14 font-mono text-xs leading-5')}
                    value={form.metadata}
                    onChange={(event) => patchForm({ metadata: event.target.value })}
                    placeholder={t('agent.profile.metadataPlaceholder')}
                  />
                </Field>
              </section>

              <section className="space-y-2">
                <h2 className="text-sm font-semibold text-[var(--foreground)]">{t('agent.profile.runtime')}</h2>
                {agentState ? (
                  <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">
                    <StateChip label={t('agent.profile.runs')} value={String(agentState.runs)} />
                    <StateChip label={t('agent.profile.consultations')} value={String(agentState.consultationsReceived)} />
                    <StateChip label={t('agent.profile.tokensIn')} value={String(agentState.tokensIn)} />
                    <StateChip label={t('agent.profile.tokensOut')} value={String(agentState.tokensOut)} />
                  </div>
                ) : (
                  <p className="rounded-lg border border-[var(--border)] px-3 py-2.5 text-xs text-[var(--muted-foreground)]">
                    {t('agent.profile.noState')}
                  </p>
                )}
              </section>
            </div>
          ) : null}
        </div>
      </OverlayScrollbar>

      {!('creating' in target) && form && agentId && (
        <footer className="flex shrink-0 items-center justify-end gap-2 border-t border-[var(--divider)] px-6 py-3">
          <Button
            type="button"
            variant="ghost"
            className="gap-1 text-[var(--destructive)] hover:bg-[color-mix(in_oklch,var(--destructive)_8%,transparent)]"
            disabled={saving}
            onClick={() => {
              if (profile) setDeleteTarget({ id: agentId, name: profile.flowix_display_name });
            }}
          >
            <Trash2 className="h-3.5 w-3.5" />
            {t('dialog.delete')}
          </Button>
          <Button type="button" disabled={saving || !form.displayName.trim()} onClick={() => void save()}>
            {saving ? <Loader2 className="animate-spin" /> : <Save />}
            {t('agent.profile.save')}
          </Button>
        </footer>
      )}

      <Dialog
        open={deleteTarget !== null}
        onOpenChange={(open) => {
          if (!open) setDeleteTarget(null);
        }}
      >
        <DialogContent className="rounded-xl border border-[var(--border-popup)] bg-[var(--card)] shadow-[0_4px_24px_-3px_rgb(0_0_0_/_0.24)]">
          <DialogHeader>
            <DialogTitle>{t('agent.profile.deleteTitle')}</DialogTitle>
            <DialogDescription>
              {t('agent.profile.deleteDescription', { name: deleteTarget?.name ?? '' })}
            </DialogDescription>
          </DialogHeader>
          <div className="mt-4 flex justify-end gap-2">
            <Button
              type="button"
              variant="ghost"
              className="h-8 px-3 text-sm"
              onClick={() => setDeleteTarget(null)}
            >
              {t('dialog.cancel')}
            </Button>
            <Button
              type="button"
              className="h-8 rounded-lg px-3 text-sm"
              disabled={saving}
              onClick={() => deleteTarget && void remove(deleteTarget.id)}
            >
              {t('agent.profile.deleteTitle')}
            </Button>
          </div>
        </DialogContent>
      </Dialog>

      {/* AGENT-MARKET M1：发布对话框（profile 预填 + agentId 关联）。 */}
      {publishOpen && profile && agentId && (
        <PublishCardDialog
          notebookPath={notebookPath}
          username={account?.username ?? ''}
          initial={{
            agentId,
            slug: myCard?.publishedSlug ?? '',
            displayName: profile.flowix_display_name,
            tags: profile.flowix_expertise.join(', '),
            visibility: myCard?.visibility && myCard.visibility !== 'private'
              ? myCard.visibility as 'public' | 'unlisted'
              : 'unlisted',
          }}
          onClose={() => setPublishOpen(false)}
          onPublished={() => {
            void reloadMyCard();
            bumpProfiles();
          }}
        />
      )}

      {/* AGENT-MARKET M1：下架确认（D-3：重发 private 卡，等价下架）。 */}
      <Dialog
        open={unpublishConfirm}
        onOpenChange={(open) => {
          if (!open) setUnpublishConfirm(false);
        }}
      >
        <DialogContent className="rounded-xl border border-[var(--border-popup)] bg-[var(--card)] shadow-[0_4px_24px_-3px_rgb(0_0_0_/_0.24)]">
          <DialogHeader>
            <DialogTitle>{t('agent.market.unpublish')}</DialogTitle>
            <DialogDescription>{t('agent.market.unpublishDescription')}</DialogDescription>
          </DialogHeader>
          <div className="mt-4 flex justify-end gap-2">
            <Button
              type="button"
              variant="ghost"
              className="h-8 px-3 text-sm"
              onClick={() => setUnpublishConfirm(false)}
            >
              {t('dialog.cancel')}
            </Button>
            <Button
              type="button"
              className="h-8 rounded-lg px-3 text-sm"
              disabled={unpublishing}
              onClick={() => void unpublish()}
            >
              {unpublishing ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : null}
              {t('agent.market.unpublish')}
            </Button>
          </div>
        </DialogContent>
      </Dialog>
    </div>
  );
}
