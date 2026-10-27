//! Built-in runtime history adapters.
//!
//! Provider-owned runtimes read their native history APIs. Product-owned
//! runtimes read the Flowix event journal first and only use transcript readers
//! as a compatibility fallback.

use std::sync::Arc;

use async_trait::async_trait;

use super::{
    ExternalRuntimeKind, HistoryAdapter, HistoryPageRequest, HistoryPolicy, journal_policy,
    provider_policy,
};
use crate::agent_external::{
    codex::CodexAppServerManager, deepseek_harness::DeepSeekHarnessManager,
    opencode::OpenCodeAcpManager,
};
use crate::agent_session::{ChatMessage, ThreadInfo, ThreadManager, ThreadMessagesPage};

pub(super) fn builtin_adapters(
    threads: Arc<ThreadManager>,
    codex: Arc<CodexAppServerManager>,
    opencode: Arc<OpenCodeAcpManager>,
    dsh: Arc<DeepSeekHarnessManager>,
) -> [Arc<dyn HistoryAdapter>; 5] {
    [
        Arc::new(CodexHistoryAdapter {
            threads: threads.clone(),
            client: codex,
        }),
        Arc::new(DshHistoryAdapter {
            client: dsh,
            threads: threads.clone(),
        }),
        Arc::new(OpenCodeHistoryAdapter { client: opencode }),
        Arc::new(ClaudeHistoryAdapter {
            threads: threads.clone(),
        }),
        Arc::new(HermesHistoryAdapter { threads }),
    ]
}

struct CodexHistoryAdapter {
    threads: Arc<ThreadManager>,
    client: Arc<CodexAppServerManager>,
}

#[async_trait]
impl HistoryAdapter for CodexHistoryAdapter {
    fn runtime(&self) -> ExternalRuntimeKind {
        ExternalRuntimeKind::Codex
    }
    fn policy(&self) -> HistoryPolicy {
        provider_policy()
    }
    async fn list_threads(&self) -> Result<Vec<ThreadInfo>, String> {
        self.client.list_threads().await
    }
    async fn read_page(
        &self,
        request: HistoryPageRequest<'_>,
    ) -> Result<ThreadMessagesPage, String> {
        let session_id = self
            .threads
            .get_external_session(request.thread_id, self.runtime().key())
            .await
            .map_err(|error| error.to_string())?
            .unwrap_or_else(|| request.thread_id.to_string());
        self.client
            .get_thread_messages_page(
                &session_id,
                request.before_sequence,
                request.snapshot_sequence,
                request.limit,
            )
            .await
    }
}

struct DshHistoryAdapter {
    client: Arc<DeepSeekHarnessManager>,
    threads: Arc<ThreadManager>,
}

#[async_trait]
impl HistoryAdapter for DshHistoryAdapter {
    fn runtime(&self) -> ExternalRuntimeKind {
        ExternalRuntimeKind::DeepSeekHarness
    }
    fn policy(&self) -> HistoryPolicy {
        provider_policy()
    }
    async fn list_threads(&self) -> Result<Vec<ThreadInfo>, String> {
        self.client.list_threads().await
    }
    async fn read_page(
        &self,
        request: HistoryPageRequest<'_>,
    ) -> Result<ThreadMessagesPage, String> {
        let mut page = self
            .client
            .session_history_page(
                request.thread_id,
                request.before_sequence,
                request.snapshot_sequence,
                request.limit,
            )
            .await?;
        // 首页 merge 本地尾巴：其他端（手机）续聊消息经云同步落
        // thread_messages（DSH 权威历史在 apply 侧过滤不落本表，见
        // thread_bridge），追加在页尾显示。历史翻页（before_sequence
        // 有值）不参与——本地行永远在时间尾部，不影响更旧分页。
        if request.before_sequence.is_none() {
            let local = self
                .threads
                .get_thread_messages_page(request.thread_id, None, 1000)
                .await
                .map_err(|error| error.to_string())?;
            page = merge_local_message_tail(page, local.messages);
        }
        Ok(page)
    }
}

/// 首页尾部追加本地消息（按 id 去重）。不动分页游标语义
/// （oldest_sequence/has_more 仍以 DSH 页为准）。
/// 已知显示局限：若桌面在手机消息之后又产生了更新的 DSH 消息，
/// 追加段时序会略微靠后（数据与同步路径不受影响，payload 投影按
/// created_at 全序排序）。
fn merge_local_message_tail(
    mut page: ThreadMessagesPage,
    local_messages: Vec<ChatMessage>,
) -> ThreadMessagesPage {
    let page_ids: std::collections::HashSet<String> = page
        .messages
        .iter()
        .map(|message| message.id.clone())
        .collect();
    for message in local_messages {
        if !page_ids.contains(&message.id) {
            page.messages.push(message);
        }
    }
    page
}

struct OpenCodeHistoryAdapter {
    client: Arc<OpenCodeAcpManager>,
}

#[async_trait]
impl HistoryAdapter for OpenCodeHistoryAdapter {
    fn runtime(&self) -> ExternalRuntimeKind {
        ExternalRuntimeKind::OpenCode
    }
    fn policy(&self) -> HistoryPolicy {
        provider_policy()
    }
    async fn list_threads(&self) -> Result<Vec<ThreadInfo>, String> {
        self.client.list_threads().await
    }
    async fn read_page(
        &self,
        request: HistoryPageRequest<'_>,
    ) -> Result<ThreadMessagesPage, String> {
        self.client
            .get_thread_messages_page(request.thread_id, request.before_sequence, request.limit)
            .await
    }
}

struct ClaudeHistoryAdapter {
    threads: Arc<ThreadManager>,
}

#[async_trait]
impl HistoryAdapter for ClaudeHistoryAdapter {
    fn runtime(&self) -> ExternalRuntimeKind {
        ExternalRuntimeKind::Claude
    }
    fn policy(&self) -> HistoryPolicy {
        journal_policy()
    }
    async fn list_threads(&self) -> Result<Vec<ThreadInfo>, String> {
        self.threads
            .list_external_threads(self.runtime().key())
            .await
            .map_err(|error| error.to_string())
    }
    async fn read_page(
        &self,
        request: HistoryPageRequest<'_>,
    ) -> Result<ThreadMessagesPage, String> {
        if let Some(page) = self
            .threads
            .get_claude_event_messages_page(
                request.thread_id,
                request.before_sequence,
                request.limit,
            )
            .await
            .map_err(|error| error.to_string())?
        {
            return Ok(page);
        }
        crate::agent_external::claude::get_session_page(
            request.thread_id,
            request.before_sequence,
            request.limit,
        )
        .await
    }
}

struct HermesHistoryAdapter {
    threads: Arc<ThreadManager>,
}

#[async_trait]
impl HistoryAdapter for HermesHistoryAdapter {
    fn runtime(&self) -> ExternalRuntimeKind {
        ExternalRuntimeKind::Hermes
    }
    fn policy(&self) -> HistoryPolicy {
        journal_policy()
    }
    async fn list_threads(&self) -> Result<Vec<ThreadInfo>, String> {
        self.threads
            .list_external_threads(self.runtime().key())
            .await
            .map_err(|error| error.to_string())
    }
    async fn read_page(
        &self,
        request: HistoryPageRequest<'_>,
    ) -> Result<ThreadMessagesPage, String> {
        if let Some(page) = self
            .threads
            .get_external_event_messages_page(
                self.runtime().key(),
                request.thread_id,
                request.before_sequence,
                request.limit,
            )
            .await
            .map_err(|error| error.to_string())?
        {
            return Ok(page);
        }
        crate::agent_external::hermes::get_session_page(
            request.thread_id,
            request.before_sequence,
            request.limit,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat_message(id: &str, content: &str) -> ChatMessage {
        ChatMessage {
            id: id.to_string(),
            role: "user".to_string(),
            message_type: None,
            content: content.to_string(),
            attachments: None,
            llm_content: None,
            system_reminder_directory: None,
            timestamp: "2026-10-27T00:00:00Z".to_string(),
            source_sequence: None,
            is_loading: None,
            tool_call_id: None,
            tool_name: None,
            tool_data: None,
            tool_input: None,
            tool_call: None,
            tool_result: None,
            tool_calls: None,
            reasoning: None,
            is_completed: None,
            error_details: None,
            is_collapsed: None,
            codex_turn_id: None,
            turn_duration_ms: None,
        }
    }

    /// 首页 merge：本地（手机）消息按 id 去重后追加在页尾；
    /// 分页游标字段保持 DSH 页原值。
    #[test]
    fn merge_local_message_tail_appends_deduped() {
        let page = ThreadMessagesPage {
            messages: vec![chat_message("dsh_1", "桌面"), chat_message("dsh_2", "回复")],
            oldest_sequence: Some(41),
            has_more: true,
            snapshot_sequence: Some(99),
            complete_turn_ids: None,
        };
        let local = vec![
            chat_message("dsh_1", "桌面"), // 与 DSH 页重叠（不应重复）
            chat_message("mob_1", "手机续聊"),
        ];
        let merged = merge_local_message_tail(page, local);
        let ids: Vec<&str> = merged.messages.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["dsh_1", "dsh_2", "mob_1"]);
        assert_eq!(merged.oldest_sequence, Some(41));
        assert!(merged.has_more);
        assert_eq!(merged.snapshot_sequence, Some(99));
    }
}
