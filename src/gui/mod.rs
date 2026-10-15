use crate::{
    catalog,
    model::SessionSummary,
    scan::Roots,
    t3db::{self, Snapshot, T3Home, control::Restart},
    trash,
};
use anyhow::Context;
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tauri::{Emitter, Manager, State};
use tauri_plugin_opener::OpenerExt;

#[derive(Default)]
struct Data {
    sessions: HashMap<String, SessionSummary>,
    home: Option<T3Home>,
    snapshot: Option<Snapshot>,
    trash: HashMap<String, trash::Planned>,
}
struct Shared {
    roots: Roots,
    home_override: Option<PathBuf>,
    data: Mutex<Data>,
    restart: Mutex<Restart>,
    busy: AtomicBool,
    signal_exit: AtomicBool,
    close_warned: AtomicBool,
}
type SharedState = Arc<Shared>;
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Error {
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    restarted: Option<Vec<Value>>,
    backup: Option<String>,
    projects_created: Vec<String>,
}
impl From<anyhow::Error> for Error {
    fn from(e: anyhow::Error) -> Self {
        let failure = e.downcast_ref::<t3db::writer::ImportFailure>();
        Self {
            message: format!("{e:#}"),
            restarted: None,
            backup: failure
                .and_then(|f| f.backup.as_ref().map(|p| p.to_string_lossy().into_owned())),
            projects_created: failure
                .map(|f| {
                    f.projects_created
                        .iter()
                        .map(|p| p.to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, Error> + Send + 'static,
) -> Result<T, Error> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| Error::from(anyhow::anyhow!(e)))?
}
struct Busy<'a>(&'a Shared);
impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0.busy.store(false, Ordering::SeqCst);
        if self.0.signal_exit.load(Ordering::SeqCst) {
            exit_after_signal(self.0);
        }
    }
}
fn write<T>(s: &Shared, work: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
    s.busy
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .map_err(|_| Error::from(anyhow::anyhow!("Another operation is running")))?;
    let _busy = Busy(s);
    work()
}
fn summaries(s: &Shared, keys: &[String]) -> anyhow::Result<Vec<SessionSummary>> {
    let data = s.data.lock().unwrap();
    let mut seen = std::collections::HashSet::new();
    keys.iter()
        .filter(|k| seen.insert((*k).clone()))
        .map(|k| {
            data.sessions
                .get(k)
                .cloned()
                .with_context(|| format!("Session not found: {k}; scan again"))
        })
        .collect()
}
fn home(s: &Shared) -> anyhow::Result<T3Home> {
    T3Home::detect(s.home_override.clone())
}
fn restart(s: &Shared) -> Vec<Value> {
    restart_with(s, Restart::run)
}
fn restart_with(
    s: &Shared,
    run: impl FnOnce(&mut Restart) -> Vec<Result<String, String>>,
) -> Vec<Value> {
    let mut pending = std::mem::take(&mut *s.restart.lock().unwrap_or_else(|e| e.into_inner()));
    let lines = run_pending(&mut pending, run);
    s.restart
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .extend(pending);
    lines
}
// The same gate protects writes and signal shutdown. A signal during close()
// waits until its stopped jobs have been recorded, including during unwinding.
fn claim_signal_exit(s: &Shared) -> bool {
    s.signal_exit.store(true, Ordering::SeqCst);
    s.busy
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
}
fn exit_after_signal(s: &Shared) {
    // A second signal while a write is still running means the user wants out
    // now, even if that write is stuck (a Trash prompt that never returns, say).
    if s.signal_exit.load(Ordering::SeqCst) && s.busy.load(Ordering::SeqCst) {
        eprintln!("Quitting while an operation was still running.");
        std::process::exit(130);
    }
    if claim_signal_exit(s) {
        restart_before_exit(s);
        std::process::exit(0);
    }
}
fn restart_on_close(s: &Shared) -> Option<String> {
    if s.close_warned.load(Ordering::SeqCst) {
        return None;
    }
    restart_on_close_with(s, || restart_before_exit(s))
}
fn restart_on_close_with(s: &Shared, attempt: impl FnOnce() -> Option<String>) -> Option<String> {
    if s.close_warned.load(Ordering::SeqCst) {
        return None;
    }
    let failure = attempt();
    if failure.is_some() {
        s.close_warned.store(true, Ordering::SeqCst);
    }
    failure
}

fn run_pending(
    pending: &mut Restart,
    run: impl FnOnce(&mut Restart) -> Vec<Result<String, String>>,
) -> Vec<Value> {
    let original = pending.clone();
    let results = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(pending))) {
        Ok(results) => results,
        Err(_) => {
            *pending = original;
            vec![Err("T3 restart panicked; pending jobs retained".into())]
        }
    };
    results
        .into_iter()
        .map(|r| match r {
            Ok(text) => json!({"ok":true,"text":text}),
            Err(text) => json!({"ok":false,"text":text}),
        })
        .collect()
}
fn restart_before_exit(s: &Shared) -> Option<String> {
    let lines = restart(s);
    for line in &lines {
        eprintln!("{}", line["text"].as_str().unwrap_or_default());
    }
    let failures: Vec<_> = lines
        .iter()
        .filter(|l| l["ok"] == false)
        .filter_map(|l| l["text"].as_str())
        .collect();
    (!failures.is_empty()).then(|| failures.join("\n"))
}
fn normalize_cwd(item: &mut SessionSummary) {
    if !item.cwd.as_os_str().is_empty() {
        if let Ok(path) = std::path::absolute(&item.cwd) {
            item.cwd = path;
        }
    }
}
fn session_dto(item: &SessionSummary, snap: Option<&Snapshot>) -> Value {
    let p = t3db::resolve_placement(&item.cwd);
    let in_t3 = catalog::in_t3(snap, item);
    let blocked = if in_t3 {
        Some("in_t3")
    } else if !item.resumable {
        Some("not_resumable")
    } else if catalog::instance_for(snap, item.provider).is_none() {
        Some("no_provider")
    } else {
        None
    };
    json!({"key":catalog::key(item),"provider":item.provider,"sessionId":item.session_id,"title":item.title,"cwd":item.cwd.to_string_lossy(),
        "createdAt":item.created_at,"updatedAt":item.updated_at,"messageCount":item.message_count,"model":item.model,"source":item.source.to_string_lossy(),
        "inT3":in_t3,"importable":catalog::selectable(snap,item),"blocked":blocked,"providerOff":catalog::provider_disabled(snap,item.provider),
        "placement":{"root":p.workspace_root.to_string_lossy(),"worktreePath":p.worktree_path.as_ref().map(|p| p.to_string_lossy()),"branch":p.branch,"worktreeMissing":p.worktree_missing}})
}
fn validate_reveal<'a>(
    sources: impl Iterator<Item = &'a SessionSummary>,
    path: &std::path::Path,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        sources.into_iter().any(|s| s.source == path),
        "Path is not a scanned session source"
    );
    Ok(())
}
fn validate_url(url: &str) -> anyhow::Result<tauri::Url> {
    let parsed = tauri::Url::parse(url).context("Invalid URL")?;
    anyhow::ensure!(
        matches!(parsed.scheme(), "http" | "https") && parsed.host_str().is_some(),
        "Only http(s) URLs are allowed"
    );
    Ok(parsed)
}
fn own_origin(url: &tauri::Url) -> bool {
    url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && matches!(
            (url.scheme(), url.host_str()),
            ("tauri", Some("localhost")) | ("http" | "https", Some("tauri.localhost"))
        )
}

fn checked(s: &Shared) -> Result<Value, Error> {
    let home = home(s)?;
    let report = t3db::check(&home)?;
    let running = if report.t3_running.is_empty() {
        vec![]
    } else {
        t3db::control::find(&home)?
    };
    Ok(
        json!({"ok":report.ok(), "running":running.iter().map(|r| json!({"name":r.name(),"plan":r.plan(),"canClose":r.can_close()})).collect::<Vec<_>>(),
        "findings":report.findings.iter().map(|f| json!({"location":f.location,"expected":f.expected,"actual":f.actual})).collect::<Vec<_>>(),
        "warnings":report.warnings,"issueUrl":report.issue_url}),
    )
}
#[tauri::command]
async fn scan(state: State<'_, SharedState>) -> Result<Value, Error> {
    let s = state.inner().clone();
    blocking(move || {
        let mut sessions = crate::scan_all(&s.roots);
        for item in &mut sessions {
            normalize_cwd(item);
        }
        let detected = home(&s).and_then(|h| t3db::snapshot(&h).map(|snap| (h,snap)));
        let (home, snapshot, error) = match detected { Ok((h,snap)) => (Some(h),Some(snap),None), Err(e) => (None,None,Some(format!("{e:#}"))) };
        let snap = snapshot.as_ref();
        let rows: Vec<_> = sessions.iter().map(|item| session_dto(item, snap)).collect();
        let result = json!({"home":dirs::home_dir().map(|p| p.to_string_lossy().into_owned()),"t3":{"found":home.is_some(),"base":home.as_ref().map(|h| h.base.to_string_lossy()),"error":error,
            "projects":snap.map(|s| s.projects.iter().map(|p| p.workspace_root.to_string_lossy().into_owned()).collect::<Vec<_>>()).unwrap_or_default(),
            "disabledProviders":snap.map(|s| { let mut p = s.disabled_providers.iter().copied().collect::<Vec<_>>(); p.sort(); p }).unwrap_or_default()},
            "sessions":rows,"pendingRestart":s.restart.lock().unwrap_or_else(|e| e.into_inner()).names()});
        let mut data = s.data.lock().unwrap();
        data.sessions = sessions.into_iter().map(|s| (catalog::key(&s),s)).collect();
        data.home = home; data.snapshot = snapshot;
        Ok(result)
    }).await
}
#[tauri::command]
async fn transcript(state: State<'_, SharedState>, key: String) -> Result<Value, Error> {
    let s = state.inner().clone();
    blocking(move || {
        let item = summaries(&s,&[key])?.remove(0);
        let transcript = crate::scan::scanner_for(&s.roots,item.provider).load(&item)?;
        Ok(json!({"messages":transcript.messages.iter().map(|m| json!({"role":m.role,"text":m.text,"createdAt":m.created_at})).collect::<Vec<_>>()}))
    }).await
}
#[tauri::command]
async fn check(state: State<'_, SharedState>) -> Result<Value, Error> {
    let s = state.inner().clone();
    blocking(move || checked(&s)).await
}
#[tauri::command]
async fn close_t3(state: State<'_, SharedState>) -> Result<Value, Error> {
    let s = state.inner().clone();
    blocking(move || {
        write(&s, || {
            let h = home(&s)?;
            let running = t3db::control::find(&h)?;
            let mut stopped = Restart::default();
            let result = t3db::control::close_into(&h, &running, &mut stopped);
            s.restart
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .extend(stopped);
            result?;
            checked(&s)
        })
    })
    .await
}
#[tauri::command]
async fn restart_t3(state: State<'_, SharedState>) -> Result<Value, Error> {
    let s = state.inner().clone();
    blocking(move || write(&s, || {
        let restarted = restart(&s);
        Ok(json!({"restarted":restarted,"pendingRestart":s.restart.lock().unwrap_or_else(|e| e.into_inner()).names()}))
    })).await
}
fn describe(p: t3db::Progress) -> String {
    match p {
        t3db::Progress::Guard => "Checking the T3 database".into(),
        t3db::Progress::Backup => "Backing up the T3 database".into(),
        t3db::Progress::CreatingProject(root) => format!("Creating project {}", root.display()),
        t3db::Progress::Writing { done, total } => format!("Writing threads {done}/{total}"),
        t3db::Progress::Verifying => "Verifying what was written".into(),
    }
}
#[tauri::command]
async fn import_sessions(
    app: tauri::AppHandle,
    state: State<'_, SharedState>,
    keys: Vec<String>,
    max_messages: Option<usize>,
) -> Result<Value, Error> {
    let s = state.inner().clone();
    blocking(move || write(&s, || {
        let result = (|| -> Result<t3db::ImportReport, Error> {
            let home = home(&s)?;
            let snap = t3db::snapshot(&home)?;
            let selected = summaries(&s,&keys)?;
            for item in &selected { catalog::require_selectable(Some(&snap), item)?; }
            let mut requests = vec![];
            let mut warnings = vec![];
            for item in selected {
                if catalog::provider_disabled(Some(&snap),item.provider) {
                    warnings.push(format!("{} is turned off in T3; enable it to continue this session",item.provider.label()));
                }
                requests.push(crate::request_for(&s.roots,&snap,&item)?);
            }
            let mut report = t3db::import(&home,requests,&t3db::ImportOptions { max_messages, dry_run:false }, &mut |p| { let _ = app.emit("import-progress",describe(p)); })?;
            report.warnings.extend(warnings);
            Ok(report)
        })();
        let restarted = restart(&s);
        match result {
            Ok(report) => Ok(json!({"imported":report.imported,"skipped":report.skipped,"projectsCreated":report.projects_created.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>(),"backup":report.backup.as_ref().map(|p| p.to_string_lossy()),"warnings":report.warnings,"restarted":restarted})),
            Err(mut error) => { error.restarted = Some(restarted); Err(error) }
        }
    })).await.map_err(|mut error| {
        error.restarted.get_or_insert_with(Vec::new);
        error
    })
}
#[tauri::command]
async fn trash_plan(state: State<'_, SharedState>, keys: Vec<String>) -> Result<Value, Error> {
    let s = state.inner().clone();
    blocking(move || {
        let selected = summaries(&s,&keys)?;
        let snap = s.data.lock().unwrap().snapshot.clone();
        let mut plans = HashMap::new(); let mut items = vec![]; let mut blocked = vec![];
        let mut in_t3 = 0; let mut recent = 0;
        for item in selected {
            if catalog::in_t3(snap.as_ref(),&item) { in_t3 += 1; }
            if trash::recently_used(&item) { recent += 1; }
            match trash::plan(&s.roots,&item) {
                Ok(plan) => { items.push(json!({"key":plan.item.key,"title":plan.item.title,"paths":plan.item.paths.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>(),"export":plan.item.export.as_ref().map(|p| p.to_string_lossy())})); plans.insert(catalog::key(&item),plan); },
                Err(e) => blocked.push(format!("{}: {e:#}",item.title)),
            }
        }
        let mut warnings = vec![];
        if in_t3 > 0 { warnings.push(format!("{in_t3} of these are in T3. Continuing them there stops working once their files are gone.")); }
        if recent > 0 { warnings.push(format!("{recent} were used in the last 2 minutes and may still be open.")); }
        // Replace the whole review, including invalid sessions, so stale plans cannot be used.
        s.data.lock().unwrap().trash = plans;
        Ok(json!({"items":items,"warnings":warnings,"blocked":if blocked.is_empty() { None } else { Some(blocked.join("\n")) }}))
    }).await
}
#[tauri::command]
async fn trash_sessions(state: State<'_, SharedState>, keys: Vec<String>) -> Result<Value, Error> {
    let s = state.inner().clone();
    blocking(move || {
        write(&s, || {
            let mut trashed = vec![];
            let mut failed = vec![];
            let mut exported = vec![];
            let mut seen = std::collections::HashSet::new();
            for key in keys.into_iter().filter(|k| seen.insert(k.clone())) {
                let pair = {
                    let mut d = s.data.lock().unwrap();
                    (d.sessions.get(&key).cloned(), d.trash.remove(&key))
                };
                let result = (|| -> anyhow::Result<()> {
                    let item = pair.0.context("Session not found; scan again")?;
                    let plan = pair
                        .1
                        .context("Review a trash plan before deleting this session")?;
                    trash::execute(&s.roots, &item, &plan, &mut exported)
                })();
                match result {
                    Ok(()) => {
                        s.data.lock().unwrap().sessions.remove(&key);
                        trashed.push(key);
                    }
                    Err(e) => failed.push((key, format!("{e:#}"))),
                }
            }
            Ok(json!({"trashed":trashed,"failed":failed,"exported":exported.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>()}))
        })
    })
    .await
}
#[tauri::command]
async fn reveal(
    app: tauri::AppHandle,
    state: State<'_, SharedState>,
    path: PathBuf,
) -> Result<(), Error> {
    let s = state.inner().clone();
    blocking(move || {
        validate_reveal(s.data.lock().unwrap().sessions.values(), &path)?;
        app.opener()
            .reveal_item_in_dir(path)
            .map_err(|e| Error::from(anyhow::anyhow!(e)))
    })
    .await
}
#[tauri::command]
async fn open_url(app: tauri::AppHandle, url: String) -> Result<(), Error> {
    blocking(move || {
        let parsed = validate_url(&url)?;
        app.opener()
            .open_url(parsed.as_str(), None::<&str>)
            .map_err(|e| Error::from(anyhow::anyhow!(e)))
    })
    .await
}
pub fn run(mut roots: Roots, home_override: Option<PathBuf>) -> anyhow::Result<()> {
    for root in [
        &mut roots.claude,
        &mut roots.codex,
        &mut roots.grok,
        &mut roots.opencode,
    ] {
        *root = std::path::absolute(&*root)?;
    }
    let shared = Arc::new(Shared {
        roots,
        home_override,
        data: Mutex::new(Data::default()),
        restart: Mutex::new(Restart::default()),
        busy: AtomicBool::new(false),
        signal_exit: AtomicBool::new(false),
        close_warned: AtomicBool::new(false),
    });
    let signal_shared = shared.clone();
    ctrlc::set_handler(move || {
        exit_after_signal(&signal_shared);
    })
    .context("Install termination handler")?;
    let app = tauri::Builder::default()
        .setup(|app| {
            let config = app
                .config()
                .app
                .windows
                .iter()
                .find(|w| w.label == "main")
                .context("Missing main window config")?;
            tauri::WebviewWindowBuilder::from_config(app, config)?
                .on_navigation(own_origin)
                .build()?;
            Ok(())
        })
        .manage(shared.clone())
        .plugin(
            tauri_plugin_opener::Builder::new()
                .open_js_links_on_click(false)
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            scan,
            transcript,
            check,
            close_t3,
            restart_t3,
            import_sessions,
            trash_plan,
            trash_sessions,
            reveal,
            open_url
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    let s = window.state::<SharedState>();
                    match write(&s, || Ok(restart_on_close(&s))) {
                        Ok(Some(message)) => {
                            api.prevent_close();
                            let _ = window.emit("restart-failed", message);
                        }
                        Err(_) => api.prevent_close(),
                        Ok(None) => {}
                    }
                }
            }
        })
        .build(tauri::generate_context!())
        .map_err(|e| {
            eprintln!("Could not start t3-import: {e}");
            e
        })?;
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::System::Console::FreeConsole();
    }
    app.run(move |_app, event| match event {
        tauri::RunEvent::ExitRequested { api, .. } => {
            if write(&shared, || {
                restart_before_exit(&shared);
                Ok(())
            })
            .is_err()
            {
                api.prevent_exit();
            }
        }
        _ => {}
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state() -> Shared {
        Shared {
            roots: Roots::detect(),
            home_override: None,
            data: Mutex::new(Data::default()),
            restart: Mutex::new(Restart::default()),
            busy: AtomicBool::new(false),
            signal_exit: AtomicBool::new(false),
            close_warned: AtomicBool::new(false),
        }
    }
    #[test]
    fn signal_waits_for_write_and_claims_exit_afterwards() {
        let s = state();
        s.busy.store(true, Ordering::SeqCst);
        assert!(!claim_signal_exit(&s));
        assert!(s.signal_exit.load(Ordering::SeqCst));
        s.busy.store(false, Ordering::SeqCst);
        assert!(claim_signal_exit(&s));
        assert!(!claim_signal_exit(&s));
    }
    #[test]
    fn failed_close_warns_once_and_next_close_can_quit() {
        let s = state();
        assert_eq!(
            restart_on_close_with(&s, || Some("failed".into())),
            Some("failed".into())
        );
        assert_eq!(restart_on_close_with(&s, || panic!("already warned")), None);
    }
    #[test]
    fn restart_does_not_lock_out_scan_and_preserves_concurrent_jobs() {
        let s = state();
        let lines = restart_with(&s, |_| {
            let mut pending = s
                .restart
                .try_lock()
                .expect("restart must not hold the mutex");
            pending.extend(Restart::default());
            vec![Err("failed".into())]
        });
        assert_eq!(lines[0]["ok"], false);
        assert!(s.restart.try_lock().is_ok());
    }
    #[test]
    fn write_gate_rejects_overlap_and_clears_busy_on_error() {
        let s = state();
        let result: Result<(), Error> = write(&s, || {
            assert!(s.busy.load(Ordering::SeqCst));
            assert!(s.data.try_lock().is_ok());
            let overlapping = write(&s, || Ok(())).unwrap_err();
            assert_eq!(overlapping.message, "Another operation is running");
            Err(anyhow::anyhow!("operation failed").into())
        });
        assert!(result.is_err());
        assert!(!s.busy.load(Ordering::SeqCst));
        assert!(write(&s, || Ok(())).is_ok());
    }
    #[test]
    fn import_errors_preserve_recovery_details() {
        let error: Error = anyhow::Error::new(t3db::writer::ImportFailure {
            message: "failed".into(),
            backup: Some("/backup".into()),
            projects_created: vec!["/project".into()],
        })
        .into();
        let value = serde_json::to_value(error).unwrap();
        assert_eq!(value["backup"], "/backup");
        assert_eq!(value["projectsCreated"], json!(["/project"]));
    }
    fn sample() -> SessionSummary {
        SessionSummary {
            provider: crate::model::Provider::Claude,
            session_id: "id".into(),
            cwd: PathBuf::new(),
            title: "test".into(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            message_count: None,
            model: None,
            source: "/session".into(),
            resumable: true,
        }
    }
    #[test]
    fn write_gate_recovers_after_panic() {
        let s = state();
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _: Result<(), Error> = write(&s, || panic!("test panic"));
            }))
            .is_err()
        );
        assert!(write(&s, || Ok(())).is_ok());
    }
    #[test]
    fn dto_keeps_empty_cwd_and_reports_importability() {
        let mut item = sample();
        normalize_cwd(&mut item);
        assert!(item.cwd.as_os_str().is_empty());
        let row = session_dto(&item, None);
        assert_eq!(row["cwd"], "");
        assert_eq!(row["blocked"], "no_provider");
        let mut snap = Snapshot::default();
        snap.provider_instances
            .insert(item.provider, vec!["claude".into()]);
        assert_eq!(session_dto(&item, Some(&snap))["importable"], true);
        item.resumable = false;
        assert_eq!(session_dto(&item, Some(&snap))["blocked"], "not_resumable");
    }
    #[cfg(unix)]
    #[test]
    fn non_utf8_paths_serialize_lossily() {
        use std::os::unix::ffi::OsStringExt;
        let mut item = sample();
        item.cwd = std::ffi::OsString::from_vec(b"/tmp/bad\xff".to_vec()).into();
        item.source = item.cwd.clone();
        let row = session_dto(&item, None);
        assert!(row["cwd"].as_str().unwrap().contains('�'));
        let error: Error = anyhow::Error::new(t3db::writer::ImportFailure {
            message: "failed".into(),
            backup: Some(item.cwd.clone()),
            projects_created: vec![item.cwd],
        })
        .into();
        assert!(serde_json::to_value(error).is_ok());
    }
    #[test]
    fn reveal_requires_exact_scanned_source() {
        let item = sample();
        assert!(validate_reveal([&item].into_iter(), &item.source).is_ok());
        assert!(validate_reveal([&item].into_iter(), std::path::Path::new("/other")).is_err());
    }
    #[test]
    fn urls_and_navigation_are_restricted() {
        assert_eq!(
            validate_url("HTTPS://Example.COM/path").unwrap().as_str(),
            "https://example.com/path"
        );
        for url in ["file:///tmp/test", "javascript:alert(1)", "not a url"] {
            assert!(validate_url(url).is_err());
        }
        for url in [
            "tauri://localhost/index.html",
            "http://tauri.localhost/",
            "https://tauri.localhost/",
        ] {
            assert!(own_origin(&tauri::Url::parse(url).unwrap()));
        }
        for url in [
            "https://example.com/",
            "https://tauri.localhost.evil/",
            "tauri://other/",
            "http://tauri.localhost:9999/",
        ] {
            assert!(!own_origin(&tauri::Url::parse(url).unwrap()));
        }
    }
    #[test]
    fn restart_panic_restores_pending_state() {
        let mut pending = Restart::default();
        let names = pending.names();
        let lines = run_pending(&mut pending, |_| panic!("restart panic"));
        assert_eq!(pending.names(), names);
        assert_eq!(lines[0]["ok"], false);
        assert!(
            lines[0]["text"]
                .as_str()
                .unwrap()
                .contains("pending jobs retained")
        );
    }
}
