import { describe, expect, it } from 'vitest';

import { planDirectory } from './pdf-batch-convert';
import type { DocTreeItem } from '@platform/tauri/client';

function entry(name: string, type: 'document' | 'folder'): DocTreeItem {
  return {
    id: name,
    fullPath: `/root/${name}`,
    name,
    type,
  } as DocTreeItem;
}

describe('planDirectory', () => {
  it('collects PDFs and skips those with an existing sibling note', () => {
    const plan = planDirectory([
      entry('a.pdf', 'document'),
      entry('a.md', 'document'),
      entry('b.pdf', 'document'),
      entry('notes', 'folder'),
      entry('c.txt', 'document'),
    ]);
    expect(plan.pdfs.map((item) => item.name)).toEqual(['b.pdf']);
    expect(plan.skipped).toBe(1);
  });

  it('matches the sibling note case-insensitively', () => {
    const plan = planDirectory([
      entry('Report.PDF', 'document'),
      entry('report.MD', 'document'),
    ]);
    expect(plan.pdfs).toHaveLength(0);
    expect(plan.skipped).toBe(1);
  });

  it('ignores folders and non-PDF documents', () => {
    const plan = planDirectory([
      entry('archive', 'folder'),
      entry('photo.png', 'document'),
      entry('data.csv', 'document'),
    ]);
    expect(plan.pdfs).toHaveLength(0);
    expect(plan.skipped).toBe(0);
  });

  it('treats numbered duplicate notes as converted', () => {
    // `x.md` is enough to skip `x.pdf`; numbered siblings imply prior runs.
    const plan = planDirectory([entry('x.pdf', 'document'), entry('x (1).md', 'document')]);
    expect(plan.pdfs).toHaveLength(1);
    expect(plan.skipped).toBe(0);
  });
});
