use super::{
    ImportOptions, ImportReport, ImportRequest, Progress, ProjectRef, Snapshot, T3Home, guard,
    payload, thread_id_for,
};
use crate::model::{Provider, Role};
use anyhow::{Context, ensure};
use chrono::{SecondsFormat, Utc};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, UNIX_EPOCH},
};

pub fn snapshot(home: &T3Home) -> anyhow::Result<Snapshot> {
    let db = guard::readonly(home)?;
    let mut snapshot = snapshot_db(&db)?;
    let settings = match std::fs::read(home.userdata().join("settings.json")) {
        Ok(raw) => serde_json::from_slice::<Value>(&raw).context("invalid T3 settings.json")?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(e) => return Err(e.into()),
    };
    if let Some(instances) = settings.get("providerInstances") {
        ensure!(
            instances.is_object(),
            "settings.providerInstances must be a map"
        );
        for instance in instances.as_object().unwrap().values() {
            ensure!(
                instance.get("driver").and_then(Value::as_str).is_some(),
                "provider instance missing driver"
            );
            if let Some(enabled) = instance.get("enabled") {
                ensure!(
                    enabled.is_boolean(),
                    "provider instance enabled must be boolean"
                );
            }
        }
    }
    let instances = settings.get("providerInstances").and_then(Value::as_object);
    for provider in Provider::ALL {
        let mut configured = Vec::new();
        let mut present = false;
        if let Some(instances) = instances {
            for (id, instance) in instances {
                if instance.get("driver").and_then(Value::as_str) == Some(provider.driver()) {
                    present = true;
                    if instance
                        .get("enabled")
                        .and_then(Value::as_bool)
                        .unwrap_or(true)
                    {
                        configured.push(id.clone());
                    }
                }
            }
        }
        if !present {
            configured.push(provider.driver().into());
        }
        configured.sort_by_key(|id| (id != provider.driver(), id.clone()));
        snapshot.provider_instances.insert(provider, configured);
    }
    Ok(snapshot)
}

fn snapshot_db(db: &Connection) -> anyhow::Result<Snapshot> {
    let mut result = Snapshot::default();
    result.projects = db.prepare("SELECT project_id,title,workspace_root FROM projection_projects WHERE deleted_at IS NULL ORDER BY project_id")?
        .query_map([], |r| Ok(ProjectRef { id: r.get(0)?, title: r.get(1)?, workspace_root: PathBuf::from(r.get::<_,String>(2)?) }))?
        .collect::<rusqlite::Result<_>>()?;
    result.thread_ids = db
        .prepare("SELECT thread_id FROM orchestration_v2_projection_threads")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(result)
}

pub(super) fn backup(source: &Connection, path: &Path) -> anyhow::Result<()> {
    // Never overwrite a previous backup, including another import started this instant.
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    drop(file);
    let mut destination = Connection::open(path)?;
    let backup = rusqlite::backup::Backup::new(source, &mut destination)?;
    backup.run_to_completion(256, Duration::from_millis(10), None)?;
    drop(backup);
    // A standalone backup must not depend on WAL sidecars; changing the destination's
    // journal mode also prevents read-only test copies from creating sidecar files.
    destination.pragma_update(None, "journal_mode", "DELETE")?;
    destination.close().map_err(|(_, e)| e)?;
    Ok(())
}

pub fn import(
    home: &T3Home,
    requests: Vec<ImportRequest>,
    options: &ImportOptions,
    progress: &mut dyn FnMut(Progress),
) -> anyhow::Result<ImportReport> {
    progress(Progress::Guard);
    guard::require_ok(guard::check(home)?)?;
    let mut report = ImportReport::default();
    let mut state = snapshot(home)?;
    let mut seen = state.thread_ids.clone();
    let mut planned = Vec::new();
    for request in requests {
        let summary = &request.transcript.summary;
        let id = thread_id_for(&request.provider_instance_id, &summary.session_id);
        if !seen.insert(id.clone()) {
            report.skipped.push((id, "thread already exists".into()));
            continue;
        }
        if !summary.resumable
            || (summary.provider == Provider::Claude && !claude_uuid(&summary.session_id))
        {
            report.skipped.push((id, "session is not resumable".into()));
            continue;
        }
        ensure!(
            state
                .provider_instances
                .get(&summary.provider)
                .is_some_and(|ids| ids.contains(&request.provider_instance_id)),
            "provider instance {} is not enabled for {}",
            request.provider_instance_id,
            summary.provider.driver()
        );
        ensure!(
            !summary.session_id.trim().is_empty()
                && !request.provider_instance_id.trim().is_empty(),
            "session and provider instance IDs must be nonempty"
        );
        let count = options
            .max_messages
            .unwrap_or(usize::MAX)
            .min(request.transcript.messages.len());
        ensure!(
            count <= 999_999,
            "thread {id} has {count} messages: ordinals would cross the runless range limit (999999)"
        );
        check_owner(&guard::readonly(home)?, &provider_thread_id(&request), &id)?;
        if request.placement.worktree_missing {
            report.warnings.push(format!(
                "{id}: original working directory is missing; resume uses {}",
                request.placement.workspace_root.display()
            ));
        }
        planned.push(request);
    }
    let mut needed: Vec<PathBuf> = Vec::new();
    for request in &planned {
        let root = &request.placement.workspace_root;
        if project_for(&state, root).is_none() && !needed.iter().any(|p| same_path(p, root)) {
            needed.push(root.clone());
        }
    }
    if options.dry_run {
        report.imported = planned
            .iter()
            .map(|r| thread_id_for(&r.provider_instance_id, &r.transcript.summary.session_id))
            .collect();
        for root in needed {
            report
                .warnings
                .push(format!("Would create project: {}", root.display()));
        }
        report.warnings.push(
            "Dry run: imported entries are planned; no files or database rows were written".into(),
        );
        return Ok(report);
    }
    if planned.is_empty() {
        return Ok(report);
    }
    guard::require_ok(guard::check(home)?)?;
    progress(Progress::Backup);
    let backup_path = home.userdata().join(format!(
        "statev2.sqlite.bak-t3import-{}",
        Utc::now().format("%Y%m%dT%H%M%S%.9fZ")
    ));
    backup(&guard::readonly(home)?, &backup_path)?;
    report.backup = Some(backup_path);
    for root in needed {
        guard::require_ok(guard::check(home)?)?;
        progress(Progress::CreatingProject(root.clone()));
        let cli = home
            .cli()
            .context("T3 CLI not found; cannot create missing project")?;
        let output = Command::new(cli)
            .arg("project")
            .arg("add")
            .arg(&root)
            .arg("--base-dir")
            .arg(&home.base)
            .env("T3CODE_HOME", &home.base)
            .output()?;
        ensure!(
            output.status.success(),
            "t3 project add failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        report.projects_created.push(root);
    }
    progress(Progress::Guard);
    guard::require_ok(guard::check(home)?)?;
    let mut db = Connection::open_with_flags(home.db_path(), OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    db.busy_timeout(Duration::from_secs(5))?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    // The guard is repeated under the write lock, and all threads share this transaction.
    guard::require_ok(guard::check_connection(home, &tx)?)?;
    state = snapshot_db(&tx)?;
    let mut written = Vec::new();
    let mut provenance = Vec::new();
    for (index, request) in planned.iter().enumerate() {
        let id = thread_id_for(
            &request.provider_instance_id,
            &request.transcript.summary.session_id,
        );
        if state.thread_ids.contains(&id) {
            report.skipped.push((id, "thread already exists".into()));
            continue;
        }
        let project = project_for(&state, &request.placement.workspace_root)
            .context("project missing after t3 project add")?;
        let rows = write_thread(&tx, request, &project.id, options)?;
        state.thread_ids.insert(id.clone());
        written.extend(rows);
        if matches!(
            request.transcript.summary.provider,
            Provider::Grok | Provider::OpenCode
        ) {
            provenance.push(json!({"threadId":id,"provider":request.transcript.summary.provider.driver(),"sessionId":request.transcript.summary.session_id,"source":request.transcript.summary.source,"cwd":request.transcript.summary.cwd}));
        }
        report.imported.push(id);
        progress(Progress::Writing {
            done: index + 1,
            total: planned.len(),
        });
    }
    if !report.imported.is_empty() {
        written.push(WrittenRow::capture(
            &tx,
            "orchestration_v2_projection_metadata",
            "projection_name",
            "thread-projections",
        )?);
    }
    progress(Progress::Verifying);
    for row in &written {
        row.verify(&tx)?;
    }
    guard::require_ok(guard::check_connection(home, &tx)?)?;
    tx.commit()?;
    if !provenance.is_empty() {
        // AgentSessionImportSource only admits Claude/Codex. Keep other provenance
        // in our own manifest rather than introducing keys into T3 runtime payloads.
        if let Err(error) = save_provenance(home, provenance) {
            report.warnings.push(format!(
                "Threads committed, but provenance manifest could not be saved: {error}"
            ));
        }
    }
    Ok(report)
}

fn save_provenance(home: &T3Home, entries: Vec<Value>) -> anyhow::Result<()> {
    let path = home.userdata().join("t3-import-provenance.json");
    let mut existing: Vec<Value> = match std::fs::read(&path) {
        Ok(raw) => serde_json::from_slice(&raw)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e.into()),
    };
    existing.extend(entries);
    let temp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&temp, serde_json::to_vec_pretty(&existing)?)?;
    std::fs::rename(temp, path)?;
    Ok(())
}
fn same_path(a: &Path, b: &Path) -> bool {
    a.canonicalize().unwrap_or_else(|_| a.into()) == b.canonicalize().unwrap_or_else(|_| b.into())
}
fn project_for<'a>(state: &'a Snapshot, root: &Path) -> Option<&'a ProjectRef> {
    state
        .projects
        .iter()
        .find(|p| same_path(&p.workspace_root, root))
}
fn claude_uuid(id: &str) -> bool {
    let bytes = id.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(i, c)| {
            if [8, 13, 18, 23].contains(&i) {
                *c == b'-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
        && (b'1'..=b'8').contains(&bytes[14])
        && matches!(bytes[19].to_ascii_lowercase(), b'8' | b'9' | b'a' | b'b')
}
fn encode_part(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}
fn provider_thread_id(r: &ImportRequest) -> String {
    format!(
        "provider-thread:provider:{}:provider-instance:{}:native-thread:{}",
        encode_part(r.transcript.summary.provider.driver()),
        encode_part(&r.provider_instance_id),
        encode_part(&r.transcript.summary.session_id)
    )
}
fn check_owner(db: &Connection, provider_id: &str, id: &str) -> anyhow::Result<()> {
    let owner: Option<Option<String>> = db.query_row("SELECT thread_id FROM orchestration_v2_projection_provider_threads WHERE provider_thread_id=?", [provider_id], |r| r.get(0)).optional()?;
    if let Some(owner) = owner {
        ensure!(
            owner.as_deref() == Some(id),
            "provider thread {provider_id} is already owned by another app thread (or unbound)"
        );
    }
    Ok(())
}
fn encoded<T: DeserializeOwned + Serialize>(value: Value) -> anyhow::Result<String> {
    Ok(serde_json::to_string(&serde_json::from_value::<T>(value)?)?)
}
fn iso(time: chrono::DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Every inserted row is captured and compared column-for-column before commit.
struct WrittenRow {
    table: &'static str,
    key: &'static str,
    id: String,
    values: Vec<rusqlite::types::Value>,
}
impl WrittenRow {
    fn capture(
        db: &Connection,
        table: &'static str,
        key: &'static str,
        id: &str,
    ) -> anyhow::Result<Self> {
        let values = Self::read(db, table, key, id)?;
        Ok(Self {
            table,
            key,
            id: id.into(),
            values,
        })
    }
    fn read(
        db: &Connection,
        table: &str,
        key: &str,
        id: &str,
    ) -> anyhow::Result<Vec<rusqlite::types::Value>> {
        let mut stmt = db.prepare(&format!("SELECT * FROM {table} WHERE {key}=?"))?;
        let count = stmt.column_count();
        Ok(stmt.query_row([id], |row| (0..count).map(|i| row.get(i)).collect())?)
    }
    fn verify(&self, db: &Connection) -> anyhow::Result<()> {
        ensure!(
            Self::read(db, self.table, self.key, &self.id)? == self.values,
            "post-write row changed in {}",
            self.table
        );
        let raw: Option<String> = match self.table {
            "orchestration_v2_turn_item_positions" | "orchestration_v2_projection_metadata" => None,
            "provider_session_runtime" => Some(db.query_row(
                "SELECT runtime_payload_json FROM provider_session_runtime WHERE thread_id=?",
                [&self.id],
                |r| r.get(0),
            )?),
            _ => Some(db.query_row(
                &format!(
                    "SELECT payload_json FROM {} WHERE {}=?",
                    self.table, self.key
                ),
                [&self.id],
                |r| r.get(0),
            )?),
        };
        if self.table == "provider_session_runtime" {
            let (driver,cursor): (String,Option<String>) = db.query_row("SELECT provider_name,resume_cursor_json FROM provider_session_runtime WHERE thread_id=?", [&self.id], |r| Ok((r.get(0)?,r.get(1)?)))?;
            if let Some(cursor) = cursor {
                guard::validate(&driver, &cursor)?;
            }
        }
        if let Some(raw) = raw {
            let value: Value = serde_json::from_str(&raw)?;
            let kind = match self.table {
                "orchestration_v2_projection_threads" => "thread",
                "orchestration_v2_projection_messages" => "message",
                "orchestration_v2_projection_turn_items" => {
                    value["type"].as_str().context("missing turn-item type")?
                }
                "orchestration_v2_projection_provider_threads" => "provider-thread",
                "provider_session_runtime" => "runtime",
                "orchestration_events" => {
                    let event_type: String = db.query_row(
                        "SELECT event_type FROM orchestration_events WHERE event_id=?",
                        [&self.id],
                        |r| r.get(0),
                    )?;
                    let metadata: String = db.query_row(
                        "SELECT metadata_json FROM orchestration_events WHERE event_id=?",
                        [&self.id],
                        |r| r.get(0),
                    )?;
                    guard::validate("metadata", &metadata)?;
                    match event_type.as_str() {
                        "thread.created" => "thread",
                        "message.updated" => "message",
                        "turn-item.updated" => {
                            value["type"].as_str().context("missing turn-item type")?
                        }
                        "provider-thread.updated" => "provider-thread",
                        _ => anyhow::bail!("unexpected inserted event"),
                    }
                }
                _ => anyhow::bail!("unexpected inserted table"),
            };
            guard::validate(kind, &raw)?;
        }
        Ok(())
    }
}

fn event(
    db: &Connection,
    id: &str,
    thread: &str,
    kind: &str,
    at: &str,
    payload: &str,
    metadata: &str,
    rows: &mut Vec<WrittenRow>,
) -> anyhow::Result<()> {
    guard::validate("metadata", metadata)?;
    db.execute("INSERT INTO orchestration_events(event_id,aggregate_kind,stream_id,stream_version,event_type,occurred_at,command_id,causation_event_id,correlation_id,actor_kind,payload_json,metadata_json,application_event_version)
        VALUES (?1,'thread',?2,coalesce((SELECT max(stream_version)+1 FROM orchestration_events WHERE aggregate_kind='thread' AND stream_id=?2),0),?3,?4,NULL,NULL,NULL,'server',?5,?6,2)", params![id,thread,kind,at,payload,metadata])?;
    rows.push(WrittenRow::capture(
        db,
        "orchestration_events",
        "event_id",
        id,
    )?);
    Ok(())
}

fn write_thread(
    db: &Connection,
    r: &ImportRequest,
    project: &str,
    options: &ImportOptions,
) -> anyhow::Result<Vec<WrittenRow>> {
    let s = &r.transcript.summary;
    let id = thread_id_for(&r.provider_instance_id, &s.session_id);
    let provider_id = provider_thread_id(r);
    check_owner(db, &provider_id, &id)?;
    let created = iso(s.created_at);
    let updated = iso(s.updated_at);
    let model = s.model.as_deref().unwrap_or(match s.provider {
        Provider::Claude => "claude-fable-5-1",
        Provider::Codex => "gpt-6-astra",
        Provider::Grok => "grok-build",
        Provider::OpenCode => "openai/gpt-5",
    });
    let title = if s.title.trim().is_empty() {
        "Untitled thread"
    } else {
        s.title.trim()
    };
    let thread = encoded::<payload::Thread>(json!({
        "createdBy":"system","creationSource":"server","id":id,"projectId":project,"title":title,
        "providerInstanceId":r.provider_instance_id,"modelSelection":{"instanceId":r.provider_instance_id,"model":model},
        "runtimeMode":"full-access","interactionMode":"default","branch":r.placement.branch,
        "worktreePath":r.placement.worktree_path,"linkedPullRequest":null,"branchPullRequest":null,
        "activeProviderThreadId":provider_id,"historyOrigin":"v1_import",
        "lineage":{"parentThreadId":null,"relationshipToParent":null,"rootThreadId":id},"forkedFrom":null,
        "createdAt":created,"updatedAt":updated,"archivedAt":null,"settledOverride":"settled","settledAt":updated,
        "unsettledAt":null,"snoozedUntil":null,"snoozedAt":null,"pinnedAt":null,"pinOrderKey":null,
        "activeOrderKey":null,"lastVisitedAt":null,"deletedAt":null
    }))?;
    guard::validate("thread", &thread)?;
    let mut rows = Vec::new();
    let prefix = "agent-session-import:v2";
    event(
        db,
        &format!("{prefix}:thread:{id}:created"),
        &id,
        "thread.created",
        &created,
        &thread,
        &encoded::<payload::Metadata>(json!({"providerInstanceId":r.provider_instance_id}))?,
        &mut rows,
    )?;
    db.execute("INSERT INTO orchestration_v2_projection_threads(thread_id,project_id,title,default_provider,runtime_mode,interaction_mode,active_provider_thread_id,created_at,updated_at,archived_at,deleted_at,payload_json,provider_instance_id) VALUES (?1,?2,?3,?4,'full-access','default',?5,?6,?7,NULL,NULL,?8,?4)", params![id,project,title,r.provider_instance_id,provider_id,created,updated,thread])?;
    let keep = options
        .max_messages
        .unwrap_or(usize::MAX)
        .min(r.transcript.messages.len());
    ensure!(
        keep <= 999_999,
        "ordinals would cross the runless range limit"
    );
    for (index, message) in r.transcript.messages[r.transcript.messages.len() - keep..]
        .iter()
        .enumerate()
    {
        let suffix = format!("{index:06}");
        let message_id = format!("{id}:{suffix}");
        let item_id = format!("{prefix}:turn-item:{id}:{suffix}");
        let at = iso(message.created_at);
        let (role, actor, kind) = match message.role {
            Role::User => ("user", "user", "user_message"),
            Role::Assistant => ("assistant", "agent", "assistant_message"),
        };
        let msg = encoded::<payload::Message>(
            json!({"createdBy":actor,"creationSource":"server","id":message_id,"threadId":id,"runId":null,"nodeId":null,"role":role,"text":message.text,"attachments":[],"streaming":false,"createdAt":at,"updatedAt":at}),
        )?;
        guard::validate("message", &msg)?;
        event(
            db,
            &format!("{prefix}:message:{id}:{suffix}"),
            &id,
            "message.updated",
            &at,
            &msg,
            "{}",
            &mut rows,
        )?;
        db.execute("INSERT INTO orchestration_v2_projection_messages(message_id,thread_id,run_id,node_id,role,streaming,created_at,updated_at,payload_json) VALUES (?1,?2,NULL,NULL,?3,0,?4,?4,?5)", params![message_id,id,role,at,msg])?;
        rows.push(WrittenRow::capture(
            db,
            "orchestration_v2_projection_messages",
            "message_id",
            &message_id,
        )?);
        let mut item = json!({"id":item_id,"threadId":id,"runId":null,"nodeId":null,"providerThreadId":null,"providerTurnId":null,"nativeItemRef":null,"parentItemId":null,"ordinal":index+1,"status":"completed","title":null,"startedAt":at,"completedAt":at,"updatedAt":at,"type":kind,"messageId":message_id,"text":message.text});
        let item = if message.role == Role::User {
            item["createdBy"] = json!("user");
            item["creationSource"] = json!("server");
            item["inputIntent"] = json!("turn_start");
            item["attachments"] = json!([]);
            encoded::<payload::UserItem>(item)?
        } else {
            item["streaming"] = json!(false);
            encoded::<payload::AssistantItem>(item)?
        };
        guard::validate(kind, &item)?;
        event(
            db,
            &item_id,
            &id,
            "turn-item.updated",
            &at,
            &item,
            "{}",
            &mut rows,
        )?;
        db.execute("INSERT INTO orchestration_v2_projection_turn_items(turn_item_id,thread_id,run_id,node_id,provider_thread_id,provider_turn_id,parent_item_id,ordinal,type,status,updated_at,payload_json) VALUES (?1,?2,NULL,NULL,NULL,NULL,NULL,?3,?4,'completed',?5,?6)", params![item_id,id,(index+1) as i64,kind,at,item])?;
        db.execute("INSERT INTO orchestration_v2_turn_item_positions(thread_id,turn_item_id,ordinal) VALUES (?1,?2,?3)", params![id,item_id,(index+1) as i64])?;
        rows.push(WrittenRow::capture(
            db,
            "orchestration_v2_projection_turn_items",
            "turn_item_id",
            &item_id,
        )?);
        rows.push(WrittenRow::capture(
            db,
            "orchestration_v2_turn_item_positions",
            "turn_item_id",
            &item_id,
        )?);
    }
    let provider = json!({"id":provider_id,"driver":s.provider.driver(),"providerInstanceId":r.provider_instance_id,"providerSessionId":null,"appThreadId":id,"ownerNodeId":null,"nativeThreadRef":{"driver":s.provider.driver(),"nativeId":s.session_id,"strength":"strong"},"nativeConversationHeadRef":null,"status":"idle","firstRunOrdinal":null,"lastRunOrdinal":null,"handoffIds":[],"forkedFrom":null,"pendingBackgroundTasks":[],"createdAt":created,"updatedAt":updated});
    let provider_event = encoded::<payload::ProviderThread>(provider.clone())?;
    event(
        db,
        &format!("{prefix}:provider-thread:{provider_id}"),
        &id,
        "provider-thread.updated",
        &updated,
        &provider_event,
        &encoded::<payload::Metadata>(
            json!({"driver":s.provider.driver(),"providerInstanceId":r.provider_instance_id}),
        )?,
        &mut rows,
    )?;
    // ProjectionStore decodes these optional defaults before serializing its provider row.
    let mut provider_projection = provider;
    for key in ["contextUsage", "nativeMetadata", "goal"] {
        provider_projection[key] = Value::Null;
    }
    let provider_projection = encoded::<payload::ProviderThread>(provider_projection)?;
    guard::validate("provider-thread", &provider_projection)?;
    db.execute("INSERT INTO orchestration_v2_projection_provider_threads(provider_thread_id,thread_id,owner_node_id,provider,provider_session_id,status,first_run_ordinal,last_run_ordinal,updated_at,payload_json,driver,provider_instance_id) VALUES (?1,?2,NULL,?3,NULL,'idle',NULL,NULL,?4,?5,?6,?3)", params![provider_id,id,r.provider_instance_id,updated,provider_projection,s.provider.driver()])?;
    // All intermediate activity timestamps are superseded by the final provider event.
    rows.push(WrittenRow::capture(
        db,
        "orchestration_v2_projection_threads",
        "thread_id",
        &id,
    )?);
    rows.push(WrittenRow::capture(
        db,
        "orchestration_v2_projection_provider_threads",
        "provider_thread_id",
        &provider_id,
    )?);
    let cursor = match s.provider {
        Provider::Claude => Some(encoded::<payload::ClaudeCursor>(
            json!({"threadId":id,"resume":s.session_id}),
        )?),
        Provider::Codex => Some(encoded::<payload::CodexCursor>(
            json!({"threadId":s.session_id}),
        )?),
        // V2 adapters resume from nativeThreadRef; no fabricated legacy cursor is needed.
        Provider::Grok | Provider::OpenCode => None,
    };
    let mut runtime = json!({"cwd":r.placement.workspace_root});
    if matches!(s.provider, Provider::Claude | Provider::Codex) {
        runtime["importedTranscripts"] = json!([source_identity(r)?]);
    }
    let runtime = encoded::<payload::Runtime>(runtime)?;
    guard::validate("runtime", &runtime)?;
    db.execute("INSERT INTO provider_session_runtime(thread_id,provider_name,adapter_key,runtime_mode,status,last_seen_at,resume_cursor_json,runtime_payload_json,provider_instance_id) VALUES (?1,?2,?2,'full-access','stopped',?3,?4,?5,?6) ON CONFLICT(thread_id) DO NOTHING", params![id,s.provider.driver(),updated,cursor,runtime,r.provider_instance_id])?;
    rows.push(WrittenRow::capture(
        db,
        "provider_session_runtime",
        "thread_id",
        &id,
    )?);
    // Runtime inserts also change last_insert_rowid: derive the actual final event sequence.
    let sequence: i64 = db.query_row("SELECT max(sequence) FROM orchestration_events WHERE application_event_version=2 AND aggregate_kind='thread'", [], |r| r.get(0))?;
    db.execute("INSERT INTO orchestration_v2_projection_metadata(projection_name,schema_version,last_sequence,updated_at) VALUES ('thread-projections',2,?1,?2) ON CONFLICT(projection_name) DO UPDATE SET schema_version=2,last_sequence=excluded.last_sequence,updated_at=excluded.updated_at", params![sequence,iso(Utc::now())])?;
    Ok(rows)
}

fn source_identity(r: &ImportRequest) -> anyhow::Result<Value> {
    let s = &r.transcript.summary;
    let meta = s.source.metadata().with_context(|| {
        format!(
            "cannot record imported transcript identity: {}",
            s.source.display()
        )
    })?;
    let millis = |time: std::io::Result<std::time::SystemTime>| {
        time.ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
    };
    #[cfg(unix)]
    let (device, inode) = {
        use std::os::unix::fs::MetadataExt;
        (meta.dev(), Some(meta.ino()))
    };
    #[cfg(not(unix))]
    let (device, inode): (u64, Option<u64>) = (0, None);
    Ok(
        json!({"provider":s.provider.driver(),"providerInstanceId":r.provider_instance_id,"providerSessionId":s.session_id,"filePath":s.source,"size":meta.len(),"mtimeMs":millis(meta.modified()),"device":device,"inode":inode,"birthtimeMs":millis(meta.created())}),
    )
}
