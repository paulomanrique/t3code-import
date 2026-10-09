use super::*;
use crate::model::{Message, Role, SessionSummary};
use chrono::{TimeZone, Utc};
use rusqlite::{Connection, OpenFlags, params};
use serde_json::{Value, json};

// Tests never open the user's database for writing and never launch the T3 app.
fn database_copy() -> anyhow::Result<Option<(tempfile::TempDir, T3Home)>> {
    let Some(real) = dirs::home_dir()
        .map(|p| p.join(".t3/userdata/statev2.sqlite"))
        .filter(|p| p.is_file())
    else {
        no_database()?;
        return Ok(None);
    };
    let dir = tempfile::tempdir()?;
    std::fs::create_dir(dir.path().join("userdata"))?;
    let source = Connection::open_with_flags(real, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let path = dir.path().join("userdata/statev2.sqlite");
    // Pin a read snapshot so a live T3 writer cannot repeatedly restart the online backup.
    source.execute_batch("BEGIN DEFERRED; SELECT name FROM sqlite_master LIMIT 1;")?;
    writer::backup(&source, &path)?;
    source.execute_batch("ROLLBACK")?;
    let home = T3Home::detect(Some(dir.path().into()))?;
    Ok(Some((dir, home)))
}
fn no_database() -> anyhow::Result<()> {
    eprintln!("SKIP: no T3 database");
    anyhow::ensure!(
        std::env::var("T3IMPORT_REQUIRE_DB").as_deref() != Ok("1"),
        "T3IMPORT_REQUIRE_DB=1 but no T3 database"
    );
    Ok(())
}
struct ClosedInventory;
impl ClosedInventory {
    fn new() -> Self {
        guard::TEST_PROCESSES.with(|v| *v.borrow_mut() = Some(vec![]));
        Self
    }
}
impl Drop for ClosedInventory {
    fn drop(&mut self) {
        guard::TEST_PROCESSES.with(|v| *v.borrow_mut() = None);
    }
}
fn request(home: &T3Home, provider: Provider, session: &str) -> anyhow::Result<ImportRequest> {
    let state = snapshot(home)?;
    let project = state.projects.first().expect("real DB has projects");
    let source = home
        .base
        .join(format!("synthetic-{}.jsonl", session.replace('/', "_")));
    std::fs::write(&source, b"synthetic transcript\n")?;
    let at = Utc.with_ymd_and_hms(2026, 10, 1, 12, 0, 0).unwrap();
    Ok(ImportRequest {
        provider_instance_id: provider.driver().into(),
        placement: Placement {
            workspace_root: project.workspace_root.clone(),
            worktree_path: None,
            branch: Some("test-branch".into()),
            worktree_missing: false,
        },
        transcript: crate::model::Transcript {
            summary: SessionSummary {
                provider,
                session_id: session.into(),
                cwd: project.workspace_root.clone(),
                title: "Synthetic transcript".into(),
                created_at: at,
                updated_at: at + chrono::Duration::seconds(2),
                message_count: Some(2),
                model: Some("test-model".into()),
                source,
                resumable: true,
            },
            messages: vec![
                Message {
                    role: Role::User,
                    text: "Synthetic user text".into(),
                    created_at: at,
                },
                Message {
                    role: Role::Assistant,
                    text: "Synthetic assistant text".into(),
                    created_at: at + chrono::Duration::seconds(1),
                },
            ],
        },
    })
}
fn findings(report: &GuardReport) -> String {
    report
        .findings
        .iter()
        .map(|f| format!("{}: {}", f.location, f.actual))
        .take(15)
        .collect::<Vec<_>>()
        .join("\n")
}
#[test]
fn real_copy_guard_accepts_current_contract() -> anyhow::Result<()> {
    let _closed = ClosedInventory::new();
    let Some((_dir, home)) = database_copy()? else {
        return Ok(());
    };
    let report = check(&home)?;
    assert!(report.ok(), "{}", findings(&report));
    std::fs::write(home.userdata().join("settings.json"), br#"{"providerInstances":{"claudeAgent":{"driver":"claudeAgent","enabled":false},"work-claude":{"driver":"claudeAgent","enabled":true},"codex":{"driver":"codex","enabled":true},"spare-codex":{"driver":"codex","enabled":true}}}"#)?;
    let state = snapshot(&home)?;
    assert_eq!(
        state.provider_instances[&Provider::Claude],
        vec!["work-claude"]
    );
    assert_eq!(
        state.provider_instances[&Provider::Codex],
        vec!["codex", "spare-codex"]
    );
    assert_eq!(state.provider_instances[&Provider::Grok], vec!["grok"]);
    assert!(state.thread_ids.iter().any(|id| id.starts_with("import:")));
    Ok(())
}
#[test]
fn imports_match_native_rows_and_reimport_skips() -> anyhow::Result<()> {
    let _closed = ClosedInventory::new();
    let Some((_dir, home)) = database_copy()? else {
        return Ok(());
    };
    assert!(check(&home)?.ok());
    let claude = request(
        &home,
        Provider::Claude,
        "a8a31111-1111-4111-8111-111111111111",
    )?;
    let mut grok = request(&home, Provider::Grok, "synthetic:grok/session")?;
    grok.transcript.summary.model = Some("  test-model  ".into());
    let requests = vec![claude.clone(), grok.clone()];
    let db = guard::readonly(&home)?;
    // A V1-only history for this ID must not force V2 stream_version back to zero.
    drop(db);
    {
        let db = Connection::open(home.db_path())?;
        db.execute("INSERT INTO orchestration_events(event_id,aggregate_kind,stream_id,stream_version,event_type,occurred_at,command_id,actor_kind,payload_json,metadata_json,application_event_version) VALUES ('synthetic-v1','thread',?1,201,'thread.created','2026-10-01T12:00:00.000Z',NULL,'client','{}','{}',1)", [thread_id_for(&claude.provider_instance_id,&claude.transcript.summary.session_id)])?;
    }
    let report = import(
        &home,
        requests.clone(),
        &ImportOptions::default(),
        &mut |_| {},
    )?;
    assert_eq!(report.imported.len(), 2);
    assert!(report.backup.as_ref().unwrap().is_file());
    assert!(report.projects_created.is_empty());
    let checked = check(&home)?;
    assert!(checked.ok(), "{}", findings(&checked));
    let db = guard::readonly(&home)?;
    for request in &requests {
        let id = thread_id_for(
            &request.provider_instance_id,
            &request.transcript.summary.session_id,
        );
        let expected_provider = match request.transcript.summary.provider {
            Provider::Claude => {
                "provider-thread:provider:claudeAgent:provider-instance:claudeAgent:native-thread:a8a31111-1111-4111-8111-111111111111"
            }
            Provider::Grok => {
                "provider-thread:provider:grok:provider-instance:grok:native-thread:synthetic%3Agrok%2Fsession"
            }
            _ => unreachable!(),
        };
        let provider_id: String = db.query_row("SELECT provider_thread_id FROM orchestration_v2_projection_provider_threads WHERE thread_id=?", [&id], |r| r.get(0))?;
        assert_eq!(provider_id, expected_provider);
        let active: String = db.query_row("SELECT active_provider_thread_id FROM orchestration_v2_projection_threads WHERE thread_id=?", [&id], |r| r.get(0))?;
        assert_eq!(active, expected_provider);
        let branch: Option<String> = db.query_row("SELECT json_extract(payload_json,'$.branch') FROM orchestration_v2_projection_threads WHERE thread_id=?", [&id], |r| r.get(0))?;
        assert_eq!(branch, None);
        let raw: String = db.query_row(
            "SELECT payload_json FROM orchestration_v2_projection_threads WHERE thread_id=?",
            [&id],
            |r| r.get(0),
        )?;
        let mut payload: Value = serde_json::from_str(&raw)?;
        assert_eq!(payload["modelSelection"]["model"], "test-model");
        for invalid in ["", "   ", " untrimmed", "untrimmed "] {
            payload["modelSelection"]["model"] = json!(invalid);
            assert!(guard::validate("thread", &serde_json::to_string(&payload)?).is_err());
        }
        assert_native_shapes(&db, &id)?;
        let versions: Vec<i64> = db.prepare("SELECT stream_version FROM orchestration_events WHERE stream_id=? AND application_event_version=2 ORDER BY sequence")?.query_map([&id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        let start = if request.transcript.summary.provider == Provider::Claude {
            202
        } else {
            0
        };
        assert_eq!(versions, (start..start + 6).collect::<Vec<_>>());
        let cursor: Option<String> = db.query_row(
            "SELECT resume_cursor_json FROM provider_session_runtime WHERE thread_id=?",
            [&id],
            |r| r.get(0),
        )?;
        if request.transcript.summary.provider == Provider::Claude {
            assert_eq!(
                cursor.unwrap(),
                format!(
                    r#"{{"threadId":"{id}","resume":"{}"}}"#,
                    request.transcript.summary.session_id
                )
            );
        } else {
            assert!(cursor.is_none());
        }
        let runtime: String = db.query_row(
            "SELECT runtime_payload_json FROM provider_session_runtime WHERE thread_id=?",
            [&id],
            |r| r.get(0),
        )?;
        let value: Value = serde_json::from_str(&runtime)?;
        assert_eq!(value["cwd"], json!(request.placement.workspace_root));
        if request.transcript.summary.provider == Provider::Claude {
            let native: String = db.query_row("SELECT runtime_payload_json FROM provider_session_runtime WHERE thread_id LIKE 'import:claudeAgent:%' AND thread_id!=? AND json_array_length(runtime_payload_json,'$.importedTranscripts')>0 LIMIT 1", [&id], |r| r.get(0))?;
            assert_eq!(key_order(&runtime)?, key_order(&native)?);
            let native: Value = serde_json::from_str(&native)?;
            assert_eq!(
                object_keys(&value["importedTranscripts"][0]),
                object_keys(&native["importedTranscripts"][0])
            );
        } else {
            assert_eq!(object_keys(&value), vec!["cwd"]);
        }
    }
    let metadata: i64 = db.query_row("SELECT last_sequence FROM orchestration_v2_projection_metadata WHERE projection_name='thread-projections'",[],|r|r.get(0))?;
    let latest: i64 = db.query_row("SELECT max(sequence) FROM orchestration_events WHERE application_event_version=2 AND aggregate_kind='thread'",[],|r|r.get(0))?;
    assert_eq!(metadata, latest);
    assert!(home.userdata().join("t3-import-provenance.json").is_file());
    let before: i64 = db.query_row("SELECT count(*) FROM orchestration_events", [], |r| {
        r.get(0)
    })?;
    drop(db);
    let again = import(&home, requests, &ImportOptions::default(), &mut |_| {})?;
    assert_eq!(again.skipped.len(), 2);
    assert!(again.imported.is_empty());
    assert!(again.backup.is_none());
    assert_eq!(
        guard::readonly(&home)?.query_row(
            "SELECT count(*) FROM orchestration_events",
            [],
            |r| r.get::<_, i64>(0)
        )?,
        before
    );
    Ok(())
}
fn object_keys(v: &Value) -> Vec<&str> {
    v.as_object().unwrap().keys().map(String::as_str).collect()
}
// serde_json's ordinary Value sorts keys: this visitor preserves the raw wire order.
fn key_order(raw: &str) -> anyhow::Result<Vec<String>> {
    struct Keys;
    impl<'de> serde::de::Visitor<'de> for Keys {
        type Value = Vec<String>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("object")
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(
            self,
            mut map: A,
        ) -> Result<Self::Value, A::Error> {
            let mut keys = Vec::new();
            while let Some((key, _)) = map.next_entry::<String, Value>()? {
                keys.push(key);
            }
            Ok(keys)
        }
    }
    use serde::Deserializer;
    Ok(serde_json::Deserializer::from_str(raw).deserialize_map(Keys)?)
}
fn assert_native_shapes(db: &Connection, id: &str) -> anyhow::Result<()> {
    let events: Vec<(String,String,String)> = db.prepare("SELECT event_type,payload_json,metadata_json FROM orchestration_events WHERE stream_id=? AND application_event_version=2 ORDER BY sequence")?.query_map([id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?.collect::<rusqlite::Result<_>>()?;
    assert_eq!(events.len(), 6);
    for (kind, raw, metadata) in events {
        let value: Value = serde_json::from_str(&raw)?;
        let item_type = value["type"].as_str();
        let native: String = db.query_row("SELECT payload_json FROM orchestration_events WHERE application_event_version=2 AND event_type=?1 AND event_id LIKE 'agent-session-import:v2:%' AND stream_id!=?2 AND (?3 IS NULL OR json_extract(payload_json,'$.type')=?3) AND (?4 IS NULL OR json_extract(payload_json,'$.role')=?4) ORDER BY sequence DESC LIMIT 1", params![kind,id,item_type,value.get("role").and_then(Value::as_str)], |r| r.get(0))?;
        assert_eq!(key_order(&raw)?, key_order(&native)?, "{kind}");
        // Exact bytes after substituting variable identities, timestamps, text and placement.
        let mut template: Value = serde_json::from_str(&native)?;
        for key in object_keys(&value) {
            template[key] = value[key].clone();
        }
        let canonical = match kind.as_str() {
            "thread.created" => {
                serde_json::to_string(&serde_json::from_value::<payload::Thread>(template)?)?
            }
            "message.updated" => {
                serde_json::to_string(&serde_json::from_value::<payload::Message>(template)?)?
            }
            "turn-item.updated" if item_type == Some("user_message") => {
                serde_json::to_string(&serde_json::from_value::<payload::UserItem>(template)?)?
            }
            "turn-item.updated" => {
                serde_json::to_string(&serde_json::from_value::<payload::AssistantItem>(template)?)?
            }
            "provider-thread.updated" => serde_json::to_string(&serde_json::from_value::<
                payload::ProviderThread,
            >(template)?)?,
            _ => panic!("unexpected event"),
        };
        assert_eq!(raw, canonical);
        let instance: String = db.query_row("SELECT provider_instance_id FROM orchestration_v2_projection_threads WHERE thread_id=?",[id],|r|r.get(0))?;
        let expected = match kind.as_str() {
            "thread.created" => format!(r#"{{"providerInstanceId":"{instance}"}}"#),
            "provider-thread.updated" => {
                format!(r#"{{"driver":"{instance}","providerInstanceId":"{instance}"}}"#)
            }
            _ => "{}".into(),
        };
        assert_eq!(metadata, expected);
        let (table, key, entity) = match kind.as_str() {
            "thread.created" => (
                "orchestration_v2_projection_threads",
                "thread_id",
                id.to_owned(),
            ),
            "message.updated" => (
                "orchestration_v2_projection_messages",
                "message_id",
                value["id"].as_str().unwrap().into(),
            ),
            "turn-item.updated" => (
                "orchestration_v2_projection_turn_items",
                "turn_item_id",
                value["id"].as_str().unwrap().into(),
            ),
            "provider-thread.updated" => (
                "orchestration_v2_projection_provider_threads",
                "provider_thread_id",
                value["id"].as_str().unwrap().into(),
            ),
            _ => unreachable!(),
        };
        let native_value: Value = serde_json::from_str(&native)?;
        let native_id = native_value["id"].as_str().unwrap();
        assert_sql_row_shape(db, table, key, &entity, native_id)?;
        let projection: String = db.query_row(
            &format!("SELECT payload_json FROM {table} WHERE {key}=?"),
            [&entity],
            |r| r.get(0),
        )?;
        let mut expected_projection = value.clone();
        if kind == "provider-thread.updated" {
            for key in ["contextUsage", "nativeMetadata", "goal"] {
                expected_projection[key] = Value::Null;
            }
        }
        assert_eq!(
            serde_json::from_str::<Value>(&projection)?,
            expected_projection
        );
        if kind == "turn-item.updated" {
            let ordinal:i64=db.query_row("SELECT ordinal FROM orchestration_v2_turn_item_positions WHERE thread_id=?1 AND turn_item_id=?2",params![id,entity],|r|r.get(0))?;
            assert_eq!(ordinal, value["ordinal"].as_i64().unwrap());
        }
    }
    let rows: i64=db.query_row("SELECT count(*) FROM orchestration_events WHERE stream_id=? AND application_event_version=2 AND (aggregate_kind!='thread' OR actor_kind!='server' OR command_id IS NOT NULL OR causation_event_id IS NOT NULL OR correlation_id IS NOT NULL)",[id],|r|r.get(0))?;
    assert_eq!(rows, 0);
    Ok(())
}
#[test]
fn structure_and_payload_drift_fail_closed() -> anyhow::Result<()> {
    let _closed = ClosedInventory::new();
    let Some((_dir, home)) = database_copy()? else {
        return Ok(());
    };
    let db = Connection::open(home.db_path())?;
    db.execute(
        "ALTER TABLE orchestration_v2_projection_messages ADD COLUMN surprise TEXT",
        [],
    )?;
    let report = check(&home)?;
    assert!(!report.ok());
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.location.contains("projection_messages"))
    );
    assert!(report.issue_url.unwrap().contains("issues/new"));
    db.execute(
        "ALTER TABLE orchestration_v2_projection_messages DROP COLUMN surprise",
        [],
    )?;
    db.execute(
        "CREATE INDEX synthetic_drift_index ON orchestration_v2_projection_messages(role)",
        [],
    )?;
    assert!(
        check(&home)?
            .findings
            .iter()
            .any(|f| f.location == "sqlite_master.synthetic_drift_index")
    );
    db.execute("DROP INDEX synthetic_drift_index", [])?;
    db.execute("UPDATE orchestration_v2_projection_threads SET updated_at='2099-01-01T00:00:00.000Z',payload_json=json_set(payload_json,'$.unexpectedFutureKey','PRIVATE-THREAD-CONTENT') WHERE thread_id=(SELECT thread_id FROM orchestration_v2_projection_threads LIMIT 1)",[])?;
    let report = check(&home)?;
    assert!(!report.ok());
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.actual.contains("unexpectedFutureKey"))
    );
    assert!(!report.issue_url.unwrap().contains("PRIVATE-THREAD-CONTENT"));
    Ok(())
}
#[test]
fn dry_run_does_not_write_and_limit_keeps_newest() -> anyhow::Result<()> {
    let _closed = ClosedInventory::new();
    let Some((_dir, home)) = database_copy()? else {
        return Ok(());
    };
    let mut request = request(&home, Provider::Grok, "dry-run")?;
    let before = std::fs::read(home.db_path())?;
    request.placement.workspace_root = home.base.join("missing-project");
    std::fs::create_dir(&request.placement.workspace_root)?;
    guard::TEST_PROCESSES.with(|v| *v.borrow_mut() = Some(vec!["PID 123: T3 service".into()]));
    let report = import(
        &home,
        vec![request.clone()],
        &ImportOptions {
            dry_run: true,
            max_messages: Some(1),
        },
        &mut |_| {},
    )?;
    assert_eq!(report.imported.len(), 1);
    assert!(report.warnings.iter().any(|w| w.contains("T3 running")));
    guard::TEST_PROCESSES.with(|v| *v.borrow_mut() = Some(vec![]));
    assert_eq!(std::fs::read(home.db_path())?, before);
    assert_eq!(std::fs::read_dir(home.userdata())?.count(), 1);
    request.placement.workspace_root = snapshot(&home)?.projects[0].workspace_root.clone();
    import(
        &home,
        vec![request.clone()],
        &ImportOptions {
            dry_run: false,
            max_messages: Some(1),
        },
        &mut |_| {},
    )?;
    let db = guard::readonly(&home)?;
    let raw: String = db.query_row(
        "SELECT payload_json FROM orchestration_v2_projection_messages WHERE thread_id=?",
        [thread_id_for("grok", "dry-run")],
        |r| r.get(0),
    )?;
    assert_eq!(
        serde_json::from_str::<Value>(&raw)?["text"],
        "Synthetic assistant text"
    );
    Ok(())
}
#[test]
fn placement_reads_linked_worktree_and_head() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let main = temp.path().join("main");
    let work = temp.path().join("work");
    std::fs::create_dir_all(main.join(".git/worktrees/work"))?;
    std::fs::create_dir(&work)?;
    std::fs::write(
        work.join(".git"),
        format!("gitdir: {}\n", main.join(".git/worktrees/work").display()),
    )?;
    std::fs::write(main.join(".git/worktrees/work/commondir"), "../..\n")?;
    std::fs::write(
        main.join(".git/worktrees/work/HEAD"),
        "ref: refs/heads/feature/test\n",
    )?;
    let p = resolve_placement(&work);
    assert_eq!(p.workspace_root, main.canonicalize()?);
    assert_eq!(p.worktree_path, Some(work.canonicalize()?));
    assert_eq!(p.branch.as_deref(), Some("feature/test"));
    assert!(resolve_placement(&temp.path().join("gone")).worktree_missing);
    Ok(())
}
#[test]
fn required_nullable_and_unknown_nested_keys() {
    assert!(
        serde_json::from_value::<payload::Lineage>(
            json!({"relationshipToParent":null,"rootThreadId":"t"})
        )
        .is_err()
    );
    assert!(guard::validate("codex", r#"{"threadId":"t","extra":1}"#).is_err());
    assert!(guard::validate("codex", r#"{"threadId":123}"#).is_err());
}

#[test]
fn readonly_contract_diagnostics() -> anyhow::Result<()> {
    let _closed = ClosedInventory::new();
    let Some(base) = dirs::home_dir()
        .map(|p| p.join(".t3"))
        .filter(|p| p.join("userdata/statev2.sqlite").is_file())
    else {
        no_database()?;
        return Ok(());
    };
    let home = T3Home { base };
    let report = guard::check(&home)?;
    assert!(report.ok(), "{}", findings(&report));
    Ok(())
}

fn assert_sql_row_shape(
    db: &Connection,
    table: &str,
    key: &str,
    id: &str,
    native_id: &str,
) -> anyhow::Result<()> {
    use rusqlite::types::Value as SqlValue;
    let mut query = db.prepare(&format!("SELECT * FROM {table} WHERE {key}=?"))?;
    let columns = query
        .column_names()
        .iter()
        .map(|c| c.to_string())
        .collect::<Vec<_>>();
    let read = |row: &rusqlite::Row<'_>| -> rusqlite::Result<Vec<SqlValue>> {
        (0..columns.len()).map(|i| row.get(i)).collect()
    };
    let actual = query.query_row([id], read)?;
    let native = query.query_row([native_id], read)?;
    assert_eq!(actual.len(), native.len());
    for ((column, actual), native) in columns.iter().zip(actual).zip(native) {
        assert_eq!(
            std::mem::discriminant(&actual),
            std::mem::discriminant(&native),
            "{table}.{column}"
        );
        if column == "payload_json" {
            let (SqlValue::Text(actual), SqlValue::Text(native)) = (actual, native) else {
                panic!("JSON text required");
            };
            assert_eq!(key_order(&actual)?, key_order(&native)?, "{table}.{column}");
        } else if [
            "runtime_mode",
            "interaction_mode",
            "role",
            "streaming",
            "status",
            "owner_node_id",
            "run_id",
            "node_id",
            "provider_session_id",
            "provider_thread_id",
            "provider_turn_id",
            "parent_item_id",
            "first_run_ordinal",
            "last_run_ordinal",
            "archived_at",
            "deleted_at",
        ]
        .contains(&column.as_str())
            && column != key
        {
            assert_eq!(actual, native, "{table}.{column}");
        }
    }
    Ok(())
}
#[test]
fn transaction_rolls_back_all_threads_on_late_failure() -> anyhow::Result<()> {
    let _closed = ClosedInventory::new();
    let Some((_dir, home)) = database_copy()? else {
        return Ok(());
    };
    let first = request(&home, Provider::Grok, "rollback-first")?;
    let mut second = request(
        &home,
        Provider::Claude,
        "b8b32222-2222-4222-8222-222222222222",
    )?;
    second.transcript.summary.source = home.base.join("nonexistent-source");
    let before = guard::readonly(&home)?.query_row(
        "SELECT count(*) FROM orchestration_events",
        [],
        |r| r.get::<_, i64>(0),
    )?;
    assert!(
        import(
            &home,
            vec![first, second],
            &ImportOptions::default(),
            &mut |_| {}
        )
        .is_err()
    );
    let db = guard::readonly(&home)?;
    assert_eq!(
        db.query_row("SELECT count(*) FROM orchestration_events", [], |r| r
            .get::<_, i64>(0))?,
        before
    );
    assert!(
        !snapshot(&home)?
            .thread_ids
            .contains(&thread_id_for("grok", "rollback-first"))
    );
    assert!(check(&home)?.ok());
    Ok(())
}
#[test]
fn running_process_refuses_before_backup() -> anyhow::Result<()> {
    let _closed = ClosedInventory::new();
    let Some((_dir, home)) = database_copy()? else {
        return Ok(());
    };
    let request = request(&home, Provider::Grok, "running")?;
    guard::TEST_PROCESSES.with(|v| *v.borrow_mut() = Some(vec!["PID 123: T3 service".into()]));
    assert!(import(&home, vec![request], &ImportOptions::default(), &mut |_| {}).is_err());
    assert_eq!(std::fs::read_dir(home.userdata())?.count(), 1);
    Ok(())
}

#[test]
fn missing_worktree_requires_registered_owner() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let main = temp.path().join("main");
    let cwd = main.join("gone");
    let admin = main.join(".git/worktrees/gone");
    std::fs::create_dir_all(&admin)?;
    std::fs::write(main.join(".git/HEAD"), "ref: refs/heads/unrelated\n")?;
    let unowned = resolve_placement(&cwd);
    assert!(unowned.worktree_missing);
    assert_eq!(unowned.worktree_path, None);
    assert_eq!(unowned.branch, None);
    std::fs::write(
        admin.join("gitdir"),
        format!("{}\n", cwd.join(".git").display()),
    )?;
    std::fs::write(admin.join("HEAD"), "ref: refs/heads/deleted-worktree\n")?;
    let owned = resolve_placement(&cwd);
    assert_eq!(owned.workspace_root, main);
    assert_eq!(owned.worktree_path, Some(cwd.clone()));
    assert_eq!(owned.branch.as_deref(), Some("deleted-worktree"));
    assert!(owned.worktree_missing);
    std::fs::remove_file(admin.join("HEAD"))?;
    assert_eq!(resolve_placement(&cwd).branch, None);
    assert_eq!(resolve_placement(&main).branch, None);
    Ok(())
}

#[test]
fn missing_unowned_folder_skips_without_project_add() -> anyhow::Result<()> {
    let _closed = ClosedInventory::new();
    let Some((_dir, home)) = database_copy()? else {
        return Ok(());
    };
    let mut req = request(&home, Provider::Grok, "gone-folder")?;
    req.transcript.summary.cwd = home.base.join("gone");
    req.placement = resolve_placement(&req.transcript.summary.cwd);
    let before = std::fs::read(home.db_path())?;
    let report = import(&home, vec![req], &ImportOptions::default(), &mut |p| {
        assert!(!matches!(p, Progress::CreatingProject(_)));
    })?;
    assert_eq!(
        report.skipped[0].1,
        format!(
            "folder no longer exists: {}",
            home.base.join("gone").display()
        )
    );
    assert!(report.backup.is_none());
    assert_eq!(std::fs::read(home.db_path())?, before);
    Ok(())
}

#[test]
fn native_session_started_by_t3_is_skipped() -> anyhow::Result<()> {
    let _closed = ClosedInventory::new();
    let Some((_dir, home)) = database_copy()? else {
        return Ok(());
    };
    let db = Connection::open(home.db_path())?;
    let (provider_id, raw): (String, String) = db.query_row("SELECT provider_thread_id,payload_json FROM orchestration_v2_projection_provider_threads WHERE thread_id IS NOT NULL LIMIT 1", [], |r| Ok((r.get(0)?,r.get(1)?)))?;
    let mut payload: Value = serde_json::from_str(&raw)?;
    payload["nativeThreadRef"] =
        json!({"driver":"grok","nativeId":"native-t3-session","strength":"strong"});
    db.execute("UPDATE orchestration_v2_projection_provider_threads SET payload_json=? WHERE provider_thread_id=?", params![serde_json::to_string(&payload)?,provider_id])?;
    let owner: String = db.query_row("SELECT thread_id FROM orchestration_v2_projection_provider_threads WHERE provider_thread_id=?", [&provider_id], |r| r.get(0))?;
    let snap = snapshot(&home)?;
    assert!(snap.has_native(Provider::Grok, "native-t3-session"));
    assert!(!snap.has_native(Provider::Codex, "native-t3-session"));
    let req = request(&home, Provider::Grok, "native-t3-session")?;
    let before: i64 = db.query_row("SELECT count(*) FROM orchestration_events", [], |r| {
        r.get(0)
    })?;
    let report = import(&home, vec![req], &ImportOptions::default(), &mut |_| {})?;
    assert_eq!(
        report.skipped[0].1,
        format!("already in T3 as thread {owner}")
    );
    assert!(report.imported.is_empty());
    assert!(report.backup.is_none());
    assert_eq!(
        db.query_row("SELECT count(*) FROM orchestration_events", [], |r| r
            .get::<_, i64>(0))?,
        before
    );
    Ok(())
}

#[test]
fn existing_runtime_merges_cwd_and_deduplicates_sources() -> anyhow::Result<()> {
    let _closed = ClosedInventory::new();
    let Some((_dir, home)) = database_copy()? else {
        return Ok(());
    };
    let mut req = request(&home, Provider::Codex, "runtime-merge")?;
    req.transcript.summary.model = Some("   ".into());
    let id = thread_id_for("codex", "runtime-merge");
    let source = json!({"provider":"codex","providerInstanceId":"codex","providerSessionId":"runtime-merge","filePath":req.transcript.summary.source,"size":0,"mtimeMs":null,"device":0,"inode":null,"birthtimeMs":null});
    let other = json!({"provider":"codex","providerInstanceId":"codex","providerSessionId":"other","filePath":"/other","size":0,"mtimeMs":null,"device":0,"inode":null,"birthtimeMs":null});
    let db = Connection::open(home.db_path())?;
    db.execute("INSERT INTO provider_session_runtime(thread_id,provider_name,adapter_key,runtime_mode,status,last_seen_at,runtime_payload_json,provider_instance_id) VALUES (?1,'codex','codex','full-access','stopped','2026-10-01T12:00:00.000Z',?2,'codex')", params![id,serde_json::to_string(&json!({"cwd":"/old","importedTranscripts":[source,other]}))?])?;
    let expected_cwd = snapshot(&home)?.projects[0].workspace_root.clone();
    // Resolve through an alias, but keep the stored project's spelling in runtime.cwd.
    #[cfg(unix)]
    {
        let alias = home.base.join("project-alias");
        std::os::unix::fs::symlink(&expected_cwd, &alias)?;
        req.placement.workspace_root = alias;
    }
    import(&home, vec![req], &ImportOptions::default(), &mut |_| {})?;
    let raw: String = db.query_row(
        "SELECT runtime_payload_json FROM provider_session_runtime WHERE thread_id=?",
        [&id],
        |r| r.get(0),
    )?;
    let runtime: Value = serde_json::from_str(&raw)?;
    assert_eq!(runtime["cwd"], json!(expected_cwd));
    assert_eq!(runtime["importedTranscripts"].as_array().unwrap().len(), 2);
    let model: String = db.query_row("SELECT json_extract(payload_json,'$.modelSelection.model') FROM orchestration_v2_projection_threads WHERE thread_id=?", [&id], |r| r.get(0))?;
    assert_eq!(model, "gpt-6-astra");
    Ok(())
}

#[test]
fn registered_missing_worktree_imports_under_owning_project() -> anyhow::Result<()> {
    let _closed = ClosedInventory::new();
    let Some((_dir, home)) = database_copy()? else {
        return Ok(());
    };
    let main = home.base.join("main");
    let cwd = main.join("gone");
    let admin = main.join(".git/worktrees/gone");
    std::fs::create_dir_all(&admin)?;
    std::fs::write(
        admin.join("gitdir"),
        format!("{}\n", cwd.join(".git").display()),
    )?;
    std::fs::write(admin.join("HEAD"), "ref: refs/heads/feature/gone\n")?;
    let project = snapshot(&home)?.projects[0].id.clone();
    let db = Connection::open(home.db_path())?;
    db.execute(
        "UPDATE projection_projects SET workspace_root=? WHERE project_id=?",
        params![main.to_string_lossy(), project],
    )?;
    let mut req = request(&home, Provider::Grok, "registered-gone")?;
    req.transcript.summary.cwd = cwd.clone();
    req.placement = resolve_placement(&cwd);
    let report = import(&home, vec![req], &ImportOptions::default(), &mut |p| {
        assert!(!matches!(p, Progress::CreatingProject(_)));
    })?;
    assert_eq!(report.imported.len(), 1);
    assert!(report.projects_created.is_empty());
    assert!(report.warnings.iter().any(|w| w.contains("missing")));
    let raw: String = db.query_row("SELECT payload_json FROM orchestration_v2_projection_threads WHERE thread_id='import:grok:registered-gone'", [], |r| r.get(0))?;
    let thread: Value = serde_json::from_str(&raw)?;
    assert_eq!(thread["projectId"], project);
    assert_eq!(thread["branch"], "feature/gone");
    assert_eq!(thread["worktreePath"], json!(cwd));
    Ok(())
}
