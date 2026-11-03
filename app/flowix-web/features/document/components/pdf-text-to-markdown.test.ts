import { describe, expect, it } from 'vitest';

import {
  pdfTextContentToMarkdown,
  type PdfTextItem,
} from './pdf-text-to-markdown';

function item(str: string, x: number, y: number, height = 10, width = 10, fontName?: string): PdfTextItem {
  return { str, x, y, width, height, fontName };
}

describe('pdfTextContentToMarkdown', () => {
  it('merges tightly spaced body lines into one paragraph', () => {
    const markdown = pdfTextContentToMarkdown([
      { items: [item('Flowix is a ', 50, 700), item('note app', 120, 700), item('for humans.', 50, 685)] },
    ]);
    expect(markdown).toBe('Flowix is a note app for humans.');
  });

  it('splits paragraphs on a large vertical gap', () => {
    const markdown = pdfTextContentToMarkdown([
      { items: [item('First paragraph.', 50, 700), item('Second paragraph.', 50, 640)] },
    ]);
    expect(markdown).toBe('First paragraph.\n\nSecond paragraph.');
  });

  it('detects headings from larger font sizes', () => {
    const markdown = pdfTextContentToMarkdown([
      {
        items: [
          item('Annual Report', 50, 760, 24),
          item('Summary', 50, 700, 15),
          item('Body text line.', 50, 680),
          item('Body text line two.', 50, 665),
        ],
      },
    ]);
    expect(markdown).toBe('# Annual Report\n\n## Summary\n\nBody text line. Body text line two.');
  });

  it('joins CJK lines without inserting spaces', () => {
    const markdown = pdfTextContentToMarkdown([
      { items: [item(' Flowix 是一款 ', 50, 700), item('桌面笔记应用', 140, 700), item('，支持全文搜索。', 50, 685)] },
    ]);
    expect(markdown).toBe('Flowix 是一款桌面笔记应用，支持全文搜索。');
  });

  it('de-hyphenates Latin words broken across lines', () => {
    const markdown = pdfTextContentToMarkdown([
      { items: [item('interop-', 50, 700), item('erable', 50, 685)] },
    ]);
    expect(markdown).toBe('interoperable');
  });

  it('keeps pages as separate blocks and ignores empty pages', () => {
    const markdown = pdfTextContentToMarkdown([
      { items: [item('Page one body.', 50, 700)] },
      { items: [] },
      { items: [item('Page two body.', 50, 700)] },
    ]);
    expect(markdown).toBe('Page one body.\n\nPage two body.');
  });

  it('returns an empty string when no page carries text', () => {
    expect(pdfTextContentToMarkdown([{ items: [item('   ', 50, 700)] }])).toBe('');
    expect(pdfTextContentToMarkdown([])).toBe('');
  });

  it('sorts items on the same line by x position', () => {
    const markdown = pdfTextContentToMarkdown([
      { items: [item('world', 120, 700), item('hello ', 50, 700)] },
    ]);
    expect(markdown).toBe('hello world');
  });

  it('rebuilds aligned multi-item rows as a Markdown table', () => {
    const markdown = pdfTextContentToMarkdown([
      {
        items: [
          item('ROI', 23, 806), item('region of interest', 77, 806), item('感兴趣区域', 161, 806),
          item('LSCM', 23, 787), item('lens shading correction monitor', 76, 787), item('镜头阴影矫正监测器', 227, 787),
          item('AEM', 23, 769), item('Auto Exposure Monitor', 74, 769), item('自动曝光监测器', 205, 769),
          item('HSM', 23, 751), item('Histogram Monitor', 73, 751), item('直方图监测器', 191, 751),
        ],
      },
    ]);
    expect(markdown).toBe(
      '| ROI | region of interest | 感兴趣区域 |\n'
      + '| --- | --- | --- |\n'
      + '| LSCM | lens shading correction monitor | 镜头阴影矫正监测器 |\n'
      + '| AEM | Auto Exposure Monitor | 自动曝光监测器 |\n'
      + '| HSM | Histogram Monitor | 直方图监测器 |',
    );
  });

  it('keeps each short CJK line as its own block', () => {
    const markdown = pdfTextContentToMarkdown([
      {
        items: [
          item('LSCM', 23, 696), item('在', 55, 696), item('BLC', 66, 696), item('之后', 86, 696),
          item('AEM', 23, 677), item('、', 46, 677), item('HSM', 56, 677), item('在', 80, 677), item('LSC', 90, 677), item('之后', 111, 677),
          item('AWB', 23, 659), item('在', 47, 659), item('AE', 57, 659), item('之后', 71, 659),
          item('AFM', 23, 640), item('在', 45, 640), item('AWB', 56, 640), item('之后', 79, 640),
        ],
      },
    ]);
    expect(markdown).toBe(
      'LSCM在BLC之后\n\nAEM、HSM在LSC之后\n\nAWB在AE之后\n\nAFM在AWB之后',
    );
  });

  it('rejoins a wrapped line that starts with continuation punctuation', () => {
    const markdown = pdfTextContentToMarkdown([
      { items: [item('Flowix 是一款桌面笔记应用', 50, 700), item('，支持全文搜索。', 50, 685)] },
    ]);
    expect(markdown).toBe('Flowix 是一款桌面笔记应用，支持全文搜索。');
  });

  it('keeps wrapped Latin paragraphs mergeable regardless of length', () => {
    const markdown = pdfTextContentToMarkdown([
      { items: [item('Flowix is a ', 50, 700), item('note app', 120, 700), item('for humans.', 50, 685)] },
    ]);
    expect(markdown).toBe('Flowix is a note app for humans.');
  });

  it('does not treat two aligned rows as a table', () => {
    const markdown = pdfTextContentToMarkdown([
      {
        items: [
          item('key', 23, 700), item('value one', 90, 700),
          item('key', 23, 685), item('value two', 90, 685),
        ],
      },
    ]);
    expect(markdown).not.toContain('| --- |');
    expect(markdown).toContain('keyvalue one keyvalue two');
  });

  it('does not treat misaligned middle columns as a table', () => {
    const markdown = pdfTextContentToMarkdown([
      {
        items: [
          item('first', 23, 700), item('cell', 100, 700), item('a', 200, 700),
          item('second', 23, 685), item('cell', 140, 685), item('b', 200, 685),
          item('third', 23, 670), item('cell', 80, 670), item('c', 200, 670),
        ],
      },
    ]);
    expect(markdown).not.toContain('| --- |');
  });

  it('escapes pipes inside table cells', () => {
    const markdown = pdfTextContentToMarkdown([
      {
        items: [
          item('a|b', 23, 700), item('1', 100, 700),
          item('c', 23, 685), item('2', 100, 685),
          item('d', 23, 670), item('3', 100, 670),
        ],
      },
    ]);
    expect(markdown).toContain('| a\\|b | 1 |');
  });

  it('wraps monospaced runs into fenced code blocks with rebuilt indentation', () => {
    const markdown = pdfTextContentToMarkdown([
      {
        items: [
          // proportional body: per-char width drifts (4.4 / 5.0 / 4.6)
          item('Intro text here ', 40, 700, 10, 67, 'prop'),
          item('more', 110, 700, 10, 20, 'prop'),
          item('Another body line', 40, 685, 10, 78.2, 'prop'),
          // monospaced code: per-char width fixed at 6
          item('name = hi1336', 44, 650, 10, 78, 'mono'),
          item('read_freq = pclk', 68, 635, 10, 96, 'mono'),
          item('video2 = slim_video', 44, 620, 10, 114, 'mono'),
          item('custom1 = custom1', 215, 620, 10, 102, 'mono'),
        ],
      },
    ]);
    expect(markdown).toBe(
      'Intro text here more Another body line\n\n'
      + '```\n'
      + 'name = hi1336\n'
      + '    read_freq = pclk\n'
      + 'video2 = slim_video          custom1 = custom1\n'
      + '```',
    );
  });

  it('keeps body text flowing when the whole page is monospaced', () => {
    const markdown = pdfTextContentToMarkdown([
      {
        items: [
          item('all mono page', 40, 700, 10, 72, 'mono'),
          item('second line', 40, 685, 10, 66, 'mono'),
          item('third line', 40, 670, 10, 60, 'mono'),
          item('fourth line', 40, 655, 10, 66, 'mono'),
        ],
      },
    ]);
    expect(markdown).not.toContain('```');
    expect(markdown).toBe('all mono page second line third line fourth line');
  });

  it('detects code runs even when body text has too few font samples', () => {
    const markdown = pdfTextContentToMarkdown([
      {
        items: [
          item('plain body line', 40, 700, 10, 66, 'prop'),
          item('mono one', 40, 685, 10, 48, 'mono'),
          item('mono two', 40, 670, 10, 48, 'mono'),
          item('mono three', 40, 655, 10, 60, 'mono'),
          item('plain body two', 40, 640, 10, 68, 'prop'),
        ],
      },
    ]);
    // 'prop' has only two samples (below the three-sample bar), so 'mono'
    // alone is detected and the three adjacent mono lines form one block.
    expect(markdown).toBe('plain body line\n\n```\nmono one\nmono two\nmono three\n```\n\nplain body two');
  });
});
