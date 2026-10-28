import { useCallback, useEffect, useRef, useState } from 'react';
import { api, CloudState, NotebookInfo } from './api';
import { LoginScreen } from './LoginScreen';
import { NotesScreen } from './NotesScreen';
import { EditorScreen } from './EditorScreen';
import { SettingsScreen } from './SettingsScreen';
import { SearchScreen } from './SearchScreen';
import { ChatListScreen } from './ChatListScreen';
import { ChatScreen } from './ChatScreen';

export interface OpenNote {
  notebookId: string;
  relativePath: string;
}

type Screen = 'boot' | 'login' | 'notes' | 'editor' | 'settings' | 'search' | 'chats' | 'chat';

/** 返回键契约：进栈 pushState，系统返回触发 popstate 回退；
 * 栈底（notes/login）再按返回交给系统默认行为。 */
function navigate(screen: Screen, note: OpenNote | null, threadId: string | null = null) {
  history.pushState({ screen, note, threadId }, '');
}

export function App() {
  const [screen, setScreen] = useState<Screen>('boot');
  const [cloud, setCloud] = useState<CloudState | null>(null);
  const [notebooks, setNotebooks] = useState<NotebookInfo[]>([]);
  const [note, setNote] = useState<OpenNote | null>(null);
  const [threadId, setThreadId] = useState<string>('');
  // 会话附件锚定笔记本（P2a）：卡片全屏进入时为该笔记所在笔记本，否则第一个笔记本
  const [chatNotebookId, setChatNotebookId] = useState<string | null>(null);
  const notesRefreshRef = useRef<() => void>(() => {});

  const enterApp = useCallback(async () => {
    setScreen('notes');
    history.replaceState({ screen: 'notes' }, '');
    try {
      const list = await api.refreshNotebooks();
      setNotebooks(list);
    } catch {
      // 未登录或网络问题：留在笔记页，等事件/重试
    }
  }, []);

  // 统一的屏内跳转（同时维护返回栈）
  const go = useCallback((next: Screen, nextNote: OpenNote | null = null) => {
    if (next === 'editor' && nextNote) setNote(nextNote);
    if (next === 'notes') setNote(null);
    setScreen(next);
    navigate(next, nextNote);
  }, []);

  const openChat = useCallback((nextThreadId: string, notebookId?: string | null) => {
    setThreadId(nextThreadId);
    setChatNotebookId(notebookId ?? null);
    setScreen('chat');
    navigate('chat', null, nextThreadId);
  }, []);

  const back = useCallback((to: Screen) => {
    setScreen(to);
    if (to === 'notes') setNote(null);
    history.back();
  }, []);

  useEffect(() => {
    (async () => {
      try {
        const state = await api.cloudState();
        setCloud(state);
        if (state.authenticated) await enterApp();
        else if (state.authStatus === 'restoring') setScreen('boot');
        else {
          setScreen('login');
          history.replaceState({ screen: 'login' }, '');
        }
      } catch {
        setScreen('login');
        history.replaceState({ screen: 'login' }, '');
      }
    })();

    const unlisten = api
      ? window.__TAURI__!.event.listen<CloudState>('cloud-state-changed', (ev) => {
          setCloud(ev.payload);
          if (ev.payload.authStatus === 'signed_out') {
            setScreen('login');
            history.replaceState({ screen: 'login' }, '');
          }
        })
      : Promise.resolve(() => {});
    const unlistenCache = window.__TAURI__!.event.listen('cache-notes-changed', () => {
      notesRefreshRef.current();
    });

    // 系统返回键 → WebView history 后退 → popstate
    const onPopState = (ev: PopStateEvent) => {
      const target = (ev.state?.screen as Screen) || 'notes';
      setScreen(target);
      if (target !== 'editor') setNote(null);
      if (target !== 'chat') setThreadId('');
      if (ev.state?.threadId && target === 'chat') setThreadId(ev.state.threadId as string);
      if (target === 'notes') notesRefreshRef.current();
    };
    window.addEventListener('popstate', onPopState);

    // 唤起立即补拉：切回前台 0 等待对端新消息（后台 webview 冻结时事件不触发）
    const onVisibility = () => {
      if (document.visibilityState === 'visible') {
        void api.syncNow().catch(() => {});
      }
    };
    document.addEventListener('visibilitychange', onVisibility);

    return () => {
      unlisten.then((fn) => fn());
      unlistenCache.then((fn) => fn());
      window.removeEventListener('popstate', onPopState);
      document.removeEventListener('visibilitychange', onVisibility);
    };
  }, [enterApp]);

  if (screen === 'boot') {
    return (
      <div className="boot">
        <div className="boot-logo">Flowix</div>
        <div className="boot-sub">
          {cloud?.authStatus === 'restoring' ? '正在恢复会话…' : '加载中…'}
        </div>
      </div>
    );
  }

  if (screen === 'login' || !cloud?.authenticated) {
    return (
      <div key="login" className="m-fade-enter" style={{ height: '100%' }}>
        <LoginScreen
          onAuthenticated={async () => {
            setCloud(await api.cloudState());
            await enterApp();
          }}
        />
      </div>
    );
  }

  if (screen === 'settings') {
    return (
      <div key="settings" className="m-push-enter" style={{ height: '100%' }}>
        <SettingsScreen
          cloud={cloud}
          notebooks={notebooks}
          onBack={() => back('notes')}
          onLogout={() => {
            setNotebooks([]);
            setScreen('login');
            history.replaceState({ screen: 'login' }, '');
          }}
        />
      </div>
    );
  }

  if (screen === 'search') {
    return (
      <div key="search" className="m-fade-enter" style={{ height: '100%' }}>
        <SearchScreen
          notebooks={notebooks}
          onBack={() => back('notes')}
          onOpenNote={(n) => go('editor', n)}
        />
      </div>
    );
  }

  if (screen === 'editor' && note) {
    return (
      <div key={`editor-${note.relativePath}`} className="m-push-enter" style={{ height: '100%' }}>
        <EditorScreen
          note={note}
          onBack={() => {
            back('notes');
            notesRefreshRef.current();
          }}
          onOpenChat={openChat}
        />
      </div>
    );
  }

  if (screen === 'chats') {
    return (
      <div key="chats" className="m-push-enter" style={{ height: '100%' }}>
        <ChatListScreen
          onOpenThread={openChat}
          onNewChat={() => openChat('')}
          onBack={() => back('notes')}
        />
      </div>
    );
  }

  if (screen === 'chat') {
    return (
      <div key={`chat-${threadId}`} className="m-push-enter" style={{ height: '100%' }}>
        <ChatScreen
          threadId={threadId}
          onBack={() => back('chats')}
          onThreadCreated={(resolved) => setThreadId(resolved)}
          attachmentNotebookId={chatNotebookId ?? notebooks[0]?.id ?? null}
          onOpenSettings={() => go('settings')}
        />
      </div>
    );
  }

  return (
    <div key="notes" className="m-fade-enter" style={{ height: '100%' }}>
      <NotesScreen
        notebooks={notebooks}
        registerRefresh={(fn) => (notesRefreshRef.current = fn)}
        onOpenNote={(n) => go('editor', n)}
        onOpenSettings={() => go('settings')}
        onOpenSearch={() => go('search')}
        onOpenChats={() => go('chats')}
      />
    </div>
  );
}
