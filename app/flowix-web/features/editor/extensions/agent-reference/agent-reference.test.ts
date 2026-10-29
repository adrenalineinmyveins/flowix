import { Editor } from '@tiptap/core';
import Document from '@tiptap/extension-document';
import HardBreak from '@tiptap/extension-hard-break';
import Paragraph from '@tiptap/extension-paragraph';
import Text from '@tiptap/extension-text';
import { Markdown } from '@tiptap/markdown';
import { afterEach, describe, expect, it } from 'vitest';

import {
  AgentReference,
  agentFullAddress,
  buildAgentLinkMarkdown,
  parseAgentLinkHref,
} from './agent-reference';

const attrs = {
  displayName: 'Alice 助手',
  address: 'alice/agent-1',
  relayUrl: 'https://relay.example.com',
};

function createEditor(): Editor {
  return new Editor({
    element: { mount: document.createElement('div') },
    extensions: [Document, Paragraph, Text, HardBreak, Markdown, AgentReference],
    content: '',
    contentType: 'markdown',
  });
}

function findAgentNodes(editor: Editor) {
  const found: Array<Record<string, unknown>> = [];
  editor.state.doc.descendants((node) => {
    if (node.type.name === 'agentReference') found.push(node.attrs);
    return true;
  });
  return found;
}

afterEach(() => {
  document.body.replaceChildren();
});

describe('parseAgentLinkHref', () => {
  it('parses a full flowix://agent href', () => {
    const parsed = parseAgentLinkHref(
      'flowix://agent?n=Alice%20%E5%8A%A9%E6%89%8B&a=alice%2Fagent-1&r=https%3A%2F%2Frelay.example.com',
    );
    expect(parsed).toEqual(attrs);
  });

  it('falls back to the address as display name when n is missing', () => {
    const parsed = parseAgentLinkHref('flowix://agent?a=alice%2Fagent-1&r=https%3A%2F%2Frelay.example.com');
    expect(parsed?.displayName).toBe('alice/agent-1');
  });

  it('rejects non-agent hrefs and addresses without user/slug', () => {
    expect(parseAgentLinkHref('https://relay.example.com')).toBeNull();
    expect(parseAgentLinkHref('flowix://agent?a=no-slash&r=x')).toBeNull();
    expect(parseAgentLinkHref('flowix://open?x=1')).toBeNull();
  });
});

describe('agentFullAddress', () => {
  it('joins address and relay, omitting empty relay', () => {
    expect(agentFullAddress(attrs)).toBe('alice/agent-1@https://relay.example.com');
    expect(agentFullAddress({ address: 'alice/agent-1', relayUrl: '' })).toBe('alice/agent-1');
  });
});

describe('AgentReference markdown roundtrip', () => {
  it('tokenizes the agent link into an agentReference node and re-serializes it', () => {
    const editor = createEditor();
    const markdown = buildAgentLinkMarkdown(attrs);
    editor.commands.setContent(`问一下 ${markdown} 吧`, { contentType: 'markdown' });

    expect(findAgentNodes(editor)).toEqual([attrs]);

    // 发送 / 落盘序列化保持标准链接形态（地址完整，可直接 consult）。
    const output = editor.getMarkdown();
    expect(output).toContain('flowix://agent?');
    expect(output).toContain(encodeURIComponent('alice/agent-1'));
    expect(parseAgentLinkHref(output.slice(output.indexOf('(flowix://agent?') + 1, output.indexOf(')', output.indexOf('(flowix://agent?')))))
      .toEqual(attrs);

    editor.destroy();
  });

  it('keeps ordinary links untouched', () => {
    const editor = createEditor();
    editor.commands.setContent('[docs](https://example.com)', { contentType: 'markdown' });
    expect(findAgentNodes(editor)).toEqual([]);
    expect(editor.state.doc.textContent).toBe('docs');
    editor.destroy();
  });
});
