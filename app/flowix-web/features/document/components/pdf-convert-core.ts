/**
 * Shared PDF → Markdown conversion kernel (no React).
 *
 * Both the single-file preview toolbar and the batch converter go through
 * this module so extraction heuristics stay in one place. pdf.js and its
 * worker are dynamically imported; neither enters the startup bundle.
 */

import type { PDFDocumentProxy } from 'pdfjs-dist';
import type { TextItem } from 'pdfjs-dist/types/src/display/api';
import { pdfTextContentToMarkdown, type PdfPageText } from './pdf-text-to-markdown';

export function decodeBase64(value: string): Uint8Array {
  const binary = window.atob(value);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  return bytes;
}

let pdfjsPromise: Promise<typeof import('pdfjs-dist')> | null = null;

/** Lazily import pdf.js once and keep the worker URL configured. */
async function loadPdfjs(): Promise<typeof import('pdfjs-dist')> {
  if (!pdfjsPromise) {
    pdfjsPromise = (async () => {
      const [pdfjsModule, workerModule] = await Promise.all([
        import('pdfjs-dist'),
        import('pdfjs-dist/build/pdf.worker.min.mjs?url'),
      ]);
      pdfjsModule.GlobalWorkerOptions.workerSrc = workerModule.default;
      return pdfjsModule;
    })();
  }
  return pdfjsPromise;
}

/** Parse base64-encoded PDF bytes into a pdf.js document. */
export async function loadPdfDocument(encoded: string): Promise<PDFDocumentProxy> {
  const pdfjs = await loadPdfjs();
  return pdfjs.getDocument({ data: decodeBase64(encoded) }).promise;
}

/** Pull the raw text layer of every page, mapped to converter input. */
export async function extractPageText(document: PDFDocumentProxy): Promise<PdfPageText[]> {
  const pages: PdfPageText[] = [];
  for (let index = 1; index <= document.numPages; index += 1) {
    const page = await document.getPage(index);
    const content = await page.getTextContent();
    pages.push({
      items: content.items
        .filter((entry): entry is TextItem => 'str' in entry)
        .map((entry) => ({
          str: entry.str,
          x: entry.transform[4],
          y: entry.transform[5],
          width: entry.width,
          height: entry.height,
          fontName: entry.fontName,
        })),
    });
  }
  return pages;
}

/** Full pipeline: document → extracted pages → Markdown text. */
export async function pdfDocumentToMarkdown(document: PDFDocumentProxy): Promise<string> {
  return pdfTextContentToMarkdown(await extractPageText(document)).trim();
}
