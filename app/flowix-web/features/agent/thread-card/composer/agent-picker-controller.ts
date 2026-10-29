import type { I18nKey, I18nParams } from '@/lib/i18n';
import { market } from '@platform/tauri/client';
import { invalidateMentionAgents } from '@features/editor/extensions/agent-reference/agent-mention-data';
import { createAnchoredPopoverController, type AnchoredPopoverController } from '../anchored-popover-controller';
import { POPUP_SEPARATOR_CLASS } from '@shared/ui/popup-separator';

/** AGENT-MARKET M4：「添加内容 → 订阅的 Agent」二级弹层。 */
export interface AgentPickerControllerOptions {
  trigger: HTMLButtonElement;
  popover: HTMLDivElement;
  t: (key: I18nKey, params?: I18nParams) => string;
  isDestroyed: () => boolean;
  injectAgentReference: (agent: {
    displayName: string;
    address: string;
    relayUrl: string;
  }) => void;
  onSelect?: () => void;
}

export class AgentPickerController {
  private open = false;
  private disposed = false;
  private loading = false;
  private submenuAnchor: HTMLElement | null = null;
  private readonly positionController: AnchoredPopoverController;

  constructor(private readonly options: AgentPickerControllerOptions) {
    this.positionController = createAnchoredPopoverController({
      isOpen: () => this.open,
      isDestroyed: options.isDestroyed,
      isHidden: () => options.popover.hidden,
      position: () => this.position(),
      observe: () => [options.trigger, options.popover],
    });
  }

  get popoverElement(): HTMLDivElement { return this.options.popover; }

  openFromParent(anchor: HTMLElement): void {
    this.submenuAnchor = anchor;
    this.setOpen(true);
    this.positionController.schedule();
  }

  setOpen(open: boolean): void {
    if (this.open === open) return;
    this.open = open;
    this.options.popover.hidden = !open;
    if (open) {
      this.render();
      this.load();
      this.positionController.start();
      this.positionController.schedule();
      document.addEventListener('pointerdown', this.onOutsidePointer, true);
    } else {
      this.submenuAnchor = null;
      this.positionController.stop();
      document.removeEventListener('pointerdown', this.onOutsidePointer, true);
    }
  }

  refresh(): void { if (this.open) this.load(); }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    this.setOpen(false);
    this.positionController.dispose();
    this.options.popover.remove();
  }

  private onOutsidePointer = (event: PointerEvent): void => {
    const target = event.target as Node | null;
    if (target && (this.options.popover.contains(target) || this.options.trigger.contains(target))) return;
    this.setOpen(false);
  };

  /** 打开时拉取订阅快照；顺带失效 @ 提及缓存，两个入口保持同源。 */
  private load(): void {
    this.loading = true;
    this.renderResults([]);
    void market.subscriptions()
      .then((list) => {
        invalidateMentionAgents();
        if (this.disposed || this.options.isDestroyed() || !this.open) return;
        this.loading = false;
        this.renderResults(list);
      })
      .catch(() => {
        if (this.disposed || this.options.isDestroyed() || !this.open) return;
        this.loading = false;
        this.renderResults([]);
      });
  }

  private render(): void {
    const list = document.createElement('div');
    list.className = 'agent-thread-card__composer-note-search-list';
    list.dataset.agentResults = '';
    const divider = document.createElement('div');
    divider.setAttribute('role', 'separator');
    divider.setAttribute('aria-hidden', 'true');
    divider.className = POPUP_SEPARATOR_CLASS;
    this.options.popover.replaceChildren(list, divider);
  }

  private renderResults(list: Awaited<ReturnType<typeof market.subscriptions>>): void {
    const container = this.options.popover.querySelector<HTMLElement>('[data-agent-results]');
    if (!container) return;
    container.replaceChildren();
    const header = document.createElement('div');
    header.className = 'agent-thread-card__composer-note-popover-header';
    const title = document.createElement('div');
    title.className = 'agent-thread-card__composer-note-popover-title';
    title.textContent = this.options.t('editor.threadCard.agentPicker.title');
    header.append(title);
    container.append(header);
    for (const agent of list) {
      const item = document.createElement('button');
      item.type = 'button';
      item.className = 'agent-thread-card__composer-note-result agent-thread-card__composer-note-item';
      item.title = agent.relayUrl ? `${agent.address}@${agent.relayUrl}` : agent.address;
      item.textContent = `@${agent.displayName}`;
      item.addEventListener('click', (event) => {
        event.stopPropagation();
        this.options.injectAgentReference({
          displayName: agent.displayName,
          address: agent.address,
          relayUrl: agent.relayUrl,
        });
        this.setOpen(false);
        this.options.onSelect?.();
      });
      container.append(item);
    }
    if (!list.length) {
      const empty = document.createElement('div');
      empty.className = 'agent-thread-card__composer-note-empty';
      empty.textContent = this.loading
        ? this.options.t('editor.threadCard.noteSearch.searching')
        : this.options.t('editor.threadCard.agentPicker.empty');
      container.append(empty);
    }
    this.positionController.schedule();
  }

  private position(): void {
    const { trigger, popover } = this.options;
    if (!this.open || !trigger.isConnected || !popover.isConnected) return;
    const rect = (this.submenuAnchor ?? trigger).getBoundingClientRect();
    const width = popover.getBoundingClientRect().width || 300;
    const height = popover.getBoundingClientRect().height || 360;
    const padding = 8;
    const left = rect.right + width + padding <= window.innerWidth
      ? rect.right + 2
      : Math.max(padding, rect.left - width - 2);
    const top = Math.max(padding, Math.min(rect.top, window.innerHeight - height - padding));
    Object.assign(popover.style, { left: `${left}px`, top: `${top}px` });
  }
}
