//! ADR 2026-10-27-project-brief D5: the project summaries a CoS chat run receives. Deterministic
//! (store reads only): the general CoS gets every non-archived project, a project CoS its own.

use task_core::{ProjectId, SqliteStore, TaskStore};
use task_worker::protocol::CosChatProjectSummary;
use time::format_description::well_known::Rfc3339;

/// Characters of `project_briefs.state` a summary keeps.
pub(crate) const STATE_SUMMARY_CHARS: usize = 400;

/// `only = None`: every non-archived project (the store's newest-first order). `Some(id)`: that
/// project alone (empty when it does not exist).
pub(crate) fn project_summaries(
    store: &SqliteStore,
    only: Option<ProjectId>,
) -> Result<Vec<CosChatProjectSummary>, String> {
    let projects = store.project_list().map_err(|e| e.to_string())?;
    let mut briefs = store.project_brief_list().map_err(|e| e.to_string())?;
    let mut activity = store.project_activity().map_err(|e| e.to_string())?;
    let rfc3339 = |at: time::OffsetDateTime| at.format(&Rfc3339).unwrap_or_default();
    Ok(projects
        .into_iter()
        .filter(|p| match only {
            Some(id) => p.id == id,
            None => p.archived_at.is_none(),
        })
        .map(|p| {
            let brief = briefs.remove(&p.id);
            let activity = activity.remove(&p.id).unwrap_or_default();
            let last = activity
                .last_task_update
                .map_or(p.updated_at, |at| at.max(p.updated_at));
            CosChatProjectSummary {
                project_id: p.id.to_string(),
                title: p.title,
                status: p.status.as_str().to_string(),
                objective: brief.as_ref().map(|b| b.objective.clone()),
                state: brief.as_ref().map(|b| cut(&b.state, STATE_SUMMARY_CHARS)),
                waiting_on: brief
                    .as_ref()
                    .map(|b| {
                        b.waiting_on
                            .iter()
                            .map(|w| format!("{}: {}", w.kind.as_str(), w.note))
                            .collect()
                    })
                    .unwrap_or_default(),
                open_tasks: activity.open_tasks(),
                last_activity_at: rfc3339(last),
                brief_updated_at: brief.as_ref().map(|b| rfc3339(b.updated_at)),
            }
        })
        .collect())
}

fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use task_core::{
        Project, ProjectBrief, ProjectBriefInput, ProjectStatus, WaitingOn, WaitingOnKind,
    };
    use time::OffsetDateTime;

    use super::*;

    fn project(store: &SqliteStore, title: &str, archived: bool) -> Project {
        let now = OffsetDateTime::now_utc();
        let p = Project {
            id: ProjectId::new(),
            title: title.into(),
            request: "依頼".into(),
            status: ProjectStatus::Active,
            workspace: None,
            archived_at: archived.then_some(now),
            paused_from: None,
            auto_advance: false,
            slug: None,
            auto_integrate: false,
            created_at: now,
            updated_at: now,
        };
        store.project_create(&p).unwrap();
        p
    }

    #[test]
    fn general_gets_every_live_project_and_project_cos_its_own() {
        let store = SqliteStore::open_in_memory().unwrap();
        let a = project(&store, "A", false);
        let b = project(&store, "B", false);
        let _archived = project(&store, "C", true);
        let long = "あ".repeat(STATE_SUMMARY_CHARS + 10);
        let brief = ProjectBrief::from_input(
            a.id,
            ProjectBriefInput {
                objective: "目的".into(),
                state: long,
                open_issues: vec!["未解決は渡さない".into()],
                next_actions: vec![],
                waiting_on: vec![WaitingOn {
                    kind: WaitingOnKind::Human,
                    target: None,
                    note: "判断".into(),
                }],
                evidence_refs: vec![],
            },
            "cos:t",
            OffsetDateTime::now_utc(),
        );
        assert!(store.project_brief_put(&brief).unwrap());

        let all = project_summaries(&store, None).unwrap();
        let ids: Vec<_> = all.iter().map(|s| s.project_id.clone()).collect();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&a.id.to_string()) && ids.contains(&b.id.to_string()));
        let sa = all
            .iter()
            .find(|s| s.project_id == a.id.to_string())
            .unwrap();
        assert_eq!(sa.objective.as_deref(), Some("目的"));
        assert_eq!(
            sa.state.as_deref().map(|s| s.chars().count()),
            Some(STATE_SUMMARY_CHARS + 1)
        );
        assert_eq!(sa.waiting_on, ["human: 判断"]);
        assert!(sa.brief_updated_at.is_some());
        let sb = all
            .iter()
            .find(|s| s.project_id == b.id.to_string())
            .unwrap();
        assert!(sb.objective.is_none() && sb.brief_updated_at.is_none());

        let own = project_summaries(&store, Some(b.id)).unwrap();
        assert_eq!(own.len(), 1);
        assert_eq!(own[0].title, "B");
    }
}
