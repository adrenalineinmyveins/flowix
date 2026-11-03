import { useCallback } from 'react';
import { createPortal } from 'react-dom';

import { toast } from '@/lib/toast';
import { useExternalFileDrop } from '@features/document/public/shell-api';
import { errorMessage } from '@/lib/error-message';
import { createLogger } from '@/lib/logger';
import { useShellSelectedNotebook } from '@features/memo/public/shell-api';
import {
  openBrowserColumnDroppedFile,
  openExternalTarget,
} from '@features/workspace/public/shell-api';
import {
  FLOWIX_EXTERNAL_ENEX_IMPORT_EVENT,
  type ExternalEnexImportRequest,
} from '@platform/open-target/types';
import { FullscreenDragOverlay } from './fullscreen-drag-overlay';

const logger = createLogger('external-file-drop-overlay');

export function ExternalFileDropOverlay() {
  const selectedNotebook = useShellSelectedNotebook();
  const handleDropError = useCallback((error: unknown) => {
    logger.warn('failed to open dropped file', { error });
    toast.error(errorMessage(error));
  }, []);
  const handleDropPaths = useCallback(async (paths: string[], destination?: 'main-third' | 'browser-column') => {
    if (paths.length === 0) return;
    // ENEX（Evernote 交换格式）整包导入走独立对话框：选笔记本后批量建笔记。
    const enexPaths = paths.filter((path) => /\.enex$/i.test(path));
    const otherPaths = enexPaths.length === 0 ? paths : paths.filter((path) => !/\.enex$/i.test(path));
    if (enexPaths.length > 0) {
      window.dispatchEvent(new CustomEvent<ExternalEnexImportRequest>(FLOWIX_EXTERNAL_ENEX_IMPORT_EVENT, {
        detail: { filePaths: enexPaths },
      }));
    }
    for (const path of otherPaths) {
      if (destination === 'browser-column') {
        await openBrowserColumnDroppedFile(path);
      } else {
        await openExternalTarget(path, {
          destination: 'main-third',
          scopePath: selectedNotebook?.path ?? null,
        });
      }
    }
  }, [selectedNotebook?.path]);
  const { isDraggingFile, draggingDestination } = useExternalFileDrop({
    onDropPaths: handleDropPaths,
    onDropError: handleDropError,
  });

  const host = draggingDestination
    ? document.querySelector<HTMLElement>(`[data-workspace-host="${draggingDestination}"]`)
    : null;
  return host ? createPortal(<FullscreenDragOverlay visible={isDraggingFile} />, host) : null;
}
