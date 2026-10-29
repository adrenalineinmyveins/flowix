import { afterEach, describe, expect, it, vi } from 'vitest';

import {
  invalidateMentionAgents,
  isMentionAgent,
  queryMentionAgents,
} from './agent-mention-data';
import type { SubscribedCard } from '@platform/tauri/client';

const mocks = vi.hoisted(() => ({
  subscriptions: vi.fn(),
}));

vi.mock('@platform/tauri/client', () => ({
  market: { subscriptions: mocks.subscriptions },
}));

function card(address: string, displayName: string): SubscribedCard {
  return {
    address,
    relayUrl: 'https://relay.example.com',
    displayName,
    specialtyTags: [],
    minRedactionLevel: 'green',
    autoAnswer: 'on',
    cardPublicKey: 'pk',
    subscribedAt: 1,
    refreshedAt: 1,
  };
}

afterEach(() => {
  invalidateMentionAgents();
  mocks.subscriptions.mockReset();
});

describe('queryMentionAgents', () => {
  it('maps subscriptions to mention items and caches between queries', async () => {
    mocks.subscriptions.mockResolvedValue([card('alice/agent-1', 'Alice 助手')]);
    const first = await queryMentionAgents('');
    expect(first).toEqual([
      {
        kind: 'agent',
        address: 'alice/agent-1',
        displayName: 'Alice 助手',
        relayUrl: 'https://relay.example.com',
      },
    ]);
    await queryMentionAgents('');
    expect(mocks.subscriptions).toHaveBeenCalledTimes(1);
  });

  it('filters by display name and address, case-insensitively', async () => {
    mocks.subscriptions.mockResolvedValue([
      card('alice/agent-1', 'Alice 助手'),
      card('bob/reviewer', 'Reviewer'),
    ]);
    expect((await queryMentionAgents('alice')).map((item) => item.address)).toEqual(['alice/agent-1']);
    expect((await queryMentionAgents('REVIEWER')).map((item) => item.address)).toEqual(['bob/reviewer']);
    expect(await queryMentionAgents('nobody')).toEqual([]);
  });

  it('surfaces fresh data after invalidateMentionAgents', async () => {
    mocks.subscriptions.mockResolvedValueOnce([card('alice/agent-1', 'Alice')]);
    await queryMentionAgents('');
    mocks.subscriptions.mockResolvedValueOnce([card('bob/reviewer', 'Reviewer')]);
    invalidateMentionAgents();
    const items = await queryMentionAgents('');
    expect(items.map((item) => item.address)).toEqual(['bob/reviewer']);
  });

  it('returns an empty list when the market call fails', async () => {
    mocks.subscriptions.mockRejectedValue(new Error('offline'));
    await expect(queryMentionAgents('')).resolves.toEqual([]);
  });
});

describe('isMentionAgent', () => {
  it('discriminates the union by kind', () => {
    expect(isMentionAgent({ kind: 'agent', address: 'a/b', displayName: 'A', relayUrl: 'r' })).toBe(true);
    expect(isMentionAgent({
      relativePath: 'a.md', filename: 'a.md', title: 'A', updatedAt: 0,
      notebookId: 'n', notebookName: 'N', notebookPath: '/n', originalPath: null,
    })).toBe(false);
  });
});
