'use client';

import { useCallback, useEffect, useRef, useState } from 'react';
import { AlertCircle, ArrowDownToLine, ChevronLeft, ChevronRight, FolderOpen, LoaderCircle, ZoomIn, ZoomOut } from 'lucide-react';
import type { PDFDocumentProxy, RenderTask } from 'pdfjs-dist';
import { files, product } from '@platform/tauri/client';
import { Button } from '@shared/ui/button';
import { useI18n } from '@/lib/i18n';
import { toast } from '@/lib/toast';
import { loadPdfDocument, pdfDocumentToMarkdown } from './pdf-convert-core';

type PreviewState = 'loading' | 'ready' | 'failed';
type ConversionState = 'idle' | 'converting' | 'success' | 'failed';

const MIN_SCALE = 0.5;
const MAX_SCALE = 4;
const SCALE_STEP = 0.25;

function fileName(path: string): string {
  return path.split(/[\\/]/).pop() || path;
}

function errorText(error: unknown): string {
  if (error instanceof Error) return error.message;
  return String(error);
}

function userFacingError(
  error: unknown,
  fallback: string,
  t: ReturnType<typeof useI18n>['t'],
): string {
  const message = errorText(error);
  if (message.includes('PDF_TOO_LARGE')) return t('pdf.tooLarge');
  if (message.includes('PDF_FORBIDDEN')) return t('pdf.forbidden');
  if (message.includes('PDF_NAME_CONFLICT')) return t('pdf.nameConflict');
  return `${fallback}: ${message.replace(/^PDF_[A-Z_]+:\s*/, '')}`;
}

export function PdfPreview({ filePath, scopePath }: { filePath: string; scopePath: string | null }) {
  const { t } = useI18n();
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const pdfRef = useRef<PDFDocumentProxy | null>(null);
  const renderTaskRef = useRef<RenderTask | null>(null);
  const requestRef = useRef(0);
  const [previewState, setPreviewState] = useState<PreviewState>('loading');
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [pageNumber, setPageNumber] = useState(1);
  const [pageCount, setPageCount] = useState(0);
  const [scale, setScale] = useState(1);
  const [conversionState, setConversionState] = useState<ConversionState>('idle');
  const [conversionError, setConversionError] = useState<string | null>(null);
  const [generatedName, setGeneratedName] = useState<string | null>(null);

  // Load the document bytes over IPC, then parse them with pdf.js. The library
  // and its worker are dynamically imported so neither enters the startup
  // bundle; `?url` lets Vite emit the worker as a bundled asset.
  useEffect(() => {
    const requestId = ++requestRef.current;
    let cancelled = false;
    renderTaskRef.current?.cancel();
    renderTaskRef.current = null;
    const previousDoc = pdfRef.current;
    pdfRef.current = null;
    if (previousDoc) void previousDoc.destroy();
    setPreviewState('loading');
    setPreviewError(null);
    setPageNumber(1);
    setPageCount(0);
    setConversionState('idle');
    setConversionError(null);
    setGeneratedName(null);

    void (async () => {
      try {
        const encoded = await files.readPdfFile(filePath, scopePath ?? undefined);
        if (cancelled || requestRef.current !== requestId) return;
        const doc = await loadPdfDocument(encoded);
        if (cancelled || requestRef.current !== requestId) {
          void doc.destroy();
          return;
        }
        pdfRef.current = doc;
        setPageCount(doc.numPages);
        setPreviewState('ready');
      } catch (error) {
        if (cancelled || requestRef.current !== requestId) return;
        setPreviewError(userFacingError(error, t('pdf.previewFailed'), t));
        setPreviewState('failed');
      }
    })();

    return () => {
      cancelled = true;
      if (requestRef.current === requestId) requestRef.current += 1;
    };
  }, [filePath, scopePath, t]);

  useEffect(() => () => {
    renderTaskRef.current?.cancel();
    renderTaskRef.current = null;
    const doc = pdfRef.current;
    pdfRef.current = null;
    if (doc) void doc.destroy();
  }, []);

  // Paint the current page whenever the document, page, or zoom changes. The
  // canvas is sized in device pixels and shrunk back with CSS for crisp text
  // on HiDPI windows.
  useEffect(() => {
    const doc = pdfRef.current;
    const canvas = canvasRef.current;
    if (previewState !== 'ready' || !doc || !canvas) return;
    let cancelled = false;
    renderTaskRef.current?.cancel();
    renderTaskRef.current = null;
    void (async () => {
      try {
        const page = await doc.getPage(pageNumber);
        if (cancelled) return;
        const context = canvas.getContext('2d');
        if (!context) return;
        const devicePixelRatio = Math.min(window.devicePixelRatio || 1, 2);
        const viewport = page.getViewport({ scale: scale * devicePixelRatio });
        canvas.width = Math.floor(viewport.width);
        canvas.height = Math.floor(viewport.height);
        canvas.style.width = `${Math.floor(viewport.width / devicePixelRatio)}px`;
        canvas.style.height = `${Math.floor(viewport.height / devicePixelRatio)}px`;
        const task = page.render({ canvasContext: context, viewport });
        renderTaskRef.current = task;
        await task.promise;
      } catch {
        // Cancelling the previous render when switching pages or zooming
        // rejects its promise; the newest effect run repaints the canvas.
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [previewState, pageNumber, scale]);

  const goToPage = useCallback((next: number) => {
    if (pageCount < 1) return;
    setPageNumber(Math.min(Math.max(1, next), pageCount));
  }, [pageCount]);

  const changeScale = useCallback((delta: number) => {
    setScale((current) => Math.min(MAX_SCALE, Math.max(MIN_SCALE, Math.round((current + delta) * 100) / 100)));
  }, []);

  const convertToMarkdown = useCallback(async () => {
    const doc = pdfRef.current;
    if (!doc || !scopePath || conversionState === 'converting') return;
    const requestId = requestRef.current;
    setConversionState('converting');
    setConversionError(null);
    setGeneratedName(null);
    try {
      // The conversion kernel (page extraction + layout heuristics) is shared
      // with the batch converter.
      const markdown = await pdfDocumentToMarkdown(doc);
      if (!markdown) throw new Error('PDF_EMPTY');
      if (requestRef.current !== requestId) return;
      const outputPath = await files.createPdfMarkdown(filePath, scopePath, `${markdown}\n`);
      if (requestRef.current !== requestId) return;
      setGeneratedName(fileName(outputPath));
      setConversionState('success');
    } catch (error) {
      if (requestRef.current !== requestId) return;
      const message = errorText(error);
      setConversionError(message.includes('PDF_EMPTY')
        ? t('pdf.emptyResult')
        : userFacingError(error, t('pdf.conversionFailed'), t));
      setConversionState('failed');
    }
  }, [conversionState, filePath, scopePath, t]);

  return (
    <section className="flex h-full min-h-0 flex-col">
      <header className="absolute left-0 right-0 top-0 z-20 flex h-10 items-center justify-between gap-3 border-b border-[color-mix(in_oklch,var(--border)_70%,transparent)] bg-[color-mix(in_oklch,var(--card)_78%,transparent)] px-3 backdrop-blur-md">
        <div className="min-w-0">
          <h2 className="flex min-w-0 items-center gap-2 text-sm font-medium" title={filePath}>
            <span className="truncate">{fileName(filePath)}</span>
            <span className="shrink-0 rounded-md border border-[color-mix(in_oklch,var(--border)_70%,transparent)] px-1.5 py-0.5 text-[10px] font-normal leading-none text-[var(--muted-foreground)]">
              {t('pdf.previewTitle')}
            </span>
          </h2>
        </div>
        <div className="flex shrink-0 items-center gap-1.5">
          <div className="flex items-center gap-0.5" role="group" aria-label={t('pdf.pageNavigation')}>
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="h-7 w-7 rounded-lg p-0"
              aria-label={t('pdf.previousPage')}
              title={t('pdf.previousPage')}
              disabled={previewState !== 'ready' || pageNumber <= 1}
              onClick={() => goToPage(pageNumber - 1)}
            >
              <ChevronLeft className="h-3.5 w-3.5" aria-hidden="true" />
            </Button>
            <span className="min-w-12 text-center text-xs tabular-nums text-[var(--muted-foreground)]">
              {previewState === 'ready' ? t('pdf.page', { page: pageNumber, total: pageCount }) : '– / –'}
            </span>
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="h-7 w-7 rounded-lg p-0"
              aria-label={t('pdf.nextPage')}
              title={t('pdf.nextPage')}
              disabled={previewState !== 'ready' || pageNumber >= pageCount}
              onClick={() => goToPage(pageNumber + 1)}
            >
              <ChevronRight className="h-3.5 w-3.5" aria-hidden="true" />
            </Button>
          </div>
          <div className="flex items-center gap-0.5" role="group" aria-label={t('pdf.zoom')}>
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="h-7 w-7 rounded-lg p-0"
              aria-label={t('pdf.zoomOut')}
              title={t('pdf.zoomOut')}
              disabled={previewState !== 'ready' || scale <= MIN_SCALE}
              onClick={() => changeScale(-SCALE_STEP)}
            >
              <ZoomOut className="h-3.5 w-3.5" aria-hidden="true" />
            </Button>
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="h-7 w-7 rounded-lg p-0"
              aria-label={t('pdf.zoomIn')}
              title={t('pdf.zoomIn')}
              disabled={previewState !== 'ready' || scale >= MAX_SCALE}
              onClick={() => changeScale(SCALE_STEP)}
            >
              <ZoomIn className="h-3.5 w-3.5" aria-hidden="true" />
            </Button>
          </div>
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => void convertToMarkdown()}
            disabled={previewState !== 'ready' || conversionState === 'converting' || !scopePath}
            className="h-7 gap-1.5 rounded-lg px-2 text-xs"
          >
            {conversionState === 'converting'
              ? <LoaderCircle className="h-3.5 w-3.5 animate-spin" aria-hidden="true" />
              : <ArrowDownToLine className="h-3.5 w-3.5" aria-hidden="true" />}
            {conversionState === 'converting' ? t('pdf.converting') : t('pdf.convert')}
          </Button>
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="h-7 w-7 rounded-lg p-0"
            aria-label={t('document.file.openContainingFolder')}
            title={t('document.file.openContainingFolder')}
            onClick={() => {
              void product.revealInFileManager(filePath).catch(() => {
                toast.error(t('memo.fileTree.openFailed'));
              });
            }}
          >
            <FolderOpen className="h-3.5 w-3.5" aria-hidden="true" />
          </Button>
        </div>
      </header>

      {previewState === 'loading' && (
        <div className="flex min-h-0 flex-1 items-center justify-center gap-2 text-sm text-[var(--muted-foreground)]" role="status">
          <LoaderCircle className="h-4 w-4 animate-spin" aria-hidden="true" />
          {t('pdf.loading')}
        </div>
      )}
      {previewState === 'failed' && (
        <div className="flex min-h-0 flex-1 items-center justify-center p-6">
          <div className="max-w-lg text-center text-sm text-[var(--destructive)]" role="alert">
            <AlertCircle className="mx-auto mb-2 h-5 w-5" aria-hidden="true" />
            {previewError}
          </div>
        </div>
      )}
      {conversionState === 'success' && generatedName && (
        <p className="shrink-0 border-b border-[var(--border)] px-4 py-2 text-xs text-[var(--muted-foreground)]" role="status">
          {t('pdf.generated', { name: generatedName })}
        </p>
      )}
      {conversionState === 'failed' && conversionError && (
        <p className="shrink-0 border-b border-[var(--border)] px-4 py-2 text-xs text-[var(--destructive)]" role="alert">
          {conversionError}
        </p>
      )}
      <div
        className="flex min-h-0 flex-1 items-start justify-center overflow-auto bg-[var(--agent-bg,var(--document-bg))] p-6"
        style={{ display: previewState === 'ready' ? undefined : 'none' }}
      >
        <canvas
          ref={canvasRef}
          className="block rounded-lg bg-white shadow-sm"
          aria-label={fileName(filePath)}
        />
      </div>
    </section>
  );
}
