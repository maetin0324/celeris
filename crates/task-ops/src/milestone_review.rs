//! 途中目標の判定を「結果の報告 → 次の提案 → ok / 議論 / ng」の対話にする（ADR-0038。Phase 41）。
//!
//! SPEC §7 のアジャイル（途中目標の達成ごとに人が判定し、Go か再設計）を成立させるための 3 つの
//! 決定的な操作をここに置く。**LLM は一切呼ばない**（レビューの文面を書くのは対話 run、達成を決めるのは人）:
//!
//! 1. `start_review` — 途中目標の仕事が止まったら、秘書の**レビューの対話 run** を 1 件起こす（D1）。
//!    印は「対話の印 + `milestone_id`」だけで、新しいタスクの種類も列も作らない。
//! 2. `record_proposal` — 対話 run の結果ファイルの `milestone_proposal` から、`status = proposed` の
//!    途中目標を 1 件作る（古い `proposed` は `redesigned` に差し替える。D1 / D2）。
//! 3. `decide` — 人の 3 つの答え（`ok` / `discuss` / `ng`）を D2 の表のとおりに適用する。
//!
//! I/O は `TaskStore` の読み書きだけ（DESIGN 原則 1）。

use task_core::{
    GenreSpec, Message, MessageId, MessageRole, Milestone, MilestoneDecision, MilestoneId,
    MilestoneStatus, OrgKind, Project, ProjectId, RoleSpec, Task, TaskId, TaskStore,
};
use time::OffsetDateTime;

use crate::conversation::{self, StartedConversation};
use crate::error::OpsError;
use crate::project_plan;

/// 案件を跨いで途中目標を引くときに見る案件の数の上限（案件はそう多くない）。
const PROJECT_SCAN: usize = 500;
/// レビューの対話を探すときに 1 案件あたりに見るタスクの上限。
const REVIEW_TASK_SCAN: usize = 1_000;
/// レビューの返事を探すときに読む対話の件数。
const REVIEW_MESSAGE_SCAN: usize = 200;

/// 途中目標とその案件を id から引く（`milestones` は案件ごとにしか引けないので順に見る）。
pub fn find(
    store: &dyn TaskStore,
    id: MilestoneId,
) -> Result<Option<(Project, Milestone)>, OpsError> {
    for project in store.project_list()?.into_iter().take(PROJECT_SCAN) {
        if let Some(found) = store
            .milestone_list(project.id)?
            .into_iter()
            .find(|m| m.id == id)
        {
            return Ok(Some((project, found)));
        }
    }
    Ok(None)
}

/// その案件の最新の `proposed` の途中目標（`seq` の大きい方。`exclude` は除く）。
pub fn latest_proposal(
    store: &dyn TaskStore,
    project_id: ProjectId,
    exclude: Option<MilestoneId>,
) -> Result<Option<Milestone>, OpsError> {
    let mut proposed: Vec<Milestone> = store
        .milestone_list(project_id)?
        .into_iter()
        // ADR-0074 D3.8（Phase F4b）: 案件計画の提案中の途中目標（`plan_key` あり）は旧い意味の
        // 「次の途中目標の提案」ではない。
        .filter(|m| {
            m.status == MilestoneStatus::Proposed && Some(m.id) != exclude && m.plan_key.is_none()
        })
        .collect();
    proposed.sort_by_key(|m| m.seq);
    Ok(proposed.pop())
}

// ---- D1: レビューの対話 run ----

/// レビューの対話に渡す本文（決定的。中身を書くのは run の仕事で、ここは「何が起きたか」だけを伝える）。
/// 指示文そのもの（ADR-0038 D1 の (a)〜(d)）は前置き（`task_worker::preamble`）が出す。
pub fn review_request_text(
    milestone: &Milestone,
    done: usize,
    failed: usize,
    waiting: usize,
) -> String {
    let mut out = format!(
        "途中目標『{}』の仕事が止まりました（done {done} / failed {failed}、Go 待ち {waiting} 件）。",
        milestone.title
    );
    if !milestone.description.trim().is_empty() {
        out.push_str(&format!(
            "この途中目標のねらい: {}。",
            milestone.description.trim()
        ));
    }
    out.push_str(
        "ここまでで得られた結果をまとめ、達成と言えるかの見立てと、次の途中目標の提案を人に返してください。",
    );
    out
}

/// 途中目標のレビューの対話 run を 1 件起こす（ADR-0038 D1）。秘書がいない構成では
/// `OpsError::Validation`（呼び出し側は「何もしない」に倒してよい）。
#[allow(clippy::too_many_arguments)]
pub fn start_review(
    store: &dyn TaskStore,
    project: &Project,
    milestone: &Milestone,
    text: &str,
    roles: &[RoleSpec],
    genres: &[GenreSpec],
    conversation_genre: &str,
    now: OffsetDateTime,
) -> Result<StartedConversation, OpsError> {
    let secretary = secretary_id(store)?;
    conversation::start_with_milestone(
        store,
        &secretary,
        Some(project.id),
        Some(milestone.id),
        text,
        roles,
        genres,
        conversation_genre,
        now,
    )
}

/// その途中目標のレビューの対話の状態（ADR-0038 D1 / D4）。GUI のカード（`GET /projects/{id}`）と
/// 通知（`milestone_ready`）とレビュー run の重複判定が、同じ 1 つの読み取りを使う。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReviewState {
    /// 一番新しいレビューの対話タスク（まだ無ければ `None`。run 中なら `reply` が `None`）。
    pub task: Option<Task>,
    /// そのタスクに紐づく秘書の返事（`messages` の `role = node`）。
    pub reply: Option<Message>,
}

impl ReviewState {
    /// 人に知らせてよいか（返事が付いているか。ADR-0038 D4: 通知は返事の後）。
    pub fn has_reply(&self) -> bool {
        self.reply.is_some()
    }
}

/// レビューの対話とその返事を引く（決定的。無ければ空の `ReviewState`）。
pub fn review_state(
    store: &dyn TaskStore,
    project_id: ProjectId,
    milestone_id: MilestoneId,
) -> Result<ReviewState, task_core::StoreError> {
    let filter = task_core::ListFilter {
        project_id: Some(project_id),
        ..task_core::ListFilter::default()
    };
    let page = store.list_page(
        &filter,
        task_core::ListOrder::CreatedDesc,
        None,
        REVIEW_TASK_SCAN,
    )?;
    let mut reviews: Vec<Task> = page
        .items
        .into_iter()
        .filter(|t| task_core::milestone_review_of(t) == Some(milestone_id))
        .collect();
    reviews.sort_by_key(|t| (t.created_at, t.id));
    let Some(task) = reviews.pop() else {
        return Ok(ReviewState::default());
    };
    let reply = match task.assignee.as_deref() {
        Some(node_id) => store
            .message_list(node_id, Some(project_id), REVIEW_MESSAGE_SCAN)?
            .into_iter()
            .rfind(|m| m.role == MessageRole::Node && m.task_id == Some(task.id)),
        None => None,
    };
    Ok(ReviewState {
        task: Some(task),
        reply,
    })
}

// ---- D1 / D2: 結果ファイルの `milestone_proposal` を `milestones` に入れる ----

/// 対話 run が宣言した次の途中目標を `status = proposed` として 1 件作る（`seq` は末尾）。
/// 既にあった `proposed`（`keep` は除く）は `redesigned` にして**差し替える**（ADR-0038 D2 の「議論」）。
/// `title` が空なら何もしない（`Ok(None)`）。
pub fn record_proposal(
    store: &dyn TaskStore,
    project_id: ProjectId,
    keep: Option<MilestoneId>,
    title: &str,
    description: &str,
) -> Result<Option<Milestone>, OpsError> {
    if title.trim().is_empty() {
        return Ok(None);
    }
    for old in store.milestone_list(project_id)? {
        // ADR-0074 D3.8（Phase F4b）: 案件計画の途中目標（`plan_key` あり。提案中の DAG の節点）は
        // この差し替えの対象にしない（計画の承認 / 却下は `project_plan::decide` だけが決める）。
        if old.status == MilestoneStatus::Proposed && Some(old.id) != keep && old.plan_key.is_none()
        {
            store.milestone_set_status(old.id, MilestoneStatus::Redesigned)?;
        }
    }
    let created = store.milestone_create(
        project_id,
        title.trim(),
        description.trim(),
        MilestoneStatus::Proposed,
    )?;
    Ok(Some(created))
}

// ---- D2: 人の 3 つの答え ----

/// `decide` の結果（API は 202 でそのまま返す）。
#[derive(Debug, Clone, PartialEq)]
pub struct Decided {
    /// 判定した途中目標（更新後）。
    pub milestone: Milestone,
    /// `ok` で承認した次の途中目標（提案が無ければ `None`）。`ng` では `redesigned` にした提案。
    pub next_milestone: Option<Milestone>,
    /// `ok` で起こした分解（計画 run）のタスク。
    pub plan_task_id: Option<TaskId>,
    /// `discuss` / `ng` で秘書に送った対話（`role = user` の行とその run のタスク）。
    pub message_id: Option<MessageId>,
    pub conversation_task_id: Option<TaskId>,
}

/// 人の答えを適用する（ADR-0038 D2 の表のとおり。決定的で、LLM は呼ばない）。
///
/// - `ok`: この途中目標を `reached`、最新の `proposed` を `approved` にして**その途中目標の分解**を起こす
///   （`POST /projects/{id}/plan` と同じ経路。`note` は計画の `note` に渡り、秘書への `messages` にも残る）。
/// - `discuss`: 状態は何も変えず、`note` を秘書への対話として送る。
/// - `ng`: この途中目標と提案を `redesigned` にし、理由 + 再設計の依頼を秘書への対話として送る。
///
/// `note` は `discuss` / `ng` では必須（空なら `OpsError::Validation` → API は 422）。
/// `reached` 済みの途中目標に対する呼び出しは**呼び出し側が 409 にする**（ここには来ない）。
#[allow(clippy::too_many_arguments)]
pub fn decide(
    store: &dyn TaskStore,
    project: &Project,
    milestone: &Milestone,
    decision: MilestoneDecision,
    note: Option<&str>,
    roles: &[RoleSpec],
    genres: &[GenreSpec],
    conversation_genre: &str,
    now: OffsetDateTime,
) -> Result<Decided, OpsError> {
    let note = note.map(str::trim).filter(|n| !n.is_empty());
    if note.is_none() && matches!(decision, MilestoneDecision::Discuss | MilestoneDecision::Ng) {
        return Err(OpsError::Validation(format!(
            "note must not be blank for the {:?} decision",
            decision.as_str()
        )));
    }
    // ADR-0074 D3.6（Phase F4b (d)）: 案件計画の途中目標（DAG の節点）では `ok` の意味を改める。
    if milestone.plan_key.is_some() {
        return decide_planned(
            store,
            project,
            milestone,
            decision,
            note,
            roles,
            genres,
            conversation_genre,
            now,
        );
    }
    let proposal = latest_proposal(store, project.id, Some(milestone.id))?;
    match decision {
        MilestoneDecision::Ok => {
            store.milestone_set_status(milestone.id, MilestoneStatus::Reached)?;
            // D2: 人の一言は秘書との対話にも残す（run は起こさない。返事を待つ場面ではない）。
            if let Some(note) = note {
                append_user_note(store, project.id, note, now)?;
            }
            let mut plan_task_id = None;
            if let Some(next) = &proposal {
                store.milestone_set_status(next.id, MilestoneStatus::Approved)?;
                // Phase 29 と同じ経路。この計画 run が次の途中目標を `in_progress` にし、`draft` の仕事を作る。
                let started =
                    project_plan::start(store, project, Some(next.id), note, roles, genres, now)?;
                plan_task_id = Some(started.task.id);
            }
            Ok(Decided {
                milestone: reload(store, project.id, milestone.id)?,
                next_milestone: match &proposal {
                    Some(next) => Some(reload(store, project.id, next.id)?),
                    None => None,
                },
                plan_task_id,
                message_id: None,
                conversation_task_id: None,
            })
        }
        MilestoneDecision::Discuss => {
            let text = discuss_text(milestone, note.unwrap_or_default());
            let started = send_to_secretary(
                store,
                project,
                &text,
                roles,
                genres,
                conversation_genre,
                now,
            )?;
            Ok(Decided {
                milestone: reload(store, project.id, milestone.id)?,
                next_milestone: proposal,
                plan_task_id: None,
                message_id: Some(started.message.id),
                conversation_task_id: Some(started.task.id),
            })
        }
        MilestoneDecision::Ng => {
            store.milestone_set_status(milestone.id, MilestoneStatus::Redesigned)?;
            if let Some(next) = &proposal {
                store.milestone_set_status(next.id, MilestoneStatus::Redesigned)?;
            }
            let text = redesign_text(milestone, note.unwrap_or_default());
            let started = send_to_secretary(
                store,
                project,
                &text,
                roles,
                genres,
                conversation_genre,
                now,
            )?;
            Ok(Decided {
                milestone: reload(store, project.id, milestone.id)?,
                next_milestone: match &proposal {
                    Some(next) => Some(reload(store, project.id, next.id)?),
                    None => None,
                },
                plan_task_id: None,
                message_id: Some(started.message.id),
                conversation_task_id: Some(started.task.id),
            })
        }
    }
}

/// ADR-0074 D3.6（Phase F4b）: 案件計画の途中目標（`plan_key` あり）への人の答え。
///
/// - `ok`: この途中目標を `reached` にするだけ。依存するマイルストーン Task の Go は
///   `ready_tasks` の判定（D3.2）で開く。次の途中目標の承認・分解の run は起こさない（それは案件計画を
///   持たない案件の旧い意味。D3.8）。
/// - `discuss`: 旧い意味と同じ（状態は変えず、秘書への対話）。
/// - `ng`: この途中目標を `redesigned`（Go は開かないまま）にし、案件の replan（D3.4 の起点 (a)）の
///   計画 run を起こす（理由は計画 run への人の一言として渡る）。
#[allow(clippy::too_many_arguments)]
fn decide_planned(
    store: &dyn TaskStore,
    project: &Project,
    milestone: &Milestone,
    decision: MilestoneDecision,
    note: Option<&str>,
    roles: &[RoleSpec],
    genres: &[GenreSpec],
    conversation_genre: &str,
    now: OffsetDateTime,
) -> Result<Decided, OpsError> {
    match decision {
        MilestoneDecision::Ok => {
            store.milestone_set_status(milestone.id, MilestoneStatus::Reached)?;
            if let Some(note) = note {
                append_user_note(store, project.id, note, now)?;
            }
            Ok(Decided {
                milestone: reload(store, project.id, milestone.id)?,
                next_milestone: None,
                plan_task_id: None,
                message_id: None,
                conversation_task_id: None,
            })
        }
        MilestoneDecision::Discuss => {
            let text = discuss_text(milestone, note.unwrap_or_default());
            let started = send_to_secretary(
                store,
                project,
                &text,
                roles,
                genres,
                conversation_genre,
                now,
            )?;
            Ok(Decided {
                milestone: reload(store, project.id, milestone.id)?,
                next_milestone: None,
                plan_task_id: None,
                message_id: Some(started.message.id),
                conversation_task_id: Some(started.task.id),
            })
        }
        MilestoneDecision::Ng => {
            store.milestone_set_status(milestone.id, MilestoneStatus::Redesigned)?;
            let reason = format!(
                "途中目標『{}』（key: {}）は達成にしません。理由: {}",
                milestone.title,
                milestone.plan_key.as_deref().unwrap_or_default(),
                note.unwrap_or_default()
            );
            let started =
                project_plan::start_replan(store, project, Some(&reason), roles, genres, now)?;
            Ok(Decided {
                milestone: reload(store, project.id, milestone.id)?,
                next_milestone: None,
                plan_task_id: Some(started.task.id),
                message_id: None,
                conversation_task_id: None,
            })
        }
    }
}

/// `discuss` で秘書に送る文面（人の言葉に、どの途中目標の話かを添えるだけ）。
pub fn discuss_text(milestone: &Milestone, note: &str) -> String {
    format!(
        "途中目標『{}』の判定について相談です。{note}",
        milestone.title
    )
}

/// `ng` で秘書に送る文面（ADR-0038 D2: 理由 + 「この途中目標の再設計を提案せよ」の定型）。
pub fn redesign_text(milestone: &Milestone, note: &str) -> String {
    format!(
        "途中目標『{}』は達成にしません。理由: {note}\n\
         この途中目標自体の再設計（やり直す／別の切り方にする）を 1 つ提案してください。\
         提案は結果ファイルの `milestone_proposal` にも書いてください。",
        milestone.title
    )
}

// ---- 小物 ----

fn secretary_id(store: &dyn TaskStore) -> Result<String, OpsError> {
    store
        .org_list()?
        .into_iter()
        .find(|n| n.kind == OrgKind::Secretary)
        .map(|n| n.id)
        .ok_or_else(|| OpsError::Validation("no secretary is configured".to_string()))
}

#[allow(clippy::too_many_arguments)]
fn send_to_secretary(
    store: &dyn TaskStore,
    project: &Project,
    text: &str,
    roles: &[RoleSpec],
    genres: &[GenreSpec],
    conversation_genre: &str,
    now: OffsetDateTime,
) -> Result<StartedConversation, OpsError> {
    let secretary = secretary_id(store)?;
    conversation::start(
        store,
        &secretary,
        Some(project.id),
        text,
        roles,
        genres,
        conversation_genre,
        now,
    )
}

/// 返事を待たない一言（`role = user` の行だけを残す。run は起こさない）。
fn append_user_note(
    store: &dyn TaskStore,
    project_id: ProjectId,
    note: &str,
    now: OffsetDateTime,
) -> Result<(), OpsError> {
    let node_id = secretary_id(store)?;
    store.message_append(&Message {
        id: MessageId::new(),
        node_id,
        project_id: Some(project_id),
        role: MessageRole::User,
        text: note.to_string(),
        run_id: None,
        task_id: None,
        metadata: None,
        created_at: now,
    })?;
    Ok(())
}

/// 状態を変えた後の行を引き直す（`milestones` は案件ごとにしか引けない）。
fn reload(
    store: &dyn TaskStore,
    project_id: ProjectId,
    id: MilestoneId,
) -> Result<Milestone, OpsError> {
    store
        .milestone_list(project_id)?
        .into_iter()
        .find(|m| m.id == id)
        .ok_or_else(|| OpsError::Validation(format!("milestone {id} disappeared")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use task_core::SqliteStore;

    fn now() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap_or(OffsetDateTime::UNIX_EPOCH)
    }

    struct Env {
        _dir: tempfile::TempDir,
        store: SqliteStore,
    }

    impl Env {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
            let store =
                SqliteStore::open(&dir.path().join("t.db")).unwrap_or_else(|e| panic!("open: {e}"));
            Self { _dir: dir, store }
        }

        fn seed_secretary(&self) {
            self.store
                .org_upsert(&task_core::OrgNode {
                    profile: Default::default(),
                    id: "secretary".into(),
                    parent_id: None,
                    name: "秘書".into(),
                    kind: OrgKind::Secretary,
                    genre: None,
                    brief: String::new(),
                    position: 0,
                    created_at: now(),
                    updated_at: now(),
                })
                .unwrap_or_else(|e| panic!("org: {e}"));
        }

        fn seed_project(&self) -> Project {
            let project = Project {
                auto_advance: false,
                slug: None,
                archived_at: None,
                paused_from: None,
                id: ProjectId::new(),
                title: "Pluvio".into(),
                request: "調べて".into(),
                status: task_core::ProjectStatus::Active,
                secretary_summary: None,
                workspace: None,
                created_at: now(),
                updated_at: now(),
            };
            self.store
                .project_create(&project)
                .unwrap_or_else(|e| panic!("project: {e}"));
            project
        }
    }

    fn legacy_task(
        project: &Project,
        milestone: MilestoneId,
        status: task_core::Status,
        depends_on: Vec<TaskId>,
    ) -> Task {
        Task {
            tree: None,
            routing: None,
            mode: Default::default(),
            skills: Vec::new(),
            repos: Vec::new(),
            id: TaskId::new(),
            parent_id: None,
            kind: task_core::TaskKind::Execute,
            title: "t".into(),
            objective: "o".into(),
            acceptance: vec![],
            inputs: vec![],
            depends_on,
            status,
            priority: 3,
            worker_hint: task_core::WorkerHint {
                tier: task_core::Tier::Standard,
                adapter: None,
            },
            workspace: task_core::WorkspaceSpec::Local {
                path: std::path::PathBuf::from("/tmp/ws"),
                mode: None,
            },
            budget: task_core::Budget {
                max_turns: 10,
                max_wall_secs: 600,
                max_retries: 1,
            },
            attempts: 0,
            lease: None,
            created_at: now(),
            updated_at: now(),
            role: None,
            genre: None,
            aggregate: false,
            project_id: Some(project.id),
            milestone_id: Some(milestone),
            assignee: None,
            conversation: None,
            labels: Vec::new(),
            category: Default::default(),
        }
    }

    /// ADR-0074 D3.8（Phase F4b (g)）: 案件計画を持たない既存の案件は旧い挙動のまま。
    /// - 途中目標（`plan_key` 無し）に属する案件直下の Task 同士の依存は、依存先が `done` になれば
    ///   （途中目標の `reached` を待たずに）開く（同じ途中目標の中でも、別の途中目標をまたいでも）。
    /// - ADR-0038 の `ok` は旧い意味（`reached` + 最新の提案を `approved` にして**分解の計画 run** を起こす）。
    /// - 案件計画の版は 1 つも無い。
    #[test]
    fn legacy_project_keeps_linear_milestones() {
        let env = Env::new();
        env.seed_secretary();
        let project = env.seed_project();
        let m1 = env
            .store
            .milestone_create(
                project.id,
                "隣接領域の調査",
                "",
                MilestoneStatus::InProgress,
            )
            .unwrap_or_else(|e| panic!("m1: {e}"));
        let m2 = env
            .store
            .milestone_create(project.id, "候補の比較", "", MilestoneStatus::Approved)
            .unwrap_or_else(|e| panic!("m2: {e}"));
        assert_eq!(m1.plan_key, None);
        let a = legacy_task(&project, m1.id, task_core::Status::Done, vec![]);
        let b = legacy_task(&project, m1.id, task_core::Status::Ready, vec![a.id]);
        let d = legacy_task(&project, m2.id, task_core::Status::Ready, vec![a.id]);
        for t in [&a, &b, &d] {
            env.store
                .insert(t)
                .unwrap_or_else(|e| panic!("insert: {e}"));
            assert!(task_core::is_milestone_task(t));
        }
        let ready: Vec<TaskId> = env
            .store
            .ready_tasks(10)
            .unwrap_or_else(|e| panic!("ready: {e}"))
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert!(ready.contains(&b.id), "same milestone: {ready:?}");
        assert!(
            ready.contains(&d.id),
            "a legacy milestone does not gate on reached: {ready:?}"
        );

        // 秘書のレビューが次の途中目標を提案し、人が ok を押す（ADR-0038 の旧い意味）。
        let next = record_proposal(&env.store, project.id, Some(m1.id), "実験計画", "3 本")
            .unwrap_or_else(|e| panic!("record: {e}"))
            .unwrap_or_else(|| panic!("no proposal"));
        let decided = decide(
            &env.store,
            &project,
            &m1,
            MilestoneDecision::Ok,
            Some("この方針で"),
            &[],
            &[],
            task_core::CONVERSATION_GENRE,
            now(),
        )
        .unwrap_or_else(|e| panic!("decide: {e}"));
        assert_eq!(decided.milestone.status, MilestoneStatus::Reached);
        assert_eq!(decided.next_milestone.as_ref().map(|m| m.id), Some(next.id));
        let plan_task_id = decided
            .plan_task_id
            .unwrap_or_else(|| panic!("the old ok starts a decomposition plan run"));
        let plan_task = env
            .store
            .get(plan_task_id)
            .unwrap_or_else(|e| panic!("get: {e}"))
            .unwrap_or_else(|| panic!("plan task"));
        assert_eq!(plan_task.kind, task_core::TaskKind::Plan);
        assert!(!task_core::is_milestones_plan_task(&plan_task));
        assert_eq!(plan_task.milestone_id, Some(next.id));
        let next_now = env
            .store
            .milestone_get(next.id)
            .unwrap_or_else(|e| panic!("get: {e}"))
            .unwrap_or_else(|| panic!("next"));
        assert_eq!(next_now.status, MilestoneStatus::InProgress);
        assert!(
            project_plan::plan_state(&env.store, project.id)
                .unwrap_or_else(|e| panic!("state: {e}"))
                .versions
                .is_empty()
        );
    }

    /// ADR-0074 D3.6（Phase F4b）: 案件計画の途中目標への `ng` は `redesigned`（Go は開かない）にし、
    /// 案件の replan の計画 run（差分を書く run）を起こす。`ok` は `reached` だけ（次の分解 run は無い）。
    #[test]
    fn planned_milestone_ok_only_reaches_and_ng_starts_a_project_replan() {
        let env = Env::new();
        env.seed_secretary();
        let project = env.seed_project();
        let started = project_plan::start_milestones(&env.store, &project, None, &[], &[], now())
            .unwrap_or_else(|e| panic!("start: {e}"));
        let spec = |key: &str, deps: &[&str]| task_core::MilestoneSpec {
            key: key.into(),
            title: key.into(),
            objective: "o".into(),
            reach_criteria: "r".into(),
            acceptance: vec![task_core::Criterion {
                text: "ok".into(),
                check: task_core::Check::Command {
                    cmd: "true".into(),
                    expect_exit: 0,
                },
            }],
            depends_on: deps.iter().map(|d| d.to_string()).collect(),
            genre: None,
            skills: vec![],
            repos: vec![],
            features: None,
            execution: None,
            pause_after: None,
        };
        let validated = task_core::validate_project_plan(
            &task_core::ProjectPlanSpec {
                schema: task_core::PROJECT_PLAN_SCHEMA.into(),
                rationale: "r".into(),
                milestones: vec![spec("survey", &[]), spec("poc", &["survey"])],
            },
            task_core::ProjectPlanLimits::default(),
            &std::collections::BTreeSet::new(),
        )
        .unwrap_or_else(|e| panic!("valid: {e:?}"));
        project_plan::propose(
            &env.store,
            &started.task,
            &project,
            &validated,
            &[],
            &[],
            now(),
        )
        .unwrap_or_else(|e| panic!("propose: {e}"));
        project_plan::decide(
            &env.store,
            &project,
            1,
            project_plan::ProjectPlanDecision::Approve,
            None,
            &[],
            &[],
            task_core::CONVERSATION_GENRE,
            now(),
        )
        .unwrap_or_else(|e| panic!("approve: {e}"));
        let milestones = env
            .store
            .milestone_list(project.id)
            .unwrap_or_else(|e| panic!("list: {e}"));
        let by_key = |k: &str| {
            milestones
                .iter()
                .find(|m| m.plan_key.as_deref() == Some(k))
                .cloned()
                .unwrap_or_else(|| panic!("{k}"))
        };
        let (survey, poc) = (by_key("survey"), by_key("poc"));

        let ok = decide(
            &env.store,
            &project,
            &survey,
            MilestoneDecision::Ok,
            None,
            &[],
            &[],
            task_core::CONVERSATION_GENRE,
            now(),
        )
        .unwrap_or_else(|e| panic!("ok: {e}"));
        assert_eq!(ok.milestone.status, MilestoneStatus::Reached);
        assert_eq!(ok.plan_task_id, None);
        assert_eq!(ok.next_milestone, None);

        let ng = decide(
            &env.store,
            &project,
            &poc,
            MilestoneDecision::Ng,
            Some("PoC の切り方が悪い"),
            &[],
            &[],
            task_core::CONVERSATION_GENRE,
            now(),
        )
        .unwrap_or_else(|e| panic!("ng: {e}"));
        assert_eq!(ng.milestone.status, MilestoneStatus::Redesigned);
        let replan = env
            .store
            .get(ng.plan_task_id.unwrap_or_else(|| panic!("replan run")))
            .unwrap_or_else(|e| panic!("get: {e}"))
            .unwrap_or_else(|| panic!("task"));
        assert!(task_core::is_milestones_replan_task(&replan));
        assert!(
            replan.objective.contains("PoC の切り方が悪い"),
            "{}",
            replan.objective
        );
    }

    /// ADR-0038 D1: 提案は 1 件だけ `proposed` で残り、古い提案は `redesigned` に差し替わる。
    #[test]
    fn a_new_proposal_replaces_the_previous_one() {
        let env = Env::new();
        let project = env.seed_project();
        let first = record_proposal(&env.store, project.id, None, "候補の絞り込み", "3 本に")
            .unwrap_or_else(|e| panic!("record: {e}"))
            .unwrap_or_else(|| panic!("no milestone"));
        let second = record_proposal(&env.store, project.id, None, "実験計画", "")
            .unwrap_or_else(|e| panic!("record: {e}"))
            .unwrap_or_else(|| panic!("no milestone"));
        assert!(second.seq > first.seq, "seq は末尾");
        let all = env
            .store
            .milestone_list(project.id)
            .unwrap_or_else(|e| panic!("list: {e}"));
        let status = |id: MilestoneId| all.iter().find(|m| m.id == id).map(|m| m.status);
        assert_eq!(status(first.id), Some(MilestoneStatus::Redesigned));
        assert_eq!(status(second.id), Some(MilestoneStatus::Proposed));
        assert_eq!(
            latest_proposal(&env.store, project.id, None)
                .unwrap_or_else(|e| panic!("latest: {e}"))
                .map(|m| m.id),
            Some(second.id)
        );
        // 題名が空なら何もしない。
        assert_eq!(
            record_proposal(&env.store, project.id, None, "  ", "d")
                .unwrap_or_else(|e| panic!("record: {e}")),
            None
        );
        // 判定中の途中目標（`keep`）は差し替えの対象にしない。
        let keep = record_proposal(&env.store, project.id, None, "そのまま", "")
            .unwrap_or_else(|e| panic!("record: {e}"))
            .unwrap_or_else(|| panic!("no milestone"));
        record_proposal(&env.store, project.id, Some(keep.id), "次", "")
            .unwrap_or_else(|e| panic!("record: {e}"));
        let all = env
            .store
            .milestone_list(project.id)
            .unwrap_or_else(|e| panic!("list: {e}"));
        assert_eq!(
            all.iter().find(|m| m.id == keep.id).map(|m| m.status),
            Some(MilestoneStatus::Proposed)
        );
    }

    /// ADR-0038 D2: `discuss` / `ng` は理由が要る（空なら 422 になる `Validation`）。
    #[test]
    fn a_blank_note_is_rejected_for_discuss_and_ng() {
        let env = Env::new();
        env.seed_secretary();
        let project = env.seed_project();
        let milestone = env
            .store
            .milestone_create(
                project.id,
                "隣接領域の調査",
                "",
                MilestoneStatus::InProgress,
            )
            .unwrap_or_else(|e| panic!("milestone: {e}"));
        for decision in [MilestoneDecision::Discuss, MilestoneDecision::Ng] {
            let err = decide(
                &env.store,
                &project,
                &milestone,
                decision,
                Some("   "),
                &[],
                &[],
                task_core::CONVERSATION_GENRE,
                now(),
            )
            .err()
            .unwrap_or_else(|| panic!("expected a validation error"));
            assert!(matches!(err, OpsError::Validation(_)), "{err:?}");
        }
        // 何も変わっていない。
        let after = env
            .store
            .milestone_list(project.id)
            .unwrap_or_else(|e| panic!("list: {e}"));
        assert_eq!(after[0].status, MilestoneStatus::InProgress);
    }

    /// ADR-0038 D2 の 3 経路（状態遷移・計画 run・対話）。
    #[test]
    fn the_three_decisions_follow_the_adr_table() {
        let env = Env::new();
        env.seed_secretary();
        let project = env.seed_project();
        let milestone = env
            .store
            .milestone_create(
                project.id,
                "隣接領域の調査",
                "",
                MilestoneStatus::InProgress,
            )
            .unwrap_or_else(|e| panic!("milestone: {e}"));
        let proposal = record_proposal(
            &env.store,
            project.id,
            Some(milestone.id),
            "候補の絞り込み",
            "3 本に",
        )
        .unwrap_or_else(|e| panic!("record: {e}"))
        .unwrap_or_else(|| panic!("no proposal"));

        // discuss: 何も変えず、対話が 1 件。
        let decided = decide(
            &env.store,
            &project,
            &milestone,
            MilestoneDecision::Discuss,
            Some("候補 B の根拠が弱い"),
            &[],
            &[],
            task_core::CONVERSATION_GENRE,
            now(),
        )
        .unwrap_or_else(|e| panic!("decide: {e}"));
        assert_eq!(decided.milestone.status, MilestoneStatus::InProgress);
        assert!(decided.plan_task_id.is_none());
        let task_id = decided
            .conversation_task_id
            .unwrap_or_else(|| panic!("no conversation"));
        let task = env
            .store
            .get(task_id)
            .unwrap_or_else(|e| panic!("get: {e}"))
            .unwrap_or_else(|| panic!("none"));
        assert!(
            task.objective.contains("候補 B の根拠が弱い"),
            "{}",
            task.objective
        );
        assert_eq!(
            task.milestone_id, None,
            "人との議論は裏方のレビュー run ではない"
        );

        // ok: reached + 提案を approved（計画 run が in_progress にする）+ 計画 run。
        let decided = decide(
            &env.store,
            &project,
            &milestone,
            MilestoneDecision::Ok,
            Some("その方針で"),
            &[],
            &[],
            task_core::CONVERSATION_GENRE,
            now(),
        )
        .unwrap_or_else(|e| panic!("decide: {e}"));
        assert_eq!(decided.milestone.status, MilestoneStatus::Reached);
        assert_eq!(
            decided.next_milestone.as_ref().map(|m| m.id),
            Some(proposal.id)
        );
        assert_eq!(
            decided.next_milestone.as_ref().map(|m| m.status),
            Some(MilestoneStatus::InProgress),
            "承認したうえで分解が始まったので in_progress"
        );
        let plan_id = decided
            .plan_task_id
            .unwrap_or_else(|| panic!("no plan run"));
        let plan = env
            .store
            .get(plan_id)
            .unwrap_or_else(|e| panic!("get: {e}"))
            .unwrap_or_else(|| panic!("none"));
        assert_eq!(plan.kind, task_core::TaskKind::Plan);
        assert_eq!(plan.milestone_id, Some(proposal.id));
        assert!(plan.objective.contains("その方針で"), "{}", plan.objective);
        // 人の一言は秘書との対話にも残る。
        let messages = env
            .store
            .message_list("secretary", Some(project.id), 50)
            .unwrap_or_else(|e| panic!("messages: {e}"));
        assert!(
            messages
                .iter()
                .any(|m| m.text == "その方針で" && m.role == MessageRole::User)
        );

        // ng: この途中目標と提案を redesigned にし、再設計の対話を送る。
        let third = env
            .store
            .milestone_create(project.id, "実験", "", MilestoneStatus::InProgress)
            .unwrap_or_else(|e| panic!("milestone: {e}"));
        let next_proposal = record_proposal(&env.store, project.id, Some(third.id), "次の次", "")
            .unwrap_or_else(|e| panic!("record: {e}"))
            .unwrap_or_else(|| panic!("no proposal"));
        let decided = decide(
            &env.store,
            &project,
            &third,
            MilestoneDecision::Ng,
            Some("切り方が違う"),
            &[],
            &[],
            task_core::CONVERSATION_GENRE,
            now(),
        )
        .unwrap_or_else(|e| panic!("decide: {e}"));
        assert_eq!(decided.milestone.status, MilestoneStatus::Redesigned);
        assert_eq!(
            decided.next_milestone.as_ref().map(|m| m.status),
            Some(MilestoneStatus::Redesigned)
        );
        assert_eq!(
            decided.next_milestone.as_ref().map(|m| m.id),
            Some(next_proposal.id)
        );
        let task_id = decided
            .conversation_task_id
            .unwrap_or_else(|| panic!("no conversation"));
        let task = env
            .store
            .get(task_id)
            .unwrap_or_else(|e| panic!("get: {e}"))
            .unwrap_or_else(|| panic!("none"));
        assert!(
            task.objective.contains("切り方が違う"),
            "{}",
            task.objective
        );
        assert!(task.objective.contains("再設計"), "{}", task.objective);
    }

    /// ADR-0038 D1: レビューの対話は秘書の裏方タスク（`support = "milestone_review"`）になる。
    #[test]
    fn the_review_run_is_a_support_conversation_carrying_the_milestone() {
        let env = Env::new();
        env.seed_secretary();
        let project = env.seed_project();
        let milestone = env
            .store
            .milestone_create(
                project.id,
                "隣接領域の調査",
                "近い分野を洗う",
                MilestoneStatus::InProgress,
            )
            .unwrap_or_else(|e| panic!("milestone: {e}"));
        let text = review_request_text(&milestone, 2, 0, 1);
        assert!(text.contains("done 2 / failed 0、Go 待ち 1 件"), "{text}");
        assert!(text.contains("近い分野を洗う"), "{text}");
        let started = start_review(
            &env.store,
            &project,
            &milestone,
            &text,
            &[],
            &[],
            task_core::CONVERSATION_GENRE,
            now(),
        )
        .unwrap_or_else(|e| panic!("start: {e}"));
        assert_eq!(started.task.milestone_id, Some(milestone.id));
        assert_eq!(started.task.assignee.as_deref(), Some("secretary"));
        assert!(task_core::is_milestone_review(&started.task));
        assert_eq!(
            task_core::support_kind(&started.task),
            Some("milestone_review")
        );
        assert_eq!(started.task.status, task_core::Status::Ready);
    }
}
