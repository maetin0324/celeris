use super::*;

#[derive(Default)]
pub(super) struct BlockedAttention {
    pub(super) phases: Vec<AttentionItem>,
    pub(super) prerequisites: Vec<AttentionItem>,
    pub(super) plans: Vec<AttentionItem>,
}

/// Task list shared only within a read pass. Never use this for leases or writes.
pub struct InboxTaskSnapshot<'a> {
    store: &'a dyn TaskStore,
    tasks: Vec<Task>,
    by_id: HashMap<TaskId, Task>,
}

impl<'a> InboxTaskSnapshot<'a> {
    pub fn read(store: &'a dyn TaskStore) -> Result<Self, OpsError> {
        let tasks = store.list(None)?;
        let by_id = tasks.iter().map(|t| (t.id, t.clone())).collect();
        Ok(Self {
            store,
            tasks,
            by_id,
        })
    }

    pub fn human_inbox_touching(
        &self,
        snapshot: Option<&DaemonSnapshot>,
        ctx: &ViewContext,
        now: OffsetDateTime,
        evidence: &dyn Fn(&Task, &str) -> Vec<EvidenceView>,
        touching: &BTreeSet<TaskId>,
    ) -> Result<crate::human_inbox::HumanInbox, OpsError> {
        let inbox = self.parts(snapshot, ctx, now, evidence, Some(touching))?;
        Ok(crate::human_inbox::retain_touching(
            crate::human_inbox::from_inbox(&inbox, &self.by_id, now),
            touching,
        ))
    }

    pub fn human_inbox(
        &self,
        snapshot: Option<&DaemonSnapshot>,
        ctx: &ViewContext,
        now: OffsetDateTime,
        evidence: &dyn Fn(&Task, &str) -> Vec<EvidenceView>,
    ) -> Result<crate::human_inbox::HumanInbox, OpsError> {
        let inbox = self.parts(snapshot, ctx, now, evidence, None)?;
        Ok(crate::human_inbox::from_inbox(&inbox, &self.by_id, now))
    }

    pub(super) fn parts(
        &self,
        snapshot: Option<&DaemonSnapshot>,
        ctx: &ViewContext,
        now: OffsetDateTime,
        evidence: &dyn Fn(&Task, &str) -> Vec<EvidenceView>,
        touching: Option<&BTreeSet<TaskId>>,
    ) -> Result<Inbox, OpsError> {
        let store = self.store;
        let all_tasks = &self.tasks;
        let by_id = &self.by_id;
        let focus = touching.map(|t| focus_of(all_tasks, t));
        let focus = focus.as_ref();

        let mut blocked_attention = BlockedAttention::default();
        let approvals = build_approvals(store, all_tasks, by_id, evidence)?;
        let questions = build_questions(store, all_tasks, focus, &mut blocked_attention)?;
        let drafts = build_drafts(store, all_tasks, by_id, ctx, now)?;
        let mut suppressed = BTreeMap::new();
        let attention = build_attention(
            store,
            all_tasks,
            by_id,
            snapshot,
            ctx,
            now,
            focus,
            blocked_attention,
        )?
        .into_iter()
        .filter(|item| {
            let rule = attention_task(item)
                .and_then(|id| by_id.get(&id))
                .and_then(|task| attention_suppression(task, by_id));
            if let Some(rule) = rule {
                *suppressed.entry(rule.key().to_string()).or_insert(0) += 1;
                false
            } else {
                true
            }
        })
        .collect::<Vec<_>>();
        let browser_waits = crate::browser::pending_items(store, by_id)?;

        let by_status = store
            .count_by_status()?
            .into_iter()
            .map(|(s, n)| (view::status_key(s).to_string(), n))
            .collect();

        // ADR-0079 D7（Phase R3a）: 未回答の決定の要求（決定を出した節点が終端でないもの。古い順）。
        let decisions = crate::decision::inbox_items(store, now)?;

        // ADR 2026-10-07-build-tmp-hygiene D4.3: `critical` に下がるまでの間だけ受信箱に出る（派生）。
        let disk_full = store
            .disk_watch_states()?
            .into_iter()
            .filter(|s| s.level == task_core::DiskLevel::Critical)
            .collect();

        // 1 回の短い読み（`credential_waits` は開いている行だけの小さな表。events は走査しない）。
        let credential_waits = store
            .credential_waits_list_waiting()?
            .into_iter()
            .filter(|w| {
                matches!(
                    w.state,
                    task_core::CredentialWaitState::Waiting
                        | task_core::CredentialWaitState::Rejected
                )
            })
            .collect();

        // 付記 S5-7: 開いている行だけの小さな表（daemon の script-guard が event から作る）。
        let script_control = store.script_inbox_open()?;

        let counts = InboxCounts {
            approvals: approvals.len() as u32,
            questions: questions.len() as u32,
            // グループ数ではなく draft タスクの件数（バッジ表示用。Phase 9 監査）。
            drafts: drafts.iter().map(|g| g.drafts.len() as u32).sum(),
            attention: attention.len() as u32,
            browser_waits: browser_waits.len() as u32,
            decisions: decisions.len() as u32,
            by_status,
        };

        Ok(Inbox {
            approvals,
            questions,
            drafts,
            attention,
            suppressed,
            browser_waits,
            decisions,
            counts,
            disk_full,
            credential_waits,
            script_control,
            stagnation: snapshot.map(|s| s.stagnation.clone()).unwrap_or_default(),
            stagnation_policy: snapshot.and_then(|s| s.stagnation_policy),
        })
    }
}

pub(crate) fn question_for_task(
    store: &dyn TaskStore,
    task: &Task,
) -> Result<Option<QuestionItem>, OpsError> {
    Ok(build_questions(
        store,
        std::slice::from_ref(task),
        None,
        &mut BlockedAttention::default(),
    )?
    .into_iter()
    .next())
}
