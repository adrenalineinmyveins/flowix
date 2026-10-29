'use client';

/**
 * 目录卡发布对话框（共享：收件箱面板 + 身份卡页）。
 * 链路：L1 过闸预览 → 确认门 → PUT 中继。AGENT-MARKET M1 起支持
 * `initial` 预填与 agentId 关联（身份卡页从 profile 派生）。
 */
import { useCallback, useState } from 'react';
import { Loader2 } from 'lucide-react';

import { redaction } from '@platform/tauri/client';
import type { DirectoryPreviewResult } from '@/lib/types/redaction';
import { Button } from '@shared/ui/button';
import { Input } from '@shared/ui/input';
import { useI18n } from '@/lib/i18n';

export const VISIBILITY_OPTIONS = ['unlisted', 'public', 'private'] as const;
export const LEVEL_OPTIONS = ['R1', 'R2', 'R3'] as const;

export interface PublishCardInitial {
  /** D-2：关联的本地身份卡（发布档按 agentId 聚合回列表徽章）。 */
  agentId?: string;
  slug?: string;
  displayName?: string;
  tags?: string;
  visibility?: (typeof VISIBILITY_OPTIONS)[number];
  minLevel?: (typeof LEVEL_OPTIONS)[number];
  autoAnswer?: 'off' | 'R3';
}

export function PublishCardDialog({
  notebookPath,
  username,
  initial,
  onClose,
  onPublished,
}: {
  notebookPath: string;
  username: string;
  initial?: PublishCardInitial;
  onClose: () => void;
  /** 发布成功回调（身份卡页刷新徽章）。 */
  onPublished?: () => void;
}) {
  const { t } = useI18n();
  const [slug, setSlug] = useState(initial?.slug ?? '');
  const [displayName, setDisplayName] = useState(initial?.displayName ?? '');
  const [tags, setTags] = useState(initial?.tags ?? '');
  const [visibility, setVisibility] = useState<(typeof VISIBILITY_OPTIONS)[number]>(
    initial?.visibility ?? 'unlisted',
  );
  const [minLevel, setMinLevel] = useState<(typeof LEVEL_OPTIONS)[number]>(initial?.minLevel ?? 'R1');
  // FD-12 M4：能力位随卡发布（判定以本地 auto-grant.json 为准，此位对外可见）。
  const [autoAnswer, setAutoAnswer] = useState<'off' | 'R3'>(initial?.autoAnswer ?? 'off');
  const [preview, setPreview] = useState<DirectoryPreviewResult | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const startPreview = useCallback(async () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      const specialtyTags = tags.split(/[,，]/).map((tag) => tag.trim()).filter(Boolean);
      setPreview(await redaction.directoryPreview(
        notebookPath, slug.trim(), displayName.trim(), specialtyTags, visibility, minLevel,
        autoAnswer, initial?.agentId,
      ));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  }, [busy, notebookPath, slug, displayName, tags, visibility, minLevel, autoAnswer, initial?.agentId]);

  const confirmAndPublish = useCallback(async () => {
    if (!preview || busy) return;
    setBusy(true);
    setError(null);
    try {
      await redaction.directoryConfirm(notebookPath, preview.draft.id, true);
      await redaction.directoryPublish(notebookPath, preview.draft.id);
      setNotice(t('agent.relay.published', { address: `${username}/${slug.trim()}` }));
      setPreview(null);
      onPublished?.();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  }, [preview, busy, notebookPath, username, slug, t, onPublished]);

  const cancelDraft = useCallback(async () => {
    if (!preview) return;
    await redaction.directoryConfirm(notebookPath, preview.draft.id, false).catch(() => undefined);
    setPreview(null);
  }, [preview, notebookPath]);

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4">
      <div className="w-full max-w-[480px] rounded-lg border border-[var(--border)] bg-[var(--card)] p-4 shadow-lg">
        <h3 className="mb-3 text-sm font-semibold">{t('agent.relay.publishTitle')}</h3>
        {error && <p className="mb-2 text-xs text-[var(--destructive)]">{error}</p>}
        {notice && (
          <p className="mb-2 text-xs text-[var(--primary)]">{notice}</p>
        )}

        {preview ? (
          <div className="space-y-3 text-xs">
            <div className="rounded-md border border-[var(--border)] p-3">
              <p className="mb-1 font-medium">{t('agent.relay.previewHeading')}</p>
              <p className="text-[var(--muted-foreground)]">
                {t('agent.relay.previewBefore')}: {preview.original}
              </p>
              <p className="text-[var(--foreground)]">
                {t('agent.relay.previewAfter')}: {preview.draft.displayName} |{' '}
                {preview.draft.specialtyTags.join(', ')}
              </p>
            </div>
            <p className="text-[var(--muted-foreground)]">{t('agent.relay.confirmHint')}</p>
            <div className="flex justify-end gap-2">
              <Button size="sm" variant="ghost" onClick={() => void cancelDraft()}>
                {t('common.cancel')}
              </Button>
              <Button size="sm" onClick={() => void confirmAndPublish()} disabled={busy}>
                {busy ? <Loader2 className="mr-1 size-3.5 animate-spin" /> : null}
                {t('agent.relay.confirmPublish')}
              </Button>
            </div>
          </div>
        ) : (
          <div className="space-y-2">
            <p className="text-xs text-[var(--muted-foreground)]">
              {t('agent.relay.namespaceHint', { username })}
            </p>
            <Input
              placeholder={t('agent.relay.slugPlaceholder')}
              value={slug}
              onChange={(event) => setSlug(event.target.value)}
              className="h-8 text-xs"
            />
            <Input
              placeholder={t('agent.relay.namePlaceholder')}
              value={displayName}
              onChange={(event) => setDisplayName(event.target.value)}
              className="h-8 text-xs"
            />
            <Input
              placeholder={t('agent.relay.tagsPlaceholder')}
              value={tags}
              onChange={(event) => setTags(event.target.value)}
              className="h-8 text-xs"
            />
            <div className="flex gap-2">
              <select
                value={visibility}
                onChange={(event) => setVisibility(event.target.value as typeof visibility)}
                className="h-8 flex-1 rounded-md border border-[var(--border)] bg-transparent px-2 text-xs"
              >
                {VISIBILITY_OPTIONS.map((option) => (
                  <option key={option} value={option}>{option}</option>
                ))}
              </select>
              <select
                value={minLevel}
                onChange={(event) => setMinLevel(event.target.value as typeof minLevel)}
                className="h-8 w-24 rounded-md border border-[var(--border)] bg-transparent px-2 text-xs"
              >
                {LEVEL_OPTIONS.map((option) => (
                  <option key={option} value={option}>{option}</option>
                ))}
              </select>
              <select
                value={autoAnswer}
                onChange={(event) => setAutoAnswer(event.target.value as typeof autoAnswer)}
                className="h-8 w-32 rounded-md border border-[var(--border)] bg-transparent px-2 text-xs"
                title={t('agent.autoGrant.autoAnswer')}
              >
                <option value="off">{t('agent.autoGrant.autoAnswerOff')}</option>
                <option value="R3">{t('agent.autoGrant.autoAnswerR3')}</option>
              </select>
            </div>
            <div className="flex justify-end gap-2 pt-1">
              <Button size="sm" variant="ghost" onClick={onClose}>
                {t('common.close')}
              </Button>
              <Button
                size="sm"
                onClick={() => void startPreview()}
                disabled={busy || !slug.trim() || !displayName.trim()}
              >
                {busy ? <Loader2 className="mr-1 size-3.5 animate-spin" /> : null}
                {t('agent.relay.preview')}
              </Button>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
