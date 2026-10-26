'use client';

import { useState } from 'react';
import { ShieldAlert } from 'lucide-react';

import { plugins } from '@platform/tauri/client';
import { Button } from '@shared/ui/button';
import { usePluginRunStore } from './plugin-run-store';

/**
 * FD-11 v2 P1-5：confirm 档 toolGrants 审批对话框。
 * 插件首次调用 confirm 档工具类别时 run 被终止并弹本框；批准写入
 * 进程级批准集（本会话有效），用户重跑插件后该类别放行。
 */
export function PluginToolConfirmDialog() {
  const confirm = usePluginRunStore((state) => state.toolConfirms[0] ?? null);
  const dismissToolConfirm = usePluginRunStore((state) => state.dismissToolConfirm);
  const [busy, setBusy] = useState(false);

  if (!confirm) return null;

  const approve = async () => {
    if (busy) return;
    setBusy(true);
    try {
      await plugins.toolGrantApprove(confirm.pluginId, confirm.category);
      dismissToolConfirm(confirm.runId);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4">
      <div className="w-full max-w-[420px] rounded-lg border border-[var(--border)] bg-[var(--card)] p-4 shadow-lg">
        <div className="mb-2 flex items-center gap-2">
          <ShieldAlert className="size-4 text-[var(--primary)]" />
          <h3 className="text-sm font-semibold">插件工具授权确认</h3>
        </div>
        <p className="text-xs leading-5 text-[var(--muted-foreground)]">
          插件 <span className="font-mono text-[var(--foreground)]">{confirm.pluginId}</span>{' '}
          请求使用 <span className="font-mono text-[var(--foreground)]">{confirm.category}</span>{' '}
          类工具（confirm 档，首次调用工具{' '}
          <span className="font-mono">{confirm.tool}</span>），本次运行已暂停。
          批准后本次会话内该类别放行，请重新运行插件；拒绝则本次会话内不再放行。
        </p>
        <div className="mt-3 flex justify-end gap-2">
          <Button size="sm" variant="ghost" disabled={busy} onClick={() => dismissToolConfirm(confirm.runId)}>
            拒绝
          </Button>
          <Button size="sm" disabled={busy} onClick={() => void approve()}>
            批准（本会话）
          </Button>
        </div>
      </div>
    </div>
  );
}
