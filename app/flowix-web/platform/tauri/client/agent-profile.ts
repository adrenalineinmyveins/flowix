import { invoke } from '@tauri-apps/api/core';

import type { AgentProfile, AgentState } from '@/lib/types/agent-profile';

// F1 身份卡 IPC（后端 commands/agent_profile.rs）。cwd 必须是笔记本根目录
// 绝对路径。update 的隐私边界参数与 model 为可选 patch：undefined = 不修改，
// redactionLevel / model 传空串 = 清除。
export interface AgentProfileUpdatePatch {
  displayName: string;
  bio: string;
  expertise: string[];
  agentType: string;
  consultable?: boolean;
  redactionLevel?: string;
  disclosureScope?: string[];
  model?: string;
}

export const agentProfile = {
  list: (cwd: string) => invoke<AgentProfile[]>('agent_profile_list', { cwd }),

  get: (cwd: string, agentId: string) =>
    invoke<AgentProfile>('agent_profile_get', { cwd, agentId }),

  create: (
    cwd: string,
    input: { displayName: string; bio: string; expertise: string[]; agentType: string },
  ) =>
    invoke<AgentProfile>('agent_profile_create', {
      cwd,
      displayName: input.displayName,
      bio: input.bio,
      expertise: input.expertise,
      agentType: input.agentType,
    }),

  update: (cwd: string, agentId: string, patch: AgentProfileUpdatePatch) =>
    invoke<AgentProfile>('agent_profile_update', {
      cwd,
      agentId,
      displayName: patch.displayName,
      bio: patch.bio,
      expertise: patch.expertise,
      agentType: patch.agentType,
      consultable: patch.consultable,
      redactionLevel: patch.redactionLevel,
      disclosureScope: patch.disclosureScope,
      model: patch.model,
    }),

  delete: (cwd: string, agentId: string) =>
    invoke<void>('agent_profile_delete', { cwd, agentId }),

  /** 本地运行状态；新 agent 未运行过返回错误码 STATE_NOT_FOUND。 */
  getState: (cwd: string, agentId: string) =>
    invoke<AgentState>('agent_state_get', { cwd, agentId }),
};
