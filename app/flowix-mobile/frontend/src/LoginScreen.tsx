import { useEffect, useState } from 'react';
import { Eye, EyeOff, Server } from 'lucide-react';
import { api } from './api';

/** 后端错误码 → 用户可读文案（桌面文案风格：简短、给动作建议）。 */
const ERROR_MESSAGES: Record<string, string> = {
  INVALID_CREDENTIALS: '邮箱或密码不正确',
  EMAIL_TAKEN: '该邮箱已注册，请直接登录',
};

function readableError(error: unknown): string {
  const text = String(error);
  for (const [code, message] of Object.entries(ERROR_MESSAGES)) {
    if (text.includes(code)) return message;
  }
  return text;
}

export function LoginScreen({ onAuthenticated }: { onAuthenticated: () => Promise<void> }) {
  const [email, setEmail] = useState('');
  const [password, setPassword] = useState('');
  const [server, setServer] = useState('');
  const [currentServer, setCurrentServer] = useState('');
  const [showServer, setShowServer] = useState(false);
  const [showPassword, setShowPassword] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [mode, setMode] = useState<'login' | 'register'>('login');

  useEffect(() => {
    api
      .cloudGetApiBase()
      .then((base) => {
        setServer(base);
        setCurrentServer(base);
      })
      .catch(() => undefined);
  }, []);

  const submit = async () => {
    if (busy || !email || !password) return;
    setBusy(true);
    setError('');
    try {
      // 服务器地址有变化：先切换（重建同步引擎），再认证
      const next = server.trim().replace(/\/+$/, '');
      if (next && next !== currentServer) {
        await api.cloudSetApiBase(next);
        setCurrentServer(next);
      }
      if (mode === 'login') {
        await api.login(email, password);
      } else {
        await api.register(email, password, email.split('@')[0]);
      }
      await onAuthenticated();
    } catch (e) {
      setError(readableError(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="login-screen">
      <div className="login-logo">Flowix</div>
      <div className="login-sub">云端笔记本 · 移动端</div>
      <div className="login-form">
        {showServer ? (
          <div className="login-field">
            <input
              className="m-input login-server"
              type="url"
              autoCapitalize="off"
              placeholder="服务器地址，如 http://192.168.1.10:8787"
              value={server}
              onChange={(e) => setServer(e.target.value)}
            />
            <button
              className="m-icon-btn eye"
              onClick={() => setShowServer(false)}
              aria-label="收起服务器设置"
            >
              <Server />
            </button>
          </div>
        ) : (
          <button className="login-server-toggle" onClick={() => setShowServer(true)}>
            服务器：{currentServer || '默认'}
          </button>
        )}
        <input
          className="m-input"
          type="email"
          placeholder="邮箱"
          value={email}
          autoComplete="off"
          onChange={(e) => setEmail(e.target.value)}
        />
        <div className="login-field">
          <input
            className="m-input"
            type={showPassword ? 'text' : 'password'}
            placeholder="密码"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && submit()}
          />
          <button
            className="m-icon-btn eye"
            onClick={() => setShowPassword(!showPassword)}
            aria-label={showPassword ? '隐藏密码' : '显示密码'}
          >
            {showPassword ? <EyeOff /> : <Eye />}
          </button>
        </div>
        <button
          className="m-btn-primary"
          disabled={busy || !email || !password}
          onClick={submit}
        >
          {busy ? (mode === 'login' ? '登录中…' : '注册中…') : mode === 'login' ? '登录' : '注册并登录'}
        </button>
        <button className="login-link" onClick={() => setMode(mode === 'login' ? 'register' : 'login')}>
          {mode === 'login' ? '没有账号？注册一个' : '已有账号？直接登录'}
        </button>
        {error && <div className="login-error">{error}</div>}
      </div>
    </div>
  );
}
