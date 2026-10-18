'use client';

import { useEffect, useState, type ReactNode } from 'react';
import { ChevronLeft, ChevronRight } from 'lucide-react';
import { Tooltip } from '@shared/ui/tooltip';
import { useI18n, type I18nKey } from '@/lib/i18n';
import { cn } from '@/lib/utils';
import folderIcon from '@/assets/folder-outline.svg?raw';
import { NotebookTreeFileIcon } from '@features/memo/components/notebook-tree-file-icon';

export type MemoListViewTab = 'folders' | 'cards' | 'conversations' | 'agents';

interface MemoListViewTabsProps {
  activeTab: MemoListViewTab;
  onChange: (tab: MemoListViewTab) => void;
  /** The active cards tab doubles as the entry point for the note navigation drawer. */
  navigationDrawerEnabled?: boolean;
  navigationDrawerOpen?: boolean;
  onToggleNavigationDrawer?: () => void;
}

type TabIconProps = {
  className?: string;
  weight: 'regular' | 'fill';
};

const TABS: ReadonlyArray<{
  value: MemoListViewTab;
  labelKey: I18nKey;
  icon: (props: TabIconProps) => ReactNode;
  dimmed?: boolean;
}> = [
  {
    value: 'folders',
    labelKey: 'memo.list.viewFolders',
    icon: ({ className }) => (
      <span className={cn(className, 'inline-flex items-center justify-center')}>
        <span
          className="block h-full w-full"
          dangerouslySetInnerHTML={{ __html: folderIcon }}
        />
      </span>
    ),
  },
  {
    value: 'cards',
    labelKey: 'memo.list.viewCards',
    icon: ({ className }) => <NotebookTreeFileIcon className={className} />,
  },
  {
    value: 'conversations',
    labelKey: 'memo.navigation.conversations',
    icon: () => (
      <svg
        xmlns="http://www.w3.org/2000/svg"
        width="32"
        height="32"
        viewBox="0 0 256 256"
        fill="currentColor"
        className="h-full w-full"
      >
        <path d="M172,112a8,8,0,0,1-8,8H96a8,8,0,0,1,0-16h68A8,8,0,0,1,172,112Zm-8,24H96a8,8,0,0,0,0,16h68a8,8,0,0,0,0-16Zm68-12A100.11,100.11,0,0,1,132,224H48a16,16,0,0,1-16-16V124a100,100,0,0,1,200,0Zm-16,0a84,84,0,0,0-168,0v84h84A84.09,84.09,0,0,0,216,124Z" />
      </svg>
    ),
  },
  {
    value: 'agents',
    labelKey: 'memo.list.viewAgents',
    icon: () => (
      <svg
        xmlns="http://www.w3.org/2000/svg"
        width="32"
        height="32"
        viewBox="0 0 256 256"
        fill="currentColor"
        className="h-full w-full"
      >
        <path d="M216,96V56a16,16,0,0,0-16-16H56A16,16,0,0,0,40,56V96a16,16,0,0,0,0,32v72a24,24,0,0,0,24,24H96a24,24,0,0,0,24-24V160h16v40a24,24,0,0,0,24,24h32a24,24,0,0,0,24-24V128a16,16,0,0,0,0-32ZM56,112H48a8,8,0,0,1,0-16h8ZM208,96v24a8,8,0,0,1-8,8H160a8,8,0,0,1-8-8V64h40a8,8,0,0,1,8,8ZM56,64h8v56a8,8,0,0,0,8,8h40a8,8,0,0,0,8-8V64h32v96H64a8,8,0,0,1-8-8Zm40,136a8,8,0,0,1-8,8H64a8,8,0,0,1-8-8V176h40Zm96,8H160a8,8,0,0,1-8-8V176h40v24A8,8,0,0,1,192,208Z" />
      </svg>
    ),
  },
];

function TabIcon({
  icon,
  className,
  weight,
  dimmed = true,
}: {
  icon: (props: TabIconProps) => ReactNode;
  className?: string;
  weight: 'regular' | 'fill';
  dimmed?: boolean;
}) {
  return (
    <span
      aria-hidden="true"
      className={cn(
        'memo-list-view-tab-icon flex h-4 w-4 shrink-0 items-center justify-center',
        dimmed && 'opacity-50',
        className,
      )}
    >
      {icon({ className: 'h-full w-full', weight })}
    </span>
  );
}

export function MemoListViewTabs({
  activeTab,
  onChange,
  navigationDrawerEnabled = false,
  navigationDrawerOpen = false,
  onToggleNavigationDrawer,
}: MemoListViewTabsProps) {
  const { t } = useI18n();
  const [indicatorTab, setIndicatorTab] = useState<MemoListViewTab>(activeTab);

  useEffect(() => {
    if (indicatorTab === activeTab) return;
    const frame = window.requestAnimationFrame(() => {
      setIndicatorTab(activeTab);
    });
    return () => window.cancelAnimationFrame(frame);
  }, [activeTab, indicatorTab]);

  const handleChange = (tab: MemoListViewTab) => {
    onChange(tab);
  };

  return (
    <div
      data-memo-list-view-tabs
      role="tablist"
      aria-label={t('memo.navigation.menuTitle')}
      className="relative flex h-[30px] shrink-0 items-center gap-0.5 rounded-xl border border-[var(--border)] bg-[color-mix(in_oklch,var(--foreground)_5%,transparent)] p-0.5"
    >
      <span
        aria-hidden="true"
        className="pointer-events-none absolute left-0.5 top-1/2 h-6 w-6 -translate-y-1/2 rounded-lg border border-[var(--border)] bg-[var(--card)] shadow-sm transition-transform duration-200 ease-out"
        style={{
          transform: `translateX(${TABS.findIndex((tab) => tab.value === indicatorTab) * 26}px) translateY(-50%)`,
        }}
      />
      {TABS.map(({ value, labelKey, icon: IconComponent, dimmed }) => {
        const active = activeTab === value;
        const label = t(labelKey);
        const opensNavigation =
          value === 'cards' &&
          active &&
          navigationDrawerEnabled &&
          Boolean(onToggleNavigationDrawer);
        const buttonLabel = opensNavigation
          ? navigationDrawerOpen
            ? t('memo.navigation.closeDrawer')
            : t('memo.navigation.menuTitle')
          : label;
        const activate = () => {
          if (opensNavigation) {
            onToggleNavigationDrawer?.();
            return;
          }
          handleChange(value);
        };
        return (
          <Tooltip key={value} content={buttonLabel} side="bottom" sideOffset={4}>
            <button
              type="button"
              role="tab"
              data-memo-list-view-tab={value}
              aria-selected={active}
              aria-label={buttonLabel}
              aria-expanded={opensNavigation ? navigationDrawerOpen : undefined}
              title={buttonLabel}
              onPointerDown={(event) => {
                // Leaving an edited BrowserColumn synchronously blurs and
                // serializes its editor during the ancestor pointerdown. Run
                // the tab intent before that focus transition can rerender the
                // trigger and swallow the later click event.
                if (event.isPrimary === false || event.button !== 0) return;
                activate();
              }}
              onClick={(event) => {
                // Pointer activation already ran on pointerdown. A detail of 0
                // is the keyboard/assistive-technology click path.
                if (event.detail === 0) activate();
              }}
              className={cn(
                'group relative z-[1] flex h-6 w-6 items-center justify-center rounded-lg transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[var(--ring)]',
                active
                  ? 'text-[var(--primary)]'
                  : 'text-[var(--muted-foreground)] hover:bg-[var(--muted)] hover:text-[var(--foreground)]',
              )}
            >
              {opensNavigation ? (
                <span className="relative block h-4 w-4" aria-hidden="true">
                  {navigationDrawerOpen ? (
                    <ChevronLeft
                      className="absolute inset-0 h-4 w-4"
                      strokeWidth={2.5}
                    />
                  ) : (
                    <>
                      <TabIcon
                        icon={IconComponent}
                        weight={active ? 'fill' : 'regular'}
                        dimmed={dimmed}
                        className="absolute inset-0 transition-opacity duration-150 group-hover:opacity-0 group-focus-visible:opacity-0"
                      />
                      <ChevronRight
                        className="absolute inset-0 h-4 w-4 opacity-0 transition-opacity duration-150 group-hover:opacity-100 group-focus-visible:opacity-100"
                        strokeWidth={2.5}
                      />
                    </>
                  )}
                </span>
              ) : (
                <TabIcon icon={IconComponent} weight={active ? 'fill' : 'regular'} dimmed={dimmed} />
              )}
            </button>
          </Tooltip>
        );
      })}
    </div>
  );
}
