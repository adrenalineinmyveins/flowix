'use client';

import { useCallback, useEffect, useRef, useState } from 'react';
import { Loader2, ShieldCheck } from 'lucide-react';

import { redaction } from '@platform/tauri/client';
import type { AutoGrantConfig } from '@/lib/types/redaction';
import { useI18n } from '@/lib/i18n';
import { Button } from '@shared/ui/button';
import { Input } from '@shared/ui/input';

/**
 * FD-12 M1：harness 远程咨询预授权（独立 UI，与会话隔离——授权变更
 * 只经此处，防会话内诱导放宽）。默认关：人审是默认态，此处是显式放宽。
 */
export function AutoGrantSection() {
  const { t } = useI18n();
  const [config, setConfig] = useState<AutoGrantConfig | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [open, setOpen] = useState(false);
  const disposedRef = useRef(false);

  useEffect(() => {
    disposedRef.current = false;
    redaction.autoGrantGet()
      .then((loaded) => { if (!disposedRef.current) setConfig(loaded); })
      .catch(() => undefined);
    return () => { disposedRef.current = true; };
  }, []);

  const patch = useCallback((next: Partial<AutoGrantConfig>) => {
    setConfig((current) => (current ? { ...current, ...next } : current));
  }, []);

  const save = useCallback(async () => {
    if (!config || busy) return;
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      await redaction.autoGrantSet(config);
      if (!disposedRef.current) setNotice(t('agent.autoGrant.saved'));
    } catch (cause) {
      if (!disposedRef.current) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  }, [config, busy, t]);

  if (!config) return null;

  const listField = (
    key: 'scopeWhitelist' | 'openScopes' | 'targets' | 'answerTargets',
    labelKey:
      | 'agent.autoGrant.scopes'
      | 'agent.autoGrant.openScopes'
      | 'agent.autoGrant.targets'
      | 'agent.autoGrant.answerTargets',
    placeholder: string,
  ) => (
    <label className="block space-y-1">
      <span className="text-xs font-medium text-[var(--foreground)]">{t(labelKey)}</span>
      <Input
        className="h-8 font-mono text-xs"
        value={config[key].join(', ')}
        placeholder={placeholder}
        onChange={(event) => patch({
          [key]: event.target.value.split(',').map((item) => item.trim()).filter(Boolean),
        } as Partial<AutoGrantConfig>)}
      />
    </label>
  );

  return (
    <section className="rounded-lg border border-[var(--border)] bg-[var(--card)] p-4">
      <div className="flex items-center gap-2">
        <ShieldCheck className="size-4 text-[var(--primary)]" />
        <h3 className="flex-1 text-sm font-semibold">{t('agent.autoGrant.title')}</h3>
        <label className="flex items-center gap-1.5 text-xs text-[var(--muted-foreground)]">
          <input
            type="checkbox"
            checked={config.enabled}
            onChange={(event) => patch({ enabled: event.target.checked })}
          />
          {t('agent.autoGrant.enabled')}
        </label>
        <Button size="sm" variant="ghost" className="h-7 px-2 text-xs" onClick={() => setOpen((value) => !value)}>
          {open ? t('agent.autoGrant.collapse') : t('agent.autoGrant.expand')}
        </Button>
      </div>
      <p className="mt-1.5 text-xs leading-5 text-[var(--muted-foreground)]">
        {t('agent.autoGrant.summary')}
      </p>
      {open && (
        <div className="mt-3 space-y-3">
          <div className="grid grid-cols-2 gap-3">
            <label className="block space-y-1">
              <span className="text-xs font-medium text-[var(--foreground)]">{t('agent.autoGrant.maxLevel')}</span>
              <select
                className="h-8 w-full rounded-md border border-[var(--border)] bg-[var(--background)] px-2 text-xs"
                value={config.maxLevel}
                onChange={(event) => patch({ maxLevel: event.target.value })}
              >
                <option value="R1">R1（事实摘要）</option>
                <option value="R2">R2（泛化摘要）</option>
                <option value="R3">R3（领域抽象 · 推荐）</option>
              </select>
            </label>
            <label className="flex items-end gap-2 pb-1 text-xs text-[var(--muted-foreground)]">
              <input
                type="checkbox"
                checked={config.fastMode}
                onChange={(event) => patch({ fastMode: event.target.checked })}
              />
              {t('agent.autoGrant.fastMode')}
            </label>
          </div>
          {listField('scopeWhitelist', 'agent.autoGrant.scopes', 'finance/*, blog')}
          {listField('openScopes', 'agent.autoGrant.openScopes', 'blog, reading/*')}
          {listField('targets', 'agent.autoGrant.targets', '* 或 alice, bob')}
          {/* FD-12 M4：B 端 auto-answer 能力位（判定读本地配置，改后即时生效）。 */}
          <div className="grid grid-cols-2 gap-3">
            <label className="block space-y-1">
              <span className="text-xs font-medium text-[var(--foreground)]">{t('agent.autoGrant.autoAnswer')}</span>
              <select
                className="h-8 w-full rounded-md border border-[var(--border)] bg-[var(--background)] px-2 text-xs"
                value={config.autoAnswer || 'off'}
                onChange={(event) => patch({ autoAnswer: event.target.value })}
              >
                <option value="off">{t('agent.autoGrant.autoAnswerOff')}</option>
                <option value="R3">{t('agent.autoGrant.autoAnswerR3')}</option>
              </select>
            </label>
            <div className="flex items-end pb-1">
              {listField('answerTargets', 'agent.autoGrant.answerTargets', '* 或 alice, bob')}
            </div>
          </div>
          <div className="grid grid-cols-2 gap-3">
            <label className="block space-y-1">
              <span className="text-xs font-medium text-[var(--foreground)]">{t('agent.autoGrant.perSession')}</span>
              <Input
                className="h-8 text-xs"
                type="number"
                min={1}
                value={config.perSessionLimit}
                onChange={(event) => patch({ perSessionLimit: Number(event.target.value) || 1 })}
              />
            </label>
            <label className="block space-y-1">
              <span className="text-xs font-medium text-[var(--foreground)]">{t('agent.autoGrant.perDay')}</span>
              <Input
                className="h-8 text-xs"
                type="number"
                min={1}
                value={config.perDayLimit}
                onChange={(event) => patch({ perDayLimit: Number(event.target.value) || 1 })}
              />
            </label>
          </div>
          {error && <p className="text-xs text-[var(--destructive)]">{error}</p>}
          {notice && <p className="text-xs text-[var(--primary)]">{notice}</p>}
          <div className="flex justify-end">
            <Button size="sm" className="h-7 gap-1.5 text-xs" disabled={busy} onClick={() => void save()}>
              {busy && <Loader2 className="size-3.5 animate-spin" />}
              {t('agent.autoGrant.save')}
            </Button>
          </div>
        </div>
      )}
    </section>
  );
}
