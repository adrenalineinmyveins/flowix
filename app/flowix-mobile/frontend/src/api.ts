// Tauri IPC 封装（withGlobalTauri = true）。

declare global {
  interface Window {
    __TAURI__?: {
      core: { invoke: <T>(cmd: string, args?: Record<string, unknown>) => Promise<T> };
      event: { listen: <T>(event: string, handler: (ev: { payload: T }) => void) => Promise<() => void> };
    };
  }
}

const invoke = <T,>(cmd: string, args?: Record<string, unknown>) =>
  window.__TAURI__!.core.invoke<T>(cmd, args);

// ── 类型 ───────────────────────────────────────────────────────────────────

export interface CloudUser {
  id: string;
  email: string;
  displayName: string;
}

export interface CloudState {
  revision: number;
  authenticated: boolean;
  authStatus: 'signed_out' | 'restoring' | 'authenticated';
  authError?: string;
  account?: { user: CloudUser; protocolEpoch: number };
}

export interface NotebookInfo {
  id: string;
  name: string;
  icon?: string | null;
  sort: number;
}

export interface NoteInfo {
  id: string;
  relativePath: string;
  filename: string;
  preview: string;
  updatedAt: number;
}

export interface SearchHit {
  notebookId: string;
  relativePath: string;
  title: string;
  snippet: string;
}

export interface SyncSummary {
  notebooks: number;
  uploaded: number;
  downloaded: number;
  deleted: number;
}

export interface AttachmentSaved {
  relativePath: string;
  absolutePath: string;
}

export interface HistoryEntry {
  revision: string;
  deleted: boolean;
  sizeBytes: number;
  syncSeq: number;
  createdAt: number;
}

export interface UpdateCheck {
  currentVersion: string;
  latestVersion: string;
  downloadUrl: string;
  notes: string;
  hasUpdate: boolean;
}

// ── AI 对话（方案 A4；AgentChunk 字段 snake_case，镜像 Rust wire）─────────

export interface AgentProviderConfig {
  apiBase: string;
  apiKey: string;
  model: string;
}

export interface ThreadSummary {
  threadId: string;
  title: string;
  agentId: string;
  updatedAt: number;
  messageCount: number;
  starred: boolean;
  archived: boolean;
}

export interface ChatAttachment {
  fileName: string;
  storageKey: string;
}

export interface ThreadMessage {
  id: string;
  role: string;
  content: string;
  reasoning?: string | null;
  toolName?: string | null;
  toolInput?: string | null;
  toolData?: string | null;
  createdAt: number;
  origin?: string | null;
  attachments?: ChatAttachment[];
}

export type AgentChunk =
  | { kind: 'stream_start'; thread_id: string }
  | { kind: 'text'; thread_id: string; delta: string }
  | { kind: 'reasoning'; thread_id: string; delta: string }
  | { kind: 'error'; thread_id: string; message: string }
  | { kind: 'stream_end'; thread_id: string };

// ── 命令 ───────────────────────────────────────────────────────────────────

export const api = {
  login: (email: string, password: string) =>
    invoke<CloudState>('cloud_login', { email, password }),
  register: (email: string, password: string, displayName: string) =>
    invoke<CloudState>('cloud_register', { email, password, displayName }),
  logout: () => invoke<CloudState>('cloud_logout'),
  cloudState: () => invoke<CloudState>('cloud_get_state'),
  cloudGetApiBase: () => invoke<string>('cloud_get_api_base'),
  cloudSetApiBase: (apiBase: string) => invoke<string>('cloud_set_api_base', { apiBase }),

  refreshNotebooks: () => invoke<NotebookInfo[]>('cloud_refresh_notebooks'),
  syncNow: () => invoke<SyncSummary>('cloud_sync_now'),

  noteList: (notebookId: string) => invoke<NoteInfo[]>('note_list', { notebookId }),
  noteRead: (notebookId: string, relativePath: string) =>
    invoke<string>('note_read', { notebookId, relativePath }),
  noteSave: (notebookId: string, relativePath: string, content: string) =>
    invoke<void>('note_save', { notebookId, relativePath, content }),
  noteCreate: (notebookId: string, title: string) =>
    invoke<string>('note_create', { notebookId, title }),
  noteDelete: (notebookId: string, relativePath: string) =>
    invoke<void>('note_delete', { notebookId, relativePath }),
  attachmentSave: (notebookId: string, fileName: string, dataBase64: string) =>
    invoke<AttachmentSaved>('attachment_save', { notebookId, fileName, dataBase64 }),
  attachmentRead: (notebookId: string, storageKey: string) =>
    invoke<string>('attachment_read', { notebookId, storageKey }),
  noteHistory: (notebookId: string, relativePath: string) =>
    invoke<HistoryEntry[]>('note_history', { notebookId, relativePath }),
  noteHistoryRead: (notebookId: string, relativePath: string, revision: string) =>
    invoke<string>('note_history_read', { notebookId, relativePath, revision }),
  checkUpdate: () => invoke<UpdateCheck>('app_check_update'),
  openUrl: (url: string) => invoke<void>('app_open_url', { url }),
  search: (query: string) => invoke<SearchHit[]>('search', { query }),

  agentProviderGet: () => invoke<AgentProviderConfig>('agent_provider_get'),
  agentProviderSet: (config: AgentProviderConfig) =>
    invoke<void>('agent_provider_set', { config }),
  agentThreadList: () => invoke<ThreadSummary[]>('agent_thread_list'),
  agentThreadMessages: (threadId: string) =>
    invoke<ThreadMessage[]>('agent_thread_messages', { threadId }),
  agentThreadDelete: (threadId: string) =>
    invoke<boolean>('agent_thread_delete', { threadId }),
  agentThreadUpdate: (
    threadId: string,
    patch: { title?: string; starred?: boolean; archived?: boolean },
  ) => invoke<void>('agent_thread_update', { threadId, ...patch }),
  agentChatSend: (
    threadId: string | null,
    content: string,
    noteContext?: { notebookId: string; relativePath: string } | null,
    attachments?: ChatAttachment[],
  ) =>
    invoke<string>('agent_chat_send', {
      threadId,
      content,
      noteContext: noteContext ?? undefined,
      attachments: attachments && attachments.length > 0 ? attachments : undefined,
    }),
  agentChatStop: (threadId: string) => invoke<void>('agent_chat_stop', { threadId }),
};

export function listenEvent<T>(event: string, handler: (payload: T) => void): Promise<() => void> {
  return window.__TAURI__!.event.listen<T>(event, (ev) => handler(ev.payload));
}
