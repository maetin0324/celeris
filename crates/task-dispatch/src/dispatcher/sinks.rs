//! ADR-0082: run 途中のイベントをストアへ流す `StoreSink`（worker run）と `ReviewerSink`（Reviewer run）。
//! browser ブランチの統合後に facade から移した（L1。`worker_task` と `review_spawn` が組み立てる）。

use super::*;

/// run 途中のイベントをストアに追記するシンク。ワーカーの出力（heartbeat）があればリースを延長する（ADR-0010 D7）。
pub(super) struct StoreSink {
    pub(super) auto_leaf: Option<auto_leaf::AutoLeafWatch>,
    pub(super) store: Arc<dyn TaskStore>,
    pub(super) task_id: TaskId,
    pub(super) run_id: String,
    /// 延長後の ttl（`idle_timeout + lease_grace`）。
    pub(super) lease_ttl: Duration,
    /// 延長の最小間隔（`lease_grace / 2`）。延長後の期限は常にアダプタの無出力タイムアウトより後になる。
    pub(super) renew_every: Duration,
    pub(super) last_renew: std::sync::Mutex<Instant>,
    /// ADR-0016 D2: 委譲の検証に使う `[[roles]]` と上限、この run で既に受け入れた件数。
    pub(super) roles: Vec<RoleSpec>,
    /// ADR-0027 D1: 委譲の分野解決・検証に使う `[[genres]]`。
    pub(super) genres: Vec<GenreSpec>,
    pub(super) delegation: DelegationLimits,
    pub(super) delegated_this_run: std::sync::atomic::AtomicUsize,
    /// ADR-0024 D4 / ADR-0025 D1: このアカウント（プールを使わなければ `None`）と、そのアダプタの観測値を記録する帳簿
    /// （呼び出し側があらかじめアダプタで解決して渡す）。
    pub(super) account: Option<String>,
    pub(super) account_book: Option<Arc<StdMutex<AccountBook>>>,
    /// ADR-0054 D1（Phase 67）: この run が継続セッションの対象なら `(node_id, kind, project_id)`。
    /// `session_established` / `session_resume_failed` がこれを使って `node_sessions` を書く。
    /// 継続セッションの対象でない run では `None`（両方 no-op）。
    pub(super) session_key: Option<(String, task_core::SessionKind, Option<ProjectId>)>,
    /// ADR-0140 D2: この run が WU の継続 session を使うなら `(task_id, work_unit_id)`。
    /// `session_resume_failed` がその session を retire し、拒否の印を run の進行に残す。
    pub(super) continuation_key: Option<(TaskId, Option<String>)>,
}

impl StoreSink {
    fn note(&self, msg: String) {
        let ev = Event::worker_progress(self.run_id.clone(), msg);
        if let Err(e) = self.store.append_event(self.task_id, &ev) {
            tracing::warn!(task_id = %self.task_id, error = %e, "failed to record delegation note");
        }
    }

    /// ADR-0016 D2 / M2 / M6: 提案を検証し、通ったものだけ子として挿入する。拒否理由は `WorkerProgress` に残し、run は失敗させない。
    fn delegate_impl(&self, tasks: &[DelegateTask]) -> Result<(), String> {
        let parent = self
            .store
            .get(self.task_id)
            .map_err(|e| format!("store: {e}"))?
            .ok_or_else(|| "task vanished".to_string())?;
        let ours = parent.status == Status::Running
            && parent.lease.as_ref().map(|l| l.worker_run_id.as_str())
                == Some(self.run_id.as_str());
        if !ours {
            return Err("task is no longer running under this run".to_string());
        }
        // ADR-0033 D4 / Phase 28: 対話 run は返事だけをする。委譲は受け付けず、理由を `progress` に残す
        // （実機で秘書が返事の代わりに research-survey へ委譲し、対話タスクが `blocked` に落ちた事故の再発防止）。
        if task_core::is_conversation(&parent) {
            return Err("対話では委譲できない。返事に『次にやりたいこと』として書け".to_string());
        }
        // ADR-0079 D4 (4)（Phase R1b）: 木の節点（計画の unit から作った子 task）は委譲を使わない。
        if parent.tree.is_some() {
            return Err(
                "木の節点（ADR-0079）では委譲できない。子 task は計画の kind task の unit から作る"
                    .to_string(),
            );
        }
        // ADR-0033 D4 / D5 / SPEC §3.1: 部をまたぐ連携は秘書が認める。別の部の課を `assignee` にした提案は
        // **子を作らずに**質問（`approvals` の 1 行になる固定の形）を残し、run の終わりに `Question` 終端へ
        // 回す。既に人が答えていれば（`once` / `standing`）その場で通す。判定は組織図と `approvals` /
        // `standing_rules` の前方一致だけを見る決定的なもので、LLM は使わない（DESIGN 原則 1）。
        // Phase 27（監査 H-2）: **バッチは分ける** — 同じ部宛ての提案はその場で子にする。
        let org = self.store.org_list().map_err(|e| format!("store: {e}"))?;
        // ADR-0069 D1（Phase 114）: 委譲（LLM）が書いた担当は使わない。担当は matching が決めるので、
        // 部をまたぐ認可（下の split）も担当を名指しした提案には起きなくなる。捨てた事実は進行に残す。
        let stripped: Vec<DelegateTask> = tasks
            .iter()
            .map(|t| {
                let mut t = t.clone();
                if let Some(a) = t.assignee.take().filter(|a| !a.trim().is_empty()) {
                    self.note(format!(
                        "delegate: 担当の指定 {a} は使わない（「{}」の担当は celeris が skills と harness から決定的に選ぶ。ADR-0069 D1）",
                        t.title
                    ));
                }
                t
            })
            .collect();
        let tasks: &[DelegateTask] = &stripped;
        let split =
            task_ops::conversation::split_delegation(self.store.as_ref(), &org, &parent, tasks)
                .map_err(|e| format!("authorization: {e}"))?;
        for denied in &split.denied {
            self.note(format!(
                "delegate denied: {} は人が認めなかった（子は作っていない）",
                denied.key()
            ));
        }
        for pending in &split.pending {
            self.store
                .append_event(
                    self.task_id,
                    &Event::QuestionRaised {
                        run_id: self.run_id.clone(),
                        text: pending.question(),
                    },
                )
                .map_err(|e| format!("store: {e}"))?;
        }
        if !split.pending.is_empty() {
            self.note(format!(
                "delegate deferred: {} 件は秘書の認可待ち（部をまたぐ委譲。子は作っていない）: {}",
                split.pending.len(),
                split
                    .pending
                    .iter()
                    .map(|c| c.key())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if split.allowed.is_empty() {
            return Ok(());
        }
        let tasks: &[DelegateTask] = &split.allowed;
        let already = self
            .delegated_this_run
            .load(std::sync::atomic::Ordering::SeqCst);
        let outcome = plan_delegation(
            self.store.as_ref(),
            &parent,
            tasks,
            already,
            &self.roles,
            &self.genres,
            &self.delegation,
            OffsetDateTime::now_utc(),
        )
        .map_err(|e| format!("validation: {e}"))?;
        for reason in &outcome.rejected {
            self.note(format!("delegate rejected: {reason}"));
        }
        // ADR-0062 B2（Phase 107）: 継承した Remote が担当の道具不足で Local に落ちたことをログに残す。
        for (child_id, reason) in &outcome.workspace_downgrades {
            tracing::info!(task_id = %self.task_id, child_id = %child_id, %reason, "workspace downgraded to local (ADR-0062 B2)");
        }
        if outcome.accepted.is_empty() {
            return Ok(());
        }
        let n = outcome.accepted.len();
        let ids = self
            .store
            .delegate_children(self.task_id, &self.run_id, outcome.accepted)
            .map_err(|e| format!("insert: {e}"))?;
        self.delegated_this_run
            .fetch_add(n, std::sync::atomic::Ordering::SeqCst);
        let listed: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
        // Phase 27（監査 H-2）: 「N 件は作った、M 件は秘書の認可待ち」がワーカーの目にも入るようにする。
        let pending = if split.pending.is_empty() {
            String::new()
        } else {
            format!("（{} 件は秘書の認可待ち）", split.pending.len())
        };
        self.note(format!(
            "delegated {n} child task(s){pending}: {}",
            listed.join(", ")
        ));
        tracing::info!(task_id = %self.task_id, run_id = %self.run_id, children = n, "delegated child tasks inserted");
        Ok(())
    }
}

impl EventSink for StoreSink {
    fn browser_wait_open(
        &self,
        request: &task_core::browser_wait::NewBrowserWait,
    ) -> Result<(), String> {
        self.store
            .browser_wait_open(self.task_id, request, OffsetDateTime::now_utc())
            .map(|_| ())
            .map_err(|e| e.code().into())
    }
    fn browser_waits(&self) -> Result<Vec<task_core::browser_wait::BrowserWait>, String> {
        self.store
            .browser_waits_for_task(self.task_id)
            .map_err(|_| "browser wait store unavailable".into())
    }
    fn browser_approval_consume(
        &self,
        wait: &task_core::browser_wait::BrowserWait,
    ) -> Result<task_core::browser_wait::ConsumedBrowserApproval, String> {
        task_core::browser_wait::consume_credential_approval(
            self.store.as_ref(),
            self.task_id,
            wait,
            OffsetDateTime::now_utc(),
        )
        .map_err(String::from)
    }
    fn browser_auth_section(
        &self,
        run_id: &str,
        session_id: &str,
        active: bool,
    ) -> Result<(), String> {
        let task_id = self.task_id.to_string();
        self.store
            .browser_session_auth_section(
                task_core::browser_store::BrowserSessionKey {
                    task_id: &task_id,
                    run_id,
                    session_id,
                },
                active,
            )
            .map(|_| ())
            .map_err(|_| "browser control store unavailable".into())
    }
    fn browser_control_gate(
        &self,
        run_id: &str,
        session_id: &str,
    ) -> Option<Arc<dyn task_worker::browser_live::ControlGate>> {
        Some(Arc::new(task_worker::browser_live::StoreGate::new(
            self.store.clone(),
            &self.task_id.to_string(),
            run_id,
            session_id,
        )))
    }
    fn browser_live(
        &self,
        run_id: &str,
        session_id: &str,
        event: &task_core::browser_live::ScrubbedLiveEvent,
    ) {
        let task_id = self.task_id.to_string();
        if let Err(e) = self.store.browser_session_live_append(
            task_core::browser_store::BrowserSessionKey {
                task_id: &task_id,
                run_id,
                session_id,
            },
            event,
        ) {
            tracing::warn!(task_id = %self.task_id, error = %e, "failed to record browser live event");
        }
    }
    fn browser_updated(&self, browser: &task_core::BrowserRun) {
        if let Err(e) = self.store.append_event(
            self.task_id,
            &Event::BrowserUpdated {
                browser: browser.clone(),
            },
        ) {
            tracing::warn!(task_id = %self.task_id, error = %e, "failed to record browser lifecycle");
        }
    }
    fn progress(&self, msg: &str) {
        let ev = Event::worker_progress(self.run_id.clone(), msg);
        if let Err(e) = self.store.append_event(self.task_id, &ev) {
            tracing::warn!(task_id = %self.task_id, error = %e, "failed to record progress");
        }
    }

    /// ADR-0048 D2（Phase 60a）: 構造化した進行をそのまま `Event::WorkerProgress` に残す
    /// （`kind` / `tool` / `summary` / `detail` を写す）。自動 leaf の構造化 compaction 印だけは監視を起こす。
    fn progress_with(&self, msg: &str, fields: &task_core::ProgressFields) {
        let ev = Event::worker_progress_with(self.run_id.clone(), msg, fields.clone());
        if let Err(e) = self.store.append_event(self.task_id, &ev) {
            tracing::warn!(task_id = %self.task_id, error = %e, "failed to record progress");
        }
        if fields.kind == Some(task_core::ProgressKind::Status)
            && fields.tool.as_deref() == Some(task_core::tree::CONTEXT_COMPACTION_TOOL)
            && let Some(watch) = &self.auto_leaf
        {
            watch.compacted();
        }
    }

    /// ADR 2026-10-07-worker-no-subagents-no-llm-cli D5/D6: 検出を `Event::WorkerPolicyViolation` に残す
    /// （run は止めない。reviewer が `review_spawn` 経由で読む）。
    fn policy_violation(&self, violation: &task_worker::tool_policy::ToolPolicyViolation) {
        let ev = Event::WorkerPolicyViolation {
            run_id: self.run_id.clone(),
            kind: violation.kind,
            tool: violation.tool.clone(),
            matched: violation.matched.clone(),
            command: violation.command.clone(),
        };
        if let Err(e) = self.store.append_event(self.task_id, &ev) {
            tracing::warn!(task_id = %self.task_id, error = %e, "failed to record tool policy violation");
        }
    }

    fn artifact(&self, artifact: &ArtifactRef) {
        let ev = Event::ArtifactProduced {
            run_id: self.run_id.clone(),
            artifact: artifact.clone(),
        };
        if let Err(e) = self.store.append_event(self.task_id, &ev) {
            tracing::warn!(task_id = %self.task_id, error = %e, "failed to record artifact");
        }
    }

    /// ADR-0044 D2（Phase 53）: ワーカーの `{"type":"comment"}` は `author_kind = node` で残す。
    /// 人は起こさない（通知は ADR-0037 の 5 種のまま）。状態は変えない。
    fn comment(&self, body: &str) {
        let author = self
            .store
            .get(self.task_id)
            .ok()
            .flatten()
            .and_then(|t| t.assignee.clone());
        match task_ops::comment::post_node_comment(
            self.store.as_ref(),
            self.task_id,
            author,
            Some(self.run_id.clone()),
            body.to_string(),
            OffsetDateTime::now_utc(),
        ) {
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(task_id = %self.task_id, run_id = %self.run_id, error = %e, "failed to record the worker comment")
            }
        }
    }

    fn delegate(&self, tasks: &[DelegateTask]) {
        if let Err(reason) = self.delegate_impl(tasks) {
            tracing::warn!(task_id = %self.task_id, run_id = %self.run_id, %reason, "delegate proposal ignored");
            self.note(format!("delegate ignored: {reason}"));
        }
    }

    /// ADR-0070 D5（Phase 116）: `renew_lease` が DB busy/locked で失敗しても、すぐには諦めない。
    /// この呼び出しの中で最大 [`RENEW_LEASE_RETRIES`] 回（[`RENEW_LEASE_RETRY_DELAY`] 間隔）やり直す。
    /// それでも失敗したら WARN のみ（run はこの呼び出しの成否に関わらず続く。DB が一時的に混んでいた
    /// だけで run を止めない）。
    fn heartbeat(&self) {
        let Ok(mut last) = self.last_renew.lock() else {
            return;
        };
        if last.elapsed() < self.renew_every {
            return;
        }
        *last = Instant::now();
        let mut attempt = 0;
        loop {
            match self
                .store
                .renew_lease(self.task_id, &self.run_id, self.lease_ttl)
            {
                Ok(true) => return,
                Ok(false) => {
                    tracing::debug!(task_id = %self.task_id, run_id = %self.run_id, "lease not renewed (no longer running under this run)");
                    return;
                }
                Err(e) if task_core::is_busy_error(&e) && attempt < RENEW_LEASE_RETRIES => {
                    attempt += 1;
                    tracing::debug!(task_id = %self.task_id, run_id = %self.run_id, attempt, "lease renewal hit a busy database; retrying (ADR-0070 D5)");
                    std::thread::sleep(RENEW_LEASE_RETRY_DELAY);
                }
                Err(e) => {
                    tracing::warn!(task_id = %self.task_id, error = %e, "failed to renew lease");
                    return;
                }
            }
        }
    }

    /// ADR-0024 D4: run の途中でも観測値を `AccountBook` に記録する（`source = "run"`）。プールを使わない run では
    /// `account` が `None` なので no-op。
    fn rate_limit(&self, obs: RateLimitObservation) {
        let Some(account) = &self.account else { return };
        let Some(book) = &self.account_book else {
            return;
        };
        let Ok(mut book) = book.lock() else { return };
        book.record_observation(account, obs, ObservationSource::Run);
        if let Err(e) = book.save() {
            tracing::warn!(task_id = %self.task_id, %account, error = %e, "failed to save account book after rate_limit observation");
        }
    }

    /// ADR-0054 D1（Phase 67）: アダプタが run の途中で確定させた id（codex / acp）を `node_sessions` へ
    /// 書く。claude-code は celeris が前もって決めた id をそのまま報告するだけなので、通常は上書きでも
    /// 値は変わらない。`session_key` が無い run（継続セッションの対象でない）では no-op。
    fn session_established(&self, session_id: &str) {
        let Some((node_id, kind, project_id)) = &self.session_key else {
            return;
        };
        if let Err(e) = self
            .store
            .node_session_set_id(node_id, *kind, *project_id, session_id)
        {
            tracing::warn!(task_id = %self.task_id, error = %e, "failed to record the established session id");
        }
    }

    /// ADR-0054 D1（Phase 67）: resume が拒否されたら、そのセッションを retire する（次の run は新規
    /// セッションになる。ADR-0054 D1「失敗も同じ経路で作り直す」）。`session_key` が無ければ no-op。
    fn session_resume_failed(&self, reason: &str) {
        // ADR-0140 D1: WU の継続 session の resume が拒否されたら retire し、次の dispatch が
        // `resume_rejected`（checkpoint 前置きの fresh）と判定できる印を残す。
        if let Some((task_id, work_unit_id)) = &self.continuation_key {
            if let Err(e) = self.store.work_unit_session_retire(
                *task_id,
                work_unit_id.as_deref(),
                OffsetDateTime::now_utc(),
            ) {
                tracing::warn!(task_id = %self.task_id, error = %e, "failed to retire the continuation session after a rejected resume");
            }
            let first_line = reason.lines().next().unwrap_or_default();
            let ev = Event::worker_progress_with(
                self.run_id.clone(),
                format!(
                    "{}: {}",
                    super::continuation_session::CONTINUATION_RESUME_REJECTED,
                    first_line.chars().take(200).collect::<String>()
                ),
                task_core::ProgressFields::of(task_core::ProgressKind::Status),
            );
            if let Err(e) = self.store.append_event(self.task_id, &ev) {
                tracing::warn!(task_id = %self.task_id, error = %e, "failed to record the rejected continuation resume");
            }
            return;
        }
        let Some((node_id, kind, project_id)) = &self.session_key else {
            return;
        };
        match self
            .store
            .node_session_retire(node_id, *kind, *project_id, OffsetDateTime::now_utc())
        {
            Ok(true) => {
                tracing::warn!(task_id = %self.task_id, node_id, %reason, "resume rejected; session retired");
            }
            Ok(false) => {}
            Err(e) => {
                tracing::warn!(task_id = %self.task_id, error = %e, "failed to retire the session after a rejected resume");
            }
        }
    }
}

/// `Reviewer` run のシンク（ADR-0007 D5 6.）。進捗は対象 run の `WorkerProgress` に
/// `reviewer run <review_run_id>: ` を付けて記録し、レビュー run の成果物は記録しない
/// （`artifacts_for_run` が対象 run の成果物だけを返すようにするため）。
pub(super) struct ReviewerSink {
    pub(super) store: Arc<dyn TaskStore>,
    pub(super) task_id: TaskId,
    pub(super) subject_run_id: String,
    pub(super) review_run_id: String,
    /// ADR-0024 D4: Reviewer run もプールのアカウントで走ることがあるので、同じ帳簿に観測値を記録する
    /// （呼び出し側があらかじめアダプタで解決して渡す。ADR-0025 D1）。
    pub(super) account: Option<String>,
    pub(super) account_book: Option<Arc<StdMutex<AccountBook>>>,
    /// ADR-0054 D1 / Phase 67b 追記: この Reviewer run が部門長の継続セッション（`kind = lead`）の
    /// 対象なら `(department_id, Lead, None)`。`session_established`/`session_resume_failed` がこれを
    /// 使って `node_sessions` を書く。部署の無い（従来の独立）Reviewer run では `None`（両方 no-op）。
    /// Phase 67 の実装では**この配線が抜けていて**、Lead セッションの resume 拒否が一切 retire
    /// されなかった（ADR-0054 D1「失敗も同じ経路で作り直す」が Lead セッションには効いていなかった）。
    pub(super) session_key: Option<(String, task_core::SessionKind, Option<ProjectId>)>,
}

impl EventSink for ReviewerSink {
    fn progress(&self, msg: &str) {
        let ev = Event::worker_progress(
            self.subject_run_id.clone(),
            format!("reviewer run {}: {msg}", self.review_run_id),
        );
        if let Err(e) = self.store.append_event(self.task_id, &ev) {
            tracing::warn!(task_id = %self.task_id, error = %e, "failed to record reviewer progress");
        }
    }

    fn artifact(&self, artifact: &ArtifactRef) {
        tracing::debug!(task_id = %self.task_id, review_run_id = %self.review_run_id, name = %artifact.name, "reviewer run artifact ignored");
    }

    /// ADR 2026-10-07-worker-no-subagents-no-llm-cli D5: reviewer run の検出も同じ event に残す（`run_id` は
    /// reviewer run の id。対象 run の判定材料には混ぜない — `review_spawn` は対象 run の id で絞る）。
    fn policy_violation(&self, violation: &task_worker::tool_policy::ToolPolicyViolation) {
        let ev = Event::WorkerPolicyViolation {
            run_id: self.review_run_id.clone(),
            kind: violation.kind,
            tool: violation.tool.clone(),
            matched: violation.matched.clone(),
            command: violation.command.clone(),
        };
        if let Err(e) = self.store.append_event(self.task_id, &ev) {
            tracing::warn!(task_id = %self.task_id, error = %e, "failed to record reviewer tool policy violation");
        }
    }

    fn rate_limit(&self, obs: RateLimitObservation) {
        let Some(account) = &self.account else { return };
        let Some(book) = &self.account_book else {
            return;
        };
        let Ok(mut book) = book.lock() else { return };
        book.record_observation(account, obs, ObservationSource::Run);
        if let Err(e) = book.save() {
            tracing::warn!(task_id = %self.task_id, %account, error = %e, "failed to save account book after reviewer rate_limit observation");
        }
    }

    /// ADR-0054 D1 / Phase 67b 追記: `StoreSink::session_established` と同じ（`node_sessions` の
    /// `session_id` を上書きする）。`session_key` が無ければ no-op。
    fn session_established(&self, session_id: &str) {
        let Some((node_id, kind, project_id)) = &self.session_key else {
            return;
        };
        if let Err(e) = self
            .store
            .node_session_set_id(node_id, *kind, *project_id, session_id)
        {
            tracing::warn!(task_id = %self.task_id, error = %e, "failed to record the established lead session id");
        }
    }

    /// ADR-0054 D1 / Phase 67b 追記: `StoreSink::session_resume_failed` と同じ（resume が拒否されたら
    /// この場で retire し、次の `resolve_node_session` が新しい Lead セッションを作る）。Phase 67 では
    /// この配線が抜けていて、部門長のレビュー run の resume 拒否が retire されずに残り続けた
    /// （本番で ULID の session_id が retire されないまま resume され続けた一因）。`session_key` が
    /// 無ければ no-op。
    fn session_resume_failed(&self, reason: &str) {
        let Some((node_id, kind, project_id)) = &self.session_key else {
            return;
        };
        match self
            .store
            .node_session_retire(node_id, *kind, *project_id, OffsetDateTime::now_utc())
        {
            Ok(true) => {
                tracing::warn!(task_id = %self.task_id, node_id, %reason, "lead session resume rejected; session retired");
            }
            Ok(false) => {}
            Err(e) => {
                tracing::warn!(task_id = %self.task_id, error = %e, "failed to retire the lead session after a rejected resume");
            }
        }
    }
}
