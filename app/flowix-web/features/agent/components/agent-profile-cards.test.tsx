import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';

const agentProfileMock = vi.hoisted(() => ({
  list: vi.fn(),
  get: vi.fn(),
  create: vi.fn(),
  update: vi.fn(),
  delete: vi.fn(),
  getState: vi.fn(),
}));

vi.mock('@platform/tauri/client', async (importOriginal) => ({
  ...(await importOriginal<Record<string, unknown>>()),
  agentProfile: agentProfileMock,
}));

import type { AgentProfile } from '@/lib/types/agent-profile';
import { AgentProfileCardsDialog } from '@features/agent/components/agent-profile-cards';

const CWD = '/notes/book';

function profile(overrides: Partial<AgentProfile> = {}): AgentProfile {
  return {
    flowix_agent_id: 'agt_first00000001',
    flowix_display_name: '财报解读助手',
    flowix_bio: '聚焦财报归因',
    flowix_expertise: ['finance/财报'],
    flowix_agent_type: 'codex',
    flowix_model: null,
    flowix_consultable: true,
    flowix_redaction_level: null,
    flowix_disclosure_scope: [],
    flowix_created_at: 1,
    flowix_updated_at: 1,
    ...overrides,
  };
}

const FIRST = profile();

let host: HTMLDivElement;
let root: Root;

async function mount() {
  await act(async () => {
    root.render(<AgentProfileCardsDialog notebookPath={CWD} onClose={() => {}} />);
  });
}

// 删除确认走 Radix Dialog（Portal 挂到 document.body），因此按 body 范围查找。
function button(text: string): HTMLButtonElement {
  const element = [...document.body.querySelectorAll('button')].find(
    (node) => node.textContent?.trim() === text,
  );
  expect(element, `button ${text}`).toBeDefined();
  return element!;
}

async function click(text: string) {
  await act(async () => button(text).click());
}

function selects(): HTMLSelectElement[] {
  return [...host.querySelectorAll('select')];
}

function textareas(): HTMLTextAreaElement[] {
  return [...host.querySelectorAll('textarea')];
}

function setValue(
  element: HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement,
  value: string,
) {
  const proto = element instanceof HTMLSelectElement
    ? HTMLSelectElement.prototype
    : element instanceof HTMLTextAreaElement
      ? HTMLTextAreaElement.prototype
      : HTMLInputElement.prototype;
  const setter = Object.getOwnPropertyDescriptor(proto, 'value')!.set!;
  setter.call(element, value);
  // text input 只监听 input 事件, select 只监听 change —— 两个都派发。
  element.dispatchEvent(new Event('input', { bubbles: true }));
  element.dispatchEvent(new Event('change', { bubbles: true }));
}

beforeEach(() => {
  vi.clearAllMocks();
  agentProfileMock.list.mockResolvedValue([FIRST]);
  agentProfileMock.get.mockResolvedValue(FIRST);
  agentProfileMock.getState.mockResolvedValue({
    version: 1,
    agentId: FIRST.flowix_agent_id,
    lastRunAt: 100,
    lastConsultedAt: null,
    runs: 3,
    consultationsReceived: 1,
    tokensIn: 120,
    tokensOut: 30,
    recentThreadIds: ['thr_1'],
    localPreferences: null,
  });
  agentProfileMock.update.mockImplementation(
    async (_cwd: string, _id: string, patch: Record<string, unknown>) =>
      profile({
        flowix_consultable: patch.consultable === true,
        flowix_redaction_level: (patch.redactionLevel as string) || null,
      }),
  );
  agentProfileMock.create.mockImplementation(
    async (_cwd: string, input: { displayName: string; agentType: string }) =>
      profile({
        flowix_agent_id: 'agt_new0000000002',
        flowix_display_name: input.displayName,
        flowix_agent_type: input.agentType,
      }),
  );
  agentProfileMock.delete.mockResolvedValue(undefined);
  host = document.createElement('div');
  document.body.appendChild(host);
  root = createRoot(host);
});

afterEach(async () => {
  await act(async () => root.unmount());
  host.remove();
});

describe('agent profile cards', () => {
  it('loads the card list, shows detail form and local run state', async () => {
    await mount();

    expect(agentProfileMock.list).toHaveBeenCalledWith(CWD);
    expect(host.textContent).toContain('财报解读助手');
    // 详情表单：名称输入框带卡片名。
    const nameInput = host.querySelector<HTMLInputElement>('input:not([type="checkbox"])')!;
    expect(nameInput.value).toBe('财报解读助手');
    // 本地运行状态面板。
    expect(agentProfileMock.getState).toHaveBeenCalledWith(CWD, FIRST.flowix_agent_id);
    expect(host.textContent).toContain('3');
  });

  it('saves privacy boundary fields through update', async () => {
    await mount();

    await act(async () => {
      const toggle = host.querySelector<HTMLInputElement>('input[type="checkbox"]')!;
      toggle.click();
    });
    // selects: [0] agent 类型, [1] 披露等级。
    await act(async () => setValue(selects()[1], 'R2'));
    // textareas: [0] 简介, [1] 专长, [2] 披露范围。
    await act(async () => setValue(textareas()[2], 'finance/财报'));

    await click('保存');

    expect(agentProfileMock.update).toHaveBeenCalledWith(
      CWD,
      FIRST.flowix_agent_id,
      expect.objectContaining({
        displayName: '财报解读助手',
        consultable: false,
        redactionLevel: 'R2',
        disclosureScope: ['finance/财报'],
        model: '',
      }),
    );
  });

  it('creates a new card from the create form', async () => {
    await mount();
    await click('新建身份卡');

    await act(async () => {
      const input = host.querySelector<HTMLInputElement>('input:not([type="checkbox"])')!;
      setValue(input, '电商观察员');
    });
    await act(async () => setValue(selects()[0], 'pi'));
    await click('创建');

    expect(agentProfileMock.create).toHaveBeenCalledWith(
      CWD,
      expect.objectContaining({ displayName: '电商观察员', agentType: 'pi', bio: '', expertise: [] }),
    );
    // 创建后刷新列表并退出新建态。
    expect(agentProfileMock.list.mock.calls.length).toBeGreaterThanOrEqual(2);
    expect(host.textContent).not.toContain('新建个人 Agent');
  });

  it('deletes the selected card after confirmation dialog', async () => {
    await mount();
    await click('删除');

    // 确认对话框（Portal）出现后再确认删除。
    await click('删除身份卡');

    expect(agentProfileMock.delete).toHaveBeenCalledWith(CWD, FIRST.flowix_agent_id);
  });
});
