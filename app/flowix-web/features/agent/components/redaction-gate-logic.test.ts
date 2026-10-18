import { describe, expect, it } from 'vitest';

import type { RedactionOutboxRecord, RedactionPreviewReport } from '@/lib/types/redaction';
import {
  describeRedactionError,
  gateViewFromRecord,
  gateViewFromReport,
  pickLatestDraft,
  remainingConfirmSeconds,
} from '@features/agent/components/redaction-gate-logic';

function record(overrides: Partial<RedactionOutboxRecord>): RedactionOutboxRecord {
  return {
    receiptId: 'rct_1',
    status: 'draft',
    createdAt: 1_000,
    settledAt: null,
    question: 'q',
    scopeEntryIds: ['nb_1'],
    targetLevel: 'r2',
    ruleSetVersion: 'rs-1',
    payload: 'payload text',
    placeholderMap: { '⟦PHONE-1⟧': '13800138000' },
    l1Hits: [{ ruleId: 'phone', count: 1 }],
    includedPaths: ['a.md'],
    droppedPaths: ['b.md'],
    attempts: 1,
    usedFallback: false,
    truncated: false,
    ...overrides,
  };
}

describe('remainingConfirmSeconds', () => {
  it('counts down from createdAt and clamps at zero', () => {
    const now = 100_000;
    expect(remainingConfirmSeconds(now - 10_000, 120, now)).toBe(110);
    expect(remainingConfirmSeconds(now - 130_000, 120, now)).toBe(0);
    expect(remainingConfirmSeconds(now, 30, now)).toBe(30);
  });
});

describe('pickLatestDraft', () => {
  it('returns the newest draft and ignores settled records', () => {
    const records = [
      record({ receiptId: 'rct_old', createdAt: 100 }),
      record({ receiptId: 'rct_confirmed', createdAt: 200, status: 'confirmed' }),
      record({ receiptId: 'rct_new', createdAt: 300 }),
      record({ receiptId: 'rct_cancelled', createdAt: 400, status: 'cancelled' }),
    ];
    expect(pickLatestDraft(records)?.receiptId).toBe('rct_new');
  });

  it('returns null when there is no draft', () => {
    expect(pickLatestDraft([record({ status: 'failed' })])).toBeNull();
    expect(pickLatestDraft([])).toBeNull();
  });
});

describe('gate view adapters', () => {
  it('maps a preview report into the gate view', () => {
    const report: RedactionPreviewReport = {
      receiptId: 'rct_p',
      finalText: 'final',
      placeholderMap: { '⟦EMAIL-1⟧': 'a@b.c' },
      l1Hits: [{ ruleId: 'email', count: 2 }],
      attempts: 2,
      usedFallback: true,
      includedPaths: ['x.md'],
      droppedPaths: [],
      truncated: true,
      targetLevel: 'r3',
      ruleSetVersion: 'rs-2',
    };
    expect(gateViewFromReport(report)).toEqual({
      receiptId: 'rct_p',
      question: '',
      finalText: 'final',
      placeholderMap: { '⟦EMAIL-1⟧': 'a@b.c' },
      l1Hits: [{ ruleId: 'email', count: 2 }],
      targetLevel: 'r3',
      includedPaths: ['x.md'],
      droppedPaths: [],
      truncated: true,
      attempts: 2,
      usedFallback: true,
      scopeEntryIds: [],
      ruleSetVersion: 'rs-2',
    });
  });

  it('maps an outbox record into the gate view', () => {
    const view = gateViewFromRecord(record({}));
    expect(view.receiptId).toBe('rct_1');
    expect(view.finalText).toBe('payload text');
    expect(view.question).toBe('q');
    expect(view.scopeEntryIds).toEqual(['nb_1']);
  });
});

describe('describeRedactionError', () => {
  it('maps known pipeline codes to readable copy and keeps details', () => {
    expect(describeRedactionError('scope_empty')).toBe('披露范围为空：请至少勾选一个已启用的授权范围。');
    expect(describeRedactionError('payload_too_large:80')).toBe('载荷超过上限（80）');
    expect(describeRedactionError('UNCONFIRMED_SEND_BLOCKED:status_draft')).toBe(
      '未经确认，发送被拦截（status_draft）',
    );
    expect(describeRedactionError('confirm_timeout:rct_9')).toBe('确认超时（rct_9）');
  });

  it('falls back to the raw message', () => {
    expect(describeRedactionError('something_new')).toBe('something_new');
    expect(describeRedactionError('')).toBe('操作失败。');
  });
});
