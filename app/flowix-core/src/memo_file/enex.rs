//! ENEX（Evernote 交换格式）导入解析。
//!
//! 纯解析与转换，无 IO：`parse_enex` 把导出文件拆成笔记列表（正文为
//! ENML 的原始 XML，资源已 base64 解码并算好 MD5 哈希）；`enml_to_markdown`
//! 把 ENML 正文转成 Markdown，`en-media` 由调用方通过闭包解析为最终链接。
//! 覆盖 Evernote / 印象笔记 / 有道云笔记导出的 enex。

use std::collections::HashMap;

use base64::Engine as _;
use md5::{Digest, Md5};
use quick_xml::events::Event;
use quick_xml::Reader;

/// 单个 enex 文件最多导入的笔记数。
pub const MAX_ENEX_NOTES: usize = 5_000;
/// 单个资源上限（与 DOCX 导入图片单张上限一致）。
pub const MAX_ENEX_RESOURCE_BYTES: usize = 50 * 1024 * 1024;

/// enex 内嵌资源（图片/附件），数据已解码。
#[derive(Debug, Clone, PartialEq)]
pub struct EnexResource {
    /// 资源字节内容的 MD5 十六进制（小写），对应正文中 en-media 的 hash。
    pub hash_hex: String,
    pub mime: String,
    pub file_name: Option<String>,
    pub bytes: Vec<u8>,
}

/// enex 中的一条笔记。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EnexNote {
    pub title: String,
    /// `<content>` CDATA 里的原始 ENML XML（含 en-note 根）。
    pub content_xml: String,
    pub tags: Vec<String>,
    pub resources: Vec<EnexResource>,
}

/// en-media 解析结果：由调用方（知道落盘位置）提供最终 Markdown 链接。
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedMedia {
    /// Markdown 链接目标（如 asset://localhost/... 或相对路径）。
    pub url: String,
    /// 链接显示名（附件文件名）。
    pub alt: String,
    /// 图片类资源用 `![]()`，否则用 `[]()`。
    pub is_image: bool,
}

/// 文本收集目标：Start 进入目标路径时设置，End 匹配时消费。
#[derive(Clone, Copy, PartialEq)]
enum TextTarget {
    Title,
    Content,
    Tag,
    ResourceData,
    ResourceMime,
    ResourceFileName,
}

impl TextTarget {
    fn path(self) -> &'static [&'static str] {
        match self {
            TextTarget::Title => &["note", "title"],
            TextTarget::Content => &["note", "content"],
            TextTarget::Tag => &["note", "tag"],
            TextTarget::ResourceData => &["note", "resource", "data"],
            TextTarget::ResourceMime => &["note", "resource", "mime"],
            TextTarget::ResourceFileName => {
                &["note", "resource", "resource-attributes", "file-name"]
            }
        }
    }
}

/// 后缀匹配：en-export 根元素（或导出器加的包装层）不参与判断。
fn path_is(path: &[String], expected: &[&str]) -> bool {
    path.len() >= expected.len()
        && path
            .iter()
            .rev()
            .zip(expected.iter().rev())
            .all(|(segment, wanted)| segment == wanted)
}

fn target_for_path(path: &[String]) -> Option<TextTarget> {
    [
        TextTarget::Title,
        TextTarget::Content,
        TextTarget::Tag,
        TextTarget::ResourceData,
        TextTarget::ResourceMime,
        TextTarget::ResourceFileName,
    ]
    .into_iter()
    .find(|target| path_is(path, target.path()))
}

/// 解析 enex 文档为笔记列表。
///
/// 容错策略：字段级问题（空资源、坏 base64）跳过该字段；结构损坏返回 Err。
pub fn parse_enex(xml: &str) -> Result<Vec<EnexNote>, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);

    let mut notes: Vec<EnexNote> = Vec::new();
    // 元素路径栈，如 ["note", "resource", "data"]。
    let mut path: Vec<String> = Vec::new();
    let mut current: Option<EnexNote> = None;
    let mut current_resource: Option<EnexResource> = None;
    let mut text_target: Option<TextTarget> = None;
    let mut buffer = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = local_name(e.name().as_ref());
                path.push(name.clone());
                if path_is(&path, &["note"]) {
                    current = Some(EnexNote::default());
                    current_resource = None;
                } else if path_is(&path, &["note", "resource"]) {
                    current_resource = Some(EnexResource {
                        hash_hex: String::new(),
                        mime: String::new(),
                        file_name: None,
                        bytes: Vec::new(),
                    });
                } else if let Some(target) = target_for_path(&path) {
                    text_target = Some(target);
                    buffer.clear();
                }
            }
            Ok(Event::Empty(e)) => {
                // 自闭合元素不产生文本，重置收集目标即可。
                let name = local_name(e.name().as_ref());
                if name == "resource" {
                    current_resource = None;
                }
                text_target = None;
            }
            Ok(Event::Text(e)) => {
                if text_target.is_some() {
                    buffer.push_str(
                        &e.unescape()
                            .map_err(|error| format!("ENEX_TEXT_INVALID: {error}"))?,
                    );
                }
            }
            Ok(Event::CData(e)) => {
                if text_target.is_some() {
                    buffer.push_str(&String::from_utf8_lossy(e.as_ref()));
                }
            }
            Ok(Event::End(e)) => {
                let name = local_name(e.name().as_ref());
                if let Some(target) = text_target {
                    if path_is(&path, target.path()) {
                        apply_text(
                            target,
                            &buffer,
                            current.as_mut(),
                            current_resource.as_mut(),
                        )?;
                        text_target = None;
                    }
                }
                if name == "resource" {
                    if let (Some(note), Some(resource)) = (current.as_mut(), current_resource.take())
                    {
                        if !resource.bytes.is_empty() {
                            note.resources.push(resource);
                        }
                    }
                } else if name == "note" {
                    if let Some(mut note) = current.take() {
                        if note.title.trim().is_empty() && note.content_xml.trim().is_empty() {
                            // 完全空的笔记跳过。
                        } else if notes.len() >= MAX_ENEX_NOTES {
                            return Err("ENEX_NOTE_LIMIT: too many notes in one export".to_string());
                        } else {
                            note.title = note.title.trim().to_string();
                            notes.push(note);
                        }
                    }
                }
                path.pop();
            }
            Ok(Event::Eof) => break,
            Ok(Event::Decl(_) | Event::DocType(_) | Event::Comment(_) | Event::PI(_)) => {}
            Err(error) => return Err(format!("ENEX_XML_INVALID: {error}")),
        }
    }

    Ok(notes)
}

fn apply_text(
    target: TextTarget,
    text: &str,
    note: Option<&mut EnexNote>,
    resource: Option<&mut EnexResource>,
) -> Result<(), String> {
    match target {
        TextTarget::Title => {
            if let Some(note) = note {
                note.title.push_str(text);
            }
        }
        TextTarget::Content => {
            if let Some(note) = note {
                note.content_xml.push_str(text);
            }
        }
        TextTarget::Tag => {
            if let Some(note) = note {
                let tag = text.trim();
                if !tag.is_empty() && !note.tags.iter().any(|existing| existing == tag) {
                    note.tags.push(tag.to_string());
                }
            }
        }
        TextTarget::ResourceMime => {
            if let Some(resource) = resource {
                resource.mime = text.trim().to_ascii_lowercase();
            }
        }
        TextTarget::ResourceFileName => {
            if let Some(resource) = resource {
                let name = text.trim();
                if !name.is_empty() {
                    resource.file_name = Some(name.to_string());
                }
            }
        }
        TextTarget::ResourceData => {
            if let Some(resource) = resource {
                let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
                if compact.is_empty() {
                    return Ok(());
                }
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(compact.as_bytes())
                    .map_err(|_| "ENEX_RESOURCE_INVALID: resource data is not valid base64".to_string())?;
                if bytes.len() > MAX_ENEX_RESOURCE_BYTES {
                    return Err("ENEX_RESOURCE_TOO_LARGE: a resource is larger than 50 MB".to_string());
                }
                let mut hasher = Md5::new();
                hasher.update(&bytes);
                resource.hash_hex = hex_lower(&hasher.finalize());
                resource.bytes = bytes;
            }
        }
    }
    Ok(())
}

fn hex_lower(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn local_name(raw: &[u8]) -> String {
    let name = String::from_utf8_lossy(raw).to_string();
    // 带命名空间前缀时取本地名（enex 一般没有，防御性处理）。
    match name.rsplit_once(':') {
        Some((_, local)) => local.to_ascii_lowercase(),
        None => name.to_ascii_lowercase(),
    }
}

// ── ENML → Markdown ───────────────────────────────────────────────────────────

/// 简化 DOM：只为转换服务，不追求保真。
enum Node {
    Text(String),
    El(El),
}

struct El {
    name: String,
    attrs: HashMap<String, String>,
    children: Vec<Node>,
}

impl El {
    fn attr(&self, key: &str) -> Option<&str> {
        self.attrs.get(key).map(String::as_str)
    }
}

fn parse_dom(xml: &str) -> Result<Vec<Node>, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut stack: Vec<El> = Vec::new();
    let mut roots: Vec<Node> = Vec::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = local_name(e.name().as_ref());
                let mut attrs = HashMap::new();
                for attr in e.attributes().with_checks(false) {
                    let attr = attr.map_err(|error| format!("ENEX_CONTENT_INVALID: {error}"))?;
                    let key = local_name(attr.key.as_ref());
                    let value = attr
                        .unescape_value()
                        .map_err(|error| format!("ENEX_CONTENT_INVALID: {error}"))?;
                    attrs.insert(key, value.to_string());
                }
                stack.push(El { name, attrs, children: Vec::new() });
            }
            Ok(Event::Empty(e)) => {
                let name = local_name(e.name().as_ref());
                let mut attrs = HashMap::new();
                for attr in e.attributes().with_checks(false) {
                    let attr = attr.map_err(|error| format!("ENEX_CONTENT_INVALID: {error}"))?;
                    let key = local_name(attr.key.as_ref());
                    let value = attr
                        .unescape_value()
                        .map_err(|error| format!("ENEX_CONTENT_INVALID: {error}"))?;
                    attrs.insert(key, value.to_string());
                }
                let el = El { name, attrs, children: Vec::new() };
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(Node::El(el));
                } else {
                    roots.push(Node::El(el));
                }
            }
            Ok(Event::Text(e)) => {
                let text = e
                    .unescape()
                    .map_err(|error| format!("ENEX_CONTENT_INVALID: {error}"))?
                    .to_string();
                if !text.is_empty() {
                    if let Some(parent) = stack.last_mut() {
                        parent.children.push(Node::Text(text));
                    } else {
                        roots.push(Node::Text(text));
                    }
                }
            }
            Ok(Event::CData(e)) => {
                let text = String::from_utf8_lossy(e.as_ref()).to_string();
                if !text.is_empty() {
                    if let Some(parent) = stack.last_mut() {
                        parent.children.push(Node::Text(text));
                    } else {
                        roots.push(Node::Text(text));
                    }
                }
            }
            Ok(Event::End(_)) => {
                if let Some(el) = stack.pop() {
                    if let Some(parent) = stack.last_mut() {
                        parent.children.push(Node::El(el));
                    } else {
                        roots.push(Node::El(el));
                    }
                }
            }
            Ok(Event::Eof) => break,
            Ok(
                Event::Decl(_) | Event::DocType(_) | Event::Comment(_) | Event::PI(_),
            ) => {}
            Err(error) => return Err(format!("ENEX_CONTENT_INVALID: {error}")),
        }
    }

    // 根应为 en-note；退化导出可能缺根或多根，一律按子节点平铺处理。
    let mut flattened: Vec<Node> = Vec::new();
    let queue: Vec<Node> = roots;
    for node in queue {
        match node {
            Node::El(el) if el.name == "en-note" => flattened.extend(el.children),
            other => flattened.push(other),
        }
    }
    Ok(flattened)
}

/// en-media 解析闭包：入参为 (en-media 的 hash 属性, type 属性)。
pub fn enml_to_markdown(
    enml: &str,
    resolve_media: &dyn Fn(&str, &str) -> Option<ResolvedMedia>,
) -> Result<String, String> {
    let roots = parse_dom(enml)?;
    let mut out = convert_blocks(&roots, resolve_media);
    // 压缩 3 个以上连续空行为 1 个空行，收尾规整。
    while out.contains("\n\n\n") {
        out = out.replace("\n\n\n", "\n\n");
    }
    Ok(out.trim().to_string())
}

fn is_block_tag(name: &str) -> bool {
    matches!(
        name,
        "h1" | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "p"
            | "div"
            | "ul"
            | "ol"
            | "li"
            | "blockquote"
            | "pre"
            | "table"
            | "hr"
            | "en-todo"
            | "en-note"
    )
}

fn has_block_child(nodes: &[Node]) -> bool {
    nodes.iter().any(|node| match node {
        Node::El(el) => is_block_tag(&el.name),
        Node::Text(_) => false,
    })
}

/// 块级转换：返回以 "\n\n" 分段的 Markdown。
fn convert_blocks(nodes: &[Node], resolve: &dyn Fn(&str, &str) -> Option<ResolvedMedia>) -> String {
    let mut out = String::new();
    let mut inline_buffer = String::new();
    let flush = |out: &mut String, inline_buffer: &mut String| {
        let text = inline_buffer.trim();
        if !text.is_empty() {
            out.push_str(text);
            out.push_str("\n\n");
        }
        inline_buffer.clear();
    };

    for node in nodes {
        let Node::El(el) = node else {
            inline_buffer.push_str(text_of(node));
            continue;
        };

        match el.name.as_str() {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                flush(&mut out, &mut inline_buffer);
                let level = el.name[1..].parse::<usize>().unwrap_or(1);
                let text = convert_inline_children(&el.children, resolve).trim().to_string();
                if !text.is_empty() {
                    out.push_str(&format!("{} {}\n\n", "#".repeat(level), text));
                }
            }
            "ul" | "ol" => {
                flush(&mut out, &mut inline_buffer);
                out.push_str(&render_list(el, 0, resolve));
            }
            "en-todo" => {
                flush(&mut out, &mut inline_buffer);
                let checked = el.attr("checked").is_some_and(|value| value == "true");
                let text = convert_inline_children(&el.children, resolve).trim().to_string();
                out.push_str(if checked { "- [x] " } else { "- [ ] " });
                out.push_str(&text);
                out.push('\n');
            }
            "blockquote" => {
                flush(&mut out, &mut inline_buffer);
                let inner = convert_blocks(&el.children, resolve);
                if !inner.trim().is_empty() {
                    for line in inner.trim_end().lines() {
                        if line.trim().is_empty() {
                            out.push_str(">\n");
                        } else {
                            out.push_str(&format!("> {line}\n"));
                        }
                    }
                    out.push('\n');
                }
            }
            "pre" => {
                flush(&mut out, &mut inline_buffer);
                let text = raw_text(&el.children);
                if !text.trim().is_empty() {
                    let fence_len = text.matches("```").map(|m| m.len()).max().unwrap_or(3).max(3);
                    let fence = "`".repeat(fence_len);
                    out.push_str(&format!("{fence}\n{}\n{fence}\n\n", text.trim_end()));
                }
            }
            "table" => {
                flush(&mut out, &mut inline_buffer);
                out.push_str(&render_table(el, resolve));
            }
            "hr" => {
                flush(&mut out, &mut inline_buffer);
                out.push_str("---\n\n");
            }
            "p" | "div" => {
                if has_block_child(&el.children) {
                    // 块级嵌套：作为透明容器递归。
                    flush(&mut out, &mut inline_buffer);
                    out.push_str(&convert_blocks(&el.children, resolve));
                } else {
                    let text = convert_inline_children(&el.children, resolve);
                    if !inline_buffer.is_empty() {
                        inline_buffer.push('\n');
                    }
                    inline_buffer.push_str(&text);
                }
            }
            // 未知元素按透明容器处理（en-note、span、字体标签等）。
            _ => {
                if is_block_tag(&el.name) || has_block_child(&el.children) {
                    flush(&mut out, &mut inline_buffer);
                    out.push_str(&convert_blocks(&el.children, resolve));
                } else {
                    inline_buffer.push_str(&convert_inline(el, resolve));
                }
            }
        }
    }
    flush(&mut out, &mut inline_buffer);
    out
}

fn render_list(list: &El, indent: usize, resolve: &dyn Fn(&str, &str) -> Option<ResolvedMedia>) -> String {
    let ordered = list.name == "ol";
    let mut out = String::new();
    let mut index = 0usize;
    for node in &list.children {
        let Node::El(el) = node else { continue };
        if el.name != "li" {
            continue;
        }
        index += 1;
        // li 的直接内容：内联节点；嵌套 ul/ol 作为子列表。
        let mut inline_parts: Vec<String> = Vec::new();
        let mut sub_lists: Vec<&El> = Vec::new();
        for child in &el.children {
            match child {
                Node::Text(text) => inline_parts.push(text.clone()),
                Node::El(child_el) if child_el.name == "ul" || child_el.name == "ol" => {
                    sub_lists.push(child_el)
                }
                Node::El(child_el) => {
                    if is_block_tag(&child_el.name) || has_block_child(&child_el.children) {
                        inline_parts.push(convert_blocks(
                            std::slice::from_ref(child),
                            resolve,
                        ));
                    } else {
                        inline_parts.push(convert_inline(child_el, resolve));
                    }
                }
            }
        }
        let text: String = merge_inline(&inline_parts);
        let marker = if ordered { format!("{index}. ") } else { "- ".to_string() };
        let pad = " ".repeat(indent);
        for (i, line) in text.lines().enumerate() {
            if i == 0 {
                out.push_str(&format!("{pad}{marker}{line}\n"));
            } else {
                out.push_str(&format!("{pad}{}  {line}\n", " ".repeat(marker.len())));
            }
        }
        for sub in sub_lists {
            out.push_str(&render_list(sub, indent + 2, resolve));
        }
    }
    out.push('\n');
    out
}

/// 合并 li 直接内容里的内联片段（块片段降为内联文本）。
fn merge_inline(parts: &[String]) -> String {
    let mut text = String::new();
    for part in parts {
        if !text.is_empty() && !text.ends_with('\n') && !text.ends_with(' ') {
            text.push(' ');
        }
        text.push_str(part.trim());
    }
    text.trim().to_string()
}

fn render_table(table: &El, resolve: &dyn Fn(&str, &str) -> Option<ResolvedMedia>) -> String {
    let mut rows: Vec<Vec<String>> = Vec::new();
    for node in &table.children {
        let Node::El(row) = node else { continue };
        if row.name != "tr" {
            continue;
        }
        let mut cells: Vec<String> = Vec::new();
        collect_row_cells(row, resolve, &mut cells);
        if !cells.is_empty() {
            rows.push(cells);
        }
    }
    if rows.is_empty() {
        return String::new();
    }
    // tbody/thead 包裹时行已平铺；补齐列数。
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    if columns == 0 {
        return String::new();
    }
    let mut out = String::new();
    for (row_index, row) in rows.iter().enumerate() {
        let mut cells = row.clone();
        cells.resize(columns, String::new());
        let line = cells
            .iter()
            .map(|cell| cell.replace('|', "\\|").replace('\n', " "))
            .collect::<Vec<_>>()
            .join(" | ");
        out.push_str(&format!("| {line} |\n"));
        if row_index == 0 {
            let separator: Vec<&str> = (0..columns).map(|_| "---").collect();
            out.push_str(&format!("| {} |\n", separator.join(" | ")));
        }
    }
    out.push('\n');
    out
}

/// tr 可能直接含 td/th，也可能经 tbody/thead 间接持有。
fn collect_row_cells(row: &El, resolve: &dyn Fn(&str, &str) -> Option<ResolvedMedia>, cells: &mut Vec<String>) {
    for node in &row.children {
        match node {
            Node::El(el) if el.name == "td" || el.name == "th" => {
                cells.push(convert_inline_children(&el.children, resolve).trim().to_string());
            }
            Node::El(el) if el.name == "tr" => collect_row_cells(el, resolve, cells),
            Node::El(el) if el.name == "tbody" || el.name == "thead" => {
                collect_row_cells(el, resolve, cells)
            }
            _ => {}
        }
    }
}

fn convert_inline_children(
    nodes: &[Node],
    resolve: &dyn Fn(&str, &str) -> Option<ResolvedMedia>,
) -> String {
    let mut out = String::new();
    for node in nodes {
        out.push_str(&match node {
            Node::Text(text) => text.clone(),
            Node::El(el) => convert_inline(el, resolve),
        });
    }
    out
}

fn convert_inline(el: &El, resolve: &dyn Fn(&str, &str) -> Option<ResolvedMedia>) -> String {
    let inner = || convert_inline_children(&el.children, resolve);
    match el.name.as_str() {
        "b" | "strong" => wrap_bold(&inner()),
        "i" | "em" => wrap_emphasis(&inner()),
        "s" | "strike" | "strikethrough" | "del" => wrap_strikethrough(&inner()),
        "u" => inner(),
        "code" => {
            let owned = inner();
            let text = owned.trim();
            if text.is_empty() {
                String::new()
            } else {
                format!("`{text}`")
            }
        }
        "br" => "\n".to_string(),
        "a" => match el.attr("href") {
            Some(href) if !href.trim().is_empty() => {
                let owned = inner();
                let text = owned.trim();
                let label = if text.is_empty() { href } else { text };
                let url = escape_markdown_url(href.trim());
                format!("[{label}]({url})")
            }
            _ => inner(),
        },
        "img" => {
            let src = el.attr("src").map(str::trim).unwrap_or("");
            if src.is_empty() {
                return String::new();
            }
            let alt = el.attr("alt").unwrap_or("image");
            format!("![{alt}]({})", escape_markdown_url(src))
        }
        "en-media" => {
            let hash = el.attr("hash").unwrap_or("");
            let media_type = el.attr("type").unwrap_or("");
            if hash.is_empty() {
                return String::new();
            }
            match resolve_media_ref(resolve, hash, media_type) {
                Some(media) if media.is_image => {
                    format!("![{}]({})", media.alt, escape_markdown_url(&media.url))
                }
                Some(media) => {
                    format!("[{}]({})", media.alt, escape_markdown_url(&media.url))
                }
                None => "*[attachment missing]*".to_string(),
            }
        }
        "en-crypt" => "*[encrypted content]*".to_string(),
        // span / font / 其它未知内联元素：透明传递。
        _ => inner(),
    }
}

fn resolve_media_ref(
    resolve: &dyn Fn(&str, &str) -> Option<ResolvedMedia>,
    hash: &str,
    media_type: &str,
) -> Option<ResolvedMedia> {
    resolve(&hash.to_ascii_lowercase(), media_type)
}

fn wrap_bold(inner: &str) -> String {
    let text = inner.trim();
    if text.is_empty() {
        String::new()
    } else {
        format!("**{text}**")
    }
}

fn wrap_emphasis(inner: &str) -> String {
    let text = inner.trim();
    if text.is_empty() {
        String::new()
    } else {
        format!("*{text}*")
    }
}

fn wrap_strikethrough(inner: &str) -> String {
    let text = inner.trim();
    if text.is_empty() {
        String::new()
    } else {
        format!("~~{text}~~")
    }
}

fn escape_markdown_url(url: &str) -> String {
    url.replace(' ', "%20").replace('(', "%28").replace(')', "%29")
}

fn text_of(node: &Node) -> &str {
    match node {
        Node::Text(text) => text,
        Node::El(_) => "",
    }
}

/// pre 的原始文本：保留后代文本原样（不解释标签）。
fn raw_text(nodes: &[Node]) -> String {
    let mut out = String::new();
    for node in nodes {
        match node {
            Node::Text(text) => out.push_str(text),
            Node::El(el) => out.push_str(&raw_text(&el.children)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_ENEX: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE en-export SYSTEM "http://xml.evernote.com/pub/evernote.dtd3">
<en-export export-date="20230501T000000Z" application="Evernote" version="10">
  <note>
    <title>读书笔记</title>
    <content><![CDATA[<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE ENNote SYSTEM "http://xml.evernote.com/pub/evernote.dtd2">
<en-note>
  <h1>第一章</h1>
  <div>核心 <b>观点</b> 与 <i>例证</i>，参考 <a href="https://example.com/a b">资料</a>。</div>
  <en-todo checked="true">已完成事项</en-todo>
  <en-todo>待办事项</en-todo>
  <ul><li>第一项</li><li>第二项<ol><li>嵌套甲</li><li>嵌套乙</li></ol></li></ul>
  <blockquote>引用内容</blockquote>
  <pre>fn main() {
    println!("hi");
}</pre>
  <table><tr><th>列A</th><th>列B</th></tr><tr><td>1</td><td>2</td></tr></table>
  <en-media type="image/png" hash="aabbccdd00112233"/>
  <en-crypt>NKLzQ...</en-crypt>
</en-note>]]></content>
    <created>20230401T080000Z</created>
    <tag>读书</tag>
    <tag>2023</tag>
    <resource>
      <data encoding="base64">aGVsbG8=</data>
      <mime>image/png</mime>
      <resource-attributes>
        <file-name>cover.png</file-name>
      </resource-attributes>
    </resource>
  </note>
  <note>
    <title>第二条</title>
    <content><![CDATA[<en-note>只有文本，&amp; 符号保留。</en-note>]]></content>
  </note>
</en-export>"#;

    fn png_bytes() -> Vec<u8> {
        // md5("hello") = 5d41402abc4b2a76b9719d911017c592
        b"hello".to_vec()
    }

    #[test]
    fn parses_notes_tags_and_resources() {
        let notes = parse_enex(SAMPLE_ENEX).unwrap();
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].title, "读书笔记");
        assert_eq!(notes[0].tags, vec!["读书".to_string(), "2023".to_string()]);
        assert_eq!(notes[0].resources.len(), 1);
        let resource = &notes[0].resources[0];
        assert_eq!(resource.bytes, png_bytes());
        assert_eq!(resource.hash_hex, "5d41402abc4b2a76b9719d911017c592");
        assert_eq!(resource.mime, "image/png");
        assert_eq!(resource.file_name.as_deref(), Some("cover.png"));
    }

    #[test]
    fn converts_enml_to_markdown() {
        let notes = parse_enex(SAMPLE_ENEX).unwrap();
        let resolve = |hash: &str, _kind: &str| -> Option<ResolvedMedia> {
            if hash == "aabbccdd00112233" {
                Some(ResolvedMedia {
                    url: "asset://localhost/C:/n/attachments/cover.png".to_string(),
                    alt: "cover.png".to_string(),
                    is_image: true,
                })
            } else {
                None
            }
        };
        let markdown = enml_to_markdown(&notes[0].content_xml, &resolve).unwrap();
        assert!(markdown.contains("# 第一章"));
        assert!(markdown.contains("**观点**"));
        assert!(markdown.contains("*例证*"));
        assert!(markdown.contains("[资料](https://example.com/a%20b)"));
        assert!(markdown.contains("- [x] 已完成事项"));
        assert!(markdown.contains("- [ ] 待办事项"));
        assert!(markdown.contains("- 第一项"));
        assert!(markdown.contains("  1. 嵌套甲"));
        assert!(markdown.contains("> 引用内容"));
        assert!(markdown.contains("fn main() {"));
        assert!(markdown.contains("| 列A | 列B |"));
        assert!(markdown.contains("| --- | --- |"));
        assert!(markdown.contains("![cover.png](asset://localhost/C:/n/attachments/cover.png)"));
        assert!(markdown.contains("*[encrypted content]*"));
        assert!(!markdown.contains("en-note"));
    }

    #[test]
    fn keeps_entities_and_plain_text() {
        let notes = parse_enex(SAMPLE_ENEX).unwrap();
        let markdown = enml_to_markdown(&notes[1].content_xml, &|_, _| None).unwrap();
        assert_eq!(markdown, "只有文本，& 符号保留。");
    }

    #[test]
    fn missing_media_resolution_leaves_marker() {
        let xml = r#"<en-note><en-media type="application/pdf" hash="ffff"/></en-note>"#;
        let markdown = enml_to_markdown(xml, &|_, _| None).unwrap();
        assert_eq!(markdown, "*[attachment missing]*");
    }

    #[test]
    fn skips_empty_resources_and_notes() {
        let xml = r#"<en-export><note><title></title><content></content></note>
<note><title>有效</title><content><![CDATA[<en-note>正文</en-note>]]></content>
<resource><data encoding="base64"></data><mime>image/png</mime></resource></note></en-export>"#;
        let notes = parse_enex(xml).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].title, "有效");
        assert!(notes[0].resources.is_empty());
    }

    #[test]
    fn rejects_invalid_base64() {
        let xml = r#"<en-export><note><title>坏数据</title><content><![CDATA[<en-note>x</en-note>]]></content>
<resource><data encoding="base64">!!!</data><mime>image/png</mime></resource></note></en-export>"#;
        assert!(parse_enex(xml).is_err());
    }

    #[test]
    fn non_image_media_uses_link_syntax() {
        let xml = r#"<en-note><en-media type="application/pdf" hash="abc"/></en-note>"#;
        let resolve = |hash: &str, _kind: &str| -> Option<ResolvedMedia> {
            (hash == "abc").then(|| ResolvedMedia {
                url: "asset://localhost/C:/n/attachments/doc.pdf".to_string(),
                alt: "doc.pdf".to_string(),
                is_image: false,
            })
        };
        let markdown = enml_to_markdown(xml, &resolve).unwrap();
        assert_eq!(markdown, "[doc.pdf](asset://localhost/C:/n/attachments/doc.pdf)");
    }
}
