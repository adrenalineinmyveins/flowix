import { afterEach, describe, expect, it, vi } from 'vitest';

import { AgentPickerController } from './agent-picker-controller';
import type { SubscribedCard } from '@platform/tauri/client';

const mocks = vi.hoisted(() => ({
  subscriptions: vi.fn(),
}));

vi.mock('@platform/tauri/client', () => ({
  market: { subscriptions: mocks.subscriptions },
}));

function card(address: string, displayName: string): SubscribedCard {
  return {
    address,
    relayUrl: 'https://relay.example.com',
    displayName,
    specialtyTags: [],
    minRedactionLevel: 'green',
    autoAnswer: 'on',
    cardPublicKey: 'pk',
    subscribedAt: 1,
    refreshedAt: 1,
  };
}

async function flush(): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

afterEach(() => {
  document.body.replaceChildren();
  mocks.subscriptions.mockReset();
});

function setup() {
  const trigger = document.createElement('button');
  const popover = document.createElement('div');
  document.body.append(trigger, popover);
  const injectAgentReference = vi.fn();
  const picker = new AgentPickerController({
    trigger,
    popover,
    t: (key) => key,
    isDestroyed: () => false,
    injectAgentReference,
    onSelect: vi.fn(),
  });
  return { picker, popover, injectAgentReference };
}

describe('AgentPickerController', () => {
  it('lists subscribed agents and injects a reference on click', async () => {
    mocks.subscriptions.mockResolvedValue([card('alice/agent-1', 'Alice 助手')]);
    const { picker, popover, injectAgentReference } = setup();
    picker.openFromParent(document.createElement('button'));
    await flush();

    const item = popover.querySelector<HTMLButtonElement>('.agent-thread-card__composer-note-item');
    expect(item?.textContent).toBe('@Alice 助手');
    expect(item?.title).toBe('alice/agent-1@https://relay.example.com');
    item?.click();
    expect(injectAgentReference).toHaveBeenCalledWith({
      displayName: 'Alice 助手',
      address: 'alice/agent-1',
      relayUrl: 'https://relay.example.com',
    });
    expect(popover.hidden).toBe(true);
    picker.dispose();
  });

  it('shows the empty hint when there are no subscriptions', async () => {
    mocks.subscriptions.mockResolvedValue([]);
    const { picker, popover } = setup();
    picker.openFromParent(document.createElement('button'));
    await flush();
    expect(popover.textContent).toContain('editor.threadCard.agentPicker.empty');
    picker.dispose();
  });

  it('closes on an outside pointer press', async () => {
    mocks.subscriptions.mockResolvedValue([card('alice/agent-1', 'Alice')]);
    const { picker, popover } = setup();
    picker.openFromParent(document.createElement('button'));
    await flush();
    expect(popover.hidden).toBe(false);

    const outside = document.createElement('div');
    document.body.append(outside);
    outside.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }));
    expect(popover.hidden).toBe(true);
    picker.dispose();
  });
});
