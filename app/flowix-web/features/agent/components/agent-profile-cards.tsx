'use client';

import { useCallback, useEffect, useRef, useState } from 'react';
import { Bot, Fingerprint, Inbox, Loader2, Plus, Save, Trash2, X } from 'lucide-react';

import { agentProfile } from '@platform/tauri/client';
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
  disclosureScope: string;
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
    disclosureScope: profile.flowix_disclosure_scope.join('\n'),
  };
}

function parseLines(raw: string): string[] {
  return raw
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);
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
  const [loading, setLoading] = useState(true);
  const target = useNoteStore((state) => state.agentProfileViewTarget);
  const version = useNoteStore((state) => state.agentProfilesVersion);
  const disposedRef = useRef(false);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const list = await agentProfile.list(notebookPath);
      if (disposedRef.current) return;
      setProfiles(list);
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

  const agentId = target && 'agentId' in target ? target.agentId : null;

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
        disclosureScope: parseLines(form.disclosureScope),
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

  // 未选择目标：空态提示（列表在中列）。
  if (!target) {
    return (
      <div className="flex h-full items-center justify-center px-6">
        <p className="max-w-[36ch] text-center text-sm leading-6 text-[var(--muted-foreground)]">
          {t('agent.profile.editorEmpty')}
        </p>
      </div>
    );
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
                <div className="grid gap-4 sm:grid-cols-2">
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
                  <Field label={t('agent.profile.disclosureScope')} hint={t('agent.profile.disclosureScopeHint')}>
                    <textarea
                      className={cn(textareaClass, 'min-h-16 font-mono text-xs leading-5')}
                      value={form.disclosureScope}
                      onChange={(event) => patchForm({ disclosureScope: event.target.value })}
                      placeholder={t('agent.profile.disclosureScopePlaceholder')}
                    />
                  </Field>
                </div>
                <p className="text-xs leading-5 text-[var(--muted-foreground)]">
                  {t('agent.profile.privacyHint')}
                </p>
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
    </div>
  );
}
