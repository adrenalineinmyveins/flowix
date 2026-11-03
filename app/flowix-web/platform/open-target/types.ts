export interface ExternalMarkdownOpenRequest {
  filePaths: string[];
}

export const FLOWIX_EXTERNAL_MARKDOWN_OPEN_EVENT = 'flowix:external-markdown-open';

export interface ExternalEnexImportRequest {
  filePaths: string[];
}

export const FLOWIX_EXTERNAL_ENEX_IMPORT_EVENT = 'flowix:external-enex-import';
