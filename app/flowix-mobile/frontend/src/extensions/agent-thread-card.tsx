import { useCallback, useEffect, useRef, useState } from 'react';
import { Node, mergeAttributes, type MarkdownToken, type NodeViewProps } from '@tiptap/core';
import { TextSelection } from '@tiptap/pm/state';
import { NodeViewWrapper, ReactNodeViewRenderer } from '@tiptap/react';
import { ArrowUp, ChevronDown, Square, Trash2 } from 'lucide-react';
import { api, listenEvent, type AgentChunk, type ThreadMessage } from '../api';
import { MessageBubble } from '../ChatScreen';
import { renderChatMarkdown } from '../chat-render';

// ── 桌面端 thread card 存储格式：纯函数在同目录 agent-thread-card-md.ts ──
import {
  FLOWIX_AGENT_THREAD_CARD_MARKER,
  parseAgentThreadCardMarkdown,
  parseFlowixAgentThreadCardComment,
  renderAgentThreadCardMarkdown,
} from './agent-thread-card-md';

// ── 会话逻辑（与 ChatScreen 同源：agent-chunk 流式 + threads-changed 刷新）──
// threadId 初始可为 null（首条消息后由后端创建）；chunk 早于 IPC 返回到达时
// 直接采纳 chunk.thread_id，避免首条回复丢失。

function useThreadChat(
  threadId: string | null,
  onThreadResolved: (threadId: string) => void,
  noteContext?: { notebookId: string; relativePath: string } | null,
) {
  const [messages, setMessages] = useState<ThreadMessage[] | null>(null);
  const [streaming, setStreaming] = useState(false);
  const [error, setError] = useState('');
  const [liveText, setLiveText] = useState('');
  const [liveReasoning, setLiveReasoning] = useState('');
  const activeThreadRef = useRef<string | null>(threadId);
  const inflightRef = useRef(false);
  const resolvedCbRef = useRef(onThreadResolved);
  resolvedCbRef.current = onThreadResolved;
  const noteContextRef = useRef(noteContext);
  noteContextRef.current = noteContext;

  const adoptThreadId = useCallback((id: string) => {
    if (activeThreadRef.current === id) return;
    activeThreadRef.current = id;
    resolvedCbRef.current(id);
  }, []);

  const reload = useCallback(async () => {
    const id = activeThreadRef.current;
    if (!id) return;
    try {
      setMessages(await api.agentThreadMessages(id));
    } catch {
      // 桌面端 runtime 卡的线程可能未同步到本机，静默保留占位提示
    }
  }, []);

  useEffect(() => {
    activeThreadRef.current = threadId;
    setMessages(null);
    setLiveText('');
    setLiveReasoning('');
    void reload();
  }, [threadId, reload]);

  useEffect(() => {
    const unlisten = listenEvent<AgentChunk>('agent-chunk', (chunk) => {
      const mine =
        chunk.thread_id === activeThreadRef.current ||
        (activeThreadRef.current === null && inflightRef.current);
      if (!mine) return;
      if (activeThreadRef.current === null) adoptThreadId(chunk.thread_id);
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
          void reload();
          break;
      }
    }).catch(() => Promise.resolve(() => {}));
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [reload, adoptThreadId]);

  useEffect(() => {
    const unlisten = listenEvent('threads-changed', () => {
      void reload();
    }).catch(() => Promise.resolve(() => {}));
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [reload]);

  const send = useCallback(
    async (content: string): Promise<boolean> => {
      const trimmed = content.trim();
      if (!trimmed || streaming) return false;
      setError('');
      inflightRef.current = true;
      // 乐观追加用户消息
      setMessages((current) => [
        ...(current ?? []),
        { id: `pending-${Date.now()}`, role: 'user', content: trimmed, createdAt: Date.now() },
      ]);
      try {
        // P1a：卡片携带所在笔记上下文（仅首条消息生效，落库恒为原文）
        const resolved = await api.agentChatSend(activeThreadRef.current, trimmed, noteContextRef.current);
        inflightRef.current = false;
        adoptThreadId(resolved);
        return true;
      } catch (e) {
        inflightRef.current = false;
        setError(String(e));
        return false;
      }
    },
    [streaming, adoptThreadId],
  );

  return { messages, liveText, liveReasoning, streaming, error, send, reload };
}

// ── 卡片视图 ────────────────────────────────────────────────────────────────
// typeKey === 'mobile'（本机/云端移动端会话）：可对话；其余桌面 runtime 卡只读。

interface ThreadCardOptions {
  /** 卡片所在笔记（P1a 上下文注入；由 EditorScreen configure 传入） */
  noteContext?: { notebookId: string; relativePath: string } | null;
}

function AgentThreadCardComponent({ node, editor, getPos, deleteNode }: NodeViewProps) {
  const attrs = node.attrs as {
    threadId: string | null;
    title: string;
    typeKey: string;
    collapsed: boolean;
  };
  // P1a：所在笔记经扩展 storage 读取（configure → addStorage），
  // 避免包装 NodeView 组件破坏 ReactNodeViewRenderer 的组件身份
  const noteContext =
    (editor?.storage?.agentThreadCard as
      | { noteContext?: { notebookId: string; relativePath: string } | null }
      | undefined)?.noteContext ?? null;
  const interactive = attrs.typeKey === 'mobile' || !attrs.typeKey;
  const [draft, setDraft] = useState('');

  const updateAttrs = useCallback(
    (patch: Record<string, unknown>) => {
      if (typeof getPos !== 'function') return;
      const pos = getPos();
      if (typeof pos !== 'number') return;
      editor.view.dispatch(
        editor.view.state.tr.setNodeMarkup(pos, undefined, {
          ...node.attrs,
          ...patch,
        }),
      );
    },
    [editor, getPos, node.attrs],
  );

  const handleThreadResolved = useCallback(
    (threadId: string) => updateAttrs({ threadId }),
    [updateAttrs],
  );

  const { messages, liveText, liveReasoning, streaming, error, send } = useThreadChat(
    attrs.threadId,
    handleThreadResolved,
    noteContext,
  );

  const onSend = async () => {
    const ok = await send(draft);
    if (ok) setDraft('');
  };

  return (
    <NodeViewWrapper as="div" className="agent-thread-card-m" data-collapsed={attrs.collapsed ? 'true' : 'false'}>
      <div className="atc-m-header">
        <button
          className="atc-m-toggle"
          aria-label={attrs.collapsed ? '展开' : '折叠'}
          onClick={() => updateAttrs({ collapsed: !attrs.collapsed })}
        >
          <ChevronDown size={16} className={attrs.collapsed ? 'atc-m-chevron-collapsed' : ''} />
        </button>
        <div className="atc-m-title">
          {attrs.title || (interactive ? 'AI 对话' : '桌面端 AI 会话')}
        </div>
        <button className="atc-m-icon" aria-label="删除卡片" onClick={() => deleteNode()}>
          <Trash2 size={14} />
        </button>
      </div>
      {!attrs.collapsed && (
        <div className="atc-m-body">
          {messages === null && attrs.threadId ? (
            <div className="atc-m-hint">加载中…</div>
          ) : null}
          {messages !== null && messages.length === 0 && !streaming && !attrs.threadId ? (
            <div className="atc-m-hint">{interactive ? '发送第一条消息开始对话' : '会话尚未同步到本机'}</div>
          ) : null}
          {messages !== null && messages.length === 0 && attrs.threadId && !streaming ? (
            <div className="atc-m-hint">会话尚未同步到本机</div>
          ) : null}
          {messages?.map((message) => (
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
              {liveText ? (
                <div
                  className="chat-bubble-text chat-md"
                  dangerouslySetInnerHTML={{ __html: renderChatMarkdown(liveText) }}
                />
              ) : (
                <div className="chat-bubble-text">
                  <span className="chat-cursor">▍</span>
                </div>
              )}
            </div>
          ) : null}
          {error ? <div className="atc-m-error">{error}</div> : null}
          {interactive ? (
            <div className="atc-m-composer">
              <textarea
                className="atc-m-input"
                value={draft}
                placeholder={streaming ? '回复生成中…' : '输入消息'}
                disabled={streaming}
                rows={Math.min(4, Math.max(1, draft.split('\n').length))}
                onChange={(event) => setDraft(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) {
                    event.preventDefault();
                    void onSend();
                  }
                }}
              />
              {streaming ? (
                <button
                  className="m-icon-btn atc-m-send atc-m-send-stop"
                  aria-label="停止生成"
                  onClick={() => attrs.threadId && void api.agentChatStop(attrs.threadId)}
                >
                  <Square size={14} />
                </button>
              ) : (
                <button
                  className="m-icon-btn atc-m-send"
                  aria-label="发送"
                  disabled={!draft.trim()}
                  onClick={() => void onSend()}
                >
                  <ArrowUp size={16} />
                </button>
              )}
            </div>
          ) : (
            <div className="atc-m-hint atc-m-readonly">桌面端会话 · 只读（在桌面端继续对话）</div>
          )}
        </div>
      )}
    </NodeViewWrapper>
  );
}

// ── Tiptap 扩展 ────────────────────────────────────────────────────────────

declare module '@tiptap/core' {
  interface Commands<ReturnType> {
    agentThreadCardMobile: {
      insertMobileAgentThreadCard: () => ReturnType;
    };
  }
}

function newInstanceId(): string {
  return typeof crypto !== 'undefined' && 'randomUUID' in crypto
    ? crypto.randomUUID()
    : `mi-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}

export const AgentThreadCardMobile = Node.create<ThreadCardOptions>({
  name: 'agentThreadCard',
  group: 'block',
  content: '',
  selectable: false,
  draggable: false,

  addOptions() {
    return { noteContext: null };
  },

  addStorage() {
    return { noteContext: this.options.noteContext ?? null };
  },

  addAttributes() {
    return {
      threadId: { default: null },
      instanceId: { default: null },
      title: { default: '' },
      typeKey: { default: 'mobile' },
      collapsed: { default: false },
      fullscreen: { default: false },
      inputDraft: { default: null },
    };
  },

  parseHTML() {
    return [
      {
        tag: 'section[data-agent-thread-card]',
        getAttrs: (dom) => {
          const element = dom as HTMLElement;
          return {
            threadId: element.getAttribute('data-thread-id') || null,
            instanceId: element.getAttribute('data-instance-id') || null,
            title: element.getAttribute('data-title') || '',
            typeKey: element.getAttribute('data-agent-type') || 'mobile',
            collapsed: element.getAttribute('data-collapsed') === 'true',
            fullscreen: element.getAttribute('data-fullscreen') === 'true',
            inputDraft: element.getAttribute('data-input-draft') || null,
          };
        },
      },
    ];
  },

  renderHTML({ node, HTMLAttributes }) {
    return [
      'section',
      mergeAttributes(HTMLAttributes, {
        'data-agent-thread-card': 'true',
        'data-thread-id': node.attrs.threadId || '',
        'data-instance-id': node.attrs.instanceId || '',
        'data-title': node.attrs.title || '',
        'data-agent-type': node.attrs.typeKey || 'mobile',
        'data-collapsed': node.attrs.collapsed ? 'true' : 'false',
        'data-fullscreen': node.attrs.fullscreen ? 'true' : 'false',
      }),
    ];
  },

  addCommands() {
    return {
      insertMobileAgentThreadCard:
        () =>
        ({ state, dispatch, tr }) => {
          const nodeType = state.schema.nodes[this.name];
          if (!nodeType) return false;
          if (!dispatch) return true;
          const node = nodeType.create({
            threadId: null,
            instanceId: newInstanceId(),
            title: '',
            typeKey: 'mobile',
            collapsed: false,
            fullscreen: false,
            inputDraft: null,
          });
          const from = state.selection.from;
          tr.replaceWith(from, state.selection.to, node);
          const after = from + node.nodeSize;
          const paragraphType = state.schema.nodes.paragraph;
          if (paragraphType) {
            tr.insert(after, paragraphType.create());
            tr.setSelection(TextSelection.create(tr.doc, after + 1));
          }
          dispatch(tr);
          return true;
        },
    };
  },

  addNodeView() {
    return ReactNodeViewRenderer(AgentThreadCardComponent);
  },

  markdownTokenizer: {
    name: 'agentThreadCard',
    level: 'block' as const,
    start(src: string) {
      const commentIndex = src.search(
        new RegExp(`^<!--[ \\t]*${FLOWIX_AGENT_THREAD_CARD_MARKER}[ \\t]+\\{`, 'm'),
      );
      const legacyIndex = src.indexOf('::agent-thread-card');
      if (commentIndex < 0) return legacyIndex;
      if (legacyIndex < 0) return commentIndex;
      return Math.min(commentIndex, legacyIndex);
    },
    tokenize(src: string) {
      const comment = parseFlowixAgentThreadCardComment(src);
      if (comment) {
        return { type: 'agentThreadCard', raw: comment.raw, metadata: comment.metadata };
      }
      // 旧版语法（桌面历史文档）
      const match = /^::agent-thread-card\{([^}]*)\}[ \t]*(?:\n|$)/.exec(src);
      if (!match) return undefined;
      return { type: 'agentThreadCard', raw: match[0], attrs: match[1] };
    },
  },

  parseMarkdown(token: MarkdownToken) {
    return parseAgentThreadCardMarkdown(token);
  },

  renderMarkdown(node: { attrs?: Record<string, unknown> }) {
    return renderAgentThreadCardMarkdown(node);
  },
});
