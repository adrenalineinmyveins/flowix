import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  Bot,
  ChevronDown,
  CloudOff,
  FileText,
  Notebook,
  Plus,
  RefreshCw,
  Search,
  Settings as SettingsIcon,
  Check,
  Trash2,
  MessageSquare,
} from 'lucide-react';
import { api, NotebookInfo, NoteInfo } from './api';
import { OpenNote } from './App';

interface Props {
  notebooks: NotebookInfo[];
  registerRefresh: (fn: () => void) => void;
  onOpenNote: (note: OpenNote) => void;
  onOpenSettings: () => void;
  onOpenSearch: () => void;
  onOpenChats: () => void;
  onOpenAgents: () => void;
}

/** 相对时间（桌面文案风格：刚刚 / N 分钟前 / N 小时前 / 日期）。 */
function relativeTime(updatedAt: number): string {
  if (!updatedAt) return '';
  const diff = Date.now() - updatedAt;
  if (diff < 60_000) return '刚刚';
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} 分钟前`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)} 小时前`;
  const date = new Date(updatedAt);
  const sameYear = date.getFullYear() === new Date().getFullYear();
  return `${sameYear ? '' : `${date.getFullYear()}年`}${date.getMonth() + 1}月${date.getDate()}日`;
}

const PULL_THRESHOLD = 64;

export function NotesScreen({
  notebooks,
  registerRefresh,
  onOpenNote,
  onOpenSettings,
  onOpenSearch,
  onOpenChats,
  onOpenAgents,
}: Props) {
  const [activeNotebook, setActiveNotebook] = useState<string>('');
  const [notes, setNotes] = useState<NoteInfo[]>([]);
  const [creating, setCreating] = useState(false);
  const [showCreate, setShowCreate] = useState(false);
  const [newTitle, setNewTitle] = useState('');
  const [syncing, setSyncing] = useState(false);
  const [syncError, setSyncError] = useState('');
  const [lastSyncNote, setLastSyncNote] = useState('');
  const [showNotebooks, setShowNotebooks] = useState(false);
  const [selected, setSelected] = useState('');
  const [deleting, setDeleting] = useState(false);
  const [pullY, setPullY] = useState(0);
  const listRef = useRef<HTMLDivElement>(null);
  const pullStart = useRef<number | null>(null);

  useEffect(() => {
    if (!activeNotebook && notebooks.length > 0) setActiveNotebook(notebooks[0].id);
  }, [notebooks, activeNotebook]);

  const reload = useCallback(async () => {
    if (!activeNotebook) return;
    try {
      setNotes(await api.noteList(activeNotebook));
    } catch (e) {
      setSyncError(String(e));
    }
  }, [activeNotebook]);

  useEffect(() => {
    reload();
  }, [reload]);

  useEffect(() => {
    registerRefresh(reload);
  }, [registerRefresh, reload]);

  const current = useMemo(
    () => notebooks.find((n) => n.id === activeNotebook),
    [notebooks, activeNotebook],
  );

  const doSync = useCallback(async () => {
    if (syncing) return;
    setSyncing(true);
    setSyncError('');
    try {
      const summary = await api.syncNow();
      setLastSyncNote(
        `已同步 · 上传 ${summary.uploaded} · 下载 ${summary.downloaded} · 删除 ${summary.deleted}`,
      );
      if (activeNotebook) setNotes(await api.noteList(activeNotebook));
    } catch (e) {
      setSyncError(String(e));
    } finally {
      setSyncing(false);
    }
  }, [syncing, activeNotebook]);

  const doCreate = async () => {
    if (!activeNotebook || creating) return;
    setCreating(true);
    try {
      const relativePath = await api.noteCreate(activeNotebook, newTitle || '未命名笔记');
      setNewTitle('');
      setShowCreate(false);
      await reload();
      onOpenNote({ notebookId: activeNotebook, relativePath });
    } catch (e) {
      setSyncError(String(e));
    } finally {
      setCreating(false);
    }
  };

  const doDelete = async () => {
    if (!activeNotebook || !selected || deleting) return;
    setDeleting(true);
    try {
      await api.noteDelete(activeNotebook, selected);
      setSelected('');
      await reload();
    } catch (e) {
      setSyncError(String(e));
    } finally {
      setDeleting(false);
    }
  };

  // 下拉刷新：列表在顶部时下拉超阈值触发同步
  const onTouchStart = (e: React.TouchEvent) => {
    if ((listRef.current?.scrollTop ?? 1) <= 0) pullStart.current = e.touches[0].clientY;
    else pullStart.current = null;
  };
  const onTouchMove = (e: React.TouchEvent) => {
    if (pullStart.current === null) return;
    const delta = e.touches[0].clientY - pullStart.current;
    if (delta > 0) setPullY(Math.min(delta * 0.4, 88));
  };
  const onTouchEnd = () => {
    if (pullY >= PULL_THRESHOLD) void doSync();
    setPullY(0);
    pullStart.current = null;
  };

  return (
    <div className="screen">
      <header className="m-bar">
        <button className="notes-bar-btn" onClick={() => setShowNotebooks(true)}>
          <Notebook size={18} />
          <span className="name">{current?.name || '笔记本'}</span>
          <ChevronDown />
        </button>
        <div style={{ flex: 1 }} />
        <button
          className={`m-icon-btn ${syncing ? 'spin' : ''}`}
          onClick={doSync}
          aria-label="立即同步"
        >
          <RefreshCw />
        </button>
        <button className="m-icon-btn" onClick={onOpenChats} aria-label="AI 对话">
          <MessageSquare />
        </button>
        <button className="m-icon-btn" onClick={onOpenAgents} aria-label="Agent">
          <Bot />
        </button>
        <button className="m-icon-btn" onClick={onOpenSettings} aria-label="设置">
          <SettingsIcon />
        </button>
      </header>

      <button className="search-entry" onClick={onOpenSearch}>
        <Search />
        搜索全部笔记…
      </button>

      <div
        ref={listRef}
        className="note-list"
        onTouchStart={onTouchStart}
        onTouchMove={onTouchMove}
        onTouchEnd={onTouchEnd}
      >
        <div
          className="pull-indicator"
          style={{ height: pullY, opacity: pullY >= PULL_THRESHOLD ? 1 : pullY / PULL_THRESHOLD }}
        >
          <RefreshCw className={syncing ? 'spin' : undefined} />
        </div>
        {notes.map((n) => (
          <div
            key={n.relativePath}
            className={`note-card ${selected === n.relativePath ? 'selected' : ''}`}
            onClick={() => {
              if (selected) {
                setSelected('');
                return;
              }
              onOpenNote({ notebookId: activeNotebook, relativePath: n.relativePath });
            }}
            onContextMenu={(e) => {
              e.preventDefault();
              setSelected(n.relativePath);
            }}
          >
            <div className="note-title">{n.filename}</div>
            {n.preview && <div className="note-preview">{n.preview}</div>}
            <div className="note-meta">{relativeTime(n.updatedAt)}</div>
          </div>
        ))}
        {notes.length === 0 && (
          <div className="m-empty">
            <FileText />
            {notebooks.length === 0 ? '桌面端开启云同步后，笔记本会出现在这里' : '暂无笔记'}
          </div>
        )}
      </div>

      <div className={`sync-status ${syncError ? 'error' : ''}`}>
        {syncError ? (
          <>
            <CloudOff />
            <span>{syncError}</span>
          </>
        ) : lastSyncNote ? (
          <>
            <Check />
            <span>{lastSyncNote}</span>
          </>
        ) : null}
      </div>

      <button className="fab" onClick={() => setShowCreate(!showCreate)} aria-label="新建笔记">
        <Plus />
      </button>

      {showCreate && (
        <>
          <div className="m-sheet-mask" onClick={() => setShowCreate(false)} />
          <div className="m-sheet">
            <div className="create-bar">
              <input
                className="m-input"
                placeholder="新笔记标题…"
                autoFocus
                value={newTitle}
                onChange={(e) => setNewTitle(e.target.value)}
                onKeyDown={(e) => e.key === 'Enter' && doCreate()}
              />
              <button className="m-btn-primary confirm" disabled={creating} onClick={doCreate}>
                {creating ? '…' : '创建'}
              </button>
            </div>
          </div>
        </>
      )}

      {selected && (
        <>
          <div className="m-sheet-mask" onClick={() => setSelected('')} />
          <div className="m-sheet">
            <div className="m-sheet-handle" />
            <div className="m-sheet-title">{selected}</div>
            <div className="sheet-action destructive" onClick={doDelete}>
              <Trash2 size={18} />
              {deleting ? '删除中…' : '删除笔记'}
            </div>
            <div className="sheet-action" onClick={() => setSelected('')}>
              取消
            </div>
          </div>
        </>
      )}

      {showNotebooks && (
        <>
          <div className="m-sheet-mask" onClick={() => setShowNotebooks(false)} />
          <div className="m-sheet">
            <div className="m-sheet-handle" />
            <div className="m-sheet-title">笔记本</div>
            {notebooks.map((nb) => (
              <div
                key={nb.id}
                className={`m-sheet-item ${nb.id === activeNotebook ? 'active' : ''}`}
                onClick={() => {
                  setActiveNotebook(nb.id);
                  setShowNotebooks(false);
                }}
              >
                <Notebook size={18} />
                <span style={{ flex: 1 }}>{nb.name || nb.id}</span>
                <span className="nb-dot" />
              </div>
            ))}
            {notebooks.length === 0 && (
              <div className="m-empty" style={{ padding: '24px' }}>
                暂无已同步笔记本
              </div>
            )}
          </div>
        </>
      )}
    </div>
  );
}
