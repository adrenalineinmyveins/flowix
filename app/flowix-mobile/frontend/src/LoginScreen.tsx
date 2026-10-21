import { useState } from 'react';
import { Eye, EyeOff } from 'lucide-react';
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
  const [showPassword, setShowPassword] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [mode, setMode] = useState<'login' | 'register'>('login');

  const submit = async () => {
    if (busy || !email || !password) return;
    setBusy(true);
    setError('');
    try {
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
