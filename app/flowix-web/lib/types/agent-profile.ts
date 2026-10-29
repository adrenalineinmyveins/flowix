/**
 * F1 个人 Agent 身份卡类型 — 镜像后端
 * `flowix-core/src/agent/mod.rs` 的 `AgentProfile` / `AgentState`。
 *
 * 注意：`AgentProfile` 的 serde 字段名是 `flowix_` 前缀（直接对应
 * profile.md frontmatter 系统键），不是 camelCase —— 此处按线上格式
 * 一比一镜像。`AgentState` 是 camelCase（state.json）。
 * 身份卡路径：`<notebook>/.agents/<agentId>/profile.md`（随 v2 云同步）；
 * 运行状态路径：`<notebook>/.flowix/agents/<agentId>/state.json`（仅本地）。
 */

/** frontmatter 键与 IPC JSON 键一致（flowix_ 前缀）。 */
export interface AgentProfile {
  flowix_agent_id: string;
  flowix_display_name: string;
  flowix_bio: string;
  flowix_expertise: string[];
  flowix_agent_type: string;
  flowix_model: string | null;
  flowix_consultable: boolean;
  flowix_redaction_level: string | null;
  /** 旧字段已停止序列化（P1 写废弃）——仅历史载荷可能出现。 */
  flowix_disclosure_scope?: string[];
  /** AGENT-PROFILE-REDESIGN：扩展字段（serde default，旧档可为空）。 */
  flowix_system?: string;
  flowix_tools?: string[];
  flowix_skills?: string[];
  flowix_metadata?: Record<string, string>;
  /** 记忆库即披露授权：tag:… / note:<nb>::<path> / collection:<id>。 */
  flowix_memory?: string[];
  flowix_created_at: number;
  flowix_updated_at: number;
}

/** 本地运行状态（camelCase）。 */
export interface AgentState {
  version: number;
  agentId: string;
  lastRunAt: number | null;
  lastConsultedAt: number | null;
  runs: number;
  consultationsReceived: number;
  tokensIn: number;
  tokensOut: number;
  recentThreadIds: string[];
  localPreferences: unknown;
}

/** 被咨询时允许的最高披露等级（空 = 拒答）。文案见 i18n agent.profile.level.*。 */
export const PROFILE_REDACTION_LEVELS = ['', 'R1', 'R2', 'R3'] as const;
