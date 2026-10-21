import { Node, mergeAttributes, type NodeViewProps } from '@tiptap/core';
import { api } from '../api';

// 与桌面端 attachment-link 同款语法：`![alt](asset://localhost/<encoded 绝对路径>)`。
// storageKey 为设备本地绝对路径；跨设备打开时按文件名回落到本笔记本 attachments/
// （attachment_read 命令实现），保证桌面/移动互读。

const MARKDOWN_IMAGE_RE = /^!\[([^\]]*)\]\(([^)\n]+)\)/;
const ASSET_URL_RE = /^(asset:\/\/|https?:\/\/asset\.localhost\/)/i;

export function decodeStorageKey(src: string): string | null {
  if (!ASSET_URL_RE.test(src)) return null;
  try {
    const encoded = src
      .replace('asset://localhost/', '')
      .replace('http://asset.localhost/', '')
      .replace('https://asset.localhost/', '')
      .replace('asset://', '');
    return decodeURIComponent(encoded);
  } catch {
    return null;
  }
}

export function assetMarkdownUrl(storageKey: string): string {
  const encoded = encodeURIComponent(storageKey).replace(/\(/g, '%28').replace(/\)/g, '%29');
  return `asset://localhost/${encoded}`;
}

interface MobileImageOptions {
  notebookId: string;
}

class MobileImageView {
  dom: HTMLElement;
  contentDOM: HTMLElement | null = null;
  private img: HTMLImageElement;
  private node: NodeViewProps['node'];
  private appliedKey = '';

  constructor(props: NodeViewProps, private options: MobileImageOptions) {
    this.node = props.node;
    this.img = document.createElement('img');
    this.img.loading = 'lazy';
    this.img.decoding = 'async';
    this.dom = document.createElement('div');
    this.dom.className = 'mobile-image-attachment';
    this.dom.contentEditable = 'false';
    this.dom.appendChild(this.img);
    this.applyAttrs(props.node);
  }

  update(node: NodeViewProps['node']): boolean {
    if (node.type !== this.node.type) return false;
    this.node = node;
    this.applyAttrs(node);
    return true;
  }

  destroy() {
    this.appliedKey = '';
  }

  private applyAttrs(node: NodeViewProps['node']) {
    const { src, alt, title, storageMode, storageKey } = node.attrs;
    if (typeof alt === 'string' && alt) this.img.alt = alt;
    if (typeof title === 'string' && title) this.img.title = title;
    const key = `${storageMode ?? ''}|${storageKey ?? ''}|${src ?? ''}`;
    if (key === this.appliedKey) return;
    this.appliedKey = key;
    if (storageMode === 'attachment' && typeof storageKey === 'string' && storageKey) {
      this.img.removeAttribute('src');
      this.dom.setAttribute('data-loading', 'true');
      this.dom.removeAttribute('data-error');
      api
        .attachmentRead(this.options.notebookId, storageKey)
        .then((dataUrl) => {
          if (key !== this.appliedKey) return;
          this.dom.removeAttribute('data-loading');
          this.img.src = dataUrl;
        })
        .catch(() => {
          if (key !== this.appliedKey) return;
          this.dom.removeAttribute('data-loading');
          this.dom.setAttribute('data-error', 'true');
        });
    } else if (typeof src === 'string' && src) {
      this.img.src = src;
    }
  }
}

declare module '@tiptap/core' {
  interface Commands<ReturnType> {
    mobileImage: {
      insertAttachmentImage: (attrs: { storageKey: string; alt?: string }) => ReturnType;
    };
  }
}

export const MobileImage = Node.create<MobileImageOptions>({
  name: 'image',
  inline: false,
  group: 'block',
  draggable: true,

  addOptions() {
    return { notebookId: '' };
  },

  addAttributes() {
    return {
      src: { default: null },
      alt: { default: null },
      title: { default: null },
      storageMode: { default: null },
      storageKey: { default: null },
    };
  },

  parseHTML() {
    return [{ tag: 'img[src]' }];
  },

  renderHTML({ HTMLAttributes }) {
    return ['img', mergeAttributes({ loading: 'lazy' }, HTMLAttributes)];
  },

  addNodeView() {
    return (props: NodeViewProps) => new MobileImageView(props, this.options);
  },

  addCommands() {
    return {
      insertAttachmentImage:
        ({ storageKey, alt }) =>
        ({ commands }) =>
          commands.insertContent({
            type: this.name,
            attrs: {
              src: assetMarkdownUrl(storageKey),
              alt: alt ?? null,
              title: null,
              storageMode: 'attachment',
              storageKey,
            },
          }),
    };
  },

  markdownTokenizer: {
    name: 'image',
    level: 'block' as const,
    start(src: string) {
      return src.indexOf('![');
    },
    tokenize(src: string) {
      const match = MARKDOWN_IMAGE_RE.exec(src);
      if (!match) return undefined;
      return {
        type: 'image',
        raw: match[0],
        href: match[2],
        text: match[1],
        title: null,
      };
    },
  },

  parseMarkdown(token: { href?: unknown; text?: unknown; raw?: unknown; title?: unknown }, helpers: { createNode: (type: string, attrs: Record<string, unknown>) => unknown }) {
    const href = typeof token.href === 'string' ? token.href : '';
    if (!href) {
      return { type: 'text', text: token.raw || '' };
    }
    const storageKey = decodeStorageKey(href);
    return helpers.createNode('image', {
      src: href,
      alt: token.text || null,
      title: token.title || null,
      storageMode: storageKey ? 'attachment' : null,
      storageKey,
    });
  },

  renderMarkdown(node: { attrs?: Record<string, unknown> | null }) {
    const { alt, storageMode, storageKey, src } = node.attrs ?? {};
    const imageSrc =
      storageMode === 'attachment' && typeof storageKey === 'string' && storageKey
        ? assetMarkdownUrl(storageKey)
        : typeof src === 'string'
          ? src
          : '';
    const altText = typeof alt === 'string' ? alt : '';
    return `![${altText}](${imageSrc})`;
  },
});
