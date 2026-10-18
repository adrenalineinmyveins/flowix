import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';

const redactionMock = vi.hoisted(() => ({
  configGet: vi.fn(),
  configSet: vi.fn(),
  l1Preview: vi.fn(),
  preview: vi.fn(),
  confirm: vi.fn(),
  confirmWait: vi.fn(),
  send: vi.fn(),
  outboxList: vi.fn(),
}));
const tagsMock = vi.hoisted(() => ({ getAll: vi.fn() }));
const agentAccessMock = vi.hoisted(() => ({ get: vi.fn(), set: vi.fn() }));

vi.mock('@platform/tauri/client', async (importOriginal) => ({
  ...(await importOriginal<Record<string, unknown>>()),
  redaction: redactionMock,
  tags: tagsMock,
  agentAccess: agentAccessMock,
}));

import type { RedactionConfig, RedactionOutboxRecord, RedactionPreviewReport } from '@/lib/types/redaction';
import { useAgentAccessStore } from '@features/agent/store/agent-access-store';
import { RedactionSendGateDialog } from '@features/agent/components/redaction-send-gate';

const CWD = '/notes/book';
const CONFIG: RedactionConfig = {
  schemaVersion: 1,
  defaultLevel: 'r2',
  levelOverrides: {},
  payloadLimitKb: 64,
  confirmTimeoutSecs: 120,
  strictLuhn: false,
  rewriteAbsoluteTime: true,
  distillAgentType: '',
  relayUrl: '',
};

const REPORT: RedactionPreviewReport = {
  receiptId: 'rct_new',
  finalText: '某电商公司 Q3 复购下滑，联系电话 ⟦PHONE-1⟧。',
  placeholderMap: { '⟦PHONE-1⟧': '13800138000' },
  l1Hits: [{ ruleId: 'phone', count: 1 }],
  attempts: 1,
  usedFallback: false,
  includedPaths: ['finance/财报.md'],
  droppedPaths: ['secret.md'],
  truncated: false,
  targetLevel: 'r2',
  ruleSetVersion: 'rs-1',
};

function draftRecord(overrides: Partial<RedactionOutboxRecord> = {}): RedactionOutboxRecord {
  return {
    receiptId: 'rct_draft',
    status: 'draft',
    createdAt: Date.now(),
    settledAt: null,
    question: '复购为什么下滑？',
    scopeEntryIds: ['nb_1'],
    targetLevel: 'r2',
    ruleSetVersion: 'rs-1',
    payload: REPORT.finalText,
    placeholderMap: REPORT.placeholderMap,
    l1Hits: REPORT.l1Hits,
    includedPaths: REPORT.includedPaths,
    droppedPaths: REPORT.droppedPaths,
    attempts: 1,
    usedFallback: false,
    truncated: false,
    ...overrides,
  };
}

let host: HTMLDivElement;
let root: Root;
let closeSpy: () => void;

async function mount() {
  await act(async () => {
    root.render(
      <RedactionSendGateDialog notebookId="nb_1" notebookPath={CWD} onClose={closeSpy} />,
    );
  });
}

function button(text: string): HTMLButtonElement {
  const element = [...host.querySelectorAll('button')].find(
    (node) => node.textContent?.trim() === text,
  );
  expect(element, `button ${text}`).toBeDefined();
  return element!;
}

async function click(text: string) {
  await act(async () => button(text).click());
}

function textArea(): HTMLTextAreaElement {
  const element = host.querySelector('textarea');
  expect(element).not.toBeNull();
  return element!;
}

async function selectFirstScope() {
  const checkbox = host.querySelector<HTMLInputElement>('input[type="checkbox"]');
  expect(checkbox).not.toBeNull();
  await act(async () => checkbox!.click());
}

async function runPreview() {
  const area = textArea();
  await act(async () => {
    // React 受控组件在 jsdom 下的惯用赋值方式：绕过实例追踪器。
    const setter = Object.getOwnPropertyDescriptor(
      HTMLTextAreaElement.prototype,
      'value',
    )!.set!;
    setter.call(area, '复购为什么下滑？');
    area.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await selectFirstScope();
  await click('生成脱敏预览');
}

beforeEach(async () => {
  vi.clearAllMocks();
  vi.useRealTimers();
  redactionMock.configGet.mockResolvedValue({ ...CONFIG });
  redactionMock.outboxList.mockResolvedValue([]);
  redactionMock.preview.mockResolvedValue({ ...REPORT });
  redactionMock.confirm.mockImplementation(async (_cwd: string, _id: string, approved: boolean) =>
    draftRecord({ status: approved ? 'confirmed' : 'cancelled' }));
  redactionMock.send.mockResolvedValue({ payload: 'envelope-bytes', byteLength: 42 });
  tagsMock.getAll.mockResolvedValue({ tags: [{ id: 't1', name: 'finance/财报' }] });
  agentAccessMock.get.mockResolvedValue({ version: 1, entries: [] });
  agentAccessMock.set.mockResolvedValue(undefined);
  useAgentAccessStore.setState({
    config: {
      version: 1,
      entries: [
        { id: 'nb_1', kind: 'notebook', path: CWD, name: 'Book', enabled: true, missing: false, addedAt: 1, updatedAt: 1 },
        { id: 'tag_1', kind: 'tag', path: '', name: 'finance/财报', tagPath: 'finance/财报', enabled: true, missing: false, addedAt: 1, updatedAt: 1 },
      ],
    },
    isLoading: false,
    hasLoaded: true,
    loadError: null,
  });
  host = document.createElement('div');
  document.body.appendChild(host);
  root = createRoot(host);
  closeSpy = vi.fn();
});

afterEach(async () => {
  await act(async () => root.unmount());
  host.remove();
});

describe('redaction send gate', () => {
  it('runs preview → confirm → send end to end', async () => {
    await mount();
    await runPreview();

    expect(redactionMock.preview).toHaveBeenCalledWith(CWD, '复购为什么下滑？', ['nb_1'], 'r2');
    expect(host.textContent).toContain(REPORT.finalText);
    expect(host.textContent).toContain('⟦PHONE-1⟧');
    expect(host.textContent).toContain('phone × 1');
    expect(host.textContent).toContain('Book');

    await click('确认内容并解锁发送');
    expect(redactionMock.confirm).toHaveBeenCalledWith(CWD, 'rct_new', true);

    await click('发送');
    expect(redactionMock.send).toHaveBeenCalledWith(CWD, 'rct_new');
    expect(host.textContent).toContain('42 字节');
  });

  it('cancels from the gate and falls back to compose', async () => {
    await mount();
    await runPreview();

    await click('取消发送');
    expect(redactionMock.confirm).toHaveBeenCalledWith(CWD, 'rct_new', false);
    expect(host.textContent).toContain('已取消');

    await click('返回修改');
    expect(host.querySelector('textarea')).not.toBeNull();
  });

  it('restores the latest draft receipt on open', async () => {
    redactionMock.outboxList.mockResolvedValue([
      draftRecord({ receiptId: 'rct_old', createdAt: Date.now() - 5_000 }),
      draftRecord(),
    ]);
    await mount();

    expect(host.textContent).toContain(REPORT.finalText);
    expect(host.textContent).toContain('rct_draft');
    expect(host.querySelector('textarea')).toBeNull();
    expect(button('确认内容并解锁发送')).toBeDefined();
  });

  it('auto-settles as cancelled when the confirm window expires', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-10-17T08:00:00Z'));
    redactionMock.outboxList.mockResolvedValue([
      draftRecord({ createdAt: Date.now() - (CONFIG.confirmTimeoutSecs + 1) * 1000 }),
    ]);
    await mount();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(1_100);
    });
    expect(redactionMock.confirm).toHaveBeenCalledWith(CWD, 'rct_draft', false);
    expect(host.textContent).toContain('确认超时');
    vi.useRealTimers();
  });

  it('surfaces a readable error when preview reports scope_empty', async () => {
    redactionMock.preview.mockRejectedValue('scope_empty');
    await mount();
    await runPreview();

    expect(host.textContent).toContain('披露范围为空');
    // 失败后停留在 compose, 可修正后重试。
    expect(host.querySelector('textarea')).not.toBeNull();
  });
});
