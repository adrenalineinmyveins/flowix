import { useCallback, useEffect, useRef, useState } from 'react';
import { ArrowUp, Check, ChevronDown, Copy, Square, Terminal } from 'lucide-react';
import { api, listenEvent, AgentChunk, ThreadMessage } from './api';
import { renderChatMarkdown, toolCallSummary } from './chat-render';
import { markThreadRead } from './thread-read';

interface Props {
  threadId: string;
  onBack: () => void;
  onThreadCreated: (threadId: string) => void;
}

export function formatMessageTime(timestamp: number): string {
  const date = new Date(timestamp);
  const now = new Date();
  const sameDay =
    date.getFullYear() === now.getFullYear() &&
    date.getMonth() === now.getMonth() &&
    date.getDate() === now.getDate();
  const hm = `${String(date.getHours()).padStart(2, '0')}:${String(date.getMinutes()).padStart(2, '0')}`;
  return sameDay ? hm : `${date.getMonth() + 1}月${date.getDate()}日 ${hm}`;
}

function CopyButton({ text, className }: { text: string; className: string }) {
  const [copied, setCopied] = useState(false);
  const onCopy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      setTimeout(() => setCopied(false), 1200);
    } catch {
      // WebView 剪贴板权限被拒时静默
    }
  };
  return (
    <button
      className={className}
      aria-label="复制"
      onClick={(event) => {
        event.stopPropagation();
        void onCopy();
      }}
    >
      {copied ? <Check size={13} /> : <Copy size={13} />}
    </button>
  );
}

/** 工具调用行：名字 + 命令摘要，点击展开入参/结果（数据已上云，只读）。 */
function ToolCallRow({ message }: { message: ThreadMessage }) {
  const [expanded, setExpanded] = useState(false);
  const summary = toolCallSummary(message.toolName, message.toolInput);
  return (
    <div className={`chat-tool-row ${expanded ? 'expanded' : ''}`}>
      <button className="chat-tool-row-header" onClick={() => setExpanded((v) => !v)}>
        <Terminal size={13} />
        <span className="chat-tool-row-name">{message.toolName || 'tool'}</span>
        {summary ? <span className="chat-tool-row-summary">{summary}</span> : null}
        <ChevronDown size={14} className="chat-tool-row-chevron" />
      </button>
      {expanded ? (
        <div className="chat-tool-row-detail">
          {message.toolInput ? (
            <pre className="chat-tool-row-block">
              <code>{message.toolInput}</code>
            </pre>
          ) : null}
          {message.toolData ? (
            <pre className="chat-tool-row-block">
              <code>{message.toolData}</code>
            </pre>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

/** 草稿持久化（P1b）：新对话统一存 'new' 键（首条消息后 threadId 才产生）。 */
function draftKey(threadId: string): string {
  return `flowix.chat.draft.${threadId || 'new'}`;
}
function loadDraft(threadId: string): string {
  try {
    return localStorage.getItem(draftKey(threadId)) ?? '';
  } catch {
    return '';
  }
}
function saveDraft(threadId: string, value: string): void {
  try {
    if (value) localStorage.setItem(draftKey(threadId), value);
    else localStorage.removeItem(draftKey(threadId));
  } catch {
    // 隐私模式/配额满：静默
  }
}

/** 单条消息富渲染：markdown（assistant/system）、工具行、复制、时间戳。
 * 与桌面 thread card 消息渲染对齐（P0），卡片与全屏对话共用。 */
export function MessageBubble({ message }: { message: ThreadMessage }) {
  // 代码块「复制」角标的事件委托：CSS ::after 角标不可单独绑定
  const onBubbleClick = (event: React.MouseEvent<HTMLDivElement>) => {
    const target = event.target as HTMLElement;
    const pre = target.closest?.('pre.chat-code');
    if (!pre) return;
    // 点击区域限制在角标附近（右上 64x24）
    const rect = pre.getBoundingClientRect();
    const nearBadge =
      event.clientX > rect.right - 72 && event.clientY < rect.top + 26;
    if (!nearBadge) return;
    const code = pre.querySelector('code')?.textContent ?? '';
    navigator.clipboard.writeText(code).catch(() => {});
  };
  if (message.role === 'user') {
    return (
      <div className="chat-bubble chat-bubble-user">
        <div className="chat-bubble-text">{message.content}</div>
        <div className="chat-bubble-meta">{formatMessageTime(message.createdAt)}</div>
      </div>
    );
  }
  if (message.role === 'tool' || (message.toolName && message.role !== 'assistant')) {
    return <ToolCallRow message={message} />;
  }
  if (message.role === 'assistant') {
    return (
      <div className="chat-bubble chat-bubble-assistant" onClick={onBubbleClick}>
        {message.reasoning ? (
          <details className="chat-reasoning">
            <summary>思考过程</summary>
            <div className="chat-reasoning-body">{message.reasoning}</div>
          </details>
        ) : null}
        {message.content ? (
          <div
            className="chat-bubble-text chat-md"
            dangerouslySetInnerHTML={{ __html: renderChatMarkdown(message.content) }}
          />
        ) : (
          <div className="chat-bubble-text">
            <span className="chat-cursor">▍</span>
          </div>
        )}
        {message.toolName ? <ToolCallRow message={message} /> : null}
        {message.content ? (
          <div className="chat-bubble-actions">
            <span className="chat-bubble-meta">{formatMessageTime(message.createdAt)}</span>
            <CopyButton text={message.content} className="chat-copy-btn" />
          </div>
        ) : null}
      </div>
    );
  }
  return (
    <div className="chat-bubble chat-bubble-system">
      <div
        className="chat-bubble-text chat-md"
        dangerouslySetInnerHTML={{ __html: renderChatMarkdown(message.content) }}
      />
    </div>
  );
}

/** 聊天屏：流式渲染走 `agent-chunk` 事件（不信 IPC await）。 */
export function ChatScreen({ threadId, onBack, onThreadCreated }: Props) {
  const [messages, setMessages] = useState<ThreadMessage[] | null>(null);
  const [draft, setDraft] = useState(() => loadDraft(threadId));
  const [streaming, setStreaming] = useState(false);
  const [error, setError] = useState('');
  const [title, setTitle] = useState('');
  // 流式中的 assistant 增量（独立于已落库消息；stream_end 后由后端落库刷新）
  const [liveText, setLiveText] = useState('');
  const [liveReasoning, setLiveReasoning] = useState('');
  const bottomRef = useRef<HTMLDivElement | null>(null);
  const scrollRef = useRef<HTMLDivElement | null>(null);
  // 滚动跟随（P1b）：仅当用户在底部附近时自动滚，向上翻阅脱离跟随
  const pinnedRef = useRef(true);
  const activeThreadRef = useRef(threadId);
  activeThreadRef.current = threadId;

  const reload = useCallback(async () => {
    try {
      setMessages(await api.agentThreadMessages(threadId));
      if (threadId) markThreadRead(threadId);
    } catch (err) {
      setError(String(err));
    }
  }, [threadId]);

  useEffect(() => {
    setMessages(null);
    setLiveText('');
    setLiveReasoning('');
    setDraft(loadDraft(threadId));
    void reload();
  }, [reload]);

  // 标题（chat.rs 首条消息自动取名 30 字；threads-changed 跟随远端改名）
  useEffect(() => {
    let cancelled = false;
    const load = async () => {
      try {
        const list = await api.agentThreadList();
        if (!cancelled) setTitle(list.find((t) => t.threadId === threadId)?.title ?? '');
      } catch {
        // 标题拿不到就用默认
      }
    };
    void load();
    const unlisten = listenEvent('threads-changed', () => void load()).catch(
      () => Promise.resolve(() => {}),
    );
    return () => {
      cancelled = true;
      void unlisten.then((fn) => fn());
    };
  }, [threadId]);

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
          // 清掉 live 增量再 reload：落库消息回来前不与流式气泡重复显示
          setLiveText('');
          setLiveReasoning('');
          // 后端已把 assistant 消息落库，重读即最终态
          void reload();
          break;
      }
    }).catch(() => Promise.resolve(() => {}));
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [reload]);

  // 远端（另一端）消息经同步到达时刷新当前会话；与本地流式互不干扰
  //（reload 幂等重读，live 增量独立渲染在已落库消息之后）。
  useEffect(() => {
    const unlisten = listenEvent('threads-changed', () => {
      void reload();
    }).catch(() => Promise.resolve(() => {}));
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [reload]);

  // 新内容自动滚底（仅当用户仍停留在底部附近）
  useEffect(() => {
    if (!pinnedRef.current) return;
    bottomRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages, liveText, liveReasoning]);

  const onScroll = () => {
    const el = scrollRef.current;
    if (!el) return;
    pinnedRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 120;
  };

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
    saveDraft(threadId, '');
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
      saveDraft(threadId, content);
    }
  };

  return (
    <div className="m-screen chat-screen">
      <header className="m-bar">
        <button className="m-icon-btn" onClick={onBack} aria-label="返回">
          <ArrowUp style={{ transform: 'rotate(-90deg)' }} size={20} />
        </button>
        <div className="m-bar-title chat-title">{title || 'AI 对话'}</div>
        <div className="m-bar-spacer" />
      </header>

      <div className="chat-scroll" ref={scrollRef} onScroll={onScroll}>
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
                <div
                  className="chat-bubble-text chat-md"
                  dangerouslySetInnerHTML={{ __html: renderChatMarkdown(liveText) }}
                >
                </div>
                {!liveText && !liveReasoning ? <span className="chat-cursor">▍</span> : null}
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
          onChange={(event) => {
            setDraft(event.target.value);
            saveDraft(threadId, event.target.value);
          }}
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
