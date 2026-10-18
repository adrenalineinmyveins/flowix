import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import { noteRepository, notebookRepository, type FilterType, type SortType } from '@features/memo/services';
import { getNoteQueryKey } from '@features/memo/services/note-query-key';
import { STORAGE_KEYS } from '@/lib/constants';
import { useTagStore } from '@features/memo/store/tag-store';
import {
  memoMatchesCustomFilter,
  useCustomFilterStore,
  type CustomFilter,
} from '@features/memo/store/custom-filter-store';
import { normalizePluginId } from '@features/plugin/plugin-note';

import type { NoteColor, NoteListItem } from '@/types/note-item';
import type { CreatedNoteDocument } from '@platform/tauri/client';
export { NOTE_COLORS } from '@/types/note-item';

// Clear the retired Memo list UI state when this module loads; Note state uses a new key.
if (typeof window !== 'undefined') {
  try {
    window.localStorage.removeItem('flowix-memo-storage');
  } catch {
    // Storage can be unavailable in restricted browser contexts.
  }
}

// 颜色筛选二级选项。'any' = 任意带色 (memo.colors.length > 0),
// 'none' = 无色 (memo.colors.length === 0), 其它值是具体颜色单选。
// 颜色值会通过独立的后端分页参数下发, 保证颜色筛选和分页结果一致。
export type ColorFilterValue = 'any' | 'none' | NoteColor;

// FilterType 增加了中间列专用的 'color' 维度。后端 filter 仍使用 all,
// 具体颜色通过 color 参数传递。
export type ExtendedFilterType = FilterType | 'color' | 'custom';

/** Which primary surface is shown in the middle column. */
export type MiddleColumnView = 'notes' | 'conversations' | 'agents';

export type NoteLibraryStartupPhase = 'idle' | 'loading' | 'ready' | 'error';

interface MemoListPageQuery {
  notebookId?: string;
  filter: ExtendedFilterType;
  sort: SortType;
  tagId?: string;
  color?: ColorFilterValue;
  pluginId: string | null;
  customFilterId: string | null;
}

const MEMO_LIST_PAGE_SIZE = 50;

export interface SelectedNoteIdentity {
  notebookId: string;
  relativePath: string;
}

// 文档颜色标签 — 跟后端 `NoteColor` 镜像 (`#[serde(rename_all = "lowercase")]`),
// 写入 memo index。单文档可挂多个色, 空数组即"无颜色"。色值在
// `NOTE_COLOR_HEX` 集中维护, picker / 列表 dot 共用。

/**
 * 7 色色板 → 返回 `var(--memo-color-<key>)`, 由 css/theme/{light,dark,rock}.css
 * 各主题文件定义实际 OKLCH 色值。这样:
 *   - 各主题能各自微调 L / C / hue, 暗底提一档亮度、浅底降 chroma 让色
 *     块"嵌进"岩灰底。
 *   - 消费点 (picker 按钮底色 / 列表小圆点) 不需要感知主题 ── 读 `style={{
 *     backgroundColor: NOTE_COLOR_HEX[c] }}` 一致, 浏览器在元素层面解析 var。
 *
 * 历史: 此前是硬编码 hex (Tailwind 500 阶), L=62–80% 偏亮、chroma 中等,
 * 在暗底上不够"立得住"。改 OKLCH + 主题感知后, 整体降 L 6–10%、提 chroma
 * 15–25%, 跨主题色相识别稳定 (hue 不动或偏移 ≤ 8°)。
 */
export const NOTE_COLOR_HEX: Record<NoteColor, string> = {
  red: 'var(--memo-color-red)',
  orange: 'var(--memo-color-orange)',
  yellow: 'var(--memo-color-yellow)',
  green: 'var(--memo-color-green)',
  cyan: 'var(--memo-color-cyan)',
  blue: 'var(--memo-color-blue)',
  gray: 'var(--memo-color-gray)',
};

export interface Notebook {
  id: string;
  name: string;
  icon?: string | null;
  /** Number of notes in the notebook when loaded for card-style selectors. */
  memoCount?: number;
  path: string;
  createdAt: number;
  updatedAt: number;
  isDefault: boolean;
  /** User-defined display order; smaller values appear first. Mirrors the
   * Rust `NotebookConfig.sort` field. */
  sort?: number;
  missing?: boolean;
}

/** 最近在资料文件树中打开的文档。只持久化路径，不缓存文档内容。 */
// 把前端的 `ExtendedFilterType` 转成后端识别的 `FilterType`。
// 'color' 是前端专用, 在后端没有意义 → 退化成 'all' 拉全量, 由前端 store
// 在 useMemo 里按 `colorFilter` 二次过滤。其他值原样下发。
function toBackendFilter(filter: ExtendedFilterType): FilterType {
  return filter === 'color' || filter === 'custom' ? 'all' : filter;
}

function compareNotes(sort: SortType) {
  return (a: NoteListItem, b: NoteListItem) => {
    if (a.favorited !== b.favorited) return Number(b.favorited) - Number(a.favorited);
    if (sort === 'filenameAsc' || sort === 'filenameDesc') {
      const order = a.filename.toLowerCase().localeCompare(b.filename.toLowerCase())
        || a.filename.localeCompare(b.filename)
        || a.relativePath.localeCompare(b.relativePath);
      return sort === 'filenameDesc' ? -order : order;
    }
    const dateOrder = sort === 'updatedAt'
      ? b.updatedAt - a.updatedAt
      : b.createdAt - a.createdAt;
    return dateOrder || b.relativePath.localeCompare(a.relativePath);
  };
}

export interface NoteLibraryStore {
  /** Note list data, keyed by notebook id and relative path. */
  notes: NoteListItem[];
  notebooks: Notebook[];
  /** Whether the backend notebook collection has completed its first load. */
  notebooksInitialized: boolean;
  /** Lifecycle of the main-window notebook + initial memo bootstrap. */
  startupPhase: NoteLibraryStartupPhase;
  startupError: string | null;
  /** Query satisfied by the initial memo load, if startup reached ready. */
  initialMemoQueryKey: string | null;
  // Selection state
  selectedNote: SelectedNoteIdentity | null;
  selectedNotebook: Notebook | null;
  /** Stable persisted identity; the full entity is hydrated from backend data. */
  selectedNotebookId: string | null;
  // UI filter/sort
  middleColumnView: MiddleColumnView;
  /** 个人 Agent 视图当前在右列打开的目标：某个身份卡 / 新建 / 收件箱 / 未打开。 */
  agentProfileViewTarget: { agentId: string } | { creating: true } | { inbox: true } | null;
  /** 身份卡数据版本：创建/保存/删除后自增，驱动中列列表刷新。 */
  agentProfilesVersion: number;
  activeFilter: ExtendedFilterType;
  activePluginId: string | null;
  activeCustomFilterId: string | null;
  activeSort: SortType;
  // 'color' 二级弹窗用的具体颜色值。'any'/'none'/具体颜色 (NOTE_COLORS)。
  // 当 activeFilter !== 'color' 时此值仍然保留, 切回颜色筛选时恢复。
  colorFilter: ColorFilterValue;
  // Reload trigger
  refreshTrigger: number;
  /** Cursor state for the currently loaded memo query. Not persisted. */
  memoListQueryKey: string | null;
  memoListQuery: MemoListPageQuery | null;
  memoListNextCursor: string | null;
  memoListHasMore: boolean;
  memoListLoadingMore: boolean;

  // Setters
  /**
   * Replace the notebook snapshot. When supplied, selectedNotebookId is
   * applied in the same state update so deletion/reconciliation cannot expose
   * a transient `null` selection to notebook synchronization effects.
   */
  setNotebooks: (notebooks: Notebook[], selectedNotebookId?: string | null) => void;
  setStartupPhase: (phase: NoteLibraryStartupPhase, error?: string | null) => void;
  setStartupReady: (initialMemoQueryKey: string) => void;
  setSelectedNote: (identity: SelectedNoteIdentity | null) => void;
  setSelectedNotebook: (notebook: Notebook | null) => void;
  /**
   * Persist a new notebook display order. `nextOrderIds` is the desired
   * sequence; the store assigns sparse sort values internally and replaces
   * the local cache with the backend's response.
   */
  reorderNotebooks: (nextOrderIds: string[]) => Promise<void>;
  setMiddleColumnView: (view: MiddleColumnView) => void;
  setAgentProfileViewTarget: (target: { agentId: string } | { creating: true } | { inbox: true } | null) => void;
  bumpAgentProfiles: () => void;
  setActiveFilter: (filter: ExtendedFilterType) => void;
  setActiveCustomFilter: (filterId: string | null) => void;
  setActivePluginId: (pluginId: string | null) => void;
  setActiveSort: (sort: SortType) => void;
  setColorFilter: (color: ColorFilterValue) => void;
  triggerRefresh: () => void;
  // Data loading
  loadNotes: (params?: { notebookId?: string; filter?: ExtendedFilterType; sort?: SortType; tagId?: string }) => Promise<boolean>;
  upsertCreatedNote: (created: CreatedNoteDocument) => void;
  updateMemoListItem: (memo: NoteListItem) => void;
  loadMoreMemos: () => Promise<boolean>;
  loadNotebooks: () => Promise<void>;
  createNote: (tag: string | undefined, notebookId: string) => Promise<CreatedNoteDocument>;
  /** Invalidate the Note list after a legacy Memo IPC event. */
  handleMemoEvent: () => void;
}


function toNoteListItem(
  note: Awaited<ReturnType<typeof noteRepository.listByPath>>['notes'][number],
  notebookId: string,
): NoteListItem {
  const relativePath = note.relativePath.replace(/\\/g, '/');
  return {
    ...note,
    kind: 'path-note',
    notebookId,
    relativePath,
    filename: relativePath.split('/').pop() || relativePath,
    thumbnail: note.thumbnail,
    properties: note.properties as Record<string, unknown>,
  };
}

export function getVisibleCreateFilter(filter: ExtendedFilterType): ExtendedFilterType {
  return filter === 'agents' || filter === 'todos' || filter === 'color' || filter === 'custom' ? 'all' : filter;
}

// 只恢复侧边栏能够表达的导航入口。颜色 / 时间等筛选属于中间列的
// 临时筛选，持久化它们会导致重启后侧边栏没有任何对应的选中项。
function isSidebarNavigationFilter(
  filter: ExtendedFilterType,
): filter is 'all' | 'agents' | 'todos' | 'tagged' | 'custom' {
  return filter === 'all'
    || filter === 'agents'
    || filter === 'todos'
    || filter === 'tagged'
    || filter === 'custom';
}

let loadMemosRequestSeq = 0;

function getCustomFilter(notebookId: string, filterId: string | null): CustomFilter | null {
  if (!filterId) return null;
  return useCustomFilterStore.getState().filtersByNotebook[notebookId]
    ?.find((filter) => filter.id === filterId) ?? null;
}

function mapMemoPage(
  notes: Awaited<ReturnType<typeof noteRepository.listByPath>>['notes'],
  notebookId: string,
  customFilter: CustomFilter | null,
): NoteListItem[] {
  return notes
    .map((note) => toNoteListItem(note, notebookId))
    .filter((note) => !customFilter || memoMatchesCustomFilter(note, customFilter));
}

function mergeMemoListItems(
  existing: readonly NoteListItem[],
  incoming: readonly NoteListItem[],
): NoteListItem[] {
  const byPath = new Map(existing.map((note) => [
    `${note.notebookId}\u0000${note.relativePath}`,
    note,
  ]));
  for (const note of incoming) {
    byPath.set(`${note.notebookId}\u0000${note.relativePath}`, note);
  }
  return [...byPath.values()];
}

function invalidatePendingMemoLoads(): void {
  loadMemosRequestSeq += 1;
}

export const useNoteStore = create<NoteLibraryStore>()(
  persist(
    (set, get) => ({
      notes: [],
      notebooks: [],
      notebooksInitialized: false,
      startupPhase: 'idle',
      startupError: null,
      initialMemoQueryKey: null,
      selectedNote: null,
      selectedNotebook: null,
      selectedNotebookId: null,
      middleColumnView: 'notes',
      agentProfileViewTarget: null,
      agentProfilesVersion: 0,
      activeFilter: 'all',
      activePluginId: null,
      activeCustomFilterId: null,
      activeSort: 'createdAt',
      colorFilter: 'any',
      refreshTrigger: 0,
      memoListQueryKey: null,
      memoListQuery: null,
      memoListNextCursor: null,
      memoListHasMore: false,
      memoListLoadingMore: false,

      setNotebooks: (notebooks, selectedNotebookIdOverride) => set((state) => {
        // Prefer the persisted id. The object fallback keeps tests and
        // pre-migration in-memory callers compatible while the first backend
        // snapshot is being applied.
        const selectedNotebookId = selectedNotebookIdOverride === undefined
          ? state.selectedNotebookId ?? state.selectedNotebook?.id ?? null
          : selectedNotebookIdOverride;
        const selectedNotebook = selectedNotebookId
          ? notebooks.find((notebook) => notebook.id === selectedNotebookId) ?? null
          : null;
        return {
          notebooks,
          selectedNotebook,
          selectedNotebookId: selectedNotebook?.id ?? null,
          selectedNote: state.selectedNote
            && state.selectedNote.notebookId === selectedNotebook?.id
            && notebooks.some((notebook) => notebook.id === state.selectedNote?.notebookId)
            ? state.selectedNote
            : null,
          notebooksInitialized: true,
        };
      }),
      setStartupPhase: (startupPhase, startupError = null) => set({
        startupPhase,
        startupError,
        ...(startupPhase === 'loading' ? { initialMemoQueryKey: null } : {}),
      }),
      setStartupReady: (initialMemoQueryKey) => set({
        startupPhase: 'ready',
        startupError: null,
        initialMemoQueryKey,
      }),
      setSelectedNote: (selectedNote) => set({ selectedNote }),
      setSelectedNotebook: (notebook) => {
        const currentNotebookId = get().selectedNotebookId
          ?? get().selectedNotebook?.id
          ?? null;
        const nextNotebookId = notebook?.id ?? null;
        if (currentNotebookId !== nextNotebookId) {
          useTagStore.getState().setSelectedTagId(null);
          set({
            selectedNotebook: notebook,
            selectedNotebookId: nextNotebookId,
            selectedNote: null,
            notes: [],
            middleColumnView: 'notes',
            activeFilter: 'all',
            activePluginId: null,
            activeCustomFilterId: null,
          });
          return;
        }
        set({ selectedNotebook: notebook, selectedNotebookId: nextNotebookId });
      },
      // Surface navigation is independent from note filtering. Keep the last
      // note filter intact while the user visits the conversation surface.
      setMiddleColumnView: (view) => {
        const previous = get();
        if (previous.middleColumnView === view) return;
        set({
          middleColumnView: view,
          ...(view === 'notes' && previous.activeFilter === 'agents'
            ? { activeFilter: 'all', activeCustomFilterId: null, activePluginId: null }
            : {}),
        });
      },
      setAgentProfileViewTarget: (target) => {
        set({ agentProfileViewTarget: target });
      },
      bumpAgentProfiles: () => {
        set((state) => ({ agentProfilesVersion: state.agentProfilesVersion + 1 }));
      },
      setActiveFilter: (filter) => {
        const previous = get();
        const selectedTagId = useTagStore.getState().selectedTagId;
        const shouldClearTag = filter !== 'tagged';
        const nextView: MiddleColumnView = filter === 'agents' ? 'conversations' : 'notes';
        // Artifact plugins use `activeFilter: 'all'` as their list fallback.
        // Clicking the notes entry must still leave that plugin view, even
        // when the filter value itself is already `all`.
        if (
          previous.activeFilter === filter
          && previous.middleColumnView === nextView
          && previous.activePluginId === null
          && (!shouldClearTag || selectedTagId === null)
        ) return;
        set({
          middleColumnView: nextView,
          activeFilter: filter,
          activePluginId: null,
          ...(filter === 'custom' ? {} : { activeCustomFilterId: null }),
        });
        if (shouldClearTag && selectedTagId !== null) {
          useTagStore.getState().setSelectedTagId(null);
        }
      },
      setActiveCustomFilter: (filterId) => {
        if (!filterId) {
          get().setActiveFilter('all');
          return;
        }
        const filter = useCustomFilterStore.getState().filtersByNotebook[get().selectedNotebook?.id ?? '']
          ?.find((item) => item.id === filterId);
        if (!filter) return;
        useTagStore.getState().setSelectedTagId(null);
        set({
          middleColumnView: 'notes',
          activeFilter: 'custom',
          activeCustomFilterId: filterId,
          activePluginId: null,
        });
      },
      setActivePluginId: (pluginId) => set({
        activePluginId: pluginId,
      }),
      setActiveSort: (sort) => set({ activeSort: sort }),
      setColorFilter: (color) => set({ colorFilter: color }),
      triggerRefresh: () => set((state) => ({ refreshTrigger: state.refreshTrigger + 1 })),

      loadNotes: async (params) => {
        const requestSeq = ++loadMemosRequestSeq;
        const state = get();
        const notebookId = params?.notebookId || state.selectedNotebook?.id;
        if (!notebookId) return false;
        await useCustomFilterStore.getState().loadNotebookFilters(notebookId);
        if (requestSeq !== loadMemosRequestSeq) return false;
        const filter = params?.filter || state.activeFilter;
        const pluginId = state.activePluginId;
        const customFilterId = filter === 'custom' ? state.activeCustomFilterId : null;
        const sort = params?.sort || state.activeSort;
        const tagId = params?.tagId
          ?? (filter === 'tagged' ? useTagStore.getState().selectedTagId ?? undefined : undefined);
        const color = filter === 'color' ? state.colorFilter : undefined;
        const queryKey = getNoteQueryKey(
          notebookId,
          filter,
          sort,
          tagId ?? null,
          color ?? state.colorFilter,
          pluginId,
          customFilterId,
        );
        const customFilter = getCustomFilter(notebookId, customFilterId);
        const query: MemoListPageQuery & { notebookId: string } = {
          notebookId,
          filter,
          sort,
          tagId,
          color,
          pluginId,
          customFilterId,
        };
        try {
          if (query.pluginId) {
            const indexed = await noteRepository.listAllByPath(query.notebookId);
            if (requestSeq !== loadMemosRequestSeq) return false;
            set({
              notes: indexed
                .filter((note) => normalizePluginId(note.properties.flowix_plugin) === query.pluginId)
                .map((note) => toNoteListItem(note, query.notebookId)),
              memoListQueryKey: queryKey,
              memoListQuery: query,
              memoListNextCursor: null,
              memoListHasMore: false,
            });
            return true;
          }

          const response = await noteRepository.listByPath({
            notebookId: query.notebookId,
            filter: toBackendFilter(query.filter),
            sort: query.sort,
            tagId: query.tagId,
            color: query.color,
            limit: MEMO_LIST_PAGE_SIZE,
          });
          if (requestSeq !== loadMemosRequestSeq) return false;
          const notes = mapMemoPage(response.notes, query.notebookId, customFilter);

          set({
            notes,
            memoListQueryKey: queryKey,
            memoListQuery: query,
            memoListNextCursor: response.nextCursor ?? null,
            memoListHasMore: response.hasMore ?? Boolean(response.nextCursor),
          });
          return true;
        } finally {
          if (requestSeq === loadMemosRequestSeq) {
            set({ memoListLoadingMore: false });
          }
        }
      },

      upsertCreatedNote: (created) => {
        const state = get();
        if (state.selectedNotebook?.id !== created.notebookId || state.activePluginId) return;
        const note = toNoteListItem(created.entry, created.notebookId);
        const filter = state.activeFilter;
        if (filter === 'favorited' && !note.favorited) return;
        if (filter === 'todos' && note.todos.length === 0) return;
        if (filter === 'agents' && note.agents.length === 0) return;
        if (filter === 'tagged') {
          const tag = useTagStore.getState().selectedTagId;
          if (!tag || !note.tags.some((value) => value === tag || value.startsWith(`${tag}/`))) return;
        }
        if (filter === 'custom') {
          const customFilter = useCustomFilterStore.getState().filtersByNotebook[created.notebookId]
            ?.find((item) => item.id === state.activeCustomFilterId);
          if (!customFilter || !memoMatchesCustomFilter(note, customFilter)) return;
        }
        invalidatePendingMemoLoads();
        set((current) => ({
          notes: [
            ...current.notes.filter((item) => (
              item.notebookId !== created.notebookId || item.relativePath !== note.relativePath
            )),
            note,
          ].sort(compareNotes(current.activeSort)),
          memoListLoadingMore: false,
        }));
      },

      updateMemoListItem: (memo) => set((state) => {
        const index = state.notes.findIndex((item) => (
          item.notebookId === memo.notebookId && item.relativePath === memo.relativePath
        ));
        if (index < 0) return state;

        const query = state.memoListQuery;
        const noLongerMatchesFavoriteFilter = query?.filter === 'favorited' && !memo.favorited;
        const noLongerMatchesColorFilter = query?.filter === 'color' && query.color
          ? query.color === 'none'
            ? memo.colors.length > 0
            : query.color !== 'any' && !memo.colors.includes(query.color)
          : false;
        const notes = [...state.notes];
        if (noLongerMatchesFavoriteFilter || noLongerMatchesColorFilter) {
          notes.splice(index, 1);
        } else {
          notes[index] = memo;
          notes.sort(compareNotes(query?.sort ?? state.activeSort));
        }
        return { notes };
      }),

      loadMoreMemos: async () => {
        const state = get();
        const query = state.memoListQuery;
        if (
          !state.memoListHasMore
          || state.memoListLoadingMore
          || !state.memoListNextCursor
          || !state.memoListQueryKey
          || !query?.notebookId
          || query.pluginId
        ) {
          return false;
        }

        // Do not let a scroll event from the previous query append into a new
        // notebook/filter while its first page is still in flight.
        const currentQueryKey = getNoteQueryKey(
              state.selectedNotebook?.id,
              state.activeFilter,
              state.activeSort,
              state.activeFilter === 'tagged'
                ? useTagStore.getState().selectedTagId
                : null,
              state.colorFilter,
              state.activePluginId,
              state.activeFilter === 'custom' ? state.activeCustomFilterId : null,
            );
        if (currentQueryKey !== state.memoListQueryKey) return false;

        const requestSeq = ++loadMemosRequestSeq;
        const cursor = state.memoListNextCursor;
        set({ memoListLoadingMore: true });
        try {
          const response = await noteRepository.listByPath({
            notebookId: query.notebookId,
            filter: toBackendFilter(query.filter),
            sort: query.sort,
            tagId: query.tagId,
            color: query.color,
            cursor,
            limit: MEMO_LIST_PAGE_SIZE,
          });
          if (requestSeq !== loadMemosRequestSeq) return false;

          const customFilter = getCustomFilter(query.notebookId, query.customFilterId);
          const notes = mapMemoPage(response.notes, query.notebookId, customFilter);
          set((current) => ({
            notes: mergeMemoListItems(current.notes, notes),
            memoListNextCursor: response.nextCursor ?? null,
            memoListHasMore: response.hasMore ?? Boolean(response.nextCursor),
          }));
          return true;
        } finally {
          if (requestSeq === loadMemosRequestSeq) {
            set({ memoListLoadingMore: false });
          }
        }
      },

      loadNotebooks: async () => {
        const nbList = await notebookRepository.list();
        get().setNotebooks(nbList as Notebook[]);
      },
      /**
       * Reorder notebooks by submitting the new id order to the backend.
       * `nextOrderIds` is the desired sequence of notebook ids; the action
       * assigns sort = (index + 1) * 10 (step 10 keeps room for future
       * inserts) and replaces the local cache with the backend's response
       * so that any normalization logic stays server-authoritative.
       */
      reorderNotebooks: async (nextOrderIds: string[]) => {
        if (nextOrderIds.length === 0) return;
        const order = nextOrderIds.map((id, index) => ({
          id,
          sort: (index + 1) * 10,
        }));
        try {
          const updated = await notebookRepository.reorder(order);
          set({ notebooks: updated as Notebook[] });
        } catch (error) {
          // 失败时重新拉一次 list 跟服务端对齐 (notebook 列表较短, 直接重拉比
          // 维护本地乐观回滚更稳)。
          console.error('[reorderNotebooks] failed', error);
          const nbList = await notebookRepository.list();
          set({ notebooks: nbList as Notebook[] });
        }
      },

      createNote: async (tag, notebookId) => {
        // v4: 不再 markLocalMemoCreated — 后端 SelfWriteSuppressor 把
        // desktop 自写的 memo-event 在 watcher 端就掐掉, 不再到前端。
        // 事件去重/抑制由后端统一负责, 前端 store 不需要任何补丁。
        const state = get();
        const selectedTagId = useTagStore.getState().selectedTagId;
        const createFilter = getVisibleCreateFilter(state.activeFilter);
        if (createFilter !== state.activeFilter) {
          useTagStore.getState().setSelectedTagId(null);
          set({ activeFilter: createFilter });
        }
        const createTag = tag ?? (createFilter === 'tagged' ? selectedTagId ?? undefined : undefined);
        const result = await noteRepository.create(createTag, notebookId);
        invalidatePendingMemoLoads();
        await get().loadNotes({ notebookId, filter: createFilter });
        // 新建 memo 可能引入新 tag (body 派生) ── 主动 bump metadata refresh,
        // 让侧栏标签树立即出现新节点 / 更新计数。后端 SelfWriteSuppressor 会
        // 掐掉 desktop 自写的 memo-event, 不会自动触发 refresh, 必须手动调。
        useTagStore.getState().triggerMetadataRefresh();
        return result;
      },
      // The IPC event is still Memo-shaped for compatibility, but the store
      // only invalidates and reloads its path-keyed Note projection.
      handleMemoEvent: () => get().triggerRefresh(),
    }),
    {
      name: STORAGE_KEYS.NOTE,
      partialize: (state) => ({
        // Persist identities only. Notebook and Note entities are backend data
        // and may be renamed, deleted, or updated while the app is closed.
        selectedNotebookId: state.selectedNotebookId ?? state.selectedNotebook?.id ?? null,
        selectedNote: state.selectedNote,
        // 侧边栏入口要和中间列一起恢复。中间列的颜色 / 时间筛选不属于
        // 侧边栏导航，因此恢复时归位到“全部”。
        middleColumnView: state.middleColumnView,
        activeFilter: isSidebarNavigationFilter(state.activeFilter)
          ? state.activeFilter
          : 'all',
        activeCustomFilterId: state.activeFilter === 'custom'
          ? state.activeCustomFilterId
          : null,
      }),
    }
  )
);
