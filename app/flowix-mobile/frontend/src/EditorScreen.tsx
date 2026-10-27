import { useCallback, useEffect, useRef, useState, type ChangeEvent } from 'react';
import { EditorContent, useEditor } from '@tiptap/react';
import {
  ArrowLeft,
  Bold,
  Bot,
  Italic,
  Strikethrough,
  Heading1,
  Heading2,
  Heading3,
  List,
  ListOrdered,
  ListTodo,
  Quote,
  Code,
  Undo2,
  Redo2,
  ImagePlus,
  Clock3,
  Highlighter,
  Table as TableIcon,
} from 'lucide-react';
import StarterKit from '@tiptap/starter-kit';
import { Markdown } from '@tiptap/markdown';
import TaskList from '@tiptap/extension-task-list';
import TaskItem from '@tiptap/extension-task-item';
import Placeholder from '@tiptap/extension-placeholder';
import Highlight from '@tiptap/extension-highlight';
import { Table } from '@tiptap/extension-table';
import TableRow from '@tiptap/extension-table-row';
import TableHeader from '@tiptap/extension-table-header';
import TableCell from '@tiptap/extension-table-cell';
import { MobileImage } from './extensions/image';
import { AgentThreadCardMobile } from './extensions/agent-thread-card';
import { api, type HistoryEntry } from './api';
import { OpenNote } from './App';

// frontmatter 与正文分离：编辑器只处理正文，保存时原样拼回，
// 避免 Tiptap 对 `---` 元数据块的解析损耗。
function splitFrontmatter(content: string): { frontmatter: string; body: string } {
  if (!content.startsWith('---\n')) return { frontmatter: '', body: content };
  const end = content.indexOf('\n---\n', 4);
  if (end === -1) return { frontmatter: '', body: content };
  return {
    frontmatter: content.slice(0, end + 5),
    body: content.slice(end + 5),
  };
}

type SaveState = 'saved' | 'saving' | 'dirty';

const AUTOSAVE_DELAY = 800;

function formatHistoryTime(timestamp: number): string {
  const date = new Date(timestamp);
  const now = new Date();
  const sameYear = date.getFullYear() === now.getFullYear();
  const pad = (value: number) => String(value).padStart(2, '0');
  const md = `${date.getMonth() + 1}月${date.getDate()}日`;
  const hm = `${pad(date.getHours())}:${pad(date.getMinutes())}`;
  return sameYear ? `${md} ${hm}` : `${date.getFullYear()}年${md} ${hm}`;
}

export function EditorScreen({
  note,
  onBack,
  onOpenChat,
}: {
  note: OpenNote;
  onBack: () => void;
  onOpenChat?: (threadId: string) => void;
}) {
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');
  const [saveState, setSaveState] = useState<SaveState>('saved');
  const [toolbarUp, setToolbarUp] = useState(false);
  const [charCount, setCharCount] = useState(0);
  const [historyOpen, setHistoryOpen] = useState(false);
  const [historyList, setHistoryList] = useState<HistoryEntry[]>([]);
  const [historyLoading, setHistoryLoading] = useState(false);
  const [historyError, setHistoryError] = useState('');
  const [historyPreview, setHistoryPreview] = useState<{ entry: HistoryEntry; content: string } | null>(null);
  const frontmatterRef = useRef('');
  const saveTimer = useRef(0);
  const latestMarkdown = useRef('');
  const saveSeq = useRef(0);
  const fileInputRef = useRef<HTMLInputElement>(null);
  // §16 修复③：已保存基线（frontmatter + markdown）+ 加载抑制标记。
  // 加载/恢复触发的 setContent 不算编辑；内容与基线等价时不标 dirty
  // 不保存——消除「打开笔记即回写 put」的回声源（桌面确认失败期间的
  // 双端回声循环即由它触发）。
  const savedContentRef = useRef<string | null>(null);
  const loadingContentRef = useRef(false);

  // 比较前规范化：桌面写盘可能是 CRLF，编辑器序列化恒 LF
  const sameAsSaved = (markdown: string): boolean => {
    const baseline = savedContentRef.current;
    if (baseline === null) return false;
    return (
      (frontmatterRef.current + markdown).replace(/\r\n/g, '\n') ===
      baseline.replace(/\r\n/g, '\n')
    );
  };

  const editor = useEditor({
    extensions: [
      StarterKit,
      Markdown,
      // 任务列表（@tiptap/markdown 支持 - [ ] 序列化；高亮/表格暂不加——
      // 官方 markdown 包不支持其序列化，会造成保存丢格式）
      TaskList,
      TaskItem.configure({ nested: true }),
      Placeholder.configure({ placeholder: '记录自己的想法' }),
      // 高亮/表格：官方包 v3.23+ 已内建 markdown 序列化（==text== / GFM 表格），
      // 与桌面语法互通（桌面 flowix:text 彩色注释在移动端会降级为默认高亮）
      Highlight,
      Table.configure({ resizable: false }),
      TableRow,
      TableHeader,
      TableCell,
      // 图片附件：asset://localhost 语法与桌面互认，跨设备按文件名回落
      MobileImage.configure({ notebookId: note.notebookId }),
      // AI 对话卡片：HTML 注释存储与桌面互认；mobile 卡可对话，桌面卡只读。
      // P1a：携带所在笔记，卡片首条消息自动注入笔记上下文；
      // P1b：卡片全屏跳转（App 的 openChat）
      AgentThreadCardMobile.configure({
        noteContext: { notebookId: note.notebookId, relativePath: note.relativePath },
        onOpenThread: onOpenChat ?? null,
      }),
    ],
    content: '',
    editable: false,
    onUpdate: ({ editor: instance }) => {
      latestMarkdown.current = instance.getMarkdown();
      setCharCount(instance.getText().length);
      // 加载触发的 setContent 回写不算编辑（基线随后在加载流程里落）
      if (loadingContentRef.current) return;
      // 内容与已保存基线等价（撤销回原点/序列化 roundtrip）：不标 dirty 不保存
      if (sameAsSaved(latestMarkdown.current)) {
        setSaveState('saved');
        return;
      }
      setSaveState('dirty');
      // 800ms 停止输入后自动保存（后端另有 3s 同步防抖，两级不冲突）
      window.clearTimeout(saveTimer.current);
      saveTimer.current = window.setTimeout(() => void performSave(), AUTOSAVE_DELAY);
    },
  });

  const performSave = useCallback(async () => {
    const markdown = latestMarkdown.current;
    const seq = ++saveSeq.current;
    setSaveState('saving');
    try {
      const full = frontmatterRef.current + markdown;
      await api.noteSave(note.notebookId, note.relativePath, full);
      // §16 修复③：保存成功推进基线（后续等价内容不再触发保存）
      savedContentRef.current = full;
      if (seq === saveSeq.current) setSaveState('saved');
    } catch (e) {
      if (seq === saveSeq.current) {
        setSaveState('dirty');
        setError(String(e));
      }
    }
  }, [note.notebookId, note.relativePath]);

  // 选图：WebView 文件选择器（wry onShowFileChooser）→ base64 → attachment_save
  // → 编辑器插入图片节点（markdown 序列化为桌面同款 asset://localhost 语法）
  const onPickImage = useCallback(
    async (ev: ChangeEvent<HTMLInputElement>) => {
      const file = ev.target.files?.[0];
      ev.target.value = '';
      if (!file || !editor) return;
      setError('');
      try {
        const dataUrl = await new Promise<string>((resolve, reject) => {
          const reader = new FileReader();
          reader.onload = () => resolve(String(reader.result));
          reader.onerror = () => reject(new Error('READ_FILE_FAILED'));
          reader.readAsDataURL(file);
        });
        const base64 = dataUrl.slice(dataUrl.indexOf(',') + 1);
        const saved = await api.attachmentSave(note.notebookId, file.name, base64);
        const alt = file.name.replace(/\.[^.]+$/, '');
        editor.chain().focus().insertAttachmentImage({ storageKey: saved.absolutePath, alt }).run();
      } catch (e) {
        setError(`图片插入失败：${String(e)}`);
      }
    },
    [editor, note.notebookId],
  );

  // 历史版本：列表（云端 v2_history）→ 预览 → 恢复（setContent 触发自动保存链路）
  const openHistory = useCallback(async () => {
    setHistoryOpen(true);
    setHistoryPreview(null);
    setHistoryError('');
    setHistoryLoading(true);
    try {
      const entries = await api.noteHistory(note.notebookId, note.relativePath);
      setHistoryList(entries);
    } catch (e) {
      setHistoryError(`历史版本加载失败：${String(e)}`);
    } finally {
      setHistoryLoading(false);
    }
  }, [note.notebookId, note.relativePath]);

  const openHistoryPreview = useCallback(
    async (entry: HistoryEntry) => {
      setHistoryError('');
      try {
        const content = await api.noteHistoryRead(note.notebookId, note.relativePath, entry.revision);
        setHistoryPreview({ entry, content });
      } catch (e) {
        setHistoryError(`版本读取失败：${String(e)}`);
      }
    },
    [note.notebookId, note.relativePath],
  );

  const restoreHistoryVersion = useCallback(() => {
    if (!editor || !historyPreview) return;
    const { frontmatter, body } = splitFrontmatter(historyPreview.content);
    frontmatterRef.current = frontmatter;
    editor.commands.setContent(body, { contentType: 'markdown' });
    setHistoryOpen(false);
    setHistoryPreview(null);
  }, [editor, historyPreview]);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      setLoading(true);
      setError('');
      try {
        const content = await api.noteRead(note.notebookId, note.relativePath);
        if (cancelled) return;
        const { frontmatter, body } = splitFrontmatter(content);
        frontmatterRef.current = frontmatter;
        // §16 修复③：加载触发的 onUpdate 不算编辑（setContent 必触发）；
        // 基线取「原始 frontmatter + 序列化 roundtrip 产物」——吸收
        // parse→serialize 的非字节级差异（如 CRLF→LF），打开即保存的回声
        // 才能被 sameAsSaved 判等拦下
        loadingContentRef.current = true;
        // contentType: 'markdown' 与桌面端一致：否则 Markdown 被当纯文本灌入，
        // 序列化时段落换行退化为空格（单行化）
        editor?.commands.setContent(body, { contentType: 'markdown' });
        loadingContentRef.current = false;
        savedContentRef.current =
          frontmatter + (editor ? editor.getMarkdown() : body);
        editor?.setEditable(true);
        setSaveState('saved');
      } catch (e) {
        loadingContentRef.current = false;
        if (!cancelled) setError(String(e));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [note.notebookId, note.relativePath, editor]);

  // 返回前落盘未保存内容。
  // saveState 走 ref：cleanup 只在卸载/换笔记时执行——若把 saveState 放 deps，
  // dirty 切换的 cleanup 会清掉刚排上的自动保存 timer（单次 setContent 的
  // 恢复/插图路径只有一次 onUpdate，被清后永不再保存）。
  const saveStateRef = useRef<SaveState>('saved');
  saveStateRef.current = saveState;
  useEffect(
    () => () => {
      window.clearTimeout(saveTimer.current);
      if (saveStateRef.current === 'dirty' || saveStateRef.current === 'saving') void performSave();
    },
    [performSave],
  );

  // 键盘弹出：工具栏悬浮键盘上方（visualViewport 跟随）
  useEffect(() => {
    const viewport = window.visualViewport;
    if (!viewport) return;
    const update = () => {
      const overlap = Math.max(0, window.innerHeight - viewport.height - viewport.offsetTop);
      const up = overlap > 60;
      setToolbarUp(up);
      document.documentElement.style.setProperty(
        '--kb-offset',
        `${up ? Math.round(overlap + viewport.offsetTop) : 0}px`,
      );
    };
    viewport.addEventListener('resize', update);
    viewport.addEventListener('scroll', update);
    update();
    return () => {
      viewport.removeEventListener('resize', update);
      viewport.removeEventListener('scroll', update);
      document.documentElement.style.removeProperty('--kb-offset');
    };
  }, []);

  if (!editor) return null;

  const headingActive = [1, 2, 3].find((level) => editor.isActive('heading', { level }));

  return (
    <div className="screen editor-screen">
      <header className="m-bar">
        <button className="m-icon-btn" onClick={onBack} aria-label="返回">
          <ArrowLeft />
        </button>
        <div className="m-bar-title small">{note.relativePath}</div>
        <button className="m-icon-btn" onClick={() => void openHistory()} aria-label="历史版本">
          <Clock3 />
        </button>
        <div className="save-state">
          <span className={`state ${saveState === 'dirty' ? 'dirty' : ''}`}>
            {saveState === 'saving' ? '保存中…' : saveState === 'dirty' ? '● 未保存' : '已保存'}
          </span>
          {charCount > 0 && <span className="word-count">{charCount} 字</span>}
        </div>
      </header>
      {loading && <div className="m-empty">加载中…</div>}
      {error && (
        <div className="login-error" onClick={() => setError('')}>
          {error}
        </div>
      )}
      <div className="editor-body">
        <div className="prose-mobile">
          <EditorContent editor={editor} />
        </div>
      </div>

      <input
        ref={fileInputRef}
        type="file"
        accept="image/*"
        className="file-input-hidden"
        onChange={(ev) => void onPickImage(ev)}
      />

      {historyOpen && (
        <>
          <div className="m-sheet-mask" onClick={() => setHistoryOpen(false)} />
          <div className="m-sheet history-sheet">
            <div className="m-sheet-handle" />
            {historyPreview ? (
              <>
                <div className="m-sheet-title">
                  {formatHistoryTime(historyPreview.entry.createdAt)} ·{' '}
                  {historyPreview.entry.sizeBytes} 字节
                </div>
                <pre className="history-preview">{historyPreview.content}</pre>
                <div className="history-actions">
                  <button className="m-btn-ghost" onClick={() => setHistoryPreview(null)}>
                    返回列表
                  </button>
                  <button className="m-btn-primary" onClick={restoreHistoryVersion}>
                    恢复此版本
                  </button>
                </div>
              </>
            ) : (
              <>
                <div className="m-sheet-title">历史版本</div>
                {historyLoading && <div className="m-empty">加载中…</div>}
                {historyError && <div className="login-error">{historyError}</div>}
                {!historyLoading && !historyError && historyList.length === 0 && (
                  <div className="m-empty">暂无历史版本</div>
                )}
                <div className="history-list">
                  {historyList.map((entry, index) => (
                    <button
                      key={entry.revision}
                      className="m-sheet-item history-item"
                      onClick={() => void openHistoryPreview(entry)}
                    >
                      <span className="history-time">{formatHistoryTime(entry.createdAt)}</span>
                      <span className="history-meta">
                        {entry.sizeBytes} 字节{index === 0 ? ' · 最新' : ''}
                      </span>
                    </button>
                  ))}
                </div>
              </>
            )}
          </div>
        </>
      )}

      <div className={`format-toolbar ${toolbarUp ? 'keyboard-up' : ''}`}>
        <button
          className={`tool-btn ${editor.isActive('bold') ? 'active' : ''}`}
          onClick={() => editor.chain().focus().toggleBold().run()}
          aria-label="粗体"
        >
          <Bold />
        </button>
        <button
          className={`tool-btn ${editor.isActive('italic') ? 'active' : ''}`}
          onClick={() => editor.chain().focus().toggleItalic().run()}
          aria-label="斜体"
        >
          <Italic />
        </button>
        <button
          className={`tool-btn ${editor.isActive('strike') ? 'active' : ''}`}
          onClick={() => editor.chain().focus().toggleStrike().run()}
          aria-label="删除线"
        >
          <Strikethrough />
        </button>
        <button
          className={`tool-btn ${editor.isActive('highlight') ? 'active' : ''}`}
          onClick={() => editor.chain().focus().toggleHighlight().run()}
          aria-label="高亮"
        >
          <Highlighter />
        </button>
        <div className="tool-sep" />
        <button
          className={`tool-btn ${headingActive === 1 ? 'active' : ''}`}
          onClick={() => editor.chain().focus().toggleHeading({ level: 1 }).run()}
          aria-label="标题一"
        >
          <Heading1 />
        </button>
        <button
          className={`tool-btn ${headingActive === 2 ? 'active' : ''}`}
          onClick={() => editor.chain().focus().toggleHeading({ level: 2 }).run()}
          aria-label="标题二"
        >
          <Heading2 />
        </button>
        <button
          className={`tool-btn ${headingActive === 3 ? 'active' : ''}`}
          onClick={() => editor.chain().focus().toggleHeading({ level: 3 }).run()}
          aria-label="标题三"
        >
          <Heading3 />
        </button>
        <div className="tool-sep" />
        <button
          className={`tool-btn ${editor.isActive('bulletList') ? 'active' : ''}`}
          onClick={() => editor.chain().focus().toggleBulletList().run()}
          aria-label="无序列表"
        >
          <List />
        </button>
        <button
          className={`tool-btn ${editor.isActive('taskList') ? 'active' : ''}`}
          onClick={() => editor.chain().focus().toggleTaskList().run()}
          aria-label="任务列表"
        >
          <ListTodo />
        </button>
        <button
          className={`tool-btn ${editor.isActive('orderedList') ? 'active' : ''}`}
          onClick={() => editor.chain().focus().toggleOrderedList().run()}
          aria-label="有序列表"
        >
          <ListOrdered />
        </button>
        <button
          className={`tool-btn ${editor.isActive('blockquote') ? 'active' : ''}`}
          onClick={() => editor.chain().focus().toggleBlockquote().run()}
          aria-label="引用"
        >
          <Quote />
        </button>
        <button
          className={`tool-btn ${editor.isActive('codeBlock') ? 'active' : ''}`}
          onClick={() => editor.chain().focus().toggleCodeBlock().run()}
          aria-label="代码块"
        >
          <Code />
        </button>
        <button
          className={`tool-btn ${editor.isActive('table') ? 'active' : ''}`}
          onClick={() =>
            editor.isActive('table')
              ? editor.chain().focus().deleteNode('table').run()
              : editor
                  .chain()
                  .focus()
                  .insertTable({ rows: 2, cols: 2, withHeaderRow: true })
                  .run()
          }
          aria-label={editor.isActive('table') ? '删除表格' : '插入表格'}
        >
          <TableIcon />
        </button>
        <button
          className="tool-btn"
          onClick={() => fileInputRef.current?.click()}
          aria-label="插入图片"
        >
          <ImagePlus />
        </button>
        <button
          className="tool-btn"
          onClick={() => editor.chain().focus().insertMobileAgentThreadCard().run()}
          aria-label="插入 AI 对话"
        >
          <Bot />
        </button>
        <div className="tool-sep" />
        <button
          className="tool-btn"
          onClick={() => editor.chain().focus().undo().run()}
          aria-label="撤销"
        >
          <Undo2 />
        </button>
        <button
          className="tool-btn"
          onClick={() => editor.chain().focus().redo().run()}
          aria-label="重做"
        >
          <Redo2 />
        </button>
      </div>
    </div>
  );
}
