import { useCallback, useEffect, useState } from 'react';
import { MessageSquare, Plus, Trash2 } from 'lucide-react';
import { api, listenEvent, ThreadSummary } from './api';

interface Props {
  onOpenThread: (threadId: string) => void;
  onNewChat: () => void;
  onBack: () => void;
}

/** 会话列表屏（方案 A4）：本地 + 云端（桌面/另一端）会话统一展示。 */
export function ChatListScreen({ onOpenThread, onNewChat, onBack }: Props) {
  const [threads, setThreads] = useState<ThreadSummary[] | null>(null);
  const [message, setMessage] = useState('');

  const refresh = useCallback(async () => {
    try {
      setThreads(await api.agentThreadList());
    } catch (error) {
      setMessage(String(error));
    }
  }, []);

  useEffect(() => {
    void refresh();
    const unlisten = listenEvent('threads-changed', () => {
      void refresh();
    }).catch(() => Promise.resolve(() => {}));
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [refresh]);

  const remove = async (threadId: string) => {
    try {
      await api.agentThreadDelete(threadId);
      await refresh();
    } catch (error) {
      setMessage(String(error));
    }
  };

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

      <div className="chats-list">
        {threads === null ? (
          <div className="m-empty">加载中…</div>
        ) : threads.length === 0 ? (
          <div className="m-empty">
            <MessageSquare size={32} opacity={0.4} />
            <div>还没有对话</div>
            <div className="m-empty-sub">点右上角 + 开始，或在设置中配置 AI 服务</div>
          </div>
        ) : (
          threads.map((thread) => (
            <div
              key={thread.threadId}
              className="m-card chat-item"
              onClick={() => onOpenThread(thread.threadId)}
            >
              <div className="chat-item-main">
                <div className="chat-item-title">{thread.title || '新对话'}</div>
                <div className="chat-item-meta">
                  {thread.messageCount} 条消息 ·{' '}
                  {new Date(thread.updatedAt).toLocaleString()}
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
          ))
        )}
      </div>
      {message ? <div className="m-toast">{message}</div> : null}
    </div>
  );
}
