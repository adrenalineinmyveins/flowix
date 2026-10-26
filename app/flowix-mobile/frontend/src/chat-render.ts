// 移动端对话消息 Markdown 渲染管线（P0 渲染层）。
// 配置从桌面 agent-thread-card-markdown.ts 移植：gfm+breaks、原生 HTML
// 全转义（只放行 <a>，协议白名单）、图片 href 白名单、CJK 加粗边界修正。
// 差异：数学公式/KaTeX 与 Shiki 高亮暂不移植（P2b 按需），代码块输出
// 带 data-lang 的结构由 CSS/事件委托增强（语言标签 + 复制）。
import { Marked } from 'marked';

function escapeHtml(value: string): string {
  return value
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');
}

function escapeHtmlAttr(value: string): string {
  return escapeHtml(value)
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
}

// 链接协议白名单（桌面 sanitizeLinkHref 的移动端简化版）：
// 只放行 http(s) 与相对路径，拒绝 javascript: 等可执行 scheme。
function sanitizeHref(rawHref: string | null): string | null {
  const href = (rawHref ?? '').trim();
  if (!href) return null;
  if (/^https?:\/\//i.test(href) || href.startsWith('/') || href.startsWith('#')) {
    return href;
  }
  if (/^[a-z][a-z0-9+.-]*:/i.test(href)) {
    return /^https?:/i.test(href) ? href : null;
  }
  return href;
}

function renderSafeAnchor(rawTag: string): string | null {
  const trimmed = rawTag.trim();
  const match = /^<a\b([^>]*)>$/i.exec(trimmed);
  if (match) {
    const hrefMatch = /\bhref\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))/i.exec(match[1]);
    const rawHref = hrefMatch?.[1] ?? hrefMatch?.[2] ?? hrefMatch?.[3];
    if (!rawHref) return null;
    // 实体解码后再过白名单，防 javascript&#x3A; 绕过（同桌面）
    const template = document.createElement('template');
    template.innerHTML = trimmed;
    const href = template.content.firstElementChild?.getAttribute('href');
    const safeHref = sanitizeHref(href);
    if (!safeHref) return null;
    return `<a href="${escapeHtmlAttr(safeHref)}" target="_blank" rel="noopener noreferrer">`;
  }
  if (/^<\/a>$/i.test(trimmed)) return '</a>';
  return null;
}

function safeImageHref(rawHref: string): string | null {
  const href = rawHref.trim();
  if (!href || /[\u0000-\u001f\u007f]/u.test(href)) return null;
  if (/^data:image\/(?:png|jpeg|gif|webp|avif);base64,/i.test(href)) return href;
  if (/^https?:\/\//i.test(href) || href.startsWith('//')) return href;
  if (/^[a-z][a-z0-9+.-]*:/i.test(href)) return sanitizeHref(href);
  return href;
}

// CommonMark 对「闭合定界符后接 CJK」不加粗，中文输出常见 **加粗**中文
// 场景——与桌面 agentCjkStrong 相同的边界修正。
const CJK_STRONG_BOUNDARY_RE =
  /^\*\*(?!\*)((?:(?!\*\*)[\s\S])+?)\*\*(?=\p{Script=Han})/u;

const chatMarked = new Marked({
  async: false,
  gfm: true,
  breaks: true,
  // 消息内容含任意 LLM/用户文本：原生 HTML 一律转义为文本，
  // 只放行 <a>（协议白名单），图片走 href 白名单。
  renderer: {
    html({ text }: { text: string }) {
      const safeAnchor = renderSafeAnchor(text);
      if (safeAnchor) return safeAnchor;
      return escapeHtml(text);
    },
    image({ href, title, text }: { href: string; title: string | null; text: string }) {
      const safeHref = safeImageHref(href);
      if (!safeHref) return escapeHtml(text || href);
      const titleAttribute = title ? ` title="${escapeHtmlAttr(title)}"` : '';
      return `<img src="${escapeHtmlAttr(safeHref)}" alt="${escapeHtmlAttr(text)}"${titleAttribute} loading="lazy" decoding="async" referrerpolicy="no-referrer">`;
    },
    code({ text, lang }: { text: string; lang?: string | null }) {
      const language = (lang ?? '').trim().split(/\s+/)[0] || '';
      return `<pre class="chat-code" data-lang="${escapeHtmlAttr(language)}"><code>${escapeHtml(text)}</code></pre>`;
    },
  },
});

chatMarked.use({
  extensions: [
    {
      name: 'agentCjkStrong',
      level: 'inline' as const,
      start(src: string) {
        const index = src.indexOf('**');
        return index >= 0 ? index : undefined;
      },
      tokenizer(src: string) {
        const match = CJK_STRONG_BOUNDARY_RE.exec(src);
        if (!match || /^\s|\s$/.test(match[1]) || !/\p{P}$/u.test(match[1])) {
          return;
        }
        return {
          type: 'agentCjkStrong',
          raw: match[0],
          tokens: this.lexer.inlineTokens(match[1]),
        };
      },
      renderer(token: { tokens?: unknown[] }) {
        return `<strong>${this.parser.parseInline(token.tokens ?? [])}</strong>`;
      },
      childTokens: ['tokens'],
    },
  ],
});

export function renderChatMarkdown(content: string): string {
  if (!content || !content.trim()) return '';
  return chatMarked.parse(content) as string;
}

// 工具调用行摘要：命令型 toolInput（cmd/args/command 等）压成单行预览，
// 其余截断 JSON——桌面 parseAgentCommandInput 的简化版。
export function toolCallSummary(
  toolName: string | null | undefined,
  toolInput: string | null | undefined,
): string {
  if (toolInput) {
    try {
      const parsed = JSON.parse(toolInput) as Record<string, unknown>;
      const command =
        (typeof parsed.command === 'string' && parsed.command) ||
        (typeof parsed.cmd === 'string' && parsed.cmd) ||
        (Array.isArray(parsed.cmd) && parsed.cmd.join(' ')) ||
        (Array.isArray(parsed.args) &&
          typeof parsed.cmd === 'string' &&
          [parsed.cmd, ...parsed.args].join(' ')) ||
        null;
      if (command) return command.slice(0, 120);
      const keys = Object.keys(parsed);
      if (keys.length) {
        const first = parsed[keys[0]];
        if (typeof first === 'string' && first) return first.slice(0, 120);
      }
    } catch {
      // 非 JSON 原样截断
    }
    return toolInput.replace(/\s+/g, ' ').slice(0, 120);
  }
  return toolName || '';
}
