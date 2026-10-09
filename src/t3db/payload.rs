//! Encoded contract models. Optional fields track historical wire compatibility;
//! required nullable fields must still be present. Unknown object keys are rejected.
#![allow(dead_code)]
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize)]
#[serde(transparent)]
pub struct Null<T>(pub Option<T>);
impl<'de, T: serde::de::DeserializeOwned> Deserialize<'de> for Null<T> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(d)?;
        serde_json::from_value(value)
            .map(Self)
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Selection {
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub instance_id: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub provider: Option<String>,
    pub model: String,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub options: Option<SelectionOptions>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Scalar {
    String(String),
    Bool(bool),
    Number(f64),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Lineage {
    pub parent_thread_id: Null<String>,
    pub relationship_to_parent: Null<Relationship>,
    pub root_thread_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LinkedPr {
    pub project_id: String,
    pub repository: String,
    pub number: u64,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrActor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_bot: Option<bool>,
    pub login: String,
    pub name: Null<String>,
    pub avatar_url: Null<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrSnapshot {
    pub state: String,
    pub title: String,
    pub head_branch: String,
    pub base_branch: String,
    pub is_draft: bool,
    pub updated_at: Null<IsoDate>,
    pub synced_at: IsoDate,
    #[serde(
        default,
        deserialize_with = "optional_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub closed_at: Option<Null<String>>,
    #[serde(
        default,
        deserialize_with = "optional_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub merged_at: Option<Null<String>>,
    #[serde(
        default,
        deserialize_with = "optional_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub author: Option<Null<PrActor>>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub additions: Option<u64>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub deletions: Option<u64>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub changed_files: Option<u64>,
    #[serde(
        default,
        deserialize_with = "optional_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub review_decision: Option<Null<String>>,
    #[serde(
        default,
        deserialize_with = "optional_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub checks_state: Option<Null<String>>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub mergeability: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrLayer {
    pub number: u64,
    pub head_branch: String,
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrStack {
    pub kind: String,
    pub id: String,
    pub number: u64,
    pub url: String,
    pub base: String,
    pub layers: Vec<PrLayer>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrWatch {
    pub started_at: IsoDate,
    pub head_sha: Null<String>,
    pub failed_checks: Vec<String>,
    pub passed: bool,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub passed_checks: Option<Vec<String>>,
    pub remarks_through: IsoDate,
    pub remark_ids: Vec<String>,
    pub conflicting: bool,
    pub wakes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrLink {
    pub host: String,
    pub repository: String,
    pub number: u64,
    pub url: String,
    pub source: String,
    pub linked_at: IsoDate,
    pub snapshot: Null<PrSnapshot>,
    pub stack: Null<PrStack>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub watch: Option<PrWatch>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestTime {
    pub request_id: String,
    pub started_at: IsoDate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestFailure {
    pub request_id: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LimitRecovery {
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub request_id: Option<String>,
    pub run_id: String,
    pub reset_at: IsoDate,
    pub auto_resume: bool,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub snooze: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ThreadFork {
    Run {
        #[serde(rename = "threadId")]
        thread_id: String,
        #[serde(rename = "runId")]
        run_id: String,
    },
    Node {
        #[serde(rename = "nodeId")]
        node_id: String,
    },
    ProviderThread {
        #[serde(rename = "providerThreadId")]
        provider_thread_id: String,
        #[serde(
            rename = "providerTurnId",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        provider_turn_id: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Thread {
    pub created_by: Actor,
    pub creation_source: Creation,
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub provider_instance_id: String,
    pub model_selection: Selection,
    pub runtime_mode: RuntimeMode,
    pub interaction_mode: Interaction,
    pub branch: Null<String>,
    pub worktree_path: Null<String>,
    #[serde(
        default,
        deserialize_with = "optional_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub linked_pull_request: Option<Null<LinkedPr>>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub pull_requests: Option<Vec<PrLink>>,
    #[serde(
        default,
        deserialize_with = "optional_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub branch_pull_request: Option<Null<LinkedPr>>,
    pub active_provider_thread_id: Null<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub history_origin: Option<History>,
    pub lineage: Lineage,
    pub forked_from: Null<ThreadFork>,
    pub created_at: IsoDate,
    pub updated_at: IsoDate,
    pub archived_at: Null<IsoDate>,
    pub settled_override: Null<Settlement>,
    pub settled_at: Null<IsoDate>,
    #[serde(
        default,
        deserialize_with = "optional_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub unsettled_at: Option<Null<IsoDate>>,
    #[serde(
        default,
        deserialize_with = "optional_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub snoozed_until: Option<Null<IsoDate>>,
    #[serde(
        default,
        deserialize_with = "optional_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub snoozed_at: Option<Null<IsoDate>>,
    #[serde(
        default,
        deserialize_with = "optional_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub limit_recovery: Option<Null<LimitRecovery>>,
    #[serde(
        default,
        deserialize_with = "optional_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub pinned_at: Option<Null<IsoDate>>,
    #[serde(
        default,
        deserialize_with = "optional_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub auto_settle_disabled_at: Option<Null<IsoDate>>,
    #[serde(
        default,
        deserialize_with = "optional_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub pin_order_key: Option<Null<String>>,
    #[serde(
        default,
        deserialize_with = "optional_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub active_order_key: Option<Null<String>>,
    pub last_visited_at: Null<IsoDate>,
    #[serde(
        default,
        deserialize_with = "optional_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub title_regeneration: Option<Null<RequestTime>>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub rollback_request_id: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub rollback_failure: Option<Null<RequestFailure>>,
    pub deleted_at: Null<IsoDate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeRef {
    pub driver: String,
    pub native_id: Null<String>,
    pub strength: Strength,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub fingerprint: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub ordinal: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderFork {
    pub provider_thread_id: String,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub provider_turn_id: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub checkpoint_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BackgroundTask {
    pub task_id: String,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub description: Option<String>,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Cost {
    pub amount: f64,
    pub currency: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Usage {
    pub used_tokens: u64,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub total_processed_tokens: Option<u64>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub max_tokens: Option<u64>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub input_tokens: Option<u64>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub cached_input_tokens: Option<u64>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub output_tokens: Option<u64>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub reasoning_output_tokens: Option<u64>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub last_used_tokens: Option<u64>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub last_input_tokens: Option<u64>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub last_cached_input_tokens: Option<u64>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub last_output_tokens: Option<u64>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub last_reasoning_output_tokens: Option<u64>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub tool_uses: Option<u64>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub duration_ms: Option<u64>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub compacts_automatically: Option<bool>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub auto_compact_threshold: Option<u64>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub cost: Option<Cost>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeMetadata {
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub model_selection: Option<Selection>,
    #[serde(
        default,
        deserialize_with = "optional_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub title: Option<Null<String>>,
    #[serde(
        default,
        deserialize_with = "optional_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub updated_at: Option<Null<String>>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub item_identity_version: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Goal {
    pub objective: String,
    pub status: String,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub tokens_used: Option<u64>,
    #[serde(
        default,
        deserialize_with = "optional_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub token_budget: Option<Null<u64>>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub time_used_seconds: Option<u64>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub checks: Option<u64>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub last_check: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderThread {
    pub id: String,
    pub driver: String,
    pub provider_instance_id: String,
    pub provider_session_id: Null<String>,
    pub app_thread_id: Null<String>,
    pub owner_node_id: Null<String>,
    pub native_thread_ref: Null<NativeRef>,
    pub native_conversation_head_ref: Null<NativeRef>,
    pub status: ProviderStatus,
    pub first_run_ordinal: Null<u64>,
    pub last_run_ordinal: Null<u64>,
    pub handoff_ids: Vec<String>,
    pub forked_from: Null<ProviderFork>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub pending_background_tasks: Option<Vec<BackgroundTask>>,
    #[serde(
        default,
        deserialize_with = "optional_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub context_usage: Option<Null<Usage>>,
    #[serde(
        default,
        deserialize_with = "optional_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub native_metadata: Option<Null<NativeMetadata>>,
    #[serde(
        default,
        deserialize_with = "optional_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub goal: Option<Null<Goal>>,
    pub created_at: IsoDate,
    pub updated_at: IsoDate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Notification {
    pub source: NotificationSource,
    pub outcome: String,
    pub summary: String,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Context {
    pub version: u64,
    pub records: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Attachment {
    #[serde(rename = "type")]
    pub type_: String,
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub size_bytes: u64,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub source: Option<AttachmentSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DelegatedCompletion {
    pub parent_run_id: String,
    pub generation: u64,
    pub task_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Message {
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub notification: Option<Notification>,
    pub created_by: Actor,
    pub creation_source: Creation,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub scheduled_task_id: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub sender_thread_id: Option<String>,
    pub id: String,
    pub thread_id: String,
    pub run_id: Null<String>,
    pub node_id: Null<String>,
    pub role: MessageRole,
    pub text: String,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub context: Option<Context>,
    pub attachments: Vec<Attachment>,
    pub streaming: bool,
    pub created_at: IsoDate,
    pub updated_at: IsoDate,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub delegated_completion: Option<DelegatedCompletion>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UserItem {
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub tool_non_execution_kind: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub tool_surface: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub tool_icon: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub tool_source: Option<String>,
    pub id: String,
    pub thread_id: String,
    pub run_id: Null<String>,
    pub node_id: Null<String>,
    pub provider_thread_id: Null<String>,
    pub provider_turn_id: Null<String>,
    pub native_item_ref: Null<NativeRef>,
    pub parent_item_id: Null<String>,
    pub ordinal: u64,
    pub status: ItemStatus,
    pub title: Null<String>,
    pub started_at: Null<IsoDate>,
    pub completed_at: Null<IsoDate>,
    pub updated_at: IsoDate,
    pub created_by: Actor,
    pub creation_source: Creation,
    #[serde(rename = "type")]
    pub type_: UserType,
    pub message_id: String,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub scheduled_task_id: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub sender_thread_id: Option<String>,
    pub input_intent: InputIntent,
    pub text: String,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub context: Option<Context>,
    pub attachments: Vec<Attachment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssistantItem {
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub tool_non_execution_kind: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub tool_surface: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub tool_icon: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub tool_source: Option<String>,
    pub id: String,
    pub thread_id: String,
    pub run_id: Null<String>,
    pub node_id: Null<String>,
    pub provider_thread_id: Null<String>,
    pub provider_turn_id: Null<String>,
    pub native_item_ref: Null<NativeRef>,
    pub parent_item_id: Null<String>,
    pub ordinal: u64,
    pub status: ItemStatus,
    pub title: Null<String>,
    pub started_at: Null<IsoDate>,
    pub completed_at: Null<IsoDate>,
    pub updated_at: IsoDate,
    #[serde(rename = "type")]
    pub type_: AssistantType,
    pub message_id: String,
    pub text: String,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub attachments: Option<Vec<Attachment>>,
    pub streaming: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Metadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub driver: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub provider_instance_id: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub raw_event_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportSource {
    pub provider: ImportProvider,
    pub provider_instance_id: String,
    pub provider_session_id: String,
    pub file_path: String,
    pub size: u64,
    pub mtime_ms: Null<serde_json::Number>,
    pub device: serde_json::Number,
    pub inode: Null<serde_json::Number>,
    pub birthtime_ms: Null<serde_json::Number>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Runtime {
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub cwd: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub imported_transcripts: Option<Vec<ImportSource>>,
    #[serde(
        default,
        deserialize_with = "optional_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub model: Option<Null<String>>,
    #[serde(
        default,
        deserialize_with = "optional_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub active_turn_id: Option<Null<String>>,
    #[serde(
        default,
        deserialize_with = "optional_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub last_error: Option<Null<String>>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub model_selection: Option<Selection>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub model_options: Option<BTreeMap<String, BTreeMap<String, Scalar>>>,
    #[serde(
        default,
        deserialize_with = "optional_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub continue_after_server_update: Option<Null<bool>>,
    #[serde(
        default,
        deserialize_with = "optional_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub continue_after_server_update_prepared: Option<Null<bool>>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub last_runtime_event: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub last_runtime_event_at: Option<IsoDate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClaudeCursor {
    pub thread_id: String,
    pub resume: String,
    #[serde(
        default,
        deserialize_with = "optional_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub resume_session_at: Option<Null<String>>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub turn_count: Option<u64>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub turn_start_message_ids: Option<Vec<Null<String>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodexCursor {
    pub thread_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpenCodeCursor {
    pub schema_version: u64,
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SelectionOptions {
    Canonical(Vec<OptionEntry>),
    Legacy(BTreeMap<String, Scalar>),
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OptionEntry {
    pub id: String,
    pub value: Scalar,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationSource {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_ids: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_thread_id: Option<String>,
}

fn optional<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}
fn optional_nullable<'de, D: serde::Deserializer<'de>, T: serde::de::DeserializeOwned>(
    d: D,
) -> Result<Option<Null<T>>, D::Error> {
    Null::<T>::deserialize(d).map(Some)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ImportProvider {
    #[serde(rename = "claudeAgent")]
    Claude,
    #[serde(rename = "codex")]
    Codex,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AttachmentSource {
    Snapshot(SnapshotSource),
    Pasted(PastedSource),
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PastedSource {
    #[serde(rename = "_tag")]
    pub tag: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SnapshotSource {
    pub kind: String,
    pub captured_at: IsoDate,
    pub app_name: String,
    pub window_title: String,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub accessible_text: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub accessibility: Option<Accessibility>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub app_identifier: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub app_icon_data_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Bounds {
    pub x: u64,
    pub y: u64,
    pub width: u64,
    pub height: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccessibilityState {
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub active: Option<bool>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub busy: Option<bool>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub checked: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub editable: Option<bool>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub enabled: Option<bool>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub expanded: Option<bool>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub focused: Option<bool>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub selected: Option<bool>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub visible: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccessibilityNode {
    pub role: String,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub name: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub value: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub description: Option<String>,
    pub bounds: Null<Bounds>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub state: Option<AccessibilityState>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub actions: Option<Vec<String>>,
    pub children: Vec<AccessibilityNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageSize {
    pub width: u64,
    pub height: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "format", deny_unknown_fields)]
pub enum Accessibility {
    #[serde(rename = "flat-text")]
    FlatText { text: String, truncated: bool },
    #[serde(rename = "element-tree")]
    ElementTree {
        #[serde(rename = "coordinateSpace")]
        coordinate_space: String,
        #[serde(rename = "imageSize")]
        image_size: ImageSize,
        truncated: bool,
        root: AccessibilityNode,
    },
}
#[derive(Debug, Clone, Serialize)]
#[serde(transparent)]
pub struct IsoDate(String);
impl<'de> Deserialize<'de> for IsoDate {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = String::deserialize(d)?;
        chrono::DateTime::parse_from_rfc3339(&value)
            .map_err(|_| serde::de::Error::custom("invalid ISO timestamp"))?;
        Ok(Self(value))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Actor {
    #[serde(rename = "user")]
    User,
    #[serde(rename = "agent")]
    Agent,
    #[serde(rename = "system")]
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Creation {
    #[serde(rename = "web")]
    Web,
    #[serde(rename = "mobile")]
    Mobile,
    #[serde(rename = "mcp")]
    Mcp,
    #[serde(rename = "provider")]
    Provider,
    #[serde(rename = "server")]
    Server,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MessageRole {
    #[serde(rename = "user")]
    User,
    #[serde(rename = "assistant")]
    Assistant,
    #[serde(rename = "system")]
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RuntimeMode {
    #[serde(rename = "approval-required")]
    ApprovalRequired,
    #[serde(rename = "auto-accept-edits")]
    AutoAcceptEdits,
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "full-access")]
    FullAccess,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Interaction {
    #[serde(rename = "default")]
    Default,
    #[serde(rename = "plan")]
    Plan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum History {
    #[serde(rename = "native")]
    Native,
    #[serde(rename = "v1_import")]
    V1Import,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Relationship {
    #[serde(rename = "fork")]
    Fork,
    #[serde(rename = "subagent")]
    Subagent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Settlement {
    #[serde(rename = "settled")]
    Settled,
    #[serde(rename = "active")]
    Active,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Strength {
    #[serde(rename = "strong")]
    Strong,
    #[serde(rename = "weak")]
    Weak,
    #[serde(rename = "none")]
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ItemStatus {
    #[serde(rename = "idle")]
    Idle,
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "waiting")]
    Waiting,
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "cancelled")]
    Cancelled,
    #[serde(rename = "interrupted")]
    Interrupted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProviderStatus {
    #[serde(rename = "not_loaded")]
    NotLoaded,
    #[serde(rename = "idle")]
    Idle,
    #[serde(rename = "active")]
    Active,
    #[serde(rename = "archived")]
    Archived,
    #[serde(rename = "closed")]
    Closed,
    #[serde(rename = "error")]
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InputIntent {
    #[serde(rename = "turn_start")]
    TurnStart,
    #[serde(rename = "queued_turn")]
    QueuedTurn,
    #[serde(rename = "steer")]
    Steer,
    #[serde(rename = "promoted_queued_to_steer")]
    PromotedQueuedToSteer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UserType {
    #[serde(rename = "user_message")]
    UserMessage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AssistantType {
    #[serde(rename = "assistant_message")]
    AssistantMessage,
}
