import { afterEach, describe, expect, it, vi } from 'vitest';
import { NotePickerController } from './note-picker-controller';
import { AgentPickerController } from '../composer/agent-picker-controller';
import { ComposerAddMenuController } from '../composer/composer-add-menu-controller';
import type { ComposerImageController } from '../composer/composer-image-controller';

const mocks = vi.hoisted(() => ({
  searchPathNotes: vi.fn(),
  agentSubscriptions: vi.fn(),
  notebooks: [{ id: 'notebook-1', name: 'Notes' }],
}));

vi.mock('@platform/tauri/client', () => ({
  notes: { search: mocks.searchPathNotes },
  market: { subscriptions: mocks.agentSubscriptions },
}));
vi.mock('@features/memo/store/note-store', () => ({
  useNoteStore: { getState: () => ({ notebooks: mocks.notebooks }) },
}));

afterEach(() => {
  vi.useRealTimers();
  document.body.replaceChildren();
  mocks.searchPathNotes.mockReset();
});

describe('NotePickerController', () => {
  it('searches indexed notes and inserts a path reference without role choices', async () => {
    vi.useFakeTimers();
    mocks.searchPathNotes.mockResolvedValue([{ relativePath: 'Projects/Plan.md', title: 'Plan' }]);
    const trigger = document.createElement('button');
    const popover = document.createElement('div');
    const addPopover = document.createElement('div');
    const agentPopover = document.createElement('div');
    document.body.append(trigger, addPopover, popover, agentPopover);
    const injectMemoReference = vi.fn();
    let addMenu: ComposerAddMenuController | null = null;
    const picker = new NotePickerController({
      trigger,
      popover,
      t: (key) => key,
      isDestroyed: () => false,
      injectMemoReference,
      onSelect: () => addMenu?.close(),
    });
    const agentPicker = new AgentPickerController({
      trigger,
      popover: agentPopover,
      t: (key) => key,
      isDestroyed: () => false,
      injectAgentReference: vi.fn(),
      onSelect: () => addMenu?.close(),
    });

    addMenu = new ComposerAddMenuController({
      trigger,
      popover: addPopover,
      notePopover: popover,
      notePicker: picker,
      agentPopover,
      agentPicker,
      images: { addFiles: vi.fn() } as unknown as ComposerImageController,
      t: (key) => key,
      isDestroyed: () => false,
      openNotebookAgentSettings: vi.fn(),
    });
    trigger.click();
    const items = Array.from(addPopover.querySelectorAll<HTMLButtonElement>('button'));
    // 菜单项顺序：添加笔记 / 订阅的 Agent / 添加附件 / 项目 AI 设置。
    expect(items.map((item) => item.textContent)).toEqual([
      'editor.threadCard.addNote',
      'editor.threadCard.addAgent',
      'editor.threadCard.addAttachment',
      'editor.threadCard.projectAiSettings',
    ]);
    const addNote = items[0];
    addNote?.dispatchEvent(new Event('pointerenter'));
    addNote?.click();
    await vi.advanceTimersByTimeAsync(SEARCH_DELAY_MS_FOR_TEST);
    expect(mocks.searchPathNotes).toHaveBeenCalledWith('notebook-1', '', 10);
    expect(popover.textContent).toContain('Plan');
    expect(popover.textContent).not.toContain('selectRole');
    popover.querySelector<HTMLButtonElement>('.agent-thread-card__composer-note-item')?.click();
    expect(injectMemoReference).toHaveBeenCalledWith(expect.objectContaining({
      notebookId: 'notebook-1',
      relativePath: 'Projects/Plan.md',
      title: 'Plan',
    }));
    expect(popover.hidden).toBe(true);
    expect(addPopover.hidden).toBe(true);
    addMenu.dispose();
    picker.dispose();
    agentPicker.dispose();
  });
});

const SEARCH_DELAY_MS_FOR_TEST = 160;
