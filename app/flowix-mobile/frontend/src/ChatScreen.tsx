import { useCallback, useEffect, useRef, useState } from 'react';
import { ArrowUp, Square } from 'lucide-react';
import { api, listenEvent, AgentChunk, ThreadMessage } from './api';

interface Props {
  threadId: string;
  onBack: () => void;
  onThreadCreated: (threadId: string) => void;
}

function MessageBubble({ message }: { message: ThreadMessage }) {
  if (message.role === 'user') {
    return <div className="chat-bubble chat-bubble-user">{message.content}</div>;
  }
  if (message.role === 'assistant') {
    return (
      <div className="chat-bubble chat-bubble-assistant">
        {message.reasoning ? (
          <details className="chat-reasoning">
            <summary>思考过程</summary>
            <div className="chat-reasoning-body">{message.reasoning}</div>
          </details>
        ) : null}
        <div className="chat-bubble-text">
          {message.content || <span className="chat-cursor">▍</span>}
        </div>
        {message.toolName ? (
          <div className="chat-tool">工具 {message.toolName}（只读快照）</div>
        ) : null}
      </div>
    );
  }
  return <div className="chat-bubble chat-bubble-system">{message.content}</div>;
}

/** 聊天屏：流式渲染走 `agent-chunk` 事件（不信 IPC await）。 */
export function ChatScreen({ threadId, onBack, onThreadCreated }: Props) {
  const [messages, setMessages] = useState<ThreadMessage[] | null>(null);
  const [draft, setDraft] = useState('');
  const [streaming, setStreaming] = useState(false);
  const [error, setError] = useState('');
  // 流式中的 assistant 增量（独立于已落库消息；stream_end 后由后端落库刷新）
  const [liveText, setLiveText] = useState('');
  const [liveReasoning, setLiveReasoning] = useState('');
  const bottomRef = useRef<HTMLDivElement | null>(null);
  const activeThreadRef = useRef(threadId);
  activeThreadRef.current = threadId;

  const reload = useCallback(async () => {
    try {
      setMessages(await api.agentThreadMessages(threadId));
    } catch (err) {
      setError(String(err));
    }
  }, [threadId]);

  useEffect(() => {
    setMessages(null);
    setLiveText('');
    setLiveReasoning('');
    void reload();
  }, [reload]);

  useEffect(() => {
    const unlisten = listenEvent<AgentChunk>('agent-chunk', (chunk) => {
      if (chunk.thread_id !== activeThreadRef.current) return;
      switch (chunk.kind) {
        case 'stream_start':
          setStreaming(true);
          setLiveText('');
          setLiveReasoning('');
          setError('');
          break;
        case 'text':
          setLiveText((current) => current + chunk.delta);
          break;
        case 'reasoning':
          setLiveReasoning((current) => current + chunk.delta);
          break;
        case 'error':
          setError(chunk.message);
          break;
        case 'stream_end':
          setStreaming(false);
          // 后端已把 assistant 消息落库，重读即最终态
          void reload();
          break;
      }
    }).catch(() => Promise.resolve(() => {}));
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [reload]);

  // 新内容自动滚底
  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages, liveText, liveReasoning]);

  // 键盘跟随（照抄 EditorScreen 的 visualViewport 方案）
  useEffect(() => {
    const onResize = () => {
      const offset = Math.max(0, window.innerHeight - visualViewport.height - visualViewport.offsetTop);
      document.documentElement.style.setProperty('--kb-offset', `${offset}px`);
    };
    visualViewport.addEventListener('resize', onResize);
    visualViewport.addEventListener('scroll', onResize);
    return () => {
      visualViewport.removeEventListener('resize', onResize);
      visualViewport.removeEventListener('scroll', onResize);
    };
  }, []);

  const send = async () => {
    const content = draft.trim();
    if (!content || streaming) return;
    setDraft('');
    setError('');
    try {
      const resolved = await api.agentChatSend(threadId, content);
      if (resolved !== threadId) onThreadCreated(resolved);
      // 乐观追加用户消息（stream_start 前先渲染，避免等待感）
      setMessages((current) => [
        ...(current ?? []),
        {
          id: `pending-${Date.now()}`,
          role: 'user',
          content,
          createdAt: Date.now(),
        },
      ]);
    } catch (err) {
      setError(String(err));
      setDraft(content);
    }
  };

  return (
    <div className="m-screen chat-screen">
      <header className="m-bar">
        <button className="m-icon-btn" onClick={onBack} aria-label="返回">
          <ArrowUp style={{ transform: 'rotate(-90deg)' }} size={20} />
        </button>
        <div className="m-bar-title chat-title">AI 对话</div>
        <div className="m-bar-spacer" />
      </header>

      <div className="chat-scroll">
        {messages === null ? (
          <div className="m-empty">加载中…</div>
        ) : (
          <>
            {messages.map((message) => (
              <MessageBubble key={message.id} message={message} />
            ))}
            {liveText || liveReasoning || streaming ? (
              <div className="chat-bubble chat-bubble-assistant">
                {liveReasoning ? (
                  <details className="chat-reasoning" open>
                    <summary>思考过程</summary>
                    <div className="chat-reasoning-body">{liveReasoning}</div>
                  </details>
                ) : null}
                <div className="chat-bubble-text">
                  {liveText || <span className="chat-cursor">▍</span>}
                </div>
              </div>
            ) : null}
            {messages.length === 0 && !streaming ? (
              <div className="m-empty">发送第一条消息开始对话</div>
            ) : null}
          </>
        )}
        <div ref={bottomRef} />
      </div>

      {error ? <div className="chat-error">{error}</div> : null}

      <footer className="chat-input-bar">
        <textarea
          className="chat-input"
          value={draft}
          placeholder={streaming ? '回复生成中…' : '输入消息'}
          disabled={streaming}
          rows={Math.min(4, Math.max(1, draft.split('\n').length))}
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) {
              event.preventDefault();
              void send();
            }
          }}
        />
        {streaming ? (
          <button
            className="m-icon-btn chat-send chat-send-stop"
            aria-label="停止生成"
            onClick={() => void api.agentChatStop(activeThreadRef.current)}
          >
            <Square size={16} />
          </button>
        ) : (
          <button
            className="m-icon-btn chat-send"
            aria-label="发送"
            disabled={!draft.trim()}
            onClick={() => void send()}
          >
            <ArrowUp size={18} />
          </button>
        )}
      </footer>
    </div>
  );
}
