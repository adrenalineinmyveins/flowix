/**
 * AGENT-MARKET：身份卡市场 IPC 客户端（M1 发布管理 / M3 订阅）。
 */
import { invoke } from '@tauri-apps/api/core';
import type { DirectoryCardDraft } from '@/lib/types/redaction';

/** 我的卡发布态聚合（契约见 docs/AGENT-MARKET.md §4）。 */
export interface MarketMyCard {
  agentId: string;
  displayName: string;
  expertise: string[];
  /** none | draft | confirmed | published */
  publishState: string;
  publishedSlug: string | null;
  visibility: string | null;
  drifted: boolean;
}

/** 订阅快照（D-5：本地关注列表，跨笔记本可 consult）。 */
export interface SubscribedCard {
  /** user/slug（不含 @relay）。 */
  address: string;
  relayUrl: string;
  displayName: string;
  specialtyTags: string[];
  minRedactionLevel: string;
  autoAnswer: string;
  cardPublicKey: string;
  subscribedAt: number;
  refreshedAt: number;
}

export const market = {
  /** 本地身份卡 × 发布档聚合（纯本地读，relay 不可达不影响）。 */
  myCards: (cwd: string) =>
    invoke<MarketMyCard[]>('market_my_cards', { cwd }),

  /** 下架：private confirmed 克隆档 → 发布覆盖中继卡（失败留 confirmed 可重试）。 */
  unpublish: (cwd: string, agentId: string): Promise<DirectoryCardDraft> =>
    invoke<DirectoryCardDraft>('market_unpublish', { cwd, agentId }),

  /** 订阅：拉单卡详情存快照（幂等覆盖）。relayUrl 缺省用账号中继。 */
  subscribe: (address: string, relayUrl?: string): Promise<SubscribedCard> =>
    invoke<SubscribedCard>('market_subscribe', { address, relayUrl }),

  /** 退订：删除快照条目。 */
  unsubscribe: (address: string): Promise<void> =>
    invoke<void>('market_unsubscribe', { address }),

  /** 我的订阅快照列表。 */
  subscriptions: (): Promise<SubscribedCard[]> =>
    invoke<SubscribedCard[]>('market_subscriptions'),

  /** 刷新订阅快照（远端 private/不存在 → SUBSCRIPTION 类错误；网络失败快照不动）。 */
  refreshSubscription: (address: string): Promise<SubscribedCard> =>
    invoke<SubscribedCard>('market_refresh_subscription', { address }),
};
