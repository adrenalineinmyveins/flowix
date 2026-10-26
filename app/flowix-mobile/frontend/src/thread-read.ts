// 会话未读状态（P1b）：本地视图，不上云。
// threadId → 最近一次已读时间戳；列表用 updatedAt > 已读时间判未读。

const KEY = 'flowix.chat.lastRead';

type ReadMap = Record<string, number>;

function readMap(): ReadMap {
  try {
    return JSON.parse(localStorage.getItem(KEY) || '{}');
  } catch {
    return {};
  }
}

function writeMap(map: ReadMap): void {
  try {
    localStorage.setItem(KEY, JSON.stringify(map));
  } catch {
    // 隐私模式/配额满：静默（未读点失效，不影响功能）
  }
}

export function isThreadUnread(updatedAt: number, messageCount: number, threadId: string): boolean {
  if (messageCount === 0) return false;
  return updatedAt > (readMap()[threadId] ?? 0);
}

export function markThreadRead(threadId: string): void {
  const map = readMap();
  map[threadId] = Date.now();
  writeMap(map);
}
