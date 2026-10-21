/**
 * 云端线程变更订阅（方案 A3 桥，桌面端）。
 *
 * 后端 `apply_v2_report` 把远端线程 upsert 进 thread.db 后广播
 * `threads-changed`；本模块收到后刷新内置（default）线程列表，让另一端
 * 的消息/新会话/删除在 UI 上即时可见（纯拉模式列表的 push 补充）。
 *
 * 模式照抄 agent-runtime-store 的 `ensureDshRuntimeStatusSubscription`：
 * 单例 unlisten + 幂等 ensure，由列表加载路径顺带挂载。
 */
import { subscribe } from '@platform/tauri/event-bus';

type ThreadsChangedHandler = () => void;

let unlisten: (() => void) | null = null;

/** 注入刷新回调（store 传入 loadLocalAgentThreadList），避免循环 import。 */
let refreshHandler: ThreadsChangedHandler | null = null;

export function ensureThreadsChangedSubscription(
  onRefresh: ThreadsChangedHandler,
): void {
  refreshHandler = onRefresh;
  if (unlisten) return;
  unlisten = subscribe('threads-changed', () => {
    try {
      refreshHandler?.();
    } catch {
      // 列表刷新失败不打断事件通道；用户手动切面板会重拉。
    }
  });
}
