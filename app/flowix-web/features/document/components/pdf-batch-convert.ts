/**
 * Batch PDF → Markdown conversion for a browsed directory tree.
 *
 * Walks the directory recursively via `get_dir_children`, skips PDFs whose
 * sibling Markdown note already exists, and runs each file through the same
 * kernel as the single-file toolbar button. Progress is reported through
 * toast updates; one failing file never aborts the batch.
 */

import type { useI18n } from '@/lib/i18n';
import { files, type DocTreeItem } from '@platform/tauri/client';
import { toast } from '@/lib/toast';
import { loadPdfDocument, pdfDocumentToMarkdown } from './pdf-convert-core';

type Translate = ReturnType<typeof useI18n>['t'];

/** Plan one directory: which PDFs still need conversion, how many are done. */
export function planDirectory(children: DocTreeItem[]): { pdfs: DocTreeItem[]; skipped: number } {
  const markdownNames = new Set(
    children
      .filter((child) => child.type === 'document' && child.name.toLowerCase().endsWith('.md'))
      .map((child) => child.name.toLowerCase()),
  );
  const pdfs: DocTreeItem[] = [];
  let skipped = 0;
  for (const child of children) {
    if (child.type !== 'document' || !child.name.toLowerCase().endsWith('.pdf')) continue;
    const stem = child.name.slice(0, -'.pdf'.length);
    if (markdownNames.has(`${stem}.md`.toLowerCase())) {
      skipped += 1;
      continue;
    }
    pdfs.push(child);
  }
  return { pdfs, skipped };
}

async function collectPendingPdfs(rootPath: string): Promise<{ pending: DocTreeItem[]; skipped: number }> {
  const pending: DocTreeItem[] = [];
  let skipped = 0;
  const walk = async (directory: string): Promise<void> => {
    const children = await files.getDirChildren(directory);
    const plan = planDirectory(children);
    pending.push(...plan.pdfs);
    skipped += plan.skipped;
    for (const child of children) {
      if (child.type === 'folder') await walk(child.fullPath);
    }
  };
  await walk(rootPath);
  return { pending, skipped };
}

async function convertOne(pdfPath: string, scopePath: string): Promise<'converted' | 'empty'> {
  const encoded = await files.readPdfFile(pdfPath, scopePath);
  const document = await loadPdfDocument(encoded);
  try {
    const markdown = await pdfDocumentToMarkdown(document);
    if (!markdown) return 'empty';
    await files.createPdfMarkdown(pdfPath, scopePath, `${markdown}\n`);
    return 'converted';
  } finally {
    void document.destroy();
  }
}

/** Convert every not-yet-converted PDF under `rootPath` into sibling notes. */
export async function runBatchConvert(rootPath: string, scopePath: string, t: Translate): Promise<void> {
  let collected: { pending: DocTreeItem[]; skipped: number };
  try {
    collected = await collectPendingPdfs(rootPath);
  } catch {
    toast.error(t('pdf.batch.scanFailed'));
    return;
  }
  if (collected.pending.length === 0) {
    toast.info(t('pdf.batch.none', { skipped: collected.skipped }));
    return;
  }
  const total = collected.pending.length;
  toast.info(t('pdf.batch.started', { total }));
  let converted = 0;
  let empty = 0;
  let failed = 0;
  for (const pdf of collected.pending) {
    try {
      const outcome = await convertOne(pdf.fullPath, scopePath);
      if (outcome === 'converted') converted += 1;
      else empty += 1;
    } catch {
      failed += 1;
    }
  }
  const summary = t('pdf.batch.done', { converted, empty, failed, skipped: collected.skipped });
  if (failed > 0 && converted === 0) {
    toast.error(summary);
  } else {
    toast.success(summary, { duration: 8000 });
  }
}
