import { useCallback, useEffect, useRef, useState } from 'react';
import { ArrowLeft, ArrowUp, Check, ChevronDown, Copy, ImagePlus, Send, Settings, Square, Terminal, X } from 'lucide-react';
import {
  api,
  listenEvent,
  AgentChunk,
  ChatAttachment,
  ThreadMessage,
  friendlyAgentError,
  isProviderUnconfigured,
} from './api';
import { renderChatMarkdown, toolCallSummary } from './chat-render';
import { markThreadRead } from './thread-read';

interface Props {
  threadId: string;
  onBack: () => void;
  onThreadCreated: (threadId: string) => void;
  /** 附件锚定笔记本（P2a）：图片存该笔记本 attachments/ 随云同步 */
  attachmentNotebookId?: string | null;
  /** D6：AI 服务未配置时的跳设置入口 */
  onOpenSettings?: (() => void) | null;
}

/** D3：单页消息数（长会话首屏只拉最新一页，向上翻加载更早） */
const MESSAGE_PAGE_SIZE = 40;

/** D4：超长消息折叠预算（字符）；超出默认折叠，展开按钮显示全量 */
const MESSAGE_COLLAPSE_CHARS = 800;

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

/** 附件缩略图（P2a）：attachment_read 读回 dataURL（跨端按文件名回落），
 * 读失败显示文件名占位；笔记本缺省（无锚点）时全部占位。 */
function AttachmentThumbs({
  attachments,
  notebookId,
}: {
  attachments: ChatAttachment[];
  notebookId?: string | null;
}) {
  const [urls, setUrls] = useState<Record<string, string>>({});
  useEffect(() => {
    let cancelled = false;
    (async () => {
      if (!notebookId || attachments.length === 0) return;
      const next: Record<string, string> = {};
      for (const attachment of attachments) {
        try {
          next[attachment.storageKey] = await api.attachmentRead(notebookId, attachment.storageKey);
        } catch {
          // 留空 → 文件名占位（附件未同步到本机等场景）
        }
      }
      if (!cancelled) setUrls(next);
    })();
    return () => {
      cancelled = true;
    };
  }, [attachments, notebookId]);
  if (attachments.length === 0) return null;
  return (
    <div className="chat-attachments">
      {attachments.map((attachment) =>
        urls[attachment.storageKey] ? (
          <img
            key={attachment.storageKey}
            className="chat-attachment-thumb"
            src={urls[attachment.storageKey]}
            alt={attachment.fileName}
          />
        ) : (
          <div key={attachment.storageKey} className="chat-attachment-missing">
            {attachment.fileName}
          </div>
        ),
      )}
    </div>
  );
}

/** 单条消息富渲染：markdown（assistant/system）、工具行、复制、时间戳。
 * 与桌面 thread card 消息渲染对齐（P0），卡片与全屏对话共用。
 * D4：超长正文默认折叠（预算 MESSAGE_COLLAPSE_CHARS），展开按钮看全量。 */
export function MessageBubble({
  message,
  attachmentNotebookId,
}: {
  message: ThreadMessage;
  attachmentNotebookId?: string | null;
}) {
  const [expanded, setExpanded] = useState(false);
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
  // D4 折叠：仅 assistant/system 的 markdown 正文参与预算（用户消息通常短）
  const collapsible =
    expanded === false &&
    (message.role === 'assistant' || message.role === 'system') &&
    message.content.length > MESSAGE_COLLAPSE_CHARS;
  const visibleContent = collapsible
    ? message.content.slice(0, MESSAGE_COLLAPSE_CHARS)
    : message.content;
  if (message.role === 'user') {
    return (
      <div className="chat-bubble chat-bubble-user">
        {message.attachments && message.attachments.length > 0 ? (
          <AttachmentThumbs attachments={message.attachments} notebookId={attachmentNotebookId} />
        ) : null}
        {message.content ? <div className="chat-bubble-text">{message.content}</div> : null}
        <div className="chat-bubble-meta">{formatMessageTime(message.createdAt)}</div>
      </div>
    );
  }
  if (message.role === 'tool' || (message.toolName && message.role !== 'assistant')) {
    return <ToolCallRow message={message} />;
  }
  if (message.role === 'assistant') {
    // 桌面同步会话只展示文字对话：思考过程与内嵌工具行不渲染（本机对话保持完整）
    const textOnly = message.origin === 'desktop';
    return (
      <div className="chat-bubble chat-bubble-assistant" onClick={onBubbleClick}>
        {message.reasoning && !textOnly ? (
          <details className="chat-reasoning">
            <summary>思考过程</summary>
            <div className="chat-reasoning-body">{message.reasoning}</div>
          </details>
        ) : null}
        {visibleContent ? (
          <>
            <div
              className={`chat-bubble-text chat-md${collapsible ? ' chat-md-collapsed' : ''}`}
              dangerouslySetInnerHTML={{ __html: renderChatMarkdown(visibleContent) }}
            />
            {collapsible ? (
              <button
                className="chat-collapse-toggle"
                onClick={() => setExpanded(true)}
              >
                <ChevronDown size={13} />
                展开全文（{message.content.length} 字）
              </button>
            ) : null}
          </>
        ) : (
          <div className="chat-bubble-text">
            <span className="chat-cursor">▍</span>
          </div>
        )}
        {message.toolName && !textOnly ? <ToolCallRow message={message} /> : null}
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
    <div className="chat-bubble chat-bubble-system" onClick={onBubbleClick}>
      <div
        className={`chat-bubble-text chat-md${collapsible ? ' chat-md-collapsed' : ''}`}
        dangerouslySetInnerHTML={{ __html: renderChatMarkdown(visibleContent) }}
      />
      {collapsible ? (
        <button className="chat-collapse-toggle" onClick={() => setExpanded(true)}>
          <ChevronDown size={13} />
          展开全文（{message.content.length} 字）
        </button>
      ) : null}
    </div>
  );
}

/** 聊天屏：流式渲染走 `agent-chunk` 事件（不信 IPC await）。 */
export function ChatScreen({ threadId, onBack, onThreadCreated, attachmentNotebookId, onOpenSettings }: Props) {
  const [messages, setMessages] = useState<ThreadMessage[] | null>(null);
  // D3 分页：更早历史按需加载（双键游标）
  const [hasMore, setHasMore] = useState(false);
  const [oldest, setOldest] = useState<{ createdAt: number; id: string } | null>(null);
  const [loadingOlder, setLoadingOlder] = useState(false);
  const [draft, setDraft] = useState(() => loadDraft(threadId));
  const [streaming, setStreaming] = useState(false);
  const [error, setError] = useState('');
  const [title, setTitle] = useState('');
  // 待发附件（P2a）：attachment_save 已落盘 + 本地预览
  const [pending, setPending] = useState<(ChatAttachment & { previewUrl: string })[]>([]);
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  // 流式中的 assistant 增量（独立于已落库消息；stream_end 后由后端落库刷新）
  const [liveText, setLiveText] = useState('');
  const [liveReasoning, setLiveReasoning] = useState('');
  // M4 网络会话人工代答回信
  const [busyReply, setBusyReply] = useState(false);
  const [replyMsg, setReplyMsg] = useState('');

  const doNetReply = async () => {
    if (busyReply) return;
    setBusyReply(true);
    setReplyMsg('');
    try {
      const result = await api.redactionReply(threadId);
      setReplyMsg(`已投递（HTTP ${result.httpStatus}，回执 ${result.replyReceiptId}）`);
      await reload();
    } catch (e) {
      setReplyMsg(String(e));
    } finally {
      setBusyReply(false);
    }
  };
  const bottomRef = useRef<HTMLDivElement | null>(null);
  const scrollRef = useRef<HTMLDivElement | null>(null);
  // 滚动跟随（P1b）：仅当用户在底部附近时自动滚，向上翻阅脱离跟随
  const pinnedRef = useRef(true);
  const activeThreadRef = useRef(threadId);
  activeThreadRef.current = threadId;
  // D3：reload 保持已加载条数（翻页后流结束/远端刷新不丢已展开的历史）
  const loadedCountRef = useRef(MESSAGE_PAGE_SIZE);

  const reload = useCallback(async () => {
    try {
      // limit 取「页大小与已加载数的较大者」：首轮 40 条，翻页后刷新不截断视图
      const limit = Math.max(MESSAGE_PAGE_SIZE, loadedCountRef.current);
      const page = await api.agentThreadMessagesPage(threadId, limit);
      setMessages(page.messages);
      setHasMore(page.hasMore);
      setOldest(
        page.oldestCreatedAt != null && page.oldestId
          ? { createdAt: page.oldestCreatedAt, id: page.oldestId }
          : null,
      );
      if (threadId) markThreadRead(threadId);
    } catch (err) {
      setError(friendlyAgentError(err));
    }
  }, [threadId]);

  // D3：加载更早一页（prepend，按 id 去重；保持滚动位置不跳）
  const loadOlder = useCallback(async () => {
    if (!oldest || loadingOlder) return;
    setLoadingOlder(true);
    try {
      const page = await api.agentThreadMessagesPage(threadId, MESSAGE_PAGE_SIZE, oldest);
      const existing = new Set((messages ?? []).map((message) => message.id));
      const older = page.messages.filter((message) => !existing.has(message.id));
      const el = scrollRef.current;
      const prevHeight = el?.scrollHeight ?? 0;
      setMessages((current) => [...older, ...(current ?? [])]);
      setHasMore(page.hasMore);
      setOldest(
        page.oldestCreatedAt != null && page.oldestId
          ? { createdAt: page.oldestCreatedAt, id: page.oldestId }
          : null,
      );
      loadedCountRef.current += older.length;
      // prepend 后把滚动位置补回到原视觉位置
      requestAnimationFrame(() => {
        const el2 = scrollRef.current;
        if (el2) el2.scrollTop += el2.scrollHeight - prevHeight;
      });
    } catch (err) {
      setError(friendlyAgentError(err));
    } finally {
      setLoadingOlder(false);
    }
  }, [threadId, oldest, loadingOlder, messages]);

  useEffect(() => {
    setMessages(null);
    setLiveText('');
    setLiveReasoning('');
    setDraft(loadDraft(threadId));
    loadedCountRef.current = MESSAGE_PAGE_SIZE;
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
          setError(friendlyAgentError(chunk.message));
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

  // 选图（P2a）：WebView 文件选择器 → base64 → attachment_save（笔记本 attachments/，
  // 随云同步；跨端按文件名回落）
  const onPickImage = async (event: React.ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0];
    event.target.value = '';
    if (!file) return;
    if (!attachmentNotebookId) {
      setError('ATTACHMENT_NOTEBOOK_UNAVAILABLE');
      return;
    }
    setError('');
    try {
      const dataUrl = await new Promise<string>((resolve, reject) => {
        const reader = new FileReader();
        reader.onload = () => resolve(String(reader.result));
        reader.onerror = () => reject(new Error('READ_FILE_FAILED'));
        reader.readAsDataURL(file);
      });
      const base64 = dataUrl.slice(dataUrl.indexOf(',') + 1);
      const saved = await api.attachmentSave(attachmentNotebookId, file.name, base64);
      setPending((current) => [
        ...current,
        { fileName: file.name, storageKey: saved.absolutePath, previewUrl: dataUrl },
      ]);
    } catch (err) {
      setError(`图片添加失败：${String(err)}`);
    }
  };

  const send = async () => {
    const content = draft.trim();
    if (streaming) return;
    if (!content && pending.length === 0) return;
    const attachments = pending.map(({ fileName, storageKey }) => ({ fileName, storageKey }));
    setDraft('');
    saveDraft(threadId, '');
    setPending([]);
    setError('');
    try {
      const resolved = await api.agentChatSend(threadId, content, undefined, attachments);
      if (resolved !== threadId) onThreadCreated(resolved);
      // 乐观追加用户消息（stream_start 前先渲染，避免等待感）
      setMessages((current) => [
        ...(current ?? []),
        {
          id: `pending-${Date.now()}`,
          role: 'user',
          content,
          createdAt: Date.now(),
          attachments,
        },
      ]);
    } catch (err) {
      setError(friendlyAgentError(err));
      setDraft(content);
      saveDraft(threadId, content);
      setPending((current) =>
        attachments.map((attachment) => ({
          ...attachment,
          previewUrl: current.find((item) => item.storageKey === attachment.storageKey)?.previewUrl ?? '',
        })),
      );
    }
  };

  return (
    <div className="m-screen chat-screen">
      <header className="m-bar">
        <button className="m-icon-btn" onClick={onBack} aria-label="返回">
          <ArrowLeft size={20} />
        </button>
        <div className="m-bar-title chat-title">{title || 'AI 对话'}</div>
        <div className="m-bar-spacer" />
      </header>

      <div className="chat-scroll" ref={scrollRef} onScroll={onScroll}>
        {messages === null ? (
          <div className="m-empty">加载中…</div>
        ) : (
          <>
            {hasMore && messages.length > 0 ? (
              <button className="chat-load-older" disabled={loadingOlder} onClick={() => void loadOlder()}>
                {loadingOlder ? '加载中…' : '加载更早消息'}
              </button>
            ) : null}
            {messages.map((message) =>
              // 桌面同步会话只展示文字对话：纯工具消息（tool 行）不渲染
              message.origin === 'desktop' &&
              (message.role === 'tool' || (message.toolName && message.role !== 'assistant')) ? null : (
                <MessageBubble
                  key={message.id}
                  message={message}
                  attachmentNotebookId={attachmentNotebookId}
                />
              ),
            )}
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

      {error ? (
        <div className="chat-error">
          <span>{error}</span>
          {isProviderUnconfigured(error) && onOpenSettings ? (
            <button className="chat-error-action" onClick={onOpenSettings}>
              <Settings size={13} />
              去设置
            </button>
          ) : null}
        </div>
      ) : null}

      <footer className="chat-input-bar">
        {threadId === 'th_net_inbox' ? (
          <div className="chat-net-reply">
            <button
              className="m-btn-ghost"
              disabled={busyReply}
              onClick={() => void doNetReply()}
            >
              <Send size={15} className={busyReply ? 'spin' : undefined} />
              {busyReply ? '回信投递中…' : '发送回信（AI 最近草稿）'}
            </button>
            {replyMsg ? <span className="chat-net-reply-msg">{replyMsg}</span> : null}
          </div>
        ) : null}
        {pending.length > 0 ? (
          <div className="chat-pending-bar">
            {pending.map((item) => (
              <div key={item.storageKey} className="chat-pending-thumb">
                <img src={item.previewUrl} alt={item.fileName} />
                <button
                  className="chat-pending-remove"
                  aria-label="移除图片"
                  onClick={() =>
                    setPending((current) => current.filter((p) => p.storageKey !== item.storageKey))
                  }
                >
                  <X size={12} />
                </button>
              </div>
            ))}
          </div>
        ) : null}
        <input
          ref={fileInputRef}
          type="file"
          accept="image/*"
          className="file-input-hidden"
          onChange={(event) => void onPickImage(event)}
        />
        <div className="chat-input-row">
          <button
            className="m-icon-btn chat-pick-btn"
            aria-label="添加图片"
            disabled={streaming}
            onClick={() => fileInputRef.current?.click()}
          >
            <ImagePlus size={19} />
          </button>
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
              disabled={!draft.trim() && pending.length === 0}
              onClick={() => void send()}
            >
              <ArrowUp size={18} />
            </button>
          )}
        </div>
      </footer>
    </div>
  );
}
