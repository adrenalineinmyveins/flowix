// 主题调度（移动端简化版，桌面真源 app/flowix-web/lib/theme/）：
// localStorage flowix-theme（与桌面同 key）+ data-theme + color-scheme。
// system 主题由 prefers-color-scheme 派生，桌面端四套主题 CSS 直接复用。

export type ThemeId = 'system' | 'light' | 'dark' | 'rock' | 'ember';

export const THEME_OPTIONS: { id: ThemeId; label: string }[] = [
  { id: 'system', label: '跟随系统' },
  { id: 'light', label: '浅色' },
  { id: 'dark', label: '深色' },
  { id: 'rock', label: '岩灰' },
  { id: 'ember', label: '余烬' },
];

const STORAGE_KEY = 'flowix-theme';

function resolve(id: ThemeId): Exclude<ThemeId, 'system'> {
  if (id !== 'system') return id;
  return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
}

export function applyTheme(id: ThemeId) {
  const resolved = resolve(id);
  const root = document.documentElement;
  root.dataset.theme = resolved;
  root.style.colorScheme = resolved === 'dark' ? 'dark' : 'light';
}

export function loadTheme(): ThemeId {
  const stored = localStorage.getItem(STORAGE_KEY);
  const id = THEME_OPTIONS.some((option) => option.id === stored)
    ? (stored as ThemeId)
    : 'system';
  return id;
}

export function saveTheme(id: ThemeId) {
  localStorage.setItem(STORAGE_KEY, id);
  applyTheme(id);
}

/** 应用启动时调用；system 模式下跟随系统切换。 */
export function initTheme(): ThemeId {
  const id = loadTheme();
  applyTheme(id);
  const media = window.matchMedia('(prefers-color-scheme: dark)');
  media.addEventListener('change', () => {
    if (loadTheme() === 'system') applyTheme('system');
  });
  return id;
}
