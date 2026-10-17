//! F4 本机协作 IPC：发起咨询、查状态、拉事件（断线重放）、取消。

use std::path::PathBuf;

use crate::consult::{
    self, ConsultEvent, ConsultMeta, ConsultRequest, ConsultStatus,
};

fn notebook_root(cwd: &str) -> Result<PathBuf, String> {
    let path = std::path::Path::new(cwd);
    if !path.is_absolute() {
        return Err("笔记本路径必须是绝对路径".to_string());
    }
    if !path.is_dir() {
        return Err("笔记本路径不是目录".to_string());
    }
    Ok(path.to_path_buf())
}

#[tauri::command]
pub fn consult_initiate(
    cwd: String,
    from_agent_id: String,
    to_agent_id: String,
    question: String,
    token_budget: Option<u64>,
    app_handle: tauri::AppHandle,
) -> Result<ConsultMeta, String> {
    let root = notebook_root(&cwd)?;
    if question.trim().is_empty() {
        return Err("QUESTION_REQUIRED".to_string());
    }
    consult::initiate_consult(
        &app_handle,
        &root,
        ConsultRequest {
            from_agent_id,
            to_agent_id,
            token_budget,
        },
        question,
    )
}

#[tauri::command]
pub fn consult_status(
    cwd: String,
    from_agent_id: String,
    consult_id: String,
) -> Result<ConsultStatusWithSeq, String> {
    let root = notebook_root(&cwd)?;
    let meta = consult::read_consult_meta(&root, &from_agent_id, &consult_id)
        .ok_or_else(|| "CONSULT_NOT_FOUND".to_string())?;
    Ok(ConsultStatusWithSeq {
        meta,
        latest_seq: consult::latest_seq(&consult_id),
    })
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsultStatusWithSeq {
    pub meta: ConsultMeta,
    pub latest_seq: u64,
}

#[tauri::command]
pub fn consult_events(
    cwd: String,
    from_agent_id: String,
    consult_id: String,
    after_seq: Option<u64>,
) -> Result<Vec<SeqEvent>, String> {
    let root = notebook_root(&cwd)?;
    if consult::read_consult_meta(&root, &from_agent_id, &consult_id).is_none() {
        return Err("CONSULT_NOT_FOUND".to_string());
    }
    Ok(consult::events_after(&consult_id, after_seq.unwrap_or(0))
        .into_iter()
        .map(|(seq, event)| SeqEvent { seq, event })
        .collect())
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeqEvent {
    pub seq: u64,
    pub event: ConsultEvent,
}

/// 取消咨询：置 Cancelled 并向信箱推终止事件。
/// 实际子 thread 取消随 DSH 方法通道接线补全。
#[tauri::command]
pub fn consult_cancel(
    cwd: String,
    from_agent_id: String,
    consult_id: String,
) -> Result<(), String> {
    let root = notebook_root(&cwd)?;
    let mut meta = consult::read_consult_meta(&root, &from_agent_id, &consult_id)
        .ok_or_else(|| "CONSULT_NOT_FOUND".to_string())?;
    if matches!(meta.status, ConsultStatus::Completed | ConsultStatus::Cancelled) {
        return Ok(());
    }
    meta.status = ConsultStatus::Cancelled;
    meta.updated_at = chrono::Utc::now().timestamp_millis();
    consult::write_consult_meta(&root, &meta);
    consult::push_event(
        &consult_id,
        ConsultEvent::Failed {
            reason: "cancelled".into(),
        },
    );
    Ok(())
}
