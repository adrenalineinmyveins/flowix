import { useCallback, useEffect, useRef, useState } from 'react';
import { Archive, ArchiveRestore, MessageSquare, Pencil, Plus, Star, Trash2 } from 'lucide-react';
import { api, listenEvent, ThreadSummary } from './api';
import { formatMessageTime } from './ChatScreen';
import { isThreadUnread } from './thread-read';

interface Props {
  onOpenThread: (threadId: string) => void;
  onNewChat: () => void;
  onBack: () => void;
}

type Filter = 'all' | 'starred' | 'archived';

const FILTERS: { key: Filter; label: string }[] = [
  { key: 'all', label: '全部' },
  { key: 'starred', label: '收藏' },
  { key: 'archived', label: '已归档' },
];

/** 会话列表屏（方案 A4）：本地 + 云端（桌面/另一端）会话统一展示。
 * P1b：日期分组、未读点、筛选（收藏/归档）、长按菜单（重命名/收藏/归档/删除）。 */
export function ChatListScreen({ onOpenThread, onNewChat, onBack }: Props) {
  const [threads, setThreads] = useState<ThreadSummary[] | null>(null);
  const [message, setMessage] = useState('');
  const [filter, setFilter] = useState<Filter>('all');
  // 长按菜单（P1b）：目标会话 + 重命名输入态
  const [menuThread, setMenuThread] = useState<ThreadSummary | null>(null);
  const [renaming, setRenaming] = useState(false);
  const [renameValue, setRenameValue] = useState('');
  const longPressTimer = useRef(0);
  const suppressClick = useRef(false);

  const refresh = useCallback(async () => {
    try {
      setThreads(await api.agentThreadList());
    } catch (error) {
      setMessage(String(error));
    }
  }, []);

  useEffect(() => {
    void refresh();
    const unlisten = listenEvent('threads-changed', () => void refresh()).catch(
      () => Promise.resolve(() => {}),
    );
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [refresh]);

  const closeMenu = () => {
    setMenuThread(null);
    setRenaming(false);
  };

  const update = async (patch: { title?: string; starred?: boolean; archived?: boolean }) => {
    if (!menuThread) return;
    try {
      await api.agentThreadUpdate(menuThread.threadId, patch);
      closeMenu();
      await refresh();
    } catch (error) {
      setMessage(String(error));
    }
  };

  const remove = async (threadId: string) => {
    try {
      await api.agentThreadDelete(threadId);
      closeMenu();
      await refresh();
    } catch (error) {
      setMessage(String(error));
    }
  };

  const startLongPress = (thread: ThreadSummary) => {
    suppressClick.current = false;
    longPressTimer.current = window.setTimeout(() => {
      suppressClick.current = true;
      setMenuThread(thread);
      setRenaming(false);
    }, 480);
  };
  const cancelLongPress = () => window.clearTimeout(longPressTimer.current);

  const visible = (threads ?? []).filter((thread) => {
    if (filter === 'starred') return thread.starred && !thread.archived;
    if (filter === 'archived') return thread.archived;
    return !thread.archived;
  });

  // 日期分组（threads 已按 updatedAt 倒序，组序随之自然降序）
  const groups: { label: string; items: ThreadSummary[] }[] = [];
  for (const thread of visible) {
    const label = dateGroupLabel(thread.updatedAt);
    const last = groups[groups.length - 1];
    if (last && last.label === label) last.items.push(thread);
    else groups.push({ label, items: [thread] });
  }

  return (
    <div className="m-screen chats-screen">
      <header className="m-bar">
        <button className="m-icon-btn" onClick={onBack} aria-label="返回">
          <Plus style={{ transform: 'rotate(45deg)' }} size={20} />
        </button>
        <div className="m-bar-title">AI 对话</div>
        <button className="m-icon-btn" onClick={onNewChat} aria-label="新对话">
          <Plus size={20} />
        </button>
      </header>

      <div className="chats-filter">
        {FILTERS.map((item) => (
          <button
            key={item.key}
            className={`chats-chip ${filter === item.key ? 'active' : ''}`}
            onClick={() => setFilter(item.key)}
          >
            {item.label}
          </button>
        ))}
      </div>

      <div className="chats-list">
        {threads === null ? (
          <div className="m-empty">加载中…</div>
        ) : threads.length === 0 ? (
          <div className="m-empty">
            <MessageSquare size={32} opacity={0.4} />
            <div>还没有对话</div>
            <div className="m-empty-sub">点右上角 + 开始，或在设置中配置 AI 服务</div>
          </div>
        ) : visible.length === 0 ? (
          <div className="m-empty">
            <div>{filter === 'starred' ? '还没有收藏的对话' : '没有已归档的对话'}</div>
            <div className="m-empty-sub">长按对话可收藏或归档</div>
          </div>
        ) : (
          groups.map((group) => (
            <section key={group.label} className="chats-group">
              <div className="chats-group-label">{group.label}</div>
              {group.items.map((thread) => (
                <div
                  key={thread.threadId}
                  className="m-card chat-item"
                  onPointerDown={() => startLongPress(thread)}
                  onPointerUp={cancelLongPress}
                  onPointerLeave={cancelLongPress}
                  onPointerCancel={cancelLongPress}
                  onPointerMove={cancelLongPress}
                  onClick={() => {
                    if (suppressClick.current) {
                      suppressClick.current = false;
                      return;
                    }
                    onOpenThread(thread.threadId);
                  }}
                >
                  {isThreadUnread(thread.updatedAt, thread.messageCount, thread.threadId) ? (
                    <span className="chat-unread-dot" aria-label="有新消息" />
                  ) : null}
                  <div className="chat-item-main">
                    <div className="chat-item-title">
                      {thread.starred ? <Star size={13} className="chat-starred" /> : null}
                      {thread.title || '新对话'}
                    </div>
                    <div className="chat-item-meta">
                      {thread.messageCount} 条消息 · {formatMessageTime(thread.updatedAt)}
                      {thread.agentId === 'mobile' ? '' : ` · ${thread.agentId}`}
                    </div>
                  </div>
                  <button
                    className="m-icon-btn chat-item-delete"
                    aria-label="删除对话"
                    onClick={(event) => {
                      event.stopPropagation();
                      void remove(thread.threadId);
                    }}
                  >
                    <Trash2 size={16} />
                  </button>
                </div>
              ))}
            </section>
          ))
        )}
      </div>
      {message ? <div className="m-toast">{message}</div> : null}

      {menuThread ? (
        <>
          <div className="m-sheet-mask" onClick={closeMenu} />
          <div className="m-sheet chats-menu-sheet">
            <div className="m-sheet-handle" />
            {renaming ? (
              <>
                <div className="m-sheet-title">重命名对话</div>
                <input
                  className="m-input chats-rename-input"
                  value={renameValue}
                  onChange={(event) => setRenameValue(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === 'Enter') void update({ title: renameValue });
                  }}
                  autoFocus
                />
                <div className="chats-menu-actions">
                  <button className="m-btn-ghost" onClick={closeMenu}>
                    取消
                  </button>
                  <button
                    className="m-btn-primary"
                    disabled={!renameValue.trim()}
                    onClick={() => void update({ title: renameValue })}
                  >
                    保存
                  </button>
                </div>
              </>
            ) : (
              <>
                <div className="m-sheet-title chats-menu-title">{menuThread.title || '新对话'}</div>
                <button
                  className="m-sheet-item"
                  onClick={() => {
                    setRenameValue(menuThread.title);
                    setRenaming(true);
                  }}
                >
                  <Pencil size={15} />
                  重命名
                </button>
                <button
                  className="m-sheet-item"
                  onClick={() => void update({ starred: !menuThread.starred })}
                >
                  <Star size={15} />
                  {menuThread.starred ? '取消收藏' : '收藏'}
                </button>
                <button
                  className="m-sheet-item"
                  onClick={() => void update({ archived: !menuThread.archived })}
                >
                  {menuThread.archived ? <ArchiveRestore size={15} /> : <Archive size={15} />}
                  {menuThread.archived ? '取消归档' : '归档'}
                </button>
                <button
                  className="m-sheet-item chats-menu-danger"
                  onClick={() => void remove(menuThread.threadId)}
                >
                  <Trash2 size={15} />
                  删除对话
                </button>
              </>
            )}
          </div>
        </>
      ) : null}
    </div>
  );
}

function dateGroupLabel(timestamp: number): string {
  const now = new Date();
  const startOfToday = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  if (timestamp >= startOfToday) return '今天';
  if (timestamp >= startOfToday - 86_400_000) return '昨天';
  if (timestamp >= startOfToday - 7 * 86_400_000) return '7 天内';
  return '更早';
}
