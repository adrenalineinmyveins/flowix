//! 移动端 AI 工具注册表（MOBILE-AGENT-NETWORK M1）：个人 agent =
//! 云模型 + 本机工具。chat.rs 的工具循环按本模块的 definitions 上报
//! 工具、按 execute 分发执行；新增工具 = 加一条定义 + 一个 match 分支。
//!
//! 边界（设计 D-2）：本机工具只读（note_search / note_read）；
//! 写类工具须走「AI 落草稿 → 用户确认」，不在本模块。

use serde_json::Value;

/// 工具执行上下文：注册表工具能触达的全部环境。
pub(crate) struct ToolCtx<'a> {
    pub state: &'a crate::cache::MobileState,
    pub client: &'a reqwest::Client,
    pub config: &'a super::AgentProviderConfig,
}

/// OpenAI function calling 工具定义（上报给模型的 schema）。
pub(crate) fn definitions() -> Vec<Value> {
    vec![
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "web_search",
                "description": "联网搜索最新信息。当问题涉及时效性、新闻、价格、版本号或你不确定的事实性内容时调用。",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "搜索关键词" }
                    },
                    "required": ["query"]
                }
            }
        }),
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "note_search",
                "description": "在用户本机缓存的全部笔记本中做全文搜索。当问题涉及用户自己的笔记、记录、备忘内容时优先调用。",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "搜索关键词" }
                    },
                    "required": ["query"]
                }
            }
        }),
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "note_read",
                "description": "读取一篇笔记的正文（前 2000 字）。note_search 结果中的 notebook_id 与 relative_path 可直接作为参数。",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "notebook_id": { "type": "string", "description": "笔记本 ID" },
                        "relative_path": { "type": "string", "description": "笔记在笔记本内的相对路径" }
                    },
                    "required": ["notebook_id", "relative_path"]
                }
            }
        }),
    ]
}

/// 按 name 分发执行。未注册名称返回 AGENT_TOOL_UNSUPPORTED（调用方
/// 记 round_failed，tool 消息内容为错误文本，回合不中断）。
pub(crate) async fn execute(ctx: &ToolCtx<'_>, name: &str, args: &Value) -> Result<String, String> {
    match name {
        "web_search" => {
            let query = str_arg(args, "query");
            if query.trim().is_empty() {
                return Err("WEB_SEARCH_EMPTY_QUERY".into());
            }
            web_search_dispatch(ctx.client, ctx.config, &query).await
        }
        "note_search" => note_search(ctx, args),
        "note_read" => note_read(ctx, args),
        other => Err(format!("AGENT_TOOL_UNSUPPORTED: {other}")),
    }
}

fn str_arg(args: &Value, key: &str) -> String {
    args[key].as_str().unwrap_or_default().to_string()
}

// ── note_search / note_read（本机笔记，只读）──────────────────────

/// 全部缓存笔记本全文搜索：每本前 5 条，输出 `标题 | 笔记本/路径` + 摘要。
fn note_search(ctx: &ToolCtx<'_>, args: &Value) -> Result<String, String> {
    let query = str_arg(args, "query");
    if query.trim().is_empty() {
        return Err("NOTE_SEARCH_EMPTY_QUERY".into());
    }
    let configs = ctx
        .state
        .memo_file
        .read_notebook_configs()
        .map_err(|e| format!("NOTE_SEARCH_READ_CONFIGS_FAILED: {e}"))?;
    let mut lines = Vec::new();
    'outer: for config in &configs {
        let hits = ctx
            .state
            .memo_file
            .search_notes(&config.id, &query, 5)
            .map_err(|e| format!("NOTE_SEARCH_FAILED: {e}"))?;
        for hit in hits {
            let snippet: String = hit.snippet.chars().take(200).collect();
            lines.push(format!(
                "{}. {} | {}/{}\n{}",
                lines.len() + 1,
                hit.title,
                config.id,
                hit.relative_path,
                snippet
            ));
            if lines.len() >= 10 {
                break 'outer;
            }
        }
    }
    if lines.is_empty() {
        return Ok("（本机笔记无命中）".into());
    }
    Ok(lines.join("\n\n"))
}

/// 读单篇笔记正文前 2000 字；取数路径与 build_note_context_suffix 一致。
fn note_read(ctx: &ToolCtx<'_>, args: &Value) -> Result<String, String> {
    let notebook_id = str_arg(args, "notebook_id");
    let relative_path = str_arg(args, "relative_path");
    if notebook_id.trim().is_empty() || relative_path.trim().is_empty() {
        return Err("NOTE_READ_MISSING_PARAMS".into());
    }
    let notebook = ctx
        .state
        .memo_file
        .get_notebook_config_by_id(&notebook_id)
        .ok_or_else(|| format!("NOTE_READ_NOTEBOOK_NOT_FOUND: {notebook_id}"))?;
    let path = crate::cache::path_from_relative(std::path::Path::new(&notebook.path), &relative_path)
        .map_err(|e| format!("NOTE_READ_PATH_INVALID: {e}"))?;
    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("NOTE_READ_FAILED: {e}"))?;
    let preview: String = content
        .replace("\r\n", "\n")
        .trim()
        .chars()
        .take(2000)
        .collect();
    if preview.is_empty() {
        return Ok("（笔记内容为空）".into());
    }
    Ok(preview)
}

// ── 联网搜索工具（自 chat.rs 迁入，方案 B 原实现）─────────────────
// Tavily（配置 key 时，结果质量高）或内置必应网页抓取（免 key 兜底）。

async fn web_search_dispatch(
    client: &reqwest::Client,
    config: &super::AgentProviderConfig,
    query: &str,
) -> Result<String, String> {
    match config
        .search_api_key
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty())
    {
        Some(key) => tavily_search(client, key, query).await,
        None => bing_search(client, query).await,
    }
}

/// Tavily API：POST /search，返回 markdown 化的结果列表。
async fn tavily_search(client: &reqwest::Client, key: &str, query: &str) -> Result<String, String> {
    let response = client
        .post("https://api.tavily.com/search")
        .bearer_auth(key)
        .json(&serde_json::json!({ "query": query, "max_results": 5 }))
        .send()
        .await
        .map_err(|e| format!("TAVILY_REQUEST_FAILED: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("TAVILY_HTTP_{}", response.status().as_u16()));
    }
    let value: Value = response
        .json()
        .await
        .map_err(|e| format!("TAVILY_PARSE_FAILED: {e}"))?;
    let mut lines = Vec::new();
    if let Some(results) = value["results"].as_array() {
        for (i, item) in results.iter().enumerate().take(5) {
            let title = item["title"].as_str().unwrap_or_default();
            let url = item["url"].as_str().unwrap_or_default();
            let snippet = item["content"].as_str().unwrap_or_default();
            lines.push(format!("{}. {} | {}\n{}", i + 1, title, url, snippet));
        }
    }
    if lines.is_empty() {
        return Err("TAVILY_EMPTY".into());
    }
    Ok(lines.join("\n\n"))
}

/// 必应网页抓取（免 key 兜底）：cn.bing.com 结果页粗解析（b_algo 块的
/// 标题/链接/摘要）。页面结构变化时会退化到空结果，模型将如实告知。
async fn bing_search(client: &reqwest::Client, query: &str) -> Result<String, String> {
    let url = format!("https://cn.bing.com/search?q={}", urlencode(query));
    let response = client
        .get(&url)
        .header("User-Agent", "Mozilla/5.0 (Linux; Android 14) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124 Mobile Safari/537.36")
        .header("Accept-Language", "zh-CN,zh;q=0.9")
        .send()
        .await
        .map_err(|e| format!("BING_REQUEST_FAILED: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("BING_HTTP_{}", response.status().as_u16()));
    }
    let html = response
        .text()
        .await
        .map_err(|e| format!("BING_READ_FAILED: {e}"))?;
    let mut lines = Vec::new();
    for block in html.split("<li class=\"b_algo").skip(1).take(5) {
        let Some(link) = capture_first(block, "href=\"", "\"") else {
            continue;
        };
        let Some(title) = capture_first(block, "<h2>", "</h2>") else {
            continue;
        };
        let snippet = capture_first(block, "<p", "</p>")
            .map(|raw| {
                // 去标签 + 反转义
                let text = strip_tags(&raw);
                html_unescape(&text)
            })
            .unwrap_or_default();
        let title = html_unescape(&strip_tags(&title));
        let snippet = snippet.chars().take(300).collect::<String>();
        if !link.starts_with("http") {
            continue;
        }
        lines.push(format!("{}. {} | {}\n{}", lines.len() + 1, title, link, snippet));
    }
    if lines.is_empty() {
        return Err("BING_EMPTY（页面结构可能已变化）".into());
    }
    Ok(lines.join("\n\n"))
}

/// 从 text 中取 start 之后、首个 end 之前的片段（片段内标签保留，调用方清洗）。
fn capture_first(text: &str, start: &str, end: &str) -> Option<String> {
    let start_idx = text.find(start)? + start.len();
    let rest = &text[start_idx..];
    let end_idx = rest.find(end)?;
    Some(rest[..end_idx].to_string())
}

fn urlencode(input: &str) -> String {
    let mut out = String::new();
    for byte in input.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn strip_tags(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

fn html_unescape(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definitions_have_unique_names() {
        let defs = definitions();
        let names: Vec<&str> = defs
            .iter()
            .map(|d| d["function"]["name"].as_str().unwrap())
            .collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(names.len(), sorted.len(), "tool names must be unique");
        for name in ["web_search", "note_search", "note_read"] {
            assert!(names.contains(&name), "missing tool: {name}");
        }
    }

    #[test]
    fn str_arg_reads_nested_string() {
        let args = serde_json::json!({ "query": "杭州天气" });
        assert_eq!(str_arg(&args, "query"), "杭州天气");
        assert_eq!(str_arg(&args, "missing"), "");
    }
}
