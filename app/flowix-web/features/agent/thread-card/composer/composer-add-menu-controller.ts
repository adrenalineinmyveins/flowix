import type { I18nKey } from "@/lib/i18n";
import type { NotePickerController } from "@features/agent/thread-card/note/note-picker-controller";
import type { AgentPickerController } from "./agent-picker-controller";
import type { ComposerImageController } from "./composer-image-controller";
import { COMPOSER_ATTACHMENT_ACCEPT } from "./composer-image-controller";

export interface ComposerAddMenuControllerOptions {
  trigger: HTMLButtonElement;
  popover: HTMLDivElement;
  notePopover: HTMLDivElement;
  notePicker: NotePickerController;
  /** AGENT-MARKET M4：订阅 agent 二级弹层。 */
  agentPopover: HTMLDivElement;
  agentPicker: AgentPickerController;
  images: ComposerImageController;
  t: (key: I18nKey) => string;
  isDestroyed: () => boolean;
  openNotebookAgentSettings: () => void;
}

export class ComposerAddMenuController {
  private open = false;
  private disposed = false;
  private noteItem: HTMLButtonElement | null = null;
  private agentItem: HTMLButtonElement | null = null;
  private submenuCloseTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(private readonly options: ComposerAddMenuControllerOptions) {
    options.trigger.addEventListener("click", this.handleTriggerClick);
    for (const popover of [options.notePopover, options.agentPopover]) {
      popover.addEventListener("pointerenter", this.cancelSubmenuClose);
      popover.addEventListener("pointerleave", this.handleSubmenuBoundaryLeave);
    }
  }

  close(): void { this.setOpen(false); }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    this.cancelSubmenuClose();
    this.setOpen(false);
    this.options.trigger.removeEventListener("click", this.handleTriggerClick);
    for (const popover of [this.options.notePopover, this.options.agentPopover]) {
      popover.removeEventListener("pointerenter", this.cancelSubmenuClose);
      popover.removeEventListener("pointerleave", this.handleSubmenuBoundaryLeave);
    }
    this.options.popover.remove();
  }

  private readonly handleTriggerClick = (event: MouseEvent): void => {
    event.preventDefault();
    event.stopPropagation();
    this.setOpen(!this.open);
  };

  private setOpen(open: boolean): void {
    this.open = open;
    this.options.popover.hidden = !open;
    this.options.trigger.setAttribute("aria-expanded", String(open));
    this.options.trigger.classList.toggle("agent-thread-card__composer-add-button--open", open);
    if (open) {
      this.render();
      this.position();
      document.addEventListener("pointerdown", this.handleOutsidePointer, true);
    } else {
      this.noteItem = null;
      this.agentItem = null;
      this.options.notePicker.setOpen(false);
      this.options.agentPicker.setOpen(false);
      document.removeEventListener("pointerdown", this.handleOutsidePointer, true);
    }
  }

  private readonly handleOutsidePointer = (event: PointerEvent): void => {
    const target = event.target as Node | null;
    if (target && (
      this.options.popover.contains(target)
      || this.options.notePopover.contains(target)
      || this.options.agentPopover.contains(target)
      || this.options.trigger.contains(target)
    )) return;
    this.setOpen(false);
  };

  private render(): void {
    this.noteItem?.removeEventListener("pointerleave", this.handleSubmenuBoundaryLeave);
    this.agentItem?.removeEventListener("pointerleave", this.handleSubmenuBoundaryLeave);
    const note = this.createItem(this.options.t("editor.threadCard.addNote"), true);
    // AGENT-MARKET M4：订阅的 Agent（与「添加笔记」同为二级弹层）。
    const agent = this.createAgentItem();
    const attachment = this.createItem(this.options.t("editor.threadCard.addAttachment"), false);
    const items = [note, agent, attachment];
    const projectAiSettings = this.createItem(this.options.t("editor.threadCard.projectAiSettings"), false, () => {
      this.setOpen(false);
      this.options.openNotebookAgentSettings();
    });
    items.push(projectAiSettings);
    this.options.popover.replaceChildren(...items);
    this.noteItem = note;
    this.agentItem = agent;
    // The parent item and level-2 popover form one continuous hover region.
    note.addEventListener("pointerleave", this.handleSubmenuBoundaryLeave);
    agent.addEventListener("pointerleave", this.handleSubmenuBoundaryLeave);
  }

  private createItem(label: string, isNote: boolean, onClick?: () => void): HTMLButtonElement {
    const item = document.createElement("button");
    item.type = "button";
    // Reuse the permission/model dropdown item contract so copy and controls
    // keep exactly the same typography, spacing and hover treatment.
    item.className = "agent-thread-card__codex-settings-item agent-thread-card__composer-add-item";
    item.setAttribute("role", "menuitem");
    item.textContent = label;
    if (isNote) {
      item.append(this.chevron());
      item.addEventListener("pointerenter", () => {
        this.cancelSubmenuClose();
        this.options.agentPicker.setOpen(false);
        this.options.notePicker.openFromParent(item);
      });
      item.addEventListener("focus", () => this.options.notePicker.openFromParent(item));
      item.addEventListener("click", (event) => {
        event.stopPropagation();
        this.options.notePicker.openFromParent(item);
      });
    } else if (onClick) {
      item.addEventListener("click", (event) => {
        event.stopPropagation();
        onClick();
      });
    } else {
      item.addEventListener("click", (event) => {
        event.stopPropagation();
        this.pickAttachments();
      });
    }
    return item;
  }

  /** 与「添加笔记」同构的二级弹层项，但指向订阅 agent 列表。 */
  private createAgentItem(): HTMLButtonElement {
    const item = document.createElement("button");
    item.type = "button";
    item.className = "agent-thread-card__codex-settings-item agent-thread-card__composer-add-item";
    item.setAttribute("role", "menuitem");
    item.textContent = this.options.t("editor.threadCard.addAgent");
    item.append(this.chevron());
    item.addEventListener("pointerenter", () => {
      this.cancelSubmenuClose();
      this.options.notePicker.setOpen(false);
      this.options.agentPicker.openFromParent(item);
    });
    item.addEventListener("focus", () => this.options.agentPicker.openFromParent(item));
    item.addEventListener("click", (event) => {
      event.stopPropagation();
      this.options.agentPicker.openFromParent(item);
    });
    return item;
  }

  private readonly handleSubmenuBoundaryLeave = (event: PointerEvent): void => {
    const next = event.relatedTarget as Node | null;
    const stillInside = (item: HTMLButtonElement | null, popover: HTMLDivElement): boolean =>
      Boolean(next && (item?.contains(next) || popover.contains(next)));
    if (
      stillInside(this.noteItem, this.options.notePopover)
      || stillInside(this.agentItem, this.options.agentPopover)
    ) return;
    // A short grace period is the usual cascade-menu behavior and covers the
    // pointer crossing the shared edge between the two fixed popovers.
    this.cancelSubmenuClose();
    this.submenuCloseTimer = setTimeout(() => {
      this.submenuCloseTimer = null;
      this.options.notePicker.setOpen(false);
      this.options.agentPicker.setOpen(false);
    }, 100);
  };

  private readonly cancelSubmenuClose = (): void => {
    if (this.submenuCloseTimer !== null) {
      clearTimeout(this.submenuCloseTimer);
      this.submenuCloseTimer = null;
    }
  };

  private pickAttachments(): void {
    const input = document.createElement("input");
    input.type = "file";
    input.accept = COMPOSER_ATTACHMENT_ACCEPT;
    input.multiple = true;
    input.hidden = true;
    input.addEventListener("change", () => {
      this.options.images.addFiles(Array.from(input.files ?? []));
      input.remove();
    }, { once: true });
    document.body.append(input);
    input.click();
    this.setOpen(false);
  }

  private position(): void {
    if (!this.open || this.disposed || this.options.isDestroyed()) return;
    const rect = this.options.trigger.getBoundingClientRect();
    const padding = 8;
    const offset = 6;
    const popoverRect = this.options.popover.getBoundingClientRect();
    const width = popoverRect.width || 168;
    const height = popoverRect.height || 80;
    const left = Math.max(padding, Math.min(rect.left, window.innerWidth - width - padding));
    const top = rect.top >= height + offset + padding
      ? rect.top - height - offset
      : rect.bottom + offset;
    Object.assign(this.options.popover.style, { left: `${left}px`, top: `${top}px` });
  }

  private chevron(): SVGSVGElement {
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.setAttribute("viewBox", "0 0 24 24");
    svg.setAttribute("aria-hidden", "true");
    svg.classList.add("agent-thread-card__composer-add-chevron");
    const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
    path.setAttribute("d", "m9 6 6 6-6 6");
    path.setAttribute("fill", "none");
    path.setAttribute("stroke", "currentColor");
    path.setAttribute("stroke-width", "2");
    path.setAttribute("stroke-linecap", "round");
    path.setAttribute("stroke-linejoin", "round");
    svg.append(path);
    return svg;
  }
}
