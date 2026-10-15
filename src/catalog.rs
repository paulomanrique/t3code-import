//! Rules shared by the session catalog and import commands.
use crate::{
    model::{Provider, SessionSummary},
    t3db::{self, Snapshot},
};

pub fn key(s: &SessionSummary) -> String {
    format!("{}:{}", s.provider.label().to_lowercase(), s.session_id)
}
pub fn instance_for(snapshot: Option<&Snapshot>, provider: Provider) -> Option<&str> {
    snapshot?
        .provider_instances
        .get(&provider)?
        .first()
        .map(String::as_str)
}
pub fn in_t3(snapshot: Option<&Snapshot>, s: &SessionSummary) -> bool {
    snapshot.is_some_and(|snap| {
        snap.has_native(s.provider, &s.session_id)
            || instance_for(Some(snap), s.provider).is_some_and(|instance| {
                snap.thread_ids
                    .contains(&t3db::thread_id_for(instance, &s.session_id))
            })
    })
}
pub fn provider_disabled(snapshot: Option<&Snapshot>, provider: Provider) -> bool {
    snapshot.is_some_and(|s| s.disabled_providers.contains(&provider))
}
pub fn selectable(snapshot: Option<&Snapshot>, s: &SessionSummary) -> bool {
    s.resumable && !in_t3(snapshot, s) && instance_for(snapshot, s.provider).is_some()
}

pub fn require_selectable(snapshot: Option<&Snapshot>, s: &SessionSummary) -> anyhow::Result<()> {
    if selectable(snapshot, s) {
        return Ok(());
    }
    let reason = if in_t3(snapshot, s) {
        "already in T3"
    } else if !s.resumable {
        "not resumable"
    } else {
        "no enabled provider in T3 settings"
    };
    anyhow::bail!("Cannot import {}: {reason}", key(s))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    fn session(cwd: std::path::PathBuf) -> SessionSummary {
        SessionSummary {
            provider: Provider::Claude,
            session_id: "id".into(),
            cwd,
            title: "test".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            message_count: None,
            model: None,
            source: "/source".into(),
            resumable: true,
        }
    }
    #[test]
    fn worktree_sessions_join_their_repository_project() {
        let temp = tempfile::tempdir().unwrap();
        let main = temp.path().join("repo");
        let work = temp.path().join("repo-feature");
        let admin = main.join(".git/worktrees/feature");
        std::fs::create_dir_all(&admin).unwrap();
        std::fs::create_dir(&work).unwrap();
        std::fs::write(work.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();
        std::fs::write(admin.join("commondir"), "../..\n").unwrap();
        std::fs::write(admin.join("HEAD"), "ref: refs/heads/feature\n").unwrap();
        let branch = t3db::resolve_placement(&session(work).cwd);
        assert_eq!(
            branch.workspace_root,
            t3db::resolve_placement(&main).workspace_root
        );
        assert_eq!(branch.branch.as_deref(), Some("feature"));
    }
    #[test]
    fn sessions_already_in_t3_hide_until_shown() {
        let s = session("/a".into());
        let mut snap = Snapshot::default();
        snap.provider_instances
            .insert(s.provider, vec!["claudeAgent".into()]);
        assert!(selectable(Some(&snap), &s));
        snap.thread_ids
            .insert(t3db::thread_id_for("claudeAgent", &s.session_id));
        assert!(in_t3(Some(&snap), &s));
        assert!(!selectable(Some(&snap), &s));
        snap.provider_instances.clear();
        snap.native_sessions
            .insert((s.provider.driver().into(), s.session_id.clone()));
        assert!(in_t3(Some(&snap), &s));
    }
    #[test]
    fn import_rejects_each_unselectable_reason() {
        let mut s = session("/a".into());
        let mut snap = Snapshot::default();
        assert!(
            require_selectable(Some(&snap), &s)
                .unwrap_err()
                .to_string()
                .contains("no enabled provider")
        );
        snap.provider_instances
            .insert(s.provider, vec!["claudeAgent".into()]);
        s.resumable = false;
        assert!(
            require_selectable(Some(&snap), &s)
                .unwrap_err()
                .to_string()
                .contains("not resumable")
        );
        s.resumable = true;
        snap.native_sessions
            .insert((s.provider.driver().into(), s.session_id.clone()));
        assert!(
            require_selectable(Some(&snap), &s)
                .unwrap_err()
                .to_string()
                .contains("already in T3")
        );
    }
}
