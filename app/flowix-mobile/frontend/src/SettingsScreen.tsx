import { useEffect, useState } from 'react';
import { ArrowLeft, LogOut, RefreshCw, Download, Sparkles, Globe, Inbox } from 'lucide-react';
import { api, AgentProviderConfig, CloudState, NotebookInfo, RedactionStatus, UpdateCheck } from './api';
import { saveTheme, THEME_OPTIONS, ThemeId } from './theme';

interface Props {
  cloud: CloudState;
  notebooks: NotebookInfo[];
  onBack: () => void;
  onLogout: () => void;
}

const FONT_SIZES: { id: string; label: string; value: string }[] = [
  { id: 'small', label: '小', value: '15px' },
  { id: 'standard', label: '标准', value: '16px' },
  { id: 'large', label: '大', value: '18px' },
];

const FONT_SIZE_KEY = 'flowix-font-size';

const AUTH_STATUS_LABELS: Record<string, string> = {
  authenticated: '已登录',
  restoring: '恢复会话中',
  signed_out: '未登录',
};

export function SettingsScreen({ cloud, notebooks, onBack, onLogout }: Props) {
  const [busy, setBusy] = useState(false);
  const [theme, setTheme] = useState<ThemeId>(
    () => (localStorage.getItem('flowix-theme') as ThemeId) || 'system',
  );
  const [fontSize, setFontSize] = useState(() => localStorage.getItem(FONT_SIZE_KEY) || 'standard');
  const [message, setMessage] = useState('');
  const [update, setUpdate] = useState<UpdateCheck | null>(null);
  const [provider, setProvider] = useState<AgentProviderConfig>({
    apiBase: '',
    apiKey: '',
    model: '',
  });
  const [net, setNet] = useState<RedactionStatus | null>(null);
  const [netUsername, setNetUsername] = useState('');
  const [netRelayUrl, setNetRelayUrl] = useState('');

  useEffect(() => {
    api
      .agentProviderGet()
      .then(setProvider)
      .catch(() => undefined);
    api
      .redactionStatus()
      .then((status) => {
        setNet(status);
        if (status.registered) setNetRelayUrl(status.relayUrl);
      })
      .catch(() => undefined);
  }, []);

  const doNetRegister = async () => {
    if (busy) return;
    setBusy(true);
    setMessage('注册网络身份中…');
    try {
      const info = await api.redactionRegister(netUsername, netRelayUrl);
      setNet({ registered: true, username: info.username, relayUrl: info.relayUrl, publicKey: info.publicKey, agentId: info.agentId });
      setMessage(`网络身份就绪：${info.agentId}`);
    } catch (e) {
      setMessage(String(e));
    } finally {
      setBusy(false);
    }
  };

  const doNetPoll = async () => {
    if (busy) return;
    setBusy(true);
    setMessage('拉取收件箱…');
    try {
      const summary = await api.redactionPollOnce();
      setMessage(`收件箱：新收 ${summary.received} 条（重复 ${summary.duplicates}）`);
    } catch (e) {
      setMessage(String(e));
    } finally {
      setBusy(false);
    }
  };

  const doSaveProvider = async () => {
    if (busy) return;
    setBusy(true);
    setMessage('保存 AI 配置…');
    try {
      await api.agentProviderSet(provider);
      setMessage('AI 配置已保存（仅存本机，不上云）');
    } catch (e) {
      setMessage(String(e));
    } finally {
      setBusy(false);
    }
  };

  const doSync = async () => {
    if (busy) return;
    setBusy(true);
    setMessage('同步中…');
    try {
      const summary = await api.syncNow();
      setMessage(
        `已同步 · 上传 ${summary.uploaded} · 下载 ${summary.downloaded} · 删除 ${summary.deleted}`,
      );
    } catch (e) {
      setMessage(String(e));
    } finally {
      setBusy(false);
    }
  };

  const doLogout = async () => {
    if (busy) return;
    setBusy(true);
    try {
      await api.logout();
      onLogout();
    } catch (e) {
      setMessage(String(e));
      setBusy(false);
    }
  };

  const doCheckUpdate = async () => {
    if (busy) return;
    setBusy(true);
    setUpdate(null);
    setMessage('检查更新中…');
    try {
      const result = await api.checkUpdate();
      setUpdate(result);
      setMessage(result.hasUpdate ? `发现新版本 ${result.latestVersion}` : '已是最新版本');
    } catch (e) {
      setMessage(`检查更新失败：${String(e)}`);
    } finally {
      setBusy(false);
    }
  };

  const user = cloud.account?.user;
  const initial = (user?.displayName || user?.email || '?').trim().charAt(0).toUpperCase();

  return (
    <div className="screen">
      <header className="m-bar">
        <button className="m-icon-btn" onClick={onBack} aria-label="返回">
          <ArrowLeft />
        </button>
        <div className="m-bar-title">设置</div>
        <div />
      </header>

      <div className="settings-list">
        <div className="m-card">
          <div className="settings-avatar-row">
            <div className="settings-avatar">{initial}</div>
            <div style={{ flex: 1, minWidth: 0 }}>
              <div style={{ fontWeight: 600 }}>{user?.displayName || '-'}</div>
              <div style={{ fontSize: 13, color: 'var(--muted-foreground)', wordBreak: 'break-all' }}>
                {user?.email || '-'}
              </div>
            </div>
          </div>
        </div>

        <div className="m-card">
          <div className="m-card-title">外观</div>
          <div className="theme-row">
            {THEME_OPTIONS.map((option) => (
              <button
                key={option.id}
                className={`theme-chip ${theme === option.id ? 'active' : ''}`}
                onClick={() => {
                  setTheme(option.id);
                  saveTheme(option.id);
                }}
              >
                {option.label}
              </button>
            ))}
          </div>
          <div className="font-size-row">
            {FONT_SIZES.map((size) => (
              <button
                key={size.id}
                className={`font-size-chip ${fontSize === size.id ? 'active' : ''}`}
                onClick={() => {
                  setFontSize(size.id);
                  localStorage.setItem(FONT_SIZE_KEY, size.id);
                  document.documentElement.style.setProperty('--app-font-size', size.value);
                }}
              >
                {size.label}
              </button>
            ))}
          </div>
        </div>

        <div className="m-card">
          <div className="m-card-title">AI 助手</div>
          <div className="card-hint" style={{ textAlign: 'left' }}>
            任意 OpenAI 兼容服务（DeepSeek / OpenAI / 自建网关等）。API Key 只保存在本机，
            绝不参与云同步；对话内容经 Flowix Cloud 与桌面端双向同步。
          </div>
          <div className="provider-form">
            <input
              className="m-input"
              placeholder="API 地址，如 https://api.deepseek.com"
              value={provider.apiBase}
              onChange={(event) => setProvider({ ...provider, apiBase: event.target.value })}
            />
            <input
              className="m-input"
              placeholder="API Key"
              type="password"
              autoCapitalize="off"
              value={provider.apiKey}
              onChange={(event) => setProvider({ ...provider, apiKey: event.target.value })}
            />
            <input
              className="m-input"
              placeholder="模型，如 deepseek-chat"
              autoCapitalize="off"
              value={provider.model}
              onChange={(event) => setProvider({ ...provider, model: event.target.value })}
            />
            <input
              className="m-input"
              placeholder="搜索 API Key（可选，Tavily；留空用内置必应搜索）"
              type="password"
              autoCapitalize="off"
              value={provider.searchApiKey ?? ''}
              onChange={(event) =>
                setProvider({ ...provider, searchApiKey: event.target.value || null })
              }
            />
          </div>
          <div style={{ padding: '12px 0' }}>
            <button className="m-btn-ghost" style={{ width: '100%' }} disabled={busy} onClick={doSaveProvider}>
              <Sparkles size={16} />
              保存 AI 配置
            </button>
          </div>
        </div>

        <div className="m-card">
          <div className="m-card-title">网络身份</div>
          <div className="card-hint" style={{ textAlign: 'left' }}>
            注册后本机拥有中继身份（Ed25519 密钥即身份），可被其他 Flowix 用户的 agent
            咨询；私钥只存本机，不上云。地址格式：用户名/mob@中继地址
          </div>
          {net?.registered ? (
            <>
              <div className="m-card-row">
                <span>身份地址</span>
                <span className="value" style={{ wordBreak: 'break-all' }}>{net.agentId}</span>
              </div>
              <div className="m-card-row">
                <span>中继</span>
                <span className="value" style={{ wordBreak: 'break-all' }}>{net.relayUrl}</span>
              </div>
              <div style={{ display: 'flex', gap: 10, padding: '12px 0' }}>
                <button className="m-btn-ghost" style={{ flex: 1 }} disabled={busy} onClick={doNetPoll}>
                  <Inbox size={16} />
                  拉取收件箱
                </button>
                <button
                  className="m-btn-danger"
                  style={{ flex: 1 }}
                  disabled={busy}
                  onClick={async () => {
                    if (busy) return;
                    setBusy(true);
                    try {
                      await api.redactionForget();
                      setNet({ registered: false, username: '', relayUrl: '', publicKey: '', agentId: '' });
                      setMessage('已忘记本机网络身份');
                    } catch (e) {
                      setMessage(String(e));
                    } finally {
                      setBusy(false);
                    }
                  }}
                >
                  <LogOut size={16} />
                  忘记身份
                </button>
              </div>
            </>
          ) : (
            <div className="provider-form">
              <input
                className="m-input"
                placeholder="用户名（如 alice）"
                autoCapitalize="off"
                value={netUsername}
                onChange={(event) => setNetUsername(event.target.value)}
              />
              <input
                className="m-input"
                placeholder="中继地址，如 http://8.160.162.251:8788"
                autoCapitalize="off"
                value={netRelayUrl}
                onChange={(event) => setNetRelayUrl(event.target.value)}
              />
              <div style={{ padding: '12px 0 0' }}>
                <button className="m-btn-ghost" style={{ width: '100%' }} disabled={busy} onClick={doNetRegister}>
                  <Globe size={16} />
                  注册 / 登录
                </button>
              </div>
            </div>
          )}
        </div>

        <div className="m-card">
          <div className="m-card-title">同步</div>
          <div className="m-card-row">
            <span>已缓存笔记本</span>
            <span className="value">{notebooks.length}</span>
          </div>
          <div className="m-card-row">
            <span>账号状态</span>
            <span className="value">{AUTH_STATUS_LABELS[cloud.authStatus] || cloud.authStatus}</span>
          </div>
          <div style={{ padding: '12px 0' }}>
            <button className="m-btn-ghost" style={{ width: '100%' }} disabled={busy} onClick={doSync}>
              <RefreshCw size={16} className={busy ? 'spin' : undefined} />
              立即同步
            </button>
          </div>
        </div>

        {message && (
          <div style={{ padding: '0 16px 12px', fontSize: 13, color: 'var(--muted-foreground)' }}>
            {message}
          </div>
        )}

        <div className="m-card">
          <div className="m-card-title">关于</div>
          <div className="m-card-row">
            <span>当前版本</span>
            <span className="value">{update?.currentVersion || '1.5.6'}</span>
          </div>
          {update?.hasUpdate && (
            <div className="m-card-row">
              <span>最新版本</span>
              <span className="value">{update.latestVersion}</span>
            </div>
          )}
          {update?.hasUpdate && update.notes && (
            <div className="card-hint" style={{ textAlign: 'left' }}>{update.notes}</div>
          )}
          <div style={{ display: 'flex', gap: 10, padding: '12px 0' }}>
            <button className="m-btn-ghost" style={{ flex: 1 }} disabled={busy} onClick={doCheckUpdate}>
              <RefreshCw size={16} className={busy ? 'spin' : undefined} />
              检查更新
            </button>
            {update?.hasUpdate && (
              <button
                className="m-btn-primary"
                style={{ flex: 1 }}
                onClick={() => void api.openUrl(update.downloadUrl)}
              >
                <Download size={16} />
                去下载
              </button>
            )}
          </div>
        </div>

        <div className="m-card" style={{ padding: '12px 16px' }}>
          <button className="m-btn-danger" style={{ width: '100%' }} disabled={busy} onClick={doLogout}>
            <LogOut size={18} />
            退出登录
          </button>
          <div className="card-hint">退出后本地缓存保留，重新登录同账号可继续使用</div>
        </div>

        <div className="version">Flowix Mobile 1.5.6 (M1)</div>
      </div>
    </div>
  );
}
