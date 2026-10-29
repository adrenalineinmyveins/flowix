// AGENT-MARKET M4：@ 提及的订阅 agent 数据源（与笔记候选混排）。
import { market } from '@platform/tauri/client';
import type { SubscribedCard } from '@platform/tauri/client';
import type { MentionNoteItem } from '@features/editor/extensions/note-mention/note-mention-data';

export interface MentionAgentItem {
  kind: 'agent';
  address: string;
  displayName: string;
  relayUrl: string;
}

export type MentionItem = MentionNoteItem | MentionAgentItem;

export function isMentionAgent(item: MentionItem): item is MentionAgentItem {
  return (item as MentionAgentItem).kind === 'agent';
}

let cachedAgents: MentionAgentItem[] | null = null;

async function fetchMentionAgents(): Promise<MentionAgentItem[]> {
  const subscriptions: SubscribedCard[] = await market.subscriptions();
  return subscriptions.map((item) => ({
    kind: 'agent' as const,
    address: item.address,
    displayName: item.displayName,
    relayUrl: item.relayUrl,
  }));
}

function loadMentionAgents(): Promise<MentionAgentItem[]> {
  if (cachedAgents) return Promise.resolve(cachedAgents);
  return fetchMentionAgents()
    .then((items) => {
      cachedAgents = items;
      return items;
    })
    .catch((err) => {
      console.warn('[agent-mention] load failed:', err);
      return [] as MentionAgentItem[];
    });
}

export function invalidateMentionAgents(): void {
  cachedAgents = null;
}

export function queryMentionAgents(query: string): Promise<MentionAgentItem[]> {
  const normalized = query.trim().toLowerCase();
  return loadMentionAgents().then((items) =>
    normalized
      ? items.filter(
          (item) =>
            item.displayName.toLowerCase().includes(normalized) ||
            item.address.toLowerCase().includes(normalized),
        )
      : items,
  );
}
