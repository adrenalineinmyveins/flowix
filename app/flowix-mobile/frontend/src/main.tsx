import React from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App';
import { initTheme } from './theme';
import './styles/theme/light.css';
import './styles/theme/dark.css';
import './styles/theme/rock.css';
import './styles/theme/ember.css';
import './styles/tokens.css';
import './styles/base.css';
import './styles/screens.css';
import './styles/editor.css';

initTheme();

// 恢复字号偏好（设置屏写入 localStorage）
(function restoreFontSize() {
  const value = { small: '15px', standard: '16px', large: '18px' }[
    localStorage.getItem('flowix-font-size') || 'standard'
  ];
  document.documentElement.style.setProperty('--app-font-size', value);
})();

createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
