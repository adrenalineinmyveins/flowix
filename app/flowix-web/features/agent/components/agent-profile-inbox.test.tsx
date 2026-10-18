import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';

const redactionMock = vi.hoisted(() => ({
  inboxList: vi.fn(),
  inboxPoll: vi.fn(),
  inboxRegister: vi.fn(),
  inboxSetStatus: vi.fn(),
  configGet: vi.fn(),
}));

const agentProfileMock = vi.hoisted(() => ({
  list: vi.fn(),
}));

vi.mock('@platform/tauri/client', async (importOriginal) => ({
  ...(await importOriginal<Record<string, unknown>>()),
  redaction: redactionMock,
  agentProfile: agentProfileMock,
}));

import type { InboundRecord, RedactionConfig } from '@/lib/types/redaction';
import { useNoteStore } from '@features/memo/store/note-store';
import { AgentProfileInbox } from '@features/agent/components/agent-profile-inbox';

const CWD = '/notes/book';

function record(overrides: Partial<InboundRecord> = {}): InboundRecord {
  return {
    receiptId: 'rct_in1',
    queueId: 'q_1',
    fromAgentId: 'agt_a',
    fromDisplayName: '财报验证助手',
    toAgentId: 'agt_b',
    payloadKind: 'consultation',
    payloadText: '远端咨询：Q3 复购怎么看？',
    rejectCode: null,
    inReplyTo: null,
    disclosure: {
      redactionLevel: 'R2',
      maxHops: 2,
      hopsRemaining: 1,
      expiresAt: Math.floor(Date.now() / 1000) + 3600,
      allowedScopes: ['tag:finance/财报'],
    },
    receivedAt: 1,
    status: 'pending',
    ...overrides,
  };
}

const CONFIG: RedactionConfig = {
  schemaVersion: 0,
  defaultLevel: 'r2',
  levelOverrides: {},
  payloadLimitKb: 64,
  confirmTimeoutSecs: 120,
  strictLuhn: false,
  rewriteAbsoluteTime: true,
  distillAgentType: '',
  relayUrl: 'http://127.0.0.1:8788',
  deviceToken: 'dev_x',
  registeredAgents: ['agt_b'],
};

let host: HTMLDivElement;
let root: Root;

async function mount() {
  await act(async () => {
    root.render(<AgentProfileInbox notebookPath={CWD} />);
  });
}

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

beforeEach(() => {
  vi.clearAllMocks();
  redactionMock.inboxList.mockResolvedValue([record()]);
  redactionMock.configGet.mockResolvedValue(CONFIG);
  redactionMock.inboxPoll.mockResolvedValue({ stored: 1, duplicates: 0, rejected: 0 });
  redactionMock.inboxSetStatus.mockResolvedValue(undefined);
  agentProfileMock.list.mockResolvedValue([
    { flowix_agent_id: 'agt_b', flowix_display_name: '乙' },
  ]);
  useNoteStore.setState({ agentProfileViewTarget: { inbox: true } });
  host = document.createElement('div');
  document.body.appendChild(host);
  root = createRoot(host);
});

afterEach(async () => {
  await act(async () => root.unmount());
  host.remove();
});

describe('agent profile inbox', () => {
  it('loads records and shows sender with disclosure summary', async () => {
    await mount();

    expect(redactionMock.inboxList).toHaveBeenCalledWith(CWD);
    expect(host.textContent).toContain('财报验证助手');
    expect(host.textContent).toContain('远端咨询：Q3 复购怎么看？');
    expect(host.textContent).toContain('R2');
  });

  it('polls the relay and refreshes the list', async () => {
    await mount();
    await click('拉取');

    expect(redactionMock.inboxPoll).toHaveBeenCalledWith(CWD);
    expect(redactionMock.inboxList).toHaveBeenCalledTimes(2);
  });

  it('accepts a pending consult and refreshes', async () => {
    await mount();
    await click('接受');

    expect(redactionMock.inboxSetStatus).toHaveBeenCalledWith(CWD, 'rct_in1', true);
    expect(redactionMock.inboxList).toHaveBeenCalledTimes(2);
  });

  it('rejects a pending consult', async () => {
    await mount();
    await click('拒绝');

    expect(redactionMock.inboxSetStatus).toHaveBeenCalledWith(CWD, 'rct_in1', false);
  });

  it('hides actions for settled or expired records', async () => {
    const expired = Math.floor(Date.now() / 1000) - 10;
    redactionMock.inboxList.mockResolvedValue([
      record({ receiptId: 'rct_done', status: 'accepted' }),
      record({ receiptId: 'rct_old', disclosure: { ...record().disclosure, expiresAt: expired } }),
    ]);
    await mount();

    expect(host.textContent).toContain('已接受');
    expect(host.textContent).toContain('已过期');
    const acceptButtons = [...document.body.querySelectorAll('button')]
      .filter((node) => node.textContent?.trim() === '接受');
    expect(acceptButtons).toHaveLength(0);
  });

  it('registers the inbox channel with local agent ids', async () => {
    redactionMock.configGet.mockResolvedValue({ ...CONFIG, registeredAgents: [] });
    redactionMock.inboxRegister.mockResolvedValue({ ...CONFIG, registeredAgents: ['agt_b'] });
    await mount();
    await click('注册收件通道');

    expect(agentProfileMock.list).toHaveBeenCalledWith(CWD);
    expect(redactionMock.inboxRegister).toHaveBeenCalledWith(['agt_b']);
  });
});
