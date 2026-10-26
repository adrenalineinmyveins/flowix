import { subscribe, type SubscribeOptions } from '@platform/tauri/event-bus';
import type { UnlistenFn } from '@tauri-apps/api/event';
import type { PluginRunEvent } from './desktop';

export function listenToPluginRuns(
  handler: (event: PluginRunEvent) => void,
  options?: SubscribeOptions,
): UnlistenFn {
  return subscribe<PluginRunEvent>('plugin-run', handler, options);
}

export function listenToPluginCatalogChanges(
  handler: () => void,
  options?: SubscribeOptions,
): UnlistenFn {
  return subscribe<unknown>('plugin-catalog-changed', handler, options);
}

/** FD-11 v2 P1-5：confirm 档工具类别首用拦截的审批请求。 */
export interface PluginToolConfirmEvent {
  pluginId: string;
  category: string;
  tool: string;
  runId: string;
}

export function listenToPluginToolConfirms(
  handler: (event: PluginToolConfirmEvent) => void,
  options?: SubscribeOptions,
): UnlistenFn {
  return subscribe<PluginToolConfirmEvent>('plugin-tool-confirm', handler, options);
}
