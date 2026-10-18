'use client';

import { useCallback, useEffect, useRef, useState } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { Bot, Fingerprint, Loader2, Plus, Save, Trash2 } from 'lucide-react';

import { agentProfile } from '@platform/tauri/client';
import type { AgentProfile, AgentState } from '@/lib/types/agent-profile';
import { PROFILE_REDACTION_LEVELS } from '@/lib/types/agent-profile';
import { AGENT_TYPES } from '@/lib/agent-types';
import { Button } from '@shared/ui/button';
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription } from '@shared/ui/dialog';
import { OverlayScrollbar } from '@shared/ui/overlay-scrollbar';
import { ListSurfaceLoadingState } from '@shared/ui/list-surface';
import { useI18n, type I18nKey } from '@/lib/i18n';
import { cn } from '@/lib/utils';

/** 披露等级下拉文案走 i18n（原 lib 硬编码中文映射随本面板迁移）。 */
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
 * 嵌入式身份卡面板：中列「个人 Agent」视图与对话框共用主体。
 * embedded=true 时按中列格式渲染（标题避让左上角悬浮视图切换器）。
 */
export function AgentProfileCardsPanel({
  notebookPath,
  embedded = false,
}: {
  notebookPath: string;
  embedded?: boolean;
}) {
  const { t } = useI18n();
  const [profiles, setProfiles] = useState<AgentProfile[] | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [form, setForm] = useState<ProfileForm | null>(null);
  const [agentState, setAgentState] = useState<AgentState | null>(null);
  const [creating, setCreating] = useState(false);
  const [createForm, setCreateForm] = useState({ displayName: '', agentType: 'codex' });
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<{ id: string; name: string } | null>(null);
  const disposedRef = useRef(false);

  const refresh = useCallback(async (keepId?: string | null) => {
    setLoading(true);
    setError(null);
    try {
      const list = await agentProfile.list(notebookPath);
      if (disposedRef.current) return;
      setProfiles(list);
      const nextId =
        keepId && list.some((profile) => profile.flowix_agent_id === keepId)
          ? keepId
          : list[0]?.flowix_agent_id ?? null;
      setSelectedId(nextId);
      const selected = list.find((profile) => profile.flowix_agent_id === nextId);
      setForm(selected ? formFromProfile(selected) : null);
      setAgentState(null);
      if (selected) {
        agentProfile
          .getState(notebookPath, selected.flowix_agent_id)
          .then((state) => {
            if (!disposedRef.current) setAgentState(state);
          })
          .catch(() => {
            // STATE_NOT_FOUND = 新 agent 尚未运行，保留 null。
          });
      }
    } catch (cause) {
      if (!disposedRef.current) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      if (!disposedRef.current) setLoading(false);
    }
  }, [notebookPath]);

  useEffect(() => {
    disposedRef.current = false;
    void refresh();
    return () => {
      disposedRef.current = true;
    };
  }, [refresh]);

  const selectProfile = useCallback(
    (profile: AgentProfile) => {
      setCreating(false);
      setSelectedId(profile.flowix_agent_id);
      setForm(formFromProfile(profile));
      setAgentState(null);
      const requestId = profile.flowix_agent_id;
      agentProfile
        .getState(notebookPath, requestId)
        .then((state) => {
          if (!disposedRef.current && selectedIdRef.current === requestId) {
            setAgentState(state);
          }
        })
        .catch(() => undefined);
    },
    [notebookPath],
  );
  const selectedIdRef = useRef(selectedId);
  selectedIdRef.current = selectedId;

  const patchForm = useCallback((patch: Partial<ProfileForm>) => {
    setForm((current) => (current ? { ...current, ...patch } : current));
  }, []);

  const save = useCallback(async () => {
    if (!form || !selectedId || saving) return;
    setSaving(true);
    setError(null);
    try {
      await agentProfile.update(notebookPath, selectedId, {
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
      await refresh(selectedId);
    } catch (cause) {
      if (!disposedRef.current) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setSaving(false);
    }
  }, [form, selectedId, saving, notebookPath, refresh]);

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
      setCreating(false);
      setCreateForm({ displayName: '', agentType: 'codex' });
      await refresh(created.flowix_agent_id);
    } catch (cause) {
      if (!disposedRef.current) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setSaving(false);
    }
  }, [createForm, saving, notebookPath, refresh]);

  const remove = useCallback(async (target: { id: string; name: string }) => {
    if (saving) return;
    setSaving(true);
    setError(null);
    try {
      await agentProfile.delete(notebookPath, target.id);
      if (disposedRef.current) return;
      setDeleteTarget(null);
      await refresh();
    } catch (cause) {
      if (!disposedRef.current) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setSaving(false);
    }
  }, [saving, notebookPath, refresh]);

  const selectableTypes = AGENT_TYPES;
  const typeLabel = (key: string) => selectableTypes.find((type) => type.key === key)?.name ?? key;

  const startCreating = () => {
    setCreating(true);
    setForm(null);
    setSelectedId(null);
    setAgentState(null);
    setCreateForm({ displayName: '', agentType: 'codex' });
  };

  return (
    <div className="flex h-full min-h-0 flex-col bg-[var(--list-bg)]">
      {/* 中列统一头部行：embedded 时标题避让左上角悬浮的视图切换器。 */}
      <div
        className={cn(
          'flex shrink-0 items-center justify-between gap-2 pb-2 pr-3',
          embedded ? 'pl-[126px]' : 'pl-4',
        )}
      >
        <span className="min-w-0 truncate text-[15px] font-medium text-[var(--foreground)]">
          {t('agent.profile.title')}
        </span>
        <Button
          type="button"
          variant="outline"
          className="h-7 shrink-0 gap-1 rounded-lg px-2 text-xs"
          onClick={startCreating}
        >
          <Plus className="h-3.5 w-3.5" />
          {t('agent.profile.newCard')}
        </Button>
      </div>

      <div className="@container flex min-h-0 flex-1 flex-col @[440px]:flex-row">
        <nav
          className="flex max-h-[45%] min-h-0 shrink-0 flex-col gap-2 border-b border-[var(--divider)] p-2 @[440px]:max-h-none @[440px]:w-56 @[440px]:border-b-0 @[440px]:border-r"
          aria-label={t('agent.profile.listLabel')}
        >
          <OverlayScrollbar className="min-h-0 flex-1" scrollerClassName="h-full overflow-y-auto px-0.5 py-0.5">
            <div className="space-y-1">
              {!loading && profiles?.length === 0 && (
                <p className="px-2 py-4 text-center text-xs text-[var(--muted-foreground)]">
                  {t('agent.profile.empty')}
                </p>
              )}
              {profiles?.map((profile) => {
                const active = !creating && profile.flowix_agent_id === selectedId;
                return (
                  <button
                    key={profile.flowix_agent_id}
                    type="button"
                    onClick={() => selectProfile(profile)}
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
                      {typeLabel(profile.flowix_agent_type)}
                      {profile.flowix_expertise.length > 0 && ` · ${profile.flowix_expertise.join(' / ')}`}
                    </span>
                  </button>
                );
              })}
            </div>
          </OverlayScrollbar>
        </nav>

        <main className="relative min-h-0 flex-1">
          {loading ? (
            <ListSurfaceLoadingState label={t('agent.profile.loading')} className="h-full w-full" />
          ) : (
            <OverlayScrollbar className="h-full" scrollerClassName="h-full overflow-y-auto px-5 py-4">
              {error && <div className="mb-3"><InlineError message={error} /></div>}

              {creating && (
                <div className="space-y-4">
                  <h2 className="text-sm font-semibold text-[var(--foreground)]">{t('agent.profile.createTitle')}</h2>
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
                    <Button type="button" variant="ghost" onClick={() => void refresh(selectedId)}>{t('dialog.cancel')}</Button>
                    <Button type="button" disabled={!createForm.displayName.trim() || saving} onClick={() => void create()}>
                      {saving ? <Loader2 className="animate-spin" /> : <Plus />}
                      {t('agent.profile.create')}
                    </Button>
                  </div>
                </div>
              )}

              {!creating && form && selectedId && (
                <div className="space-y-5">
                  <section className="space-y-4">
                    <h2 className="text-sm font-semibold text-[var(--foreground)]">{t('agent.profile.basicInfo')}</h2>
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
              )}

              {!creating && !form && profiles?.length === 0 && (
                <div className="flex min-h-full flex-col items-center justify-center py-16 text-center">
                  <span className="flex h-12 w-12 items-center justify-center rounded-2xl bg-[color-mix(in_oklch,var(--muted)_72%,var(--background))] text-[var(--muted-foreground)]">
                    <Fingerprint className="h-5 w-5" />
                  </span>
                  <p className="mt-4 max-w-[36ch] text-sm leading-6 text-[var(--muted-foreground)]">
                    {t('agent.profile.emptyHint')}
                  </p>
                </div>
              )}
            </OverlayScrollbar>
          )}
        </main>
      </div>

      {!creating && form && selectedId && (
        <footer className="flex shrink-0 items-center justify-between gap-3 border-t border-[var(--divider)] px-5 py-3">
          <Button
            type="button"
            variant="ghost"
            className="gap-1 text-[var(--destructive)] hover:bg-[color-mix(in_oklch,var(--destructive)_8%,transparent)]"
            disabled={saving}
            onClick={() => {
              const selected = profiles?.find((profile) => profile.flowix_agent_id === selectedId);
              if (selected) setDeleteTarget({ id: selected.flowix_agent_id, name: selected.flowix_display_name });
            }}
          >
            <Trash2 className="h-3.5 w-3.5" />
            {t('dialog.delete')}
          </Button>
          <div className="flex items-center gap-2">
            <Button type="button" disabled={saving || !form.displayName.trim()} onClick={() => void save()}>
              {saving ? <Loader2 className="animate-spin" /> : <Save />}
              {t('agent.profile.save')}
            </Button>
          </div>
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
              onClick={() => deleteTarget && void remove(deleteTarget)}
            >
              {t('agent.profile.deleteTitle')}
            </Button>
          </div>
        </DialogContent>
      </Dialog>
    </div>
  );
}

/** 对话框壳：Esc/点击遮罩关闭，主体复用面板。 */
export function AgentProfileCardsDialog({
  notebookPath,
  onClose,
}: {
  notebookPath: string;
  onClose: () => void;
}) {
  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        onClose();
      }
    };
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [onClose]);

  return (
    <div
      className="fixed inset-0 z-[135] flex items-center justify-center bg-black/50 p-3"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label="个人 Agent 身份卡"
        className="flex h-[min(720px,calc(100vh-80px))] w-full max-w-[920px] flex-col overflow-hidden rounded-2xl bg-[var(--background)] shadow-2xl"
      >
        <AgentProfileCardsPanel notebookPath={notebookPath} />
      </div>
    </div>
  );
}

export class AgentProfileCardsDialogController {
  private container: HTMLDivElement | null = null;
  private root: Root | null = null;

  open(notebookPath: string): void {
    this.close();
    const container = document.createElement('div');
    container.dataset.agentProfileCardsDialog = 'true';
    document.body.append(container);
    this.container = container;
    this.root = createRoot(container);
    this.root.render(
      <AgentProfileCardsDialog notebookPath={notebookPath} onClose={() => this.close()} />,
    );
  }

  close(): void {
    this.root?.unmount();
    this.root = null;
    this.container?.remove();
    this.container = null;
  }
}
