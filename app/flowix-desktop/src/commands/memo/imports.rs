//! ENEX（Evernote 交换格式）批量导入。
//!
//! 一次导入整个导出文件：解析（flowix-core）→ 附件落盘笔记本 `attachments/`
//! → ENML 转 Markdown（en-media 解析为 asset:// 链接）→ 按标题建笔记到
//! `<默认新建目录>/<enex 文件名>/` 子目录。单条笔记失败不中断整批。

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager, WebviewWindow};

use flowix_core::memo_file::{
    atomic_create_bytes, base_filename, enml_to_markdown, parse_enex, replace_frontmatter_tags,
    sanitize_filename_component, EnexNote, MemoFile, ResolvedMedia,
};
use flowix_core::NoteService;

use crate::app::state::AppState;
use crate::commands::file::docx_asset_markdown_url;
use crate::commands::helpers::{can_access_document_path, start_security_bookmark_access};
use crate::lock_utils::read_lock;
use crate::watcher::runtime::mark_self_write_for;

use super::creates::default_parent_relative_path;

/// 整个 enex 文件上限（含 base64 内嵌资源）。
const MAX_ENEX_FILE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ENEX_ATTACHMENTS_PER_NOTE: usize = 200;

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnexImportResult {
    pub notebook_id: String,
    /// 笔记写入的父目录（笔记本内相对路径）。
    pub parent_relative_path: String,
    pub imported_notes: usize,
    pub failed_notes: usize,
    pub attachments: usize,
}

fn authorized_enex_path(
    file_path: &str,
    window_label: &str,
    state: &tauri::State<AppState>,
) -> Result<PathBuf, String> {
    let requested = Path::new(file_path);
    if !requested.is_absolute() {
        return Err("ENEX_PATH_INVALID: ENEX path must be absolute".to_string());
    }
    let path = dunce::canonicalize(requested)
        .map_err(|error| format!("ENEX_UNAVAILABLE: could not open file: {error}"))?;
    if !path.is_file() {
        return Err("ENEX_UNAVAILABLE: path is not a file".to_string());
    }
    if !path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("enex"))
    {
        return Err("ENEX_UNSUPPORTED: only .enex files are supported".to_string());
    }
    if !can_access_document_path(&path, window_label, state) {
        return Err("ENEX_FORBIDDEN: file is outside its authorized space".to_string());
    }
    start_security_bookmark_access(state, &path);
    Ok(path)
}

fn attachment_file_name(note: &EnexNote, note_index: usize, res_index: usize) -> String {
    if let Some(name) = note.resources
        .get(res_index)
        .and_then(|resource| resource.file_name.as_deref())
    {
        let sanitized = sanitize_filename_component(name);
        if !sanitized.trim_matches(|c: char| c == '.' || c.is_whitespace()).is_empty() {
            return sanitized;
        }
    }
    let mime = note.resources.get(res_index).map(|r| r.mime.as_str()).unwrap_or("");
    let extension = mime_guess::get_mime_extensions_str(mime)
        .and_then(|extensions| extensions.first().copied())
        .unwrap_or("bin");
    format!("enex-asset-{:03}-{:03}.{extension}", note_index + 1, res_index + 1)
}

/// 落盘附件：同名同内容复用；同名不同内容追加 -1/-2 后缀（与 DOCX / 移动端一致）。
fn write_enex_attachment(directory: &Path, file_name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    let (stem, extension) = file_name
        .rsplit_once('.')
        .map(|(stem, extension)| (stem.to_string(), Some(extension.to_string())))
        .unwrap_or((file_name.to_string(), None));
    let mut suffix = 0u32;
    loop {
        let candidate = match (suffix, &extension) {
            (0, Some(extension)) => format!("{stem}.{extension}"),
            (0, None) => stem.clone(),
            (attempt, Some(extension)) => format!("{stem}-{attempt}.{extension}"),
            (attempt, None) => format!("{stem}-{attempt}"),
        };
        let target = directory.join(&candidate);
        match atomic_create_bytes(&target, bytes) {
            Ok(()) => return Ok(target),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                if fs::read(&target).map(|existing| existing == bytes).unwrap_or(false) {
                    return Ok(target);
                }
                suffix += 1;
                if suffix > 9999 {
                    return Err("ENEX_ATTACHMENT_CONFLICT: could not find an unused attachment filename".to_string());
                }
            }
            Err(error) => return Err(format!("ENEX_ATTACHMENT_WRITE_FAILED: {error}")),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn import_single_note(
    app: &AppHandle,
    memo_file: &MemoFile,
    notebook_id: &str,
    parent_relative: &str,
    note: &EnexNote,
    note_index: usize,
    attachment_directory: &Path,
) -> Result<usize, String> {
    // 1. 附件落盘，记录 hash → asset 链接映射。
    let mut media: HashMap<String, ResolvedMedia> = HashMap::new();
    let mut written = 0usize;
    let attachment_count = note.resources.len().min(MAX_ENEX_ATTACHMENTS_PER_NOTE);
    for res_index in 0..attachment_count {
        let resource = &note.resources[res_index];
        if resource.bytes.is_empty() || resource.hash_hex.is_empty() {
            continue;
        }
        let name = attachment_file_name(note, note_index, res_index);
        let target = write_enex_attachment(attachment_directory, &name, &resource.bytes)?;
        let display_name = target
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| name.clone());
        media.insert(
            resource.hash_hex.clone(),
            ResolvedMedia {
                url: docx_asset_markdown_url(&target),
                alt: display_name,
                is_image: resource.mime.starts_with("image/"),
            },
        );
        written += 1;
    }

    // 2. ENML → Markdown。
    let markdown = enml_to_markdown(&note.content_xml, &|hash: &str, _kind: &str| {
        media.get(hash).cloned()
    })?;

    // 3. Evernote 标签写入 frontmatter。
    let body = if note.tags.is_empty() {
        markdown
    } else {
        replace_frontmatter_tags(&markdown, &note.tags)
            .map_err(|error| format!("ENEX_TAGS_INVALID: {error}"))?
    };

    // 4. 建笔记（create_note_by_path 负责文件名清洗与 -N 去重）。
    let title = if note.title.trim().is_empty() {
        "Untitled"
    } else {
        note.title.trim()
    };
    let created = NoteService::new(memo_file)
        .create(notebook_id, Some(parent_relative), title, &body)
        .map_err(|error| format!("ENEX_NOTE_CREATE_FAILED: {error}"))?;
    mark_self_write_for(app, &created.path);
    Ok(written)
}

#[tauri::command]
pub async fn import_enex_file(
    window: WebviewWindow,
    file_path: String,
    notebook_id: String,
    app: AppHandle,
) -> Result<EnexImportResult, String> {
    let window_label = window.label().to_owned();
    crate::document_io::run("import_enex_file", move || {
        let state = app.state::<AppState>();
        let source = authorized_enex_path(&file_path, &window_label, &state)?;
        let metadata = fs::metadata(&source).map_err(|error| error.to_string())?;
        if metadata.len() > MAX_ENEX_FILE_BYTES {
            return Err(format!(
                "ENEX_TOO_LARGE: file is larger than {} MB",
                MAX_ENEX_FILE_BYTES / (1024 * 1024)
            ));
        }
        let bytes = fs::read(&source).map_err(|error| format!("ENEX_READ_FAILED: {error}"))?;
        let xml = String::from_utf8(bytes)
            .map_err(|_| "ENEX_UTF8_INVALID: export file is not valid UTF-8".to_string())?;
        let notes = parse_enex(&xml)?;
        if notes.is_empty() {
            return Err("ENEX_EMPTY: no notes found in the export".to_string());
        }

        let memo_file = read_lock(&state.memo_file, "memo_file");
        let notebook = memo_file
            .get_notebook_config_by_id(&notebook_id)
            .ok_or_else(|| "NOTEBOOK_NOT_FOUND".to_string())?;
        let notebook_root = PathBuf::from(&notebook.path);

        // 目标子目录：<默认新建目录>/<enex 文件名 stem>。
        let stem = source
            .file_stem()
            .and_then(|stem| stem.to_str())
            .filter(|stem| !stem.trim().is_empty())
            .unwrap_or("ENEX Import");
        let group = base_filename(stem);
        let default_parent = default_parent_relative_path(&memo_file, &notebook_id)
            .map_err(|error| format!("NOTEBOOK_PARENT_INVALID: {error}"))?;
        let parent_relative = match default_parent {
            Some(parent) if !parent.is_empty() => format!("{parent}/{group}"),
            _ => group,
        };
        let parent_path = notebook_root.join(parent_relative.replace('\\', "/"));
        fs::create_dir_all(&parent_path)
            .map_err(|error| format!("ENEX_DIR_CREATE_FAILED: {error}"))?;

        let attachment_directory = notebook_root.join("attachments");
        fs::create_dir_all(&attachment_directory)
            .map_err(|error| format!("ENEX_ATTACHMENT_DIR_FAILED: {error}"))?;
        start_security_bookmark_access(&state, &attachment_directory);

        let mut imported_notes = 0usize;
        let mut failed_notes = 0usize;
        let mut attachments = 0usize;
        for (note_index, note) in notes.iter().enumerate() {
            match import_single_note(
                &app,
                &memo_file,
                &notebook_id,
                &parent_relative,
                note,
                note_index,
                &attachment_directory,
            ) {
                Ok(written) => {
                    imported_notes += 1;
                    attachments += written;
                }
                Err(error) => {
                    tracing::warn!(notebook_id, note_index, title = %note.title, "ENEX note import failed: {error}");
                    failed_notes += 1;
                }
            }
        }

        Ok(EnexImportResult {
            notebook_id,
            parent_relative_path: parent_relative,
            imported_notes,
            failed_notes,
            attachments,
        })
    })
    .await?
}
