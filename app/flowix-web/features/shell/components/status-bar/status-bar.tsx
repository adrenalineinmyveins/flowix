'use client';

import { useCallback, useEffect, useRef, useState } from 'react';
import { CloudUpload, Info, ListTodo } from 'lucide-react';
import type { CSSProperties } from 'react';
import mcpPluginIcon from '@/assets/mcp-plugin.svg';
import { Tooltip } from '@shared/ui/tooltip';
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuItem,
} from '@shared/ui/dropdown-menu';
import type { Notebook } from '@features/memo/store/note-store';
import { NotebookSelectorPopup } from '@features/shell/components/status-bar/notebook-selector-popup';
import { ProductUpdatePill } from '@features/shell/components/status-bar/product-update-pill';
import { ProductIntroDialog } from '@features/shell/components/status-bar/product-intro-dialog';
import {
  AgentConversationStatusBar,
  AgentIcon,
  createAndOpenPiConversation,
} from '@features/agent/public/shell-api';
import { useAgentRuntimeStore } from '@features/agent/store/agent-runtime-store';
import { normalizeAgentRuntimeStatus } from '@features/agent/runtime/agent-runtime-status';
import { useI18n } from '@/lib/i18n';
import { toast } from '@/lib/toast';
import { cloudSyncErrorMessage } from '@platform/tauri/errors';
import { useDocumentMetricsStore } from '@features/document/store/document-metrics-store';
import { useDocumentStore } from '@features/document/store/document-store';
import { useNoteStore } from '@features/memo/store/note-store';
import { CloudStatusIcon } from '@shared/icons/cloud-status-icon';
import { TagSvgIcon } from '@shared/ui/tag-icon';
import {
  cloud,
  listenToCloudStateChanges,
  listenToCloudSyncStatusChanges,
  type CloudSyncStatus,
  type DshDownloadProgress,
} from '@platform/tauri/client';
import type { AppUpdaterState } from '@features/shell/hooks/use-app-updater';

interface StatusBarProps {
  onSelectNotebook: (notebook: Notebook) => void;
  onEditNotebook: (notebook: Notebook) => void;
  onDeleteNotebook: (notebook: Notebook) => void;
  onCreateNotebook: () => void;
  onOpenTodos: () => void;
  onOpenTagPanel: () => void;
  productIntroOpen: boolean;
  onProductIntroOpenChange: (open: boolean) => void;
  onOpenMcpPreferences: () => void;
  onOpenDshPreferences: () => void;
  dshDownload: DshDownloadProgress | null;
  updater: AppUpdaterState;
}

function DshDownloadProgressIcon({ percent }: { percent: number | null | undefined }) {
  const radius = 5;
  const circumference = 2 * Math.PI * radius;
  const progress = percent == null ? 0.25 : Math.min(100, Math.max(0, percent)) / 100;

  return (
    <svg
      aria-hidden="true"
      className={`h-3.5 w-3.5 shrink-0${percent == null ? ' animate-spin' : ''}`}
      viewBox="0 0 12 12"
    >
      <circle
        cx="6"
        cy="6"
        r={radius}
        fill="none"
        stroke="currentColor"
        strokeOpacity="0.25"
        strokeWidth="1.5"
      />
      <circle
        cx="6"
        cy="6"
        r={radius}
        fill="none"
        stroke="currentColor"
        strokeLinecap="round"
        strokeWidth="1.5"
        strokeDasharray={circumference}
        strokeDashoffset={circumference * (1 - progress)}
        transform="rotate(-90 6 6)"
      />
    </svg>
  );
}

function PiRuntimeStatusIndicator() {
  const { t } = useI18n();
  const piStatus = useAgentRuntimeStore((state) => state.statusByType.pi);
  const isChecking = useAgentRuntimeStore((state) => state.isChecking);
  const refreshIfStale = useAgentRuntimeStore((state) => state.refreshIfStale);

  useEffect(() => {
    void refreshIfStale();
  }, [refreshIfStale]);

  const runtimeStatus = normalizeAgentRuntimeStatus(piStatus, isChecking);
  const statusText = runtimeStatus.state === 'ready'
    ? t('agent.status.available')
    : runtimeStatus.state === 'checking'
      ? t('agent.status.checking')
      : runtimeStatus.state === 'unknown'
        ? t('agent.status.notChecked')
        : t('agent.status.setup');
  const label = `${t('agent.types.pi.name')} · ${statusText}`;

  return (
    <Tooltip content={label} side="top">
      <button
        type="button"
        onClick={createAndOpenPiConversation}
        className="h-full flex items-center justify-center px-1.5 py-0 hover:bg-[var(--muted)]"
        aria-label={label}
      >
        <AgentIcon
          typeKey="pi"
          alt=""
          className="h-3.5 w-3.5"
        />
      </button>
    </Tooltip>
  );
}

/**
 * Bottom status bar for the main window.
 *
 * Layout (two columns):
 *   [NotebookSwitcher] | [Todos] [char count]   …flex spacer…   [Note Nav] [AI Chat] [⚙]
 *
 * The left column is the notebook switcher (fixed width by its own button
 * content); the right column takes the remaining width for the status actions.
 *
 * Renders no chrome of its own — it assumes it lives in a `h-[26px]` flex strip.
 */
export function StatusBar({
  onSelectNotebook,
  onEditNotebook,
  onDeleteNotebook,
  onCreateNotebook,
  onOpenTodos,
  onOpenTagPanel,
  productIntroOpen,
  onProductIntroOpenChange,
  onOpenMcpPreferences,
  onOpenDshPreferences,
  dshDownload,
  updater,
}: StatusBarProps) {
  const { t } = useI18n();
  const [notebookPopupOpen, setNotebookPopupOpen] = useState(false);
  const [cloudSyncStatuses, setCloudSyncStatuses] = useState<Map<string, CloudSyncStatus>>(
    () => new Map(),
  );
  const cloudStateRequestRef = useRef(0);
  const [cloudSyncedNotebookIds, setCloudSyncedNotebookIds] = useState<Set<string>>(
    () => new Set(),
  );
  const [cloudSyncAvailable, setCloudSyncAvailable] = useState(false);
  const [cloudPendingOperationCount, setCloudPendingOperationCount] = useState(0);
  const notebooks = useNoteStore((state) => state.notebooks);
  const selectedNotebook = useNoteStore((state) => state.selectedNotebook);
  const setNotebooks = useNoteStore((state) => state.setNotebooks);
  const charCount = useDocumentMetricsStore((state) => state.charCount);

  useEffect(() => {
    const refreshCloudSyncedNotebookIds = () => {
      const requestId = ++cloudStateRequestRef.current;
      void Promise.all([cloud.getState(), cloud.listNotebookStates()])
        .then(([cloudState, links]) => {
          if (requestId !== cloudStateRequestRef.current) return;
          setCloudSyncAvailable(cloudState.authenticated);
          setCloudSyncedNotebookIds(
            new Set(links.filter((link) => link.enabled).map((link) => link.notebookId)),
          );
        })
        .catch(() => {
          if (requestId !== cloudStateRequestRef.current) return;
          setCloudSyncAvailable(false);
          setCloudSyncedNotebookIds(new Set());
        });
    };
    refreshCloudSyncedNotebookIds();
    return listenToCloudStateChanges((cloudState) => {
      setCloudSyncAvailable(cloudState.authenticated);
      refreshCloudSyncedNotebookIds();
    });
  }, []);

  useEffect(() => {
    const handleToggle = () => setNotebookPopupOpen((open) => !open);
    window.addEventListener('flowix:toggle-notebook-switcher', handleToggle);
    return () => window.removeEventListener('flowix:toggle-notebook-switcher', handleToggle);
  }, []);

  useEffect(() => {
    return listenToCloudSyncStatusChanges((status) => {
      setCloudSyncStatuses((previous) => {
        const current = previous.get(status.notebookId);
        if (current && status.startedAt < current.startedAt) return previous;
        const next = new Map(previous);
        next.set(status.notebookId, status);
        return next;
      });
    });
  }, []);

  const cloudSyncInProgress = Array.from(cloudSyncStatuses.values()).some((status) =>
    status.state === 'queued'
    || status.state === 'checking'
    || status.state === 'syncing'
    || status.state === 'finalizing',
  );

  // 立即同步（手动 pass）：全账户范围（threads + 全部已启用笔记本）。
  // 自动 10s 轮询之外的即时入口；进行中禁用防重复排队。
  const handleSyncNow = useCallback(() => {
    void cloud.syncNow().catch((error) => {
      toast.error(cloudSyncErrorMessage(error, t));
    });
  }, [t]);

  // 「复制到云端笔记本」：把当前打开的笔记复制到任一启用云同步的其他
  // 笔记本。语义是加入双向同步（复制后云端/移动端可见，之后双向续传）。
  const activeSession = useDocumentStore((state) => state.activeExternalSession);
  const currentNoteRef = activeSession?.notebookId && activeSession?.relativePath
    ? { notebookId: activeSession.notebookId, relativePath: activeSession.relativePath }
    : null;
  const cloudCopyTargets = currentNoteRef
    ? notebooks.filter((notebook) =>
      cloudSyncedNotebookIds.has(notebook.id) && notebook.id !== currentNoteRef.notebookId)
    : [];
  const copyNoteToCloud = useCallback((targetId: string, targetName: string) => {
    if (!currentNoteRef) return;
    void cloud
      .copyNoteToCloudNotebook(currentNoteRef.notebookId, currentNoteRef.relativePath, targetId)
      .then(() => {
        toast.success(t('shell.statusBar.copyToCloudDone', { name: targetName }));
      })
      .catch((error) => {
        toast.error(cloudSyncErrorMessage(error, t));
      });
  }, [currentNoteRef, t]);
  const handleCopyToCloudClick = useCallback(() => {
    if (cloudCopyTargets.length === 0) {
      toast.info(t('shell.statusBar.copyToCloudNoTarget'));
    } else if (cloudCopyTargets.length === 1) {
      copyNoteToCloud(cloudCopyTargets[0].id, cloudCopyTargets[0].name);
    }
  }, [cloudCopyTargets, copyNoteToCloud, t]);

  useEffect(() => {
    if (!cloudSyncInProgress) {
      setCloudPendingOperationCount(0);
      return;
    }

    let cancelled = false;
    let requestInFlight = false;
    const refreshPendingOperationCount = () => {
      if (requestInFlight) return;
      requestInFlight = true;
      void cloud.listPendingFileOperationCounts()
        .then((counts) => {
          if (cancelled) return;
          setCloudPendingOperationCount(
            Object.values(counts).reduce((total, count) => total + count, 0),
          );
        })
        .catch(() => {
          // Keep the last known count through a transient IPC/database error.
        })
        .finally(() => {
          requestInFlight = false;
        });
    };

    refreshPendingOperationCount();
    const interval = window.setInterval(refreshPendingOperationCount, 1000);
    return () => {
      cancelled = true;
      window.clearInterval(interval);
    };
  }, [cloudSyncInProgress]);

  return (
    <div className="flex h-[26px] shrink-0 select-none items-stretch text-xs text-[var(--muted-foreground)]">
      {/* Left column: notebook switcher (fixed width by its own button content). */}
      <div className="shrink-0 flex items-center">
        <NotebookSelectorPopup
          open={notebookPopupOpen}
          onOpenChange={setNotebookPopupOpen}
          notebooks={notebooks}
          selectedNotebook={selectedNotebook}
          onSelect={onSelectNotebook}
          onEdit={onEditNotebook}
          onDelete={onDeleteNotebook}
          onCreateNotebook={onCreateNotebook}
          onRefresh={setNotebooks}
          cloudSyncedNotebookIds={cloudSyncedNotebookIds}
          cloudSyncAvailable={cloudSyncAvailable}
        />
      </div>
      {/* Right column: full-width content area. */}
      <div className="flex-1 min-w-0 flex items-center gap-0.5 pl-0.5 pr-2">
        <Tooltip content={t('shell.statusBar.noteNavTooltip')} shortcut="panel.noteNavigation.toggle">
          <button
            type="button"
            onClick={onOpenTagPanel}
            className="h-full flex items-center gap-0.5 px-1.5 py-0 hover:bg-[var(--muted)]"
            aria-label={t('shell.statusBar.noteNav')}
          >
            <TagSvgIcon className="w-4 h-4" />
          </button>
        </Tooltip>
        <button
          type="button"
          className="h-full inline-flex items-center gap-0.5 px-1.5 text-[var(--muted-foreground)] hover:bg-[var(--muted)]"
          aria-label={t('status.todos')}
          onClick={onOpenTodos}
        >
          <ListTodo className="w-3.5 h-3.5 shrink-0" />
        </button>
        <AgentConversationStatusBar />
        <div className="flex-1" />
        {dshDownload && (
          <button
            type="button"
            onClick={onOpenDshPreferences}
            className="inline-flex h-[22px] items-center gap-0.5 rounded-md px-2 text-xs leading-none text-[var(--primary)] hover:bg-[var(--muted)]"
            title={t('preferences.dsh.runtime.downloadProgress')}
          >
            <DshDownloadProgressIcon percent={dshDownload.percent} />
            <span>{t('preferences.dsh.runtime.downloading')}</span>
          </button>
        )}
        <ProductUpdatePill updater={updater} />
        {cloudSyncAvailable && cloudCopyTargets.length > 1 && (
          <DropdownMenu>
            <Tooltip content={t('shell.statusBar.copyToCloud')} side="top">
              <DropdownMenuTrigger asChild>
                <button
                  type="button"
                  className="h-full flex items-center justify-center px-1.5 py-0 text-[var(--muted-foreground)] hover:bg-[var(--muted)] hover:text-[var(--foreground)]"
                  aria-label={t('shell.statusBar.copyToCloud')}
                >
                  <CloudUpload className="h-3.5 w-3.5" />
                </button>
              </DropdownMenuTrigger>
            </Tooltip>
            <DropdownMenuContent align="end" side="top">
              {cloudCopyTargets.map((target) => (
                <DropdownMenuItem key={target.id} onClick={() => copyNoteToCloud(target.id, target.name)}>
                  {target.name}
                </DropdownMenuItem>
              ))}
            </DropdownMenuContent>
          </DropdownMenu>
        )}
        {cloudSyncAvailable && cloudCopyTargets.length <= 1 && (
          <Tooltip content={t('shell.statusBar.copyToCloud')} side="top">
            <button
              type="button"
              onClick={handleCopyToCloudClick}
              disabled={!currentNoteRef}
              className="h-full flex items-center justify-center px-1.5 py-0 text-[var(--muted-foreground)] hover:bg-[var(--muted)] hover:text-[var(--foreground)] disabled:cursor-default disabled:opacity-40 disabled:hover:bg-transparent"
              aria-label={t('shell.statusBar.copyToCloud')}
            >
              <CloudUpload className="h-3.5 w-3.5" />
            </button>
          </Tooltip>
        )}
        {cloudSyncAvailable && (
          <Tooltip content={t('preferences.cloud.syncNow')} side="top">
            <button
              type="button"
              onClick={handleSyncNow}
              disabled={cloudSyncInProgress}
              className="h-full flex items-center justify-center px-1.5 py-0 text-[var(--muted-foreground)] hover:bg-[var(--muted)] hover:text-[var(--foreground)] disabled:cursor-default disabled:opacity-60 disabled:hover:bg-transparent"
              aria-label={t('preferences.cloud.syncNow')}
            >
              <CloudStatusIcon status="connected" size={14} />
            </button>
          </Tooltip>
        )}
        {cloudSyncInProgress && (
          <div
            className="inline-flex h-[22px] items-center gap-1 px-1.5 text-xs leading-none text-[var(--muted-foreground)]"
            role="status"
            aria-live="polite"
            aria-label={
              cloudPendingOperationCount > 0
                ? `${t('shell.statusBar.syncing')} ${cloudPendingOperationCount}`
                : t('shell.statusBar.syncing')
            }
          >
            <CloudStatusIcon
              status="connecting"
              size={14}
              className="!opacity-100"
            />
            {cloudPendingOperationCount > 0 && (
              <span className="font-mono tabular-nums">
                {cloudPendingOperationCount}
              </span>
            )}
          </div>
        )}
        {charCount > 0 && (
          <span className="h-full inline-flex items-center gap-0.5 px-1.5 text-[var(--muted-foreground)] hover:bg-[var(--muted)]">
            {t('status.characters')}{' '}
            <span className="font-mono tabular-nums">{charCount}</span>
          </span>
        )}
        <PiRuntimeStatusIndicator />
        <Tooltip content={t('preferences.tabs.mcp')} side="top">
          <button
            type="button"
            onClick={onOpenMcpPreferences}
            className="h-full flex items-center justify-center px-1.5 py-0 hover:bg-[var(--muted)] text-[var(--muted-foreground)] hover:text-[var(--foreground)]"
            aria-label={t('preferences.tabs.mcp')}
          >
            <span
              aria-hidden="true"
              className="h-3.5 w-3.5 shrink-0 bg-current"
              style={{
                mask: `url("${mcpPluginIcon}") center / contain no-repeat`,
                WebkitMask: `url("${mcpPluginIcon}") center / contain no-repeat`,
              } as CSSProperties}
            />
          </button>
        </Tooltip>
        <Tooltip content={t('shell.productIntro.button')} side="top">
          <button
            type="button"
            onClick={() => onProductIntroOpenChange(true)}
            className="h-full flex items-center justify-center px-1.5 py-0 text-[var(--muted-foreground)] hover:bg-[var(--muted)] hover:text-[var(--foreground)]"
            aria-label={t('shell.productIntro.button')}
          >
            <Info className="h-3.5 w-3.5" aria-hidden="true" />
          </button>
        </Tooltip>
      </div>
      <ProductIntroDialog open={productIntroOpen} onOpenChange={onProductIntroOpenChange} />
    </div>
  );
}
