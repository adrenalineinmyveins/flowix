import { type MouseEvent } from 'react';
import { LoaderCircle } from 'lucide-react';
import { useSelectedItemScroll } from '@features/editor/extensions/shared/use-selected-item-scroll';
import { OverlayScrollbar } from '@shared/ui/overlay-scrollbar';
import type { MentionNoteItem } from '@features/editor/extensions/note-mention/note-mention-data';
import {
  isMentionAgent,
  type MentionItem,
} from '@features/editor/extensions/agent-reference/agent-mention-data';
import { agentFullAddress } from '@features/editor/extensions/agent-reference/agent-reference';
import { useI18n } from '@/lib/i18n';

export interface NoteMentionDropdownProps {
  items: MentionItem[];
  selectedIndex: number;
  scrollSelectedItem: boolean;
  hasMore: boolean;
  loading: boolean;
  onSelect: (item: MentionItem) => void;
  onHover: (index: number) => void;
  onLoadMore: () => void;
}

export function NoteMentionDropdown({
  items,
  selectedIndex,
  scrollSelectedItem,
  hasMore,
  loading,
  onSelect,
  onHover,
  onLoadMore,
}: NoteMentionDropdownProps) {
  const { t } = useI18n();
  const { scrollerRef, itemRefs } = useSelectedItemScroll({
    items,
    selectedIndex,
    scrollSelectedItem,
  });
  const handleItemMouseMove = (
    event: MouseEvent<HTMLButtonElement>,
    index: number
  ) => {
    if (event.movementX === 0 && event.movementY === 0) return;
    onHover(index);
  };
  // AGENT-MARKET M4：混排菜单里有 agent 时换标题，纯笔记时保持原文案。
  const hasAgent = items.some(isMentionAgent);

  return (
    <div
      className={`mention-note-dropdown${scrollSelectedItem ? ' is-keyboard-navigation' : ''}`}
      role="listbox"
      aria-label="Notes"
    >
      <div className="mention-note-header" aria-label="Mention type">
        <span>{hasAgent ? t('editor.noteMention.mixedHeader') : t('editor.noteMention.header')}</span>
        {loading && (
          <LoaderCircle
            className="mention-note-header-spinner"
            aria-hidden="true"
          />
        )}
      </div>
      <OverlayScrollbar
        className="mention-note-items-frame"
        scrollerClassName="mention-note-items"
        scrollerRef={scrollerRef}
        onScroll={(event) => {
            const el = event.currentTarget;
            if (el.scrollTop + el.clientHeight >= el.scrollHeight - 24) {
              onLoadMore();
            }
        }}
      >
          {loading && items.length === 0 ? (
            <div className="mention-note-empty mention-note-empty--loading">
              <span className="mention-note-loading-title">{t('editor.noteMention.loading')}</span>
            </div>
          ) : items.length === 0 ? (
            <div className="mention-note-empty">{t('editor.noteMention.empty')}</div>
          ) : (
            items.map((item, index) => {
              const selected = index === selectedIndex;
              const key = isMentionAgent(item)
                ? `agent:${item.address}@${item.relayUrl}`
                : `${(item as MentionNoteItem).notebookId}:${(item as MentionNoteItem).relativePath}`;
              return (
                <button
                  key={key}
                  ref={(node) => {
                    itemRefs.current[index] = node;
                  }}
                  type="button"
                  role="option"
                  aria-selected={selected}
                  className={`mention-note-item${selected ? ' is-selected' : ''}`}
                  title={isMentionAgent(item) ? agentFullAddress(item) : undefined}
                  onMouseMove={(event) => handleItemMouseMove(event, index)}
                  onMouseDown={(event) => {
                    event.preventDefault();
                    onSelect(item);
                  }}
                >
                  {isMentionAgent(item) ? (
                    <span className="mention-note-title mention-note-title--agent">
                      @{item.displayName}
                    </span>
                  ) : (
                    <span className="mention-note-title">{item.title}</span>
                  )}
                </button>
              );
            })
          )}
          {hasMore && (
            <button
              type="button"
              className="mention-note-more"
              onMouseDown={(event) => {
                event.preventDefault();
                onLoadMore();
              }}
            >
              {t('editor.noteMention.loadMore')}
            </button>
          )}
      </OverlayScrollbar>
    </div>
  );
}
