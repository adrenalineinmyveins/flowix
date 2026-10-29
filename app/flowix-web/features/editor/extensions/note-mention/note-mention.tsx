import { type Editor } from '@tiptap/core';
import type { EditorView } from '@tiptap/pm/view';
import {
  createSuggestionExtension,
  openSuggestionMenuFromEditor,
  type SuggestionMenuConfig,
} from '@features/editor/extensions/shared/suggestion-menu';
import { NoteMentionDropdown } from '@features/editor/extensions/note-mention/note-mention-dropdown';
import {
  queryMentionNotes,
  toNoteReferenceAttrs,
} from '@features/editor/extensions/note-mention/note-mention-data';
import {
  isMentionAgent,
  queryMentionAgents,
  type MentionItem,
} from '@features/editor/extensions/agent-reference/agent-mention-data';

const TRIGGER = '@';
const WIDTH = 280;
/** @ 菜单里订阅 agent 的展示上限（笔记另有 200 条截断）。 */
const MENTION_AGENT_LIMIT = 10;

const noteMentionConfig: SuggestionMenuConfig<MentionItem> = {
  trigger: TRIGGER,
  width: WIDTH,

  parseQuery: (view: EditorView, triggerFrom, trigger) => {
    const { selection } = view.state;
    if (!selection.empty || selection.from < triggerFrom + 1) return null;

    const $trigger = view.state.doc.resolve(triggerFrom);
    const $cursor = view.state.doc.resolve(selection.from);
    if (!$trigger.sameParent($cursor)) return null;

    const text = view.state.doc.textBetween(triggerFrom, selection.from, '\n', '\n');
    if (!text.startsWith(trigger)) return null;

    const query = text.slice(1);
    // Note titles often contain punctuation; only whitespace closes the query.
    if (/\s/.test(query)) return null;
    return query;
  },

  // AGENT-MARKET M4：订阅 agent 与笔记混排（agent 在前），两者数据源各自缓存。
  fetchItems: (query) =>
    Promise.all([queryMentionAgents(query), queryMentionNotes(query)]).then(
      ([agents, notes]) => [...agents.slice(0, MENTION_AGENT_LIMIT), ...notes],
    ),

  render: ({ items, selectedIndex, scrollSelectedItem, hasMore, loading, onSelect, onHover, onLoadMore }) => (
    <NoteMentionDropdown
      items={items}
      selectedIndex={selectedIndex}
      scrollSelectedItem={scrollSelectedItem}
      hasMore={hasMore}
      loading={loading}
      onSelect={onSelect}
      onHover={onHover}
      onLoadMore={onLoadMore}
    />
  ),

  onSelect: ({ editor, item, deleteTriggerText }) => {
    if (isMentionAgent(item)) {
      deleteTriggerText();
      if (!editor.schema.nodes.agentReference) return;
      editor.commands.insertContent({
        type: 'agentReference',
        attrs: {
          displayName: item.displayName,
          address: item.address,
          relayUrl: item.relayUrl,
        },
      });
      return;
    }
    deleteTriggerText();
    if (!editor.schema.nodes.noteReference) return;
    editor.commands.insertContent({
      type: 'noteReference',
      attrs: toNoteReferenceAttrs(item),
    });
  },

  onError: (err) => console.warn('[note-mention] query failed:', err),
};

export function openNoteMention(editor: Editor): boolean {
  return openSuggestionMenuFromEditor(editor, noteMentionConfig);
}

/**
 * `@` trigger for note references and subscribed agents. Selecting an item
 * inserts a `noteReference` / `agentReference` inline card.
 */
export const NoteMention = createSuggestionExtension<MentionItem>(noteMentionConfig);

const wikiNoteMentionConfig: SuggestionMenuConfig<MentionItem> = {
  ...noteMentionConfig,
  trigger: '[[',
  parseQuery: (view: EditorView, triggerFrom, trigger) => {
    const { selection } = view.state;
    if (!selection.empty || selection.from < triggerFrom + trigger.length) return null;
    const $trigger = view.state.doc.resolve(triggerFrom);
    const $cursor = view.state.doc.resolve(selection.from);
    if (!$trigger.sameParent($cursor)) return null;
    const text = view.state.doc.textBetween(triggerFrom, selection.from, '\n', '\n');
    if (!text.startsWith(trigger)) return null;
    const query = text.slice(trigger.length);
    if (query.includes(']') || query.includes('\n')) return null;
    return query;
  },
  // `[[` 保持纯笔记选择器：Obsidian 语义只指向笔记。
  fetchItems: (query) => queryMentionNotes(query),
  onSelect: ({ editor, item, deleteTriggerText }) => {
    if (isMentionAgent(item)) return;
    deleteTriggerText();
    if (!editor.schema.nodes.noteReference) return;
    editor.commands.insertContent({
      type: 'noteReference',
      attrs: {
        ...toNoteReferenceAttrs(item),
        heading: null,
      },
    });
  },
};

/** Obsidian-style `[[` note picker. */
export const WikiNoteMention = createSuggestionExtension<MentionItem>(wikiNoteMentionConfig);
