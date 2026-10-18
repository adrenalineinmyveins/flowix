'use client';

import { useCallback, useEffect, useRef, useState } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { Bot, Fingerprint, Loader2, Plus, Save, Trash2, X } from 'lucide-react';

import { agentProfile } from '@platform/tauri/client';
import type { AgentProfile, AgentState } from '@/lib/types/agent-profile';
import { PROFILE_REDACTION_LEVELS, PROFILE_REDACTION_LEVEL_LABELS } from '@/lib/types/agent-profile';
import { AGENT_TYPES } from '@/lib/agent-types';
import { Button } from '@shared/ui/button';
import { cn } from '@/lib/utils';

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

export function AgentProfileCardsDialog({
  notebookPath,
  onClose,
}: {
  notebookPath: string;
  onClose: () => void;
}) {
  const [profiles, setProfiles] = useState<AgentProfile[] | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [form, setForm] = useState<ProfileForm | null>(null);
  const [agentState, setAgentState] = useState<AgentState | null>(null);
  const [creating, setCreating] = useState(false);
  const [createForm, setCreateForm] = useState({ displayName: '', agentType: 'codex' });
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
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
  }, [createForm, creating, notebookPath, refresh]);

  const remove = useCallback(async () => {
    const selected = profiles?.find((profile) => profile.flowix_agent_id === selectedId);
    if (!selected || saving) return;
    if (!window.confirm(`确定删除「${selected.flowix_display_name}」？将级联清理其记忆与本地运行数据。`)) return;
    setSaving(true);
    setError(null);
    try {
      await agentProfile.delete(notebookPath, selectedId!);
      if (disposedRef.current) return;
      await refresh();
    } catch (cause) {
      if (!disposedRef.current) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setSaving(false);
    }
  }, [profiles, selectedId, saving, notebookPath, refresh]);

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

  const selectableTypes = AGENT_TYPES;
  const typeLabel = (key: string) => selectableTypes.find((type) => type.key === key)?.name ?? key;

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
        aria-label="个人 Agent 身份卡"
        className="flex h-[min(720px,calc(100vh-80px))] w-full max-w-[920px] flex-col overflow-hidden rounded-2xl bg-[var(--background)] shadow-2xl"
      >
        <header className="flex shrink-0 items-center gap-3 border-b border-[var(--divider)] px-5 py-3">
          <Fingerprint className="h-5 w-5 shrink-0 text-[var(--primary)]" />
          <div className="min-w-0 flex-1">
            <h1 className="truncate text-base font-semibold tracking-[-0.02em] text-[var(--foreground)]">
              个人 Agent 身份卡
            </h1>
            <p className="truncate text-xs text-[var(--muted-foreground)]" title={notebookPath}>
              身份卡存于笔记本 .agents/ 目录，随云同步；隐私边界决定 F5 互咨时的披露范围。
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

        <div className="flex min-h-0 flex-1 flex-col md:flex-row">
          <nav className="flex shrink-0 flex-col border-b border-[var(--divider)] bg-[color-mix(in_oklch,var(--card)_45%,transparent)] p-3 md:w-64 md:border-b-0 md:border-r" aria-label="Agent 列表">
            <Button
              type="button"
              variant="outline"
              className="mb-2 h-8 w-full gap-1 rounded-lg px-2 text-xs"
              onClick={() => {
                setCreating(true);
                setForm(null);
                setSelectedId(null);
                setAgentState(null);
                setCreateForm({ displayName: '', agentType: 'codex' });
              }}
            >
              <Plus className="h-3.5 w-3.5" />
              新建身份卡
            </Button>
            <div className="min-h-0 flex-1 space-y-1 overflow-y-auto">
              {loading && (
                <p className="px-2 py-4 text-center text-xs text-[var(--muted-foreground)]">加载中…</p>
              )}
              {!loading && profiles?.length === 0 && (
                <p className="px-2 py-4 text-center text-xs text-[var(--muted-foreground)]">
                  还没有个人 Agent，点击上方新建。
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
                        title={profile.flowix_consultable ? '可被咨询' : '未开放咨询'}
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
          </nav>

          <main className="min-h-0 flex-1 overflow-y-auto px-5 py-4 [scrollbar-gutter:stable]">
            {error && <div className="mb-3"><InlineError message={error} /></div>}

            {creating && (
              <div className="space-y-4">
                <h2 className="text-sm font-semibold text-[var(--foreground)]">新建个人 Agent</h2>
                <Field label="名称">
                  <input
                    className={inputClass}
                    value={createForm.displayName}
                    onChange={(event) => setCreateForm((current) => ({ ...current, displayName: event.target.value }))}
                    placeholder="例如：财报解读助手"
                    autoFocus
                  />
                </Field>
                <Field label="Agent 类型">
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
                  <Button type="button" variant="ghost" onClick={() => void refresh(selectedId)}>取消</Button>
                  <Button type="button" disabled={!createForm.displayName.trim() || saving} onClick={() => void create()}>
                    {saving ? <Loader2 className="animate-spin" /> : <Plus />}
                    创建
                  </Button>
                </div>
              </div>
            )}

            {!creating && form && selectedId && (
              <div className="space-y-5">
                <section className="space-y-4">
                  <h2 className="text-sm font-semibold text-[var(--foreground)]">基本信息</h2>
                  <div className="grid gap-4 sm:grid-cols-2">
                    <Field label="名称">
                      <input
                        className={inputClass}
                        value={form.displayName}
                        onChange={(event) => patchForm({ displayName: event.target.value })}
                      />
                    </Field>
                    <Field label="Agent 类型">
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
                  <Field label="简介" hint="≤500 字，F2 喂养可自动改写">
                    <textarea
                      className={textareaClass}
                      value={form.bio}
                      onChange={(event) => patchForm({ bio: event.target.value })}
                      placeholder="一句话说明这个 Agent 擅长什么"
                    />
                  </Field>
                  <div className="grid gap-4 sm:grid-cols-2">
                    <Field label="专长标签" hint="每行一个标签路径">
                      <textarea
                        className={cn(textareaClass, 'min-h-16 font-mono text-xs leading-5')}
                        value={form.expertise}
                        onChange={(event) => patchForm({ expertise: event.target.value })}
                        placeholder={'finance/财报\nindustry/电商'}
                      />
                    </Field>
                    <Field label="模型覆盖" hint="留空跟随类型默认">
                      <input
                        className={inputClass}
                        value={form.model}
                        onChange={(event) => patchForm({ model: event.target.value })}
                        placeholder="默认模型"
                      />
                    </Field>
                  </div>
                </section>

                <section className="space-y-3 rounded-xl border border-[var(--border)] bg-[color-mix(in_oklch,var(--card)_82%,var(--background))] p-4">
                  <h2 className="text-sm font-semibold text-[var(--foreground)]">隐私边界（F5 网络互咨时生效）</h2>
                  <Toggle
                    checked={form.consultable}
                    onChange={(consultable) => patchForm({ consultable })}
                    label="允许被其他 Agent 咨询"
                  />
                  <div className="grid gap-4 sm:grid-cols-2">
                    <Field label="最高披露等级">
                      <select
                        className={cn(inputClass, 'py-0')}
                        value={form.redactionLevel}
                        onChange={(event) => patchForm({ redactionLevel: event.target.value })}
                      >
                        {PROFILE_REDACTION_LEVELS.map((level) => (
                          <option key={level} value={level}>
                            {PROFILE_REDACTION_LEVEL_LABELS[level]}
                          </option>
                        ))}
                      </select>
                    </Field>
                    <Field label="披露范围" hint="标签路径，每行一个">
                      <textarea
                        className={cn(textareaClass, 'min-h-16 font-mono text-xs leading-5')}
                        value={form.disclosureScope}
                        onChange={(event) => patchForm({ disclosureScope: event.target.value })}
                        placeholder={'finance/财报'}
                      />
                    </Field>
                  </div>
                  <p className="text-xs leading-5 text-[var(--muted-foreground)]">
                    喂养取材范围与对外披露范围共用同一字段；等级未设置时按设计将拒绝被咨询。
                  </p>
                </section>

                <section className="space-y-2">
                  <h2 className="text-sm font-semibold text-[var(--foreground)]">本地运行状态</h2>
                  {agentState ? (
                    <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">
                      <StateChip label="运行次数" value={String(agentState.runs)} />
                      <StateChip label="被咨询" value={String(agentState.consultationsReceived)} />
                      <StateChip label="tokens 入" value={String(agentState.tokensIn)} />
                      <StateChip label="tokens 出" value={String(agentState.tokensOut)} />
                    </div>
                  ) : (
                    <p className="rounded-lg border border-[var(--border)] px-3 py-2.5 text-xs text-[var(--muted-foreground)]">
                      尚未运行过（无本地状态）。
                    </p>
                  )}
                </section>
              </div>
            )}

            {!creating && !form && !loading && profiles?.length === 0 && (
              <div className="flex min-h-full flex-col items-center justify-center py-16 text-center">
                <span className="flex h-12 w-12 items-center justify-center rounded-2xl bg-[color-mix(in_oklch,var(--muted)_72%,var(--background))] text-[var(--muted-foreground)]">
                  <Fingerprint className="h-5 w-5" />
                </span>
                <p className="mt-4 max-w-[36ch] text-sm leading-6 text-[var(--muted-foreground)]">
                  从左侧新建第一张身份卡。卡片数据存于笔记本 .agents/ 目录，可手动编辑 profile.md。
                </p>
              </div>
            )}
          </main>
        </div>

        {!creating && form && selectedId && (
          <footer className="flex shrink-0 items-center justify-between gap-3 border-t border-[var(--divider)] bg-[color-mix(in_oklch,var(--background)_92%,var(--card))] px-5 py-3">
            <Button
              type="button"
              variant="ghost"
              className="gap-1 text-[var(--destructive)] hover:bg-[color-mix(in_oklch,var(--destructive)_8%,transparent)]"
              disabled={saving}
              onClick={() => void remove()}
            >
              <Trash2 className="h-3.5 w-3.5" />
              删除
            </Button>
            <div className="flex items-center gap-2">
              <Button type="button" variant="ghost" onClick={requestClose}>关闭</Button>
              <Button type="button" disabled={saving || !form.displayName.trim()} onClick={() => void save()}>
                {saving ? <Loader2 className="animate-spin" /> : <Save />}
                保存
              </Button>
            </div>
          </footer>
        )}
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
