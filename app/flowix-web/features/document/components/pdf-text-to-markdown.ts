/**
 * Rebuild Markdown from pdf.js text-layer items.
 *
 * PDF text extraction has no structural information: every glyph run is an
 * item with position + approximate font size. Structure is inferred with
 * layout heuristics:
 * - items are grouped into lines by vertical position,
 * - body font size is the most frequent line size,
 * - lines clearly larger than body become headings (three size buckets),
 * - vertically close body lines merge into one paragraph; CJK joins without
 *   spaces, Latin joins with a space and de-hyphenates line breaks.
 *
 * Known limitations (accepted for v1): multi-column layouts interleave, table
 * structure is flattened, list indentation is not detected.
 */

export interface PdfTextItem {
  str: string;
  /** X position in PDF units (pdf.js `transform[4]`). */
  x: number;
  /** Y position in PDF units (pdf.js `transform[5]`, grows upward). */
  y: number;
  /** Item width in PDF units. */
  width: number;
  /** Item height, approximately the font size. */
  height: number;
  /** pdf.js internal font id (`g_d0_f1`); used for monospace detection. */
  fontName?: string;
}

export interface PdfPageText {
  items: PdfTextItem[];
}

interface Line {
  text: string;
  x: number;
  y: number;
  height: number;
  /** x-sorted non-empty items of the visual line (cell candidates). */
  items: PdfTextItem[];
}

const CJK_BOUNDARY = /[\u4e00-\u9fff\u3040-\u30ff\uac00-\ud7af\u3000-\u303f\uff00-\uff60]/;

function isCJKBoundary(value: string): boolean {
  return value.length > 0 && CJK_BOUNDARY.test(value);
}

function round1(value: number): number {
  return Math.round(value * 10) / 10;
}

/** Join two fragments on the same visual line. Items carry their own spacing;
 * only strip whitespace when CJK text (incl. fullwidth punctuation) sits on
 * both sides of the boundary. */
function joinSameLineFragments(previous: string, next: string): string {
  const previousTail = previous.replace(/\s+$/, '');
  const nextHead = next.replace(/^\s+/, '');
  if (isCJKBoundary(previousTail.slice(-1)) && isCJKBoundary(nextHead.slice(0, 1))) {
    return previousTail + nextHead;
  }
  return previous + next;
}

/** Group one page's items into visual lines (y-sorted, x-joined). */
function itemsToLines(items: PdfTextItem[]): Line[] {
  const usable = items.filter((item) => item.str.trim().length > 0);
  if (usable.length === 0) return [];
  const sorted = [...usable].sort((a, b) => b.y - a.y || a.x - b.x);
  const typicalHeight = sorted[Math.floor(sorted.length / 2)].height || 10;
  const tolerance = Math.max(2, typicalHeight * 0.6);
  const lines: Line[] = [];
  let parts: PdfTextItem[] = [];
  const pushLine = () => {
    if (parts.length === 0) return;
    parts.sort((a, b) => a.x - b.x);
    const text = parts.map((part) => part.str).reduce(joinSameLineFragments, '').trim();
    if (text.length > 0) {
      lines.push({
        text,
        x: parts[0].x,
        y: parts[0].y,
        height: Math.max(...parts.map((part) => part.height)),
        items: [...parts],
      });
    }
    parts = [];
  };
  for (const item of sorted) {
    if (parts.length > 0 && Math.abs(parts[0].y - item.y) > tolerance) {
      pushLine();
    }
    parts.push(item);
  }
  pushLine();
  return lines;
}

/** The body size is the most frequent rounded line height on the page. */
function bodyLineHeight(lines: Line[]): number {
  const counts = new Map<number, number>();
  for (const line of lines) {
    const size = round1(line.height);
    counts.set(size, (counts.get(size) ?? 0) + 1);
  }
  let best = 0;
  let bestCount = -1;
  for (const [size, count] of counts) {
    if (count > bestCount || (count === bestCount && size > best)) {
      best = size;
      bestCount = count;
    }
  }
  return best || 10;
}

function headingLevel(size: number, body: number): 0 | 1 | 2 | 3 {
  if (size >= body * 1.7) return 1;
  if (size >= body * 1.35) return 2;
  if (size >= body * 1.12) return 3;
  return 0;
}

function joinBodyLines(previous: string, next: string): string {
  if (previous.endsWith('-') && /^[a-z]/.test(next)) {
    return `${previous.slice(0, -1)}${next}`;
  }
  if (isCJKBoundary(previous.slice(-1)) || isCJKBoundary(next.slice(0, 1))) {
    return `${previous}${next}`;
  }
  return `${previous} ${next}`;
}

/** Line-initial punctuation that marks a line as the wrapped tail of the
 * previous sentence rather than a new block. */
const CONTINUATION_PUNCTUATION = /^[，、；：）】》」』]/;

/** Full-width equivalent length: CJK/full-width chars count 1, others 0.5. */
function visualLength(text: string): number {
  let weight = 0;
  for (const character of text) {
    weight += (character.codePointAt(0) ?? 0) > 0x2e80 ? 1 : 0.5;
  }
  return weight;
}

/** Short CJK-dominant lines are treated as standalone remarks (one line per
 * item in note exports) instead of wrapped body lines. Latin-only lines are
 * exempt: wrapped Latin paragraphs are routinely this short. */
function isShortCJKLine(text: string): boolean {
  return isCJKBoundary(text) && visualLength(text) <= 14;
}

interface MonospaceProfile {
  /** font ids whose Latin items share an (almost) constant advance width. */
  fonts: Set<string>;
  /** advance width per character for those fonts (space reconstruction). */
  charWidth: number;
}

/** Code in exported PDFs is rendered in a monospaced font: every Latin item
 * of that font has the same width/char ratio. Proportional body text drifts
 * by ~5-10% between items. Only items made purely of narrow (Latin) glyphs
 * participate: mixed CJK/Latin lines carry full-width glyph advances that
 * would pollute the ratio. */
function detectMonospace(pages: PdfPageText[]): MonospaceProfile {
  const byFont = new Map<string, number[]>();
  for (const page of pages) {
    for (const item of page.items) {
      if (!item.fontName || !item.str.trim()) continue;
      if ([...item.str].some((character) => character.charCodeAt(0) >= 0x2e80)) continue;
      if (item.str.trim().length < 2) continue;
      const ratio = item.width / item.str.trim().length;
      const samples = byFont.get(item.fontName) ?? [];
      samples.push(ratio);
      byFont.set(item.fontName, samples);
    }
  }
  const fonts = new Set<string>();
  let charWidth = 0;
  for (const [font, samples] of byFont) {
    if (samples.length < 3) continue;
    const average = samples.reduce((sum, value) => sum + value, 0) / samples.length;
    const spread = Math.max(...samples) - Math.min(...samples);
    if (spread < 0.15 && average > 0) {
      fonts.add(font);
      charWidth = Math.max(charWidth, average);
    }
  }
  return { fonts, charWidth };
}

/** A line is a code line when its widest item is pure narrow-glyph text set
 * in one of the detected monospace fonts. Mixed-script body lines (prose
 * sharing the code font) carry CJK items or full-width punctuation, so their
 * widest item is not pure ASCII and they stay prose. */
function isCodeLine(line: Line, monospace: MonospaceProfile): boolean {
  if (monospace.fonts.size === 0 || line.items.length === 0) return false;
  let widest: PdfTextItem | null = null;
  for (const item of line.items) {
    if (!item.fontName) continue;
    if (widest === null || item.width > widest.width) widest = item;
  }
  if (!widest?.fontName) return false;
  if ([...widest.str].some((character) => character.charCodeAt(0) >= 0x2e80)) return false;
  return monospace.fonts.has(widest.fontName);
}

/** Rebuild one code line with leading indent and inter-item gaps converted
 * back to spaces using the monospace advance width. */
function renderCodeLine(line: Line, baseX: number, charWidth: number): string {
  const spaceWidth = charWidth > 0 ? charWidth : 6;
  let text = '';
  let cursor = baseX;
  for (const item of line.items) {
    const gap = Math.round((item.x - cursor) / spaceWidth);
    if (gap > 0) text += ' '.repeat(gap);
    text += item.str;
    cursor = item.x + item.width;
  }
  return text.trimEnd();
}

function pageToBlocks(page: PdfPageText, monospace: MonospaceProfile): string[] {
  const lines = itemsToLines(page.items);
  if (lines.length === 0) return [];
  // Guard: when (nearly) every line on a page is monospaced the whole
  // document is set in a mono font and code detection would swallow the body.
  const codeLines = lines.filter((line) => isCodeLine(line, monospace)).length;
  const codeEnabled = monospace.fonts.size > 0 && codeLines / lines.length <= 0.8;
  const body = bodyLineHeight(lines);
  const blocks: string[] = [];
  let paragraph: string | null = null;
  let lastBodyLine: Line | null = null;
  const flush = () => {
    if (paragraph !== null && paragraph.trim().length > 0) blocks.push(paragraph.trim());
    paragraph = null;
    lastBodyLine = null;
  };
  let index = 0;
  while (index < lines.length) {
    const table = tableRunToMarkdown(lines, index);
    if (table) {
      flush();
      blocks.push(table.markdown);
      index += table.rows;
      continue;
    }
    if (codeEnabled && isCodeLine(lines[index], monospace)) {
      const run: Line[] = [];
      // Loose-note exporters use up to ~2.8x line spacing inside a block,
      // while paragraph breaks start beyond ~3.3x: 3.2x separates them.
      while (
        index + run.length < lines.length
        && isCodeLine(lines[index + run.length], monospace)
        && (run.length === 0
          || Math.abs(lines[index + run.length].y - run[run.length - 1].y) < lines[index].height * 3.2)
      ) {
        run.push(lines[index + run.length]);
      }
      if (run.length >= 2) {
        flush();
        // Indent relative to the first line: hanging continuations that
        // start further left keep their text but must not indent the block.
        const baseX = run[0].x;
        blocks.push([
          '```',
          ...run.map((line) => renderCodeLine(line, baseX, monospace.charWidth)),
          '```',
        ].join('\n'));
        index += run.length;
        continue;
      }
      // An isolated monospaced line is inline-ish content; keep it as text.
    }
    const line = lines[index];
    index += 1;
    const level = headingLevel(line.height, body);
    if (level > 0) {
      flush();
      blocks.push(`${'#'.repeat(level)} ${line.text}`);
      continue;
    }
    const continuesSentence = CONTINUATION_PUNCTUATION.test(line.text);
    if (paragraph === null || lastBodyLine === null) {
      paragraph = line.text;
      lastBodyLine = line;
    } else if (continuesSentence) {
      // A line starting with a closing/comma punctuation is the wrapped tail
      // of the previous sentence — always rejoin it, gap notwithstanding.
      paragraph = joinBodyLines(paragraph, line.text);
      lastBodyLine = line;
    } else {
      // PDF y grows upward: baseline distance between consecutive body lines.
      const gap = lastBodyLine.y - line.y;
      if (gap > lastBodyLine.height * 1.9) {
        flush();
        paragraph = line.text;
        lastBodyLine = line;
      } else {
        paragraph = joinBodyLines(paragraph, line.text);
        lastBodyLine = line;
      }
    }
    // A short CJK line reads as a standalone remark (list-like notes) rather
    // than a wrapped paragraph line: close the block after it, unless the
    // next line starts with continuation punctuation (wrapped sentence tail).
    const nextText = index < lines.length ? lines[index].text : '';
    if (isShortCJKLine(line.text) && !CONTINUATION_PUNCTUATION.test(nextText)) {
      flush();
    }
  }
  flush();
  return blocks;
}

/** Detect a table starting at `start`: at least three vertically adjacent
 * lines with the same per-line item count where every column except the last
 * starts at nearly the same x. The last column is exempt because PDF table
 * exporters often left/right-shift trailing cells with their content width.
 * Returns the Markdown table and the number of lines consumed. */
function tableRunToMarkdown(lines: Line[], start: number): { markdown: string; rows: number } | null {
  const first = lines[start];
  const columnCount = first.items.length;
  if (columnCount < 2) return null;
  let end = start;
  while (
    end + 1 < lines.length
    && lines[end + 1].items.length === columnCount
    && Math.abs(lines[end + 1].y - lines[end].y) < lines[end].height * 2.5
  ) {
    end += 1;
  }
  const rows = lines.slice(start, end + 1);
  if (rows.length < 3) return null;
  for (let column = 0; column < columnCount - 1; column += 1) {
    const starts = rows.map((row) => row.items[column].x);
    if (Math.max(...starts) - Math.min(...starts) > 12) return null;
  }
  const cell = (text: string) => text.trim().replace(/\|/g, '\\|');
  const toRow = (row: Line) => `| ${row.items.map((item) => cell(item.str)).join(' | ')} |`;
  const [header, ...bodyRows] = rows;
  const separator = `| ${Array.from({ length: columnCount }, () => '---').join(' | ')} |`;
  return {
    markdown: [toRow(header), separator, ...bodyRows.map(toRow)].join('\n'),
    rows: rows.length,
  };
}

/** Convert extracted page text into a single Markdown document. */
export function pdfTextContentToMarkdown(pages: PdfPageText[]): string {
  const monospace = detectMonospace(pages);
  const blocks = pages.flatMap((page) => pageToBlocks(page, monospace));
  return blocks.join('\n\n').trim();
}
