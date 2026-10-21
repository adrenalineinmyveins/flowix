'use client';

import { useCallback, useEffect, useState } from 'react';
import { Input } from '@shared/ui/input';
import { SectionHeader } from '@features/preferences/sections/primitives';
import { Button } from '@shared/ui/button';
import { useI18n } from '@/lib/i18n';
import { toast } from '@/lib/toast';
import { agent } from '@platform/tauri/client';

interface MobileProviderForm {
  baseUrl: string;
  apiKey: string;
  model: string;
}

/** 云同步跨端会话（mobile runtime）provider 配置。
 *
 * 平文件 `<config_dir>/mobile-agent-provider.json`（API key 不上云）。
 * OpenAI 兼容端点：baseUrl 填到根（自动补 /v1/chat/completions）或完整路径。
 */
export function MobileChatSettingsSection() {
  const { t } = useI18n();
  const [form, setForm] = useState<MobileProviderForm>({ baseUrl: '', apiKey: '', model: '' });
  const [loaded, setLoaded] = useState(false);
  const [saving, setSaving] = useState(false);

  const load = useCallback(async () => {
    try {
      const config = await agent.mobileProviderGet();
      setForm({
        baseUrl: config.baseUrl ?? '',
        apiKey: config.apiKey ?? '',
        model: config.model ?? '',
      });
    } catch (value) {
      toast.error(value instanceof Error ? value.message : String(value));
    } finally {
      setLoaded(true);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const save = useCallback(async () => {
    setSaving(true);
    try {
      const saved = await agent.mobileProviderSet(form);
      setForm({
        baseUrl: saved.baseUrl ?? '',
        apiKey: saved.apiKey ?? '',
        model: saved.model ?? '',
      });
      toast.success(t('preferences.mobileChat.saved'));
    } catch (value) {
      toast.error(value instanceof Error ? value.message : String(value));
    } finally {
      setSaving(false);
    }
  }, [form, t]);

  if (!loaded) return null;

  const ready = form.baseUrl.trim() && form.apiKey.trim() && form.model.trim();

  return (
    <div className="space-y-6 pb-10">
      <SectionHeader
        title={t('preferences.mobileChat.title')}
        description={t('preferences.mobileChat.description')}
      />
      <div className="space-y-4 rounded-lg border border-[var(--border)] p-4">
        <label className="block space-y-1.5">
          <span className="text-sm font-medium">{t('preferences.mobileChat.baseUrl')}</span>
          <Input
            value={form.baseUrl}
            onChange={(event) => setForm((prev) => ({ ...prev, baseUrl: event.target.value }))}
            placeholder="https://api.deepseek.com"
            spellCheck={false}
          />
          <span className="text-xs text-[var(--muted-foreground)]">
            {t('preferences.mobileChat.baseUrlHint')}
          </span>
        </label>
        <label className="block space-y-1.5">
          <span className="text-sm font-medium">{t('preferences.mobileChat.apiKey')}</span>
          <Input
            type="password"
            value={form.apiKey}
            onChange={(event) => setForm((prev) => ({ ...prev, apiKey: event.target.value }))}
            placeholder="sk-..."
            spellCheck={false}
          />
        </label>
        <label className="block space-y-1.5">
          <span className="text-sm font-medium">{t('preferences.mobileChat.model')}</span>
          <Input
            value={form.model}
            onChange={(event) => setForm((prev) => ({ ...prev, model: event.target.value }))}
            placeholder="deepseek-chat"
            spellCheck={false}
          />
        </label>
        <div className="flex items-center justify-between gap-3 pt-1">
          <span className="text-xs text-[var(--muted-foreground)]">
            {ready
              ? t('preferences.mobileChat.ready')
              : t('preferences.mobileChat.notReady')}
          </span>
          <Button size="sm" onClick={() => void save()} disabled={saving || !ready}>
            {saving ? t('preferences.mobileChat.saving') : t('preferences.mobileChat.save')}
          </Button>
        </div>
      </div>
    </div>
  );
}
