import { subscribe, type SubscribeOptions } from '@platform/tauri/event-bus';
import { invoke } from '@tauri-apps/api/core';
import type { UnlistenFn } from '@tauri-apps/api/event';
import type { PluginRunEvent } from './desktop';

/** F3 技能注册表条目（commands/skill.rs skill_registry_list）。 */
export interface SkillDescriptor {
  source: string;
  id: string;
  displayName: string;
  description: string;
  kind: string;
  publishState: string;
  enabled: boolean;
}

export function skillRegistryList(cwd?: string): Promise<SkillDescriptor[]> {
  return invoke<SkillDescriptor[]>('skill_registry_list', { cwd });
}

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
