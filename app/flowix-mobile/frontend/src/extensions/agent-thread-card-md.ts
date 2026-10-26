// 桌面端 thread card 存储格式（app/flowix-web/features/agent/thread-card）──
// markdown 中的 HTML 注释：<!-- flowix:agent-thread-card {"version":1,...} -->
// 解析/序列化纯函数从桌面 agent-thread-card-markdown.ts 移植（去掉桌面专属依赖），
// 保证跨端互认。本文件保持零依赖，方便独立验证往返。

export const FLOWIX_AGENT_THREAD_CARD_METADATA_VERSION = 1;
export const FLOWIX_AGENT_THREAD_CARD_MARKER = 'flowix:agent-thread-card';

interface FlowixAgentThreadCardMetadata {
  version: number;
  instanceId: string;
  threadId: string;
  agentType: string;
  collapsed: boolean;
  fullscreen: boolean;
  inputDraft: string;
  inputImages: unknown[];
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return !!value && typeof value === 'object' && !Array.isArray(value);
}

export function parseFlowixAgentThreadCardComment(
  source: string,
): { raw: string; metadata: Record<string, unknown> } | null {
  const match = new RegExp(
    `^<!--[ \\t]*${FLOWIX_AGENT_THREAD_CARD_MARKER}[ \\t]+(\\{[^\\r\\n]*\\})[ \\t]*-->(?:\\r?\\n|$)`,
  ).exec(source);
  if (!match) return null;
  try {
    const metadata = JSON.parse(match[1]) as unknown;
    if (!isRecord(metadata)) return null;
    if (metadata.version !== FLOWIX_AGENT_THREAD_CARD_METADATA_VERSION) return null;
    return { raw: match[0], metadata };
  } catch {
    return null;
  }
}

// 旧版 `::agent-thread-card{...}` 语法的属性解析（桌面历史文档兼容）
function unescapeAgentThreadCardAttr(value: string): string {
  return value.replace(/\\"/g, '"').replace(/\\\\/g, '\\');
}

function parseAgentThreadCardAttrs(rawAttrs: string): Record<string, string> {
  const attrs: Record<string, string> = {};
  const attrRe = /(\w+)="((?:\\"|\\\\|[^"])*)"/g;
  let match: RegExpExecArray | null;
  while ((match = attrRe.exec(rawAttrs))) {
    attrs[match[1]] = unescapeAgentThreadCardAttr(match[2]);
  }
  return attrs;
}

function decodeAgentThreadCardInputDraft(value: string | null | undefined): string {
  if (!value) return '';
  try {
    return decodeURIComponent(value);
  } catch {
    return value;
  }
}

function parseAgentThreadCardNodeAttrs(
  values: Record<string, unknown>,
  inputDraftEncoded: boolean,
): Record<string, unknown> {
  const stringValue = (value: unknown): string =>
    typeof value === 'string' ? value : '';
  const inputDraftValue = stringValue(values.inputDraft);
  return {
    threadId: stringValue(values.threadId) || null,
    instanceId: stringValue(values.instanceId) || null,
    title: stringValue(values.title),
    typeKey: stringValue(values.agentType) || 'mobile',
    collapsed: values.collapsed === true || values.collapsed === 'true',
    fullscreen: values.fullscreen === true || values.fullscreen === 'true',
    inputDraft: inputDraftValue
      ? inputDraftEncoded
        ? decodeAgentThreadCardInputDraft(inputDraftValue)
        : inputDraftValue
      : null,
  };
}

export function parseAgentThreadCardMarkdown(token: unknown) {
  const tokenRecord = isRecord(token) ? token : {};
  const rawAttrs = typeof tokenRecord.attrs === 'string' ? tokenRecord.attrs : '';
  const comment = parseFlowixAgentThreadCardComment(
    typeof tokenRecord.metadata === 'string'
      ? tokenRecord.metadata
      : rawAttrs || (typeof tokenRecord.raw === 'string' ? tokenRecord.raw : ''),
  );
  const metadata = isRecord(tokenRecord.metadata) ? tokenRecord.metadata : comment?.metadata;
  if (metadata) {
    return { type: 'agentThreadCard', attrs: parseAgentThreadCardNodeAttrs(metadata, false) };
  }
  return { type: 'agentThreadCard', attrs: parseAgentThreadCardNodeAttrs(parseAgentThreadCardAttrs(rawAttrs), true) };
}

export function renderAgentThreadCardMarkdown(node: { attrs?: Record<string, unknown> }): string {
  const stringValue = (value: unknown): string =>
    typeof value === 'string' ? value : '';
  const metadata: FlowixAgentThreadCardMetadata = {
    version: FLOWIX_AGENT_THREAD_CARD_METADATA_VERSION,
    instanceId: stringValue(node.attrs?.instanceId),
    threadId: stringValue(node.attrs?.threadId),
    agentType: stringValue(node.attrs?.typeKey) || 'mobile',
    collapsed: !!node.attrs?.collapsed,
    fullscreen: !!node.attrs?.fullscreen,
    inputDraft: stringValue(node.attrs?.inputDraft),
    inputImages: [],
  };
  return `<!-- ${FLOWIX_AGENT_THREAD_CARD_MARKER} ${JSON.stringify(metadata)} -->\n`;
}
