import { afterEach, describe, expect, it } from 'vitest';
import { createAgentThreadCardDom } from './agent-thread-card-dom-factory';

describe('agent thread card message scrollbar', () => {
  afterEach(() => {
    delete document.documentElement.dataset.platform;
  });

  it('uses the shared overlay scrollbar for the macOS message scroller', () => {
    document.documentElement.dataset.platform = 'mac';
    const parts = createAgentThreadCardDom({
      inputDraft: '',
      t: (key) => key,
      onCardMouseDown: () => {},
      onTitleDoubleClick: () => {},
      onDeleteClick: () => {},
      onFullscreenClick: () => {},
      onCollapseClick: () => {},
      onSaveProfileClick: () => {},
      onBodyClick: () => {},
      onBodyScroll: () => {},
      onBodyWheel: () => {},
    });

    const frame = parts.body.parentElement;
    expect(frame?.classList.contains('overlay-scrollbar-frame--custom-mac')).toBe(true);
    expect(frame?.parentElement).toBe(parts.container);
    expect(parts.body.classList.contains('overlay-scrollbar')).toBe(true);
    expect(frame?.querySelector('.overlay-scrollbar-thumb')).not.toBeNull();
    parts.disposeBodyScrollbar();
  });

  /** AGENT-ASSET-ROADMAP P1-C：头部「保存为个人 Agent」按钮接线。 */
  it('renders the save-as-agent action and forwards its click', () => {
    const clicks: number[] = [];
    const parts = createAgentThreadCardDom({
      inputDraft: '',
      t: (key) => key,
      onCardMouseDown: () => {},
      onTitleDoubleClick: () => {},
      onDeleteClick: () => {},
      onFullscreenClick: () => {},
      onCollapseClick: () => {},
      onSaveProfileClick: () => clicks.push(1),
      onBodyClick: () => {},
      onBodyScroll: () => {},
      onBodyWheel: () => {},
    });

    expect(parts.saveProfileButton.getAttribute('aria-label')).toBe('editor.threadCard.saveAsAgent');
    expect(parts.saveProfileButton.hidden).toBe(false);
    parts.saveProfileButton.click();
    expect(clicks).toHaveLength(1);
  });
});
