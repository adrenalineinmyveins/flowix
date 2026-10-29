// AGENT-MARKET M4：订阅 agent 的行内引用。
// Markdown form: `[displayName](flowix://agent?a=<urlenc user/slug>&r=<urlenc relay>)`
// —— 标准链接形态，落盘/对话发送零自定义序列化；NodeView 渲染为行内
// chip，点击复制完整地址（user/slug@relay），供粘贴到对话直接咨询。

import { Node, type JSONContent, type MarkdownToken } from '@tiptap/core';

import { toast } from '@/lib/toast';

export interface AgentReferenceAttrs {
  displayName: string;
  /** `user/slug`（不含 @relay）。 */
  address: string;
  relayUrl: string;
}

export function agentFullAddress(attrs: Pick<AgentReferenceAttrs, 'address' | 'relayUrl'>): string {
  return attrs.relayUrl ? `${attrs.address}@${attrs.relayUrl}` : attrs.address;
}

/** `[name](flowix://agent?a=..&r=..)` 解析。 */
export function parseAgentLinkHref(href: string): AgentReferenceAttrs | null {
  if (!/^flowix:\/\/agent\?/i.test(href)) return null;
  try {
    const url = new URL(href);
    const address = url.searchParams.get('a') ?? '';
    const relayUrl = url.searchParams.get('r') ?? '';
    const displayName = url.searchParams.get('n') ?? '';
    if (!address.includes('/')) return null;
    return { displayName: displayName || address, address, relayUrl };
  } catch {
    return null;
  }
}

export function buildAgentLinkMarkdown(attrs: AgentReferenceAttrs): string {
  const name = encodeURIComponent(attrs.displayName);
  const address = encodeURIComponent(attrs.address);
  const relay = encodeURIComponent(attrs.relayUrl);
  return `[${attrs.displayName.replace(/[[\]]/g, '')}](flowix://agent?n=${name}&a=${address}&r=${relay})`;
}

export const AgentReference = Node.create({
  name: 'agentReference',
  group: 'inline',
  inline: true,
  atom: true,
  selectable: true,
  draggable: true,
  priority: 1000,

  addAttributes() {
    return {
      displayName: { default: '' },
      address: { default: '' },
      relayUrl: { default: '' },
    };
  },

  parseHTML() {
    return [
      {
        tag: 'agent-reference',
        getAttrs: (el: HTMLElement) => ({
          displayName: el.getAttribute('display-name') || el.textContent || '',
          address: el.getAttribute('address') || '',
          relayUrl: el.getAttribute('relay-url') || '',
        }),
      },
    ];
  },

  renderHTML({ node }) {
    const a = node.attrs as AgentReferenceAttrs;
    return [
      'agent-reference',
      { 'display-name': a.displayName, address: a.address, 'relay-url': a.relayUrl },
      a.displayName,
    ];
  },

  markdownTokenizer: {
    name: 'agentReference',
    level: 'inline' as const,
    start(src: string) {
      const hrefIndex = src.indexOf('(flowix://agent?');
      if (hrefIndex < 0) return -1;
      return Math.max(0, src.lastIndexOf('[', hrefIndex));
    },
    tokenize(src: string) {
      if (!src.startsWith('[')) return undefined;
      const close = src.indexOf('](', 1);
      if (close < 0) return undefined;
      const end = src.indexOf(')', close);
      if (end < 0) return undefined;
      const href = src.slice(close + 2, end);
      if (!parseAgentLinkHref(href)) return undefined;
      return { type: 'agentReference', raw: src.slice(0, end + 1), href, text: src.slice(1, close) };
    },
  },

  parseMarkdown(token: MarkdownToken) {
    const attrs = parseAgentLinkHref(String(token.href ?? ''));
    if (!attrs) return { type: 'agentReference', attrs: { displayName: '', address: '', relayUrl: '' } };
    // 链接文本优先于 URL 参数里的名字（用户可改显示名）。
    const text = String(token.text ?? '').trim();
    return { type: 'agentReference', attrs: { ...attrs, displayName: text || attrs.displayName } };
  },

  renderMarkdown(node: JSONContent) {
    const a = (node?.attrs ?? {}) as AgentReferenceAttrs;
    if (!a.address) return a.displayName || '';
    return buildAgentLinkMarkdown(a);
  },

  // 行内 chip：点击复制完整地址（复制即得 consult_remote 可用的 target）。
  addNodeView() {
    return ({ node }) => {
      const a = node.attrs as AgentReferenceAttrs;
      const dom = document.createElement('span');
      dom.className = 'agent-reference-chip';
      dom.contentEditable = 'false';
      Object.assign(dom.style, {
        display: 'inline-flex',
        alignItems: 'center',
        borderRadius: '9999px',
        border: '1px solid var(--border)',
        padding: '0 6px',
        margin: '0 1px',
        fontSize: '0.85em',
        cursor: 'pointer',
        whiteSpace: 'nowrap',
        background: 'color-mix(in oklch, var(--primary) 8%, transparent)',
      } satisfies Partial<CSSStyleDeclaration>);
      dom.textContent = `@${a.displayName || a.address}`;
      dom.title = `${agentFullAddress(a)} — click to copy`;
      dom.addEventListener('click', (event) => {
        event.preventDefault();
        event.stopPropagation();
        void navigator.clipboard
          .writeText(agentFullAddress(a))
          .then(() => toast.success(`Copied: ${agentFullAddress(a)}`))
          .catch(() => undefined);
      });
      return { dom };
    };
  },
});
