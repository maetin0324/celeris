//! `celerisctl projects ls` / `celerisctl projects show <id>` / `celerisctl projects plan approve|reject`
//! （ADR-0054 D2。Phase 68。ADR-0074 D3.3、Phase F4a (c)）。
//!
//! `ls` / `show` は CoS の対話 run に許す**読み取りだけの道具**の一部
//! （`docs/adr/0054-stateful-sessions-and-streaming-chat.md` D2: 「celerisctl knowledge search|get、
//! タスク・案件の一覧と詳細の read API」）。`ls`/`show`（`query.rs`）と同じ流儀（DB を開いて読むだけ、
//! プレーンテキスト出力）。
//!
//! `plan approve|reject` は書き込み（`POST /projects/{id}/project-plan/{version}/decide` の CLI 版。
//! ADR-0074 D3.3「個々の draft を 1 件ずつ Accept する既存の操作は、案件計画の draft には使わせない
//! （まとまりで承認する）」）で、**`CONVERSATION_READONLY_CELERISCTL` には入れない**（CoS の対話 run
//! からは使えない。人だけが押す HUMAN GATE のまま）。

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Subcommand};
use task_core::{MilestoneStatus, ProjectId, TaskStore};
use task_ops::project_plan::ProjectPlanDecision;
use time::OffsetDateTime;

use crate::error::CliError;
use crate::outln;

#[derive(Subcommand, Debug)]
pub enum ProjectsCommand {
    /// 案件の一覧（id・状態・題名）。
    Ls(ProjectsLsArgs),
    /// 1 件の案件の詳細（依頼文・状態・途中目標）。
    Show(ProjectsShowArgs),
    /// ADR-0074 D3.3（Phase F4a (c)）: 案件計画（マイルストーン DAG）の提案を承認/却下する
    /// （`POST /projects/{id}/project-plan/{version}/decide` の CLI 版）。
    Plan {
        #[command(subcommand)]
        command: ProjectPlanCommand,
    },
}

#[derive(Args, Debug)]
pub struct ProjectsLsArgs {
    /// アーカイブ済みの案件も出す（既定は隠す。`GET /projects` と同じ既定）。
    #[arg(long)]
    pub all: bool,
}

#[derive(Args, Debug)]
pub struct ProjectsShowArgs {
    pub id: String,
}

#[derive(Subcommand, Debug)]
pub enum ProjectPlanCommand {
    /// 提案の全途中目標を `approved`、全 Task を `ready` にする（1 トランザクション）。
    Approve(ProjectPlanDecideArgs),
    /// 提案の全途中目標を `redesigned`、全 Task を `cancelled` にし、`--note` を秘書へ送る。
    Reject(ProjectPlanDecideArgs),
}

#[derive(Args, Debug)]
pub struct ProjectPlanDecideArgs {
    pub project_id: String,
    /// 提案の版（F4a では常に 1）。
    #[arg(default_value_t = 1)]
    pub version: u32,

    /// `reject` では必須（空なら弾く）。`approve` では任意で、秘書へは送らない。
    #[arg(long)]
    pub note: Option<String>,

    /// `[[roles]]`/`[[genres]]` を読む `config.toml`（`reject` の秘書への対話にだけ効く）。
    #[arg(long)]
    pub config: Option<PathBuf>,
}

pub fn run(store: &dyn TaskStore, command: ProjectsCommand) -> Result<ExitCode, CliError> {
    match command {
        ProjectsCommand::Ls(args) => run_ls(store, args),
        ProjectsCommand::Show(args) => run_show(store, args),
        ProjectsCommand::Plan {
            command: ProjectPlanCommand::Approve(args),
        } => run_plan_decide(store, ProjectPlanDecision::Approve, args),
        ProjectsCommand::Plan {
            command: ProjectPlanCommand::Reject(args),
        } => run_plan_decide(store, ProjectPlanDecision::Reject, args),
    }
}

fn run_plan_decide(
    store: &dyn TaskStore,
    decision: ProjectPlanDecision,
    args: ProjectPlanDecideArgs,
) -> Result<ExitCode, CliError> {
    let project_id: ProjectId = args.project_id.parse().map_err(|_| {
        CliError::Message(format!("`{}` is not a project id (ULID)", args.project_id))
    })?;
    let project = store
        .project_get(project_id)?
        .ok_or_else(|| CliError::Message(format!("project not found: {}", args.project_id)))?;

    let (roles, genres, conversation_genre) = match &args.config {
        Some(path) => {
            let config = celeris::Config::load(path).map_err(|e| {
                CliError::msg(format!("failed to load config {}: {e}", path.display()))
            })?;
            (
                config.role_specs(),
                config.genre_specs(),
                config.conversation_genre_id().to_string(),
            )
        }
        None => (
            Vec::new(),
            Vec::new(),
            task_core::CONVERSATION_GENRE.to_string(),
        ),
    };

    let decided = task_ops::project_plan::decide(
        store,
        &project,
        args.version,
        decision,
        args.note.as_deref(),
        &roles,
        &genres,
        &conversation_genre,
        OffsetDateTime::now_utc(),
    )?;
    outln!(
        "{:?}: {} milestone(s), {} task(s)",
        decided.decision,
        decided.milestones.len(),
        decided.tasks.len()
    );
    Ok(ExitCode::SUCCESS)
}

fn run_ls(store: &dyn TaskStore, args: ProjectsLsArgs) -> Result<ExitCode, CliError> {
    let mut projects = store.project_list()?;
    if !args.all {
        projects.retain(|p| p.archived_at.is_none());
    }
    projects.sort_by_key(|p| p.created_at);
    for project in &projects {
        outln!("{} {:?} {}", project.id, project.status, project.title);
    }
    Ok(ExitCode::SUCCESS)
}

fn run_show(store: &dyn TaskStore, args: ProjectsShowArgs) -> Result<ExitCode, CliError> {
    let id: ProjectId = args
        .id
        .parse()
        .map_err(|_| CliError::Message(format!("`{}` is not a project id (ULID)", args.id)))?;
    let project = store
        .project_get(id)?
        .ok_or_else(|| CliError::Message(format!("project not found: {}", args.id)))?;

    outln!("id: {}", project.id);
    outln!("title: {}", project.title);
    outln!("status: {:?}", project.status);
    outln!("request: {}", project.request);
    outln!(
        "secretary_summary: {}",
        project.secretary_summary.as_deref().unwrap_or("(none)")
    );
    match project.archived_at {
        Some(at) => outln!("archived_at: {at}"),
        None => outln!("archived_at: (none)"),
    }

    outln!("milestones:");
    for milestone in store.milestone_list(id)? {
        let mark = match milestone.status {
            MilestoneStatus::Proposed => "proposed",
            MilestoneStatus::Approved => "approved",
            MilestoneStatus::InProgress => "in_progress",
            MilestoneStatus::Reached => "reached",
            MilestoneStatus::Redesigned => "redesigned",
            MilestoneStatus::Paused => "paused",
            MilestoneStatus::Cancelled => "cancelled",
        };
        outln!("  - [{mark}] {} — {}", milestone.id, milestone.title);
    }

    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use task_core::{Project, ProjectStatus, SqliteStore};
    use time::OffsetDateTime;

    fn sample_project(status: ProjectStatus, archived: bool) -> Project {
        let now = OffsetDateTime::now_utc();
        Project {
            auto_advance: false,
            id: ProjectId::new(),
            title: "Pluvio".into(),
            request: "降水予測の研究".into(),
            status,
            secretary_summary: None,
            workspace: None,
            archived_at: if archived { Some(now) } else { None },
            paused_from: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// 読み取り専用: `ls` は既定でアーカイブ済みを隠し、`--all` で出す。
    #[test]
    fn run_ls_hides_archived_projects_unless_all() {
        let store = SqliteStore::open_in_memory().expect("open store");
        store
            .project_create(&sample_project(ProjectStatus::Active, false))
            .expect("create");
        store
            .project_create(&sample_project(ProjectStatus::Active, true))
            .expect("create");

        assert!(run(&store, ProjectsCommand::Ls(ProjectsLsArgs { all: false })).is_ok());
        assert!(run(&store, ProjectsCommand::Ls(ProjectsLsArgs { all: true })).is_ok());
        // アーカイブ有無で `project_list` の中身が違うことを確かめておく（表示は目視）。
        let archived_count = store
            .project_list()
            .expect("list")
            .iter()
            .filter(|p| p.archived_at.is_some())
            .count();
        assert_eq!(archived_count, 1);
    }

    /// 読み取り専用: `show` は milestone も一緒に出し、無い id は 404 相当のエラー。
    #[test]
    fn run_show_reads_the_project_and_its_milestones() {
        let store = SqliteStore::open_in_memory().expect("open store");
        let project = sample_project(ProjectStatus::Active, false);
        store.project_create(&project).expect("create");

        let ok = run(
            &store,
            ProjectsCommand::Show(ProjectsShowArgs {
                id: project.id.to_string(),
            }),
        );
        assert!(ok.is_ok());

        let missing = run(
            &store,
            ProjectsCommand::Show(ProjectsShowArgs {
                id: task_core::ProjectId::new().to_string(),
            }),
        );
        assert!(missing.is_err());

        let bad_id = run(
            &store,
            ProjectsCommand::Show(ProjectsShowArgs {
                id: "not-a-ulid".into(),
            }),
        );
        assert!(bad_id.is_err());
    }

    fn seed_secretary(store: &SqliteStore) {
        let now = OffsetDateTime::now_utc();
        store
            .org_upsert(&task_core::OrgNode {
                profile: Default::default(),
                id: "secretary".into(),
                parent_id: None,
                name: "秘書".into(),
                kind: task_core::OrgKind::Secretary,
                genre: None,
                brief: String::new(),
                position: 0,
                created_at: now,
                updated_at: now,
            })
            .expect("secretary");
    }

    fn milestone_spec(key: &str) -> task_core::MilestoneSpec {
        task_core::MilestoneSpec {
            pause_after: None,
            key: key.into(),
            title: format!("title-{key}"),
            objective: "o".into(),
            reach_criteria: "c".into(),
            acceptance: vec![
                task_core::Criterion {
                    text: "d".into(),
                    check: task_core::Check::Human,
                },
                task_core::Criterion {
                    text: "a".into(),
                    check: task_core::Check::ArtifactExists {
                        name: "r.md".into(),
                    },
                },
            ],
            depends_on: vec![],
            genre: None,
            skills: vec![],
            repos: vec![],
            features: None,
            execution: None,
        }
    }

    /// ADR-0074 D3.3（Phase F4a (c)）: `celerisctl projects plan approve|reject` は
    /// `task_ops::project_plan::decide` と同じ操作（approve は Task を ready、reject は cancelled）。
    #[test]
    fn run_plan_decide_approves_and_rejects_a_proposal() {
        let store = SqliteStore::open_in_memory().expect("open store");
        seed_secretary(&store);
        let project = sample_project(ProjectStatus::Active, false);
        store.project_create(&project).expect("create");

        let started = task_ops::project_plan::start_milestones(
            &store,
            &project,
            None,
            &[],
            &[],
            OffsetDateTime::now_utc(),
        )
        .expect("start_milestones");
        let plan_spec = task_core::ProjectPlanSpec {
            schema: task_core::PROJECT_PLAN_SCHEMA.into(),
            rationale: "r".into(),
            milestones: vec![milestone_spec("survey")],
        };
        let validated = task_core::validate_project_plan(
            &plan_spec,
            task_core::ProjectPlanLimits::default(),
            &std::collections::BTreeSet::new(),
        )
        .expect("valid");
        task_ops::project_plan::propose(
            &store,
            &started.task,
            &project,
            &validated,
            &[],
            &[],
            OffsetDateTime::now_utc(),
        )
        .expect("propose");

        // note の無い reject はエラー（下敷きの `decide` が 422 相当を返す）。
        let err = run(
            &store,
            ProjectsCommand::Plan {
                command: ProjectPlanCommand::Reject(ProjectPlanDecideArgs {
                    project_id: project.id.to_string(),
                    version: 1,
                    note: None,
                    config: None,
                }),
            },
        );
        assert!(err.is_err());

        let ok = run(
            &store,
            ProjectsCommand::Plan {
                command: ProjectPlanCommand::Approve(ProjectPlanDecideArgs {
                    project_id: project.id.to_string(),
                    version: 1,
                    note: None,
                    config: None,
                }),
            },
        );
        assert!(ok.is_ok(), "{ok:?}");

        let milestones = store.milestone_list(project.id).expect("list");
        assert_eq!(milestones[0].status, MilestoneStatus::Approved);
    }

    /// `projects plan approve <id> [version]` / `projects plan reject <id> --note ...` で引数が解ける
    /// （版は省略すると 1）。
    #[test]
    fn plan_approve_and_reject_parse_with_a_default_version() {
        use clap::Parser;
        #[derive(Parser, Debug)]
        struct Wrap {
            #[command(subcommand)]
            command: ProjectsCommand,
        }
        let w = Wrap::try_parse_from(["x", "plan", "approve", "01ARZ3NDEKTSV4RRFFQ69G5FAV"])
            .expect("parse approve");
        assert!(matches!(
            w.command,
            ProjectsCommand::Plan {
                command: ProjectPlanCommand::Approve(ProjectPlanDecideArgs { version: 1, .. })
            }
        ));
        let w = Wrap::try_parse_from([
            "x",
            "plan",
            "reject",
            "01ARZ3NDEKTSV4RRFFQ69G5FAV",
            "2",
            "--note",
            "scope",
        ])
        .expect("parse reject");
        match w.command {
            ProjectsCommand::Plan {
                command: ProjectPlanCommand::Reject(args),
            } => {
                assert_eq!(args.version, 2);
                assert_eq!(args.note.as_deref(), Some("scope"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
