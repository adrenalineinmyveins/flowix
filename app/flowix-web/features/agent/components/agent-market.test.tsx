import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';

const redactionMock = vi.hoisted(() => ({
  accountStatus: vi.fn(),
  directorySearch: vi.fn(),
  accountRegister: vi.fn(),
  accountForget: vi.fn(),
  relayUsage: vi.fn(),
}));

const marketMock = vi.hoisted(() => ({
  subscriptions: vi.fn(),
  subscribe: vi.fn(),
  unsubscribe: vi.fn(),
  refreshSubscription: vi.fn(),
}));

vi.mock('@platform/tauri/client', async (importOriginal) => ({
  ...(await importOriginal<Record<string, unknown>>()),
  redaction: redactionMock,
  market: marketMock,
}));

import { AgentMarket } from '@features/agent/components/agent-market';

const CWD = '/notes/book';

const CARD = {
  address: 'alice/caibao',
  username: 'alice',
  agentSlug: 'caibao',
  displayName: '财报解读助手',
  visibility: 'public',
  card: { minRedactionLevel: 'R2', autoAnswer: 'off', visibility: 'public' },
  updatedAt: 1793200000,
};

let host: HTMLDivElement;
let root: Root;

async function mount() {
  await act(async () => {
    root.render(<AgentMarket notebookPath={CWD} />);
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  useAccount(null);
  useCards([]);
  marketMock.subscriptions.mockResolvedValue([]);
  host = document.createElement('div');
  document.body.appendChild(host);
  root = createRoot(host);
});

afterEach(async () => {
  await act(async () => root.unmount());
  host.remove();
});

function useAccount(account: unknown) {
  redactionMock.accountStatus.mockResolvedValue(account);
}

function useCards(cards: unknown[]) {
  redactionMock.directorySearch.mockResolvedValue(cards);
}

describe('agent market (AGENT-MARKET M2)', () => {
  it('shows the sign-in guide when no relay account', async () => {
    await mount();
    expect(host.textContent).toContain('身份卡市场');
    expect(host.textContent).toContain('中继账号');
    expect(redactionMock.directorySearch).not.toHaveBeenCalled();
  });

  it('browses the first page on mount once signed in', async () => {
    useAccount({ username: 'alice', relayUrl: 'http://relay.test' });
    await mount();
    expect(redactionMock.directorySearch).toHaveBeenCalledWith('', undefined);
  });

  it('renders result rows and expands card detail on click', async () => {
    useAccount({ username: 'alice', relayUrl: 'http://relay.test' });
    useCards([CARD]);
    await mount();

    expect(host.textContent).toContain('财报解读助手');
    expect(host.textContent).toContain('alice/caibao');

    await act(async () => {
      const row = [...host.querySelectorAll('button')]
        .find((node) => node.textContent?.includes('财报解读助手'))!;
      row.click();
    });
    expect(host.textContent).toContain('完整地址');
    expect(host.textContent).toContain('alice/caibao@http://relay.test');
    expect(host.textContent).toContain('R2');
  });

  it('surfaces search errors with relay text', async () => {
    useAccount({ username: 'alice', relayUrl: 'http://relay.test' });
    redactionMock.directorySearch.mockRejectedValue(new Error('CARD_SEARCH_REJECTED:429'));
    await mount();
    expect(host.textContent).toContain('CARD_SEARCH_REJECTED:429');
  });

  it('shows the empty hint when the first page has no cards', async () => {
    useAccount({ username: 'alice', relayUrl: 'http://relay.test' });
    await mount();
    expect(host.textContent).toContain('没有匹配的公开身份卡');
  });

  it('subscribes via the card row and lists it in my subscriptions (M3)', async () => {
    useAccount({ username: 'alice', relayUrl: 'http://relay.test' });
    useCards([CARD]);
    marketMock.subscribe.mockImplementation(async (address: string) => {
      const snapshot = {
        address,
        relayUrl: 'http://relay.test',
        displayName: CARD.displayName,
        specialtyTags: [],
        minRedactionLevel: 'R2',
        autoAnswer: 'off',
        cardPublicKey: '',
        subscribedAt: 1,
        refreshedAt: 1,
      };
      // 模拟落库后的列表刷新。
      marketMock.subscriptions.mockResolvedValue([snapshot]);
      return snapshot;
    });
    await mount();
    expect(host.textContent).toContain('暂无订阅');

    await act(async () => {
      const button = [...host.querySelectorAll('button')]
        .find((node) => node.textContent?.trim() === '订阅')!;
      button.click();
    });
    expect(marketMock.subscribe).toHaveBeenCalledWith(CARD.address, 'http://relay.test');
    // 订阅后列表刷新：我的订阅分区显示完整地址。
    expect(host.textContent).toContain('alice/caibao@http://relay.test');
  });

  it('unsubscribes from my subscriptions section', async () => {
    useAccount({ username: 'alice', relayUrl: 'http://relay.test' });
    marketMock.subscriptions.mockResolvedValue([
      {
        address: 'alice/caibao',
        relayUrl: 'http://relay.test',
        displayName: '财报解读助手',
        specialtyTags: [],
        minRedactionLevel: 'R2',
        autoAnswer: 'off',
        cardPublicKey: '',
        subscribedAt: 1,
        refreshedAt: 1,
      },
    ]);
    await mount();
    expect(host.textContent).toContain('我的订阅');

    await act(async () => {
      const button = [...host.querySelectorAll('button')]
        .find((node) => node.textContent?.trim() === '退订')!;
      button.click();
    });
    expect(marketMock.unsubscribe).toHaveBeenCalledWith('alice/caibao');
  });

  it('refreshes a subscription snapshot from the refresh button', async () => {
    useAccount({ username: 'alice', relayUrl: 'http://relay.test' });
    marketMock.subscriptions.mockResolvedValue([
      {
        address: 'alice/caibao',
        relayUrl: 'http://relay.test',
        displayName: '财报解读助手',
        specialtyTags: [],
        minRedactionLevel: 'R2',
        autoAnswer: 'off',
        cardPublicKey: '',
        subscribedAt: 1,
        refreshedAt: 1,
      },
    ]);
    marketMock.refreshSubscription.mockResolvedValue({
      address: 'alice/caibao',
      relayUrl: 'http://relay.test',
      displayName: '财报解读助手（更新）',
      specialtyTags: [],
      minRedactionLevel: 'R2',
      autoAnswer: 'off',
      cardPublicKey: '',
      subscribedAt: 1,
      refreshedAt: 2,
    });
    await mount();

    await act(async () => {
      const button = [...host.querySelectorAll('button')]
        .find((node) => node.title === '刷新快照')!;
      button.click();
    });
    expect(marketMock.refreshSubscription).toHaveBeenCalledWith('alice/caibao');
  });
});
