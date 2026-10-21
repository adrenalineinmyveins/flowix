//! Flowix Cloud authentication and synchronization engine.
//!
//! This crate deliberately has no Tauri dependency. `flowix-desktop` owns the
//! local Markdown adapter and secret persistence; this crate owns the Cloud
//! HTTP contract, sync state database, revisions, cursors and conflict plans.

mod client;
mod error;
mod manager;
mod models;
mod store;
mod v2;

pub use client::CloudClient;
pub use error::SyncError;
pub use manager::SyncManager;
pub use models::{
    AppleAuthChallenge, AppleAuthorization, ApiErrorBody, ApiErrorEnvelope, AuthData,
    CloudAuthStatus, CloudCheckout, CloudMembership, CloudNotebook, CloudPrice, CloudProduct,
    CloudSession, CloudState, CloudUser, DataEnvelope, LocalChangeKind, MeData, ProductDuration,
    ProductEntitlement, RefreshData, RefreshTokenStore,
};
pub use store::SyncStore;
pub use v2::{
    collect_v2_attachments, new_v2_operation_id, referenced_attachment_paths, v2_content_hash,
    v2_local_content_diverged, v2_merge_thread_payloads, v2_notebook_metadata_hash,
    v2_path_note_id, V2AccountSyncReport, V2Attachment,
    V2BlobDownload, V2BlobDownloadCapability, V2BlobDownloadEnvelope, V2BlobReservation,
    V2BlobReservationEnvelope, V2BlobUpload, V2Bootstrap, V2BootstrapNote, V2BootstrapNotebook,
    V2BootstrapThread, V2Change, V2ChangesPage, V2CloudAccount,
    V2History, V2HistoryRevision,
    V2ConflictMaterial,
    V2DirtyEntity, V2EntityType, V2FreezeOperation, V2InflightOperation, V2LocalAttachment,
    V2LocalNote, V2LocalNotebook, V2LocalThread, V2NoteState, V2NotebookState, V2NotebookPut, V2NotePut, V2OperationData, V2OperationError,
    V2OperationKind, V2OperationResult, V2ThreadPayload, V2ThreadMessage, V2ThreadPut, V2ThreadState, V2Usage,
    V2PendingMove,
    V2PushOperation, V2PushResult, V2RemoteApply, V2RemoteAttachment, V2SyncStatus,
    V2SyncedNotebook, PROTOCOL_EPOCH,
};

pub const DEFAULT_CLOUD_API_BASE: &str = "https://cloud.flowix.cc";
pub mod text_merge;
