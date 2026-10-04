//! run の文脈の組み立て（run_extras・session・knowledge・skills・milestone）。ADR-0082 の L1。

use super::*;

impl Dispatcher {
    /// ADR-0016 D1 / D3, ADR-0027 D1: run 開始時にワーカーへ渡す役割の指示文、委譲できる run なら使える
    /// 分野の一覧、集約 run なら子の要約。
    pub(super) fn run_extras(
        &self,
        task: &Task,
        worktree: Option<&task_worker::TaskWorkspaces>,
        // ADR-0054 D1（Phase 67）: 選んだアカウントとアダプタ id。CoS の対話・部門長のレビュー run の
        // 継続セッション（`session` / `session_diff`）を決めるのに要る。他の全ての値の計算には使わない。
        account: Option<&str>,
        adapter_id: &str,
    ) -> Result<RunExtras, DispatchError> {
        let role = task.role.as_deref().map(|id| RoleContext {
            id: id.to_string(),
            instructions: RoleSpec::find(&self.config.roles, id)
                .and_then(|r| r.instructions.clone())
                .unwrap_or_default(),
        });
        // ADR-0027 D1 / ADR-0028 D3: 委譲の指示文を出す run（Execute/Approval）と、子の分野を選べる
        // Plan run（`build_plan_prompt` も同じ節を出す）にだけ使える分野の一覧を渡す
        // （プロンプト側の条件と同じ。`claude_code::build_prompt` 参照）。
        // ADR-0033 D4 / Phase 28: 対話 run は委譲できないので渡さない（`delegate.json` を書かせない）。
        let is_conv = task_core::is_conversation(task);
        // ADR-0079 D4 (4)（Phase R1b）: 木の節点（計画の unit から作った子 task）の run は委譲できない
        // （子を作る入口は計画の kind task の unit だけ）。
        let available_genres = if !is_conv
            && task.tree.is_none()
            && matches!(
                task.kind,
                TaskKind::Execute | TaskKind::Approval | TaskKind::Plan
            ) {
            self.config
                .genres
                .iter()
                .map(|g| GenreContext::from_spec(g, &self.config.roles))
                .collect()
        } else {
            Vec::new()
        };
        // ADR-0033 D4 / D6（Phase 24）: この run をする「人」（担当のノード）、その記憶、この案件での
        // 直近のやり取り、そして分解・委譲できる run には組織図。全てストアとファイルの読み取りだけで、
        // LLM は使わない（DESIGN 原則 1）。
        let org = if task.assignee.is_some() || !available_genres.is_empty() {
            self.store.org_list()?
        } else {
            Vec::new()
        };
        let assigned = task
            .assignee
            .as_deref()
            .and_then(|id| org.iter().find(|n| n.id == id));
        let node = assigned.map(|n| NodeContext {
            id: n.id.clone(),
            name: n.name.clone(),
            brief: n.brief.clone(),
        });
        // ADR-0033 D5（Phase 26）: 担当がいる run にだけ、その担当宛て + 全員向けの永続の認可を注入する
        // （担当がいない run に、たまたま同じ id を持つ他ノード宛ての規則を混ぜないため。memory/conversation
        // と同じ条件）。
        let standing_rules = match assigned {
            Some(n) => self
                .store
                .standing_rule_list(Some(&n.id))?
                .into_iter()
                .map(|r| r.rule)
                .collect(),
            None => Vec::new(),
        };
        let memory = match (&self.config.memory_dir, assigned) {
            (Some(dir), Some(n)) => Some(
                MemoryDir::new(dir).load(&n.id, task.project_id.map(|p| p.to_string()).as_deref()),
            ),
            _ => None,
        };
        let conversation = match assigned {
            Some(n) => {
                let mut turns: Vec<ConversationTurn> = self
                    .store
                    .message_list(
                        &n.id,
                        task.project_id,
                        task_ops::conversation::CONVERSATION_HISTORY,
                    )?
                    .iter()
                    .map(|m| ConversationTurn {
                        role: m.role,
                        text: m.text.clone(),
                    })
                    .collect();
                // 監査 L-6: 今回の本文（`objective`）と同じ最後の `user` の行は落とす（二重に載せない）。
                if let Some(last) = turns.last()
                    && last.role == task_core::MessageRole::User
                    && last.text == task.objective
                {
                    turns.pop();
                }
                turns
            }
            None => Vec::new(),
        };
        // ADR-0033 D4 / Phase 28: 対話 run にだけ、相手が秘書（ADR-0046 D6 の CoS）かそれ以外かを渡す
        // （`preamble` が「作業を始めるな、返事だけ書け」の指示文を出し分けるためだけの印。担当が組織に
        // 無ければ CoS 以外扱いにする。判定は決定的で LLM は使わない）。
        let conversation_addressee = if is_conv {
            Some(match assigned {
                Some(n) if n.kind == OrgKind::Secretary => ConversationAddressee::Secretary,
                _ => ConversationAddressee::Other,
            })
        } else {
            None
        };
        // 分解・委譲できる run（`available_genres` を渡す run と同じ条件）にだけ組織図を渡す。
        // ADR-0046 D6: **CoS の対話 run** にも渡す（誰が何をできるかを見せる。人選はしない）。
        let is_cos_conversation = conversation_addressee == Some(ConversationAddressee::Secretary);
        // ADR-0054 D1（Phase 67）: CoS の対話は継続セッション（全体で 1 本。`project_id` は常に
        // `None`。案件を開いているときも前置きに案件の文脈を足すだけでセッションは同じ）。対応しない
        // アダプタ（claude-code/codex/acp 以外）では継続しない（`None` のまま。前置きは Phase 66 までと
        // バイト単位で同じ）。organization / node / memory / conversation はこの直後で、継続中
        // （`resume = true`）なら差分だけにする（毎回流し直さない。D1「前置きは継続中は差分だけ」）。
        let (session, session_diff) = if is_cos_conversation {
            self.resolve_node_session(
                task_core::COS_ID,
                task_core::SessionKind::Conversation,
                None,
                adapter_id,
                account,
                OffsetDateTime::now_utc(),
            )?
        } else {
            (None, Vec::new())
        };
        // ADR-0054 D1: 継続中（`resume = true`）の CoS 対話 run だけ、この後の brief・記憶・組織の一覧・
        // 直近のやり取り・進行中の案件を差分に置き換える（省く）。新規セッション（`resume = false`。
        // rollover・アカウント変更・resume 失敗の後を含む）では、これまでどおり全量を渡す。
        let continuing = is_cos_conversation && session.as_ref().is_some_and(|s| s.resume);
        let node = if continuing { None } else { node };
        let memory = if continuing { None } else { memory };
        let conversation = if continuing { Vec::new() } else { conversation };
        let organization = if (available_genres.is_empty() && !is_cos_conversation) || continuing {
            Vec::new()
        } else {
            org.iter()
                .map(|n| OrgNodeContext::with_profile(n, &task_core::resolve_profile(&org, &n.id)))
                .collect()
        };
        // ADR-0046 D1（Phase 59）: 担当ノードの実効 profile（根→葉の merge ＋ タスクの上書き）。
        // profile を 1 つも書いていない組織では `None`（前置きは Phase 58 までとバイト単位で同じ）。
        let profile = assigned.and_then(|n| {
            let effective = task_core::resolve_profile(&org, &n.id);
            if effective.is_trivial() {
                None
            } else {
                Some(effective.with_task(task))
            }
        });
        // ADR-0046 D4（Phase 59）: 既定（`production`）の進め方は渡さない（前置きを変えない）。
        let mode = if task.mode == task_core::TaskMode::Production {
            None
        } else {
            Some(task.mode)
        };
        // Phase 30（ADR-0033 D4 追記）: 対話は常に対話用分野で走る（`task.genre`）。その人が自分の仕事で
        // 何を使うかを知って答えられるように、対話 run にだけ、担当ノード**自身**の分野
        // （`node.genre`。対話用分野とは別物）を「仕事で使う道具」として渡す。決定的（`[[genres]]` の
        // manifest を引くだけ）。
        let work_genre = if is_conv {
            assigned
                .and_then(|n| n.genre.as_deref())
                .and_then(|id| GenreSpec::find(&self.config.genres, id))
                .map(|g| GenreContext::from_spec(g, &self.config.roles))
        } else {
            None
        };
        // Phase 33（ADR-0033 D4 追記。実機の事故 — 担当が自分の直近の失敗を知らずに「対象タスク ID が
        // 必要です」と聞き返した — の再発防止）: 対話 run にだけ、担当の直近の仕事を渡す。
        let recent_work = if is_conv {
            match assigned {
                Some(n) => self.recent_work_of(&n.id, task.project_id)?,
                None => Vec::new(),
            }
        } else {
            Vec::new()
        };
        // Phase 41（ADR-0038 D1）: 途中目標レビューの対話 run にだけ、その途中目標とそこまでの仕事の成果を
        // 渡す（集めるのは決定的: ストアのタスク・イベントと成果物ファイルを読むだけ）。
        let milestone_review = match task_core::milestone_review_of(task) {
            Some(milestone_id) => self.milestone_review_of(task.project_id, milestone_id)?,
            None => None,
        };
        // Phase 43（ADR-0039 D3）: 案件が作業場所を決めていれば、その場所を前置きに出す（決定的:
        // `projects.workspace` を引いて 1 行にするだけ）。対話 run（秘書との会話・途中目標のレビュー）には
        // 出さない（会話は編集をしないので、人のリポジトリの中で走らせる理由が無い。ADR-0039 D2）。
        let mut workspace_note = match conversation_addressee {
            Some(_) => None,
            None => task_ops::delegate::project_workspace(self.store.as_ref(), task)
                .map_err(ops_to_store)?
                .as_ref()
                .map(task_worker::preamble::workspace_note),
        };
        // ADR-0041 D1 / ADR-0043 D2 / D8: 作業ツリーを切る run には、リポジトリ一覧（ブランチ・base）、
        // 検査コマンド、成果物の置き場を足す。
        if conversation_addressee.is_none()
            && let Some(ws) = worktree
        {
            let mut line = task_worker::preamble::repos_note(&self.repo_notes(ws));
            // ADR-0066 D1（Phase 110b）: 共有ビルドキャッシュが有効で、かつ git のリポジトリが
            // 1 つでもあれば「`target/` は共有キャッシュにある」の 1 行を足す。
            if self.config.shared_build_cache && ws.repos.iter().any(|r| r.is_git()) {
                line.push_str(task_worker::preamble::shared_build_cache_note());
            }
            workspace_note = Some(match workspace_note {
                Some(note) => format!("{note}\n{line}"),
                None => line,
            });
        }
        // ADR-0043 D2: 計画 run には**案件のリポジトリの一覧**（名前 / 種類 / 説明）を渡す。
        // プランナーは子タスクごとに `repos: ["benchfs"]` と名前で指定する。
        if task.kind == TaskKind::Plan && conversation_addressee.is_none() {
            let listing =
                task_worker::preamble::project_repos_note(&self.project_repo_notes(task)?);
            if !listing.is_empty() {
                workspace_note = Some(match workspace_note {
                    Some(note) => format!("{note}\n{listing}"),
                    None => listing,
                });
            }
        }
        let events = self.store.events_for(task.id)?;
        // 集約 run（ADR-0016 D3）と、子の失敗によるやり直し run（ADR-0021 D1）は、子の結果を見て判断する。
        let children = if (task.aggregate && has_aggregate_transition(&events))
            || has_child_failed_transition(&events)
        {
            let mut out = Vec::new();
            for child in self.store.children(task.id)? {
                if child.kind == TaskKind::Approval {
                    continue;
                }
                let child_events = self.store.events_for(child.id)?;
                let outcome = child_events.iter().rev().find_map(|(_, e)| match e {
                    Event::WorkerFinished {
                        outcome,
                        role: None,
                        ..
                    } => Some(outcome.clone()),
                    _ => None,
                });
                let artifacts = child_events
                    .iter()
                    .filter_map(|(_, e)| match e {
                        Event::ArtifactProduced { artifact, .. } => Some(artifact.clone()),
                        _ => None,
                    })
                    .collect();
                // ADR-0041 D1: 子は親の作業場所を継ぐので、子ごとに別の worktree になる。親が成果を
                // 統合するときは子のブランチを merge する（統合は LLM の仕事）。
                let child_worktree = self.task_workspaces_for(&child);
                out.push(ChildSummary {
                    id: child.id,
                    title: child.title.clone(),
                    role: child.role.clone(),
                    status: child.status,
                    outcome,
                    artifacts,
                    workspace: child_worktree
                        .as_ref()
                        .map(|ws| ws.task_dir.clone())
                        .or_else(|| self.task_dir(&child)),
                    branch: child_worktree.and_then(|ws| {
                        ws.repos
                            .first()
                            .and_then(|r| r.branch().map(str::to_string))
                    }),
                });
            }
            out
        } else {
            Vec::new()
        };
        // ADR-0044 D2（Phase 53）: コメントの糸（最新 20 件、古い順）と、直前の run を止めた人のコメント。
        // どちらも決定的に引くだけ（LLM は関与しない）。
        let all_comments = self.store.comments_for(task.id)?;
        // ADR-0140 付記 comment-resume: resume を拒否された run は前置きを受け取っていないので、割り込みの
        // 消化に数えない（session が無くて checkpoint の fresh に倒れた run にもコメントを載せる）。
        let interrupt = task_ops::comment::interrupting_comment(
            &super::continuation_session::without_resume_rejected_finishes(&events),
            &all_comments,
        )
        .map(|c| c.body.clone());
        let comments: Vec<CommentContext> = all_comments
            .iter()
            .skip(
                all_comments
                    .len()
                    .saturating_sub(task_core::PREAMBLE_COMMENTS),
            )
            .map(CommentContext::from)
            .collect();
        // ADR-0047 D2（Phase 61）/ ADR-0046 D1（Phase 59 追記）: 実効マウント（担当ノードの実効
        // profile が継いだ知識 ＋ 設定の既定 ＋ 案件の `projects/<slug>`）と、その索引。
        // 決定的（`index.json` とファイルを読むだけ。LLM も判断も無い）。
        let profile_knowledge: Vec<task_core::KnowledgeMount> = assigned
            .map(|n| task_core::resolve_profile(&org, &n.id).knowledge)
            .unwrap_or_default();
        let knowledge =
            self.knowledge_context(task, assigned.map(|n| n.id.as_str()), &profile_knowledge);
        // ADR-0056 D3（Phase 79）: 担当ノードの実効 profile が継いだ skills mount を KB から解決する。
        // 決定的（ファイルを読むだけ。LLM は関与しない）。見つからない名前は run を落とさず、呼び出し元
        // （`dispatch_ready`）が `status` の進行イベントを 1 行出す。
        let profile_skills_mounts: Vec<String> = assigned
            .map(|n| task_core::resolve_profile(&org, &n.id).skills_mounts)
            .unwrap_or_default();
        let (skills, missing_skills) =
            self.skills_context(&profile_skills_mounts, task_ops::knowledge::SkillUse::Work);
        // ADR-0048 D3（Phase 60b）: CoS の対話 run にだけ、進行中の案件とその途中目標を渡す
        // （`actions` の `create_task.project` を選ぶ材料。決定的にストアを
        // 読むだけ。CoS 以外の run・継続中の run（ADR-0054 D1: 差分に「新しい案件」が乗る）では常に空）。
        let active_projects = if is_cos_conversation && !continuing {
            self.active_projects_context()?
        } else {
            Vec::new()
        };
        // ADR-0059 D6（Phase 99）/ Phase 99b 追記: CoS が `create_task.workspace` を組む材料として、
        // 既知のクラスタとその実効 work_dir を渡す（未登録なら `work_dir: null` なので、CoS は `path`
        // を省略すべきと分かる。D4 の指示文と対）。`recent_work` / `knowledge` / `profile` / `role` と
        // 同じく、継続中の run でも毎回渡す（1 回渡してもクラスタの登録・接続状態は変わりうる「いまの
        // 状態」であり、`active_projects` のような「前回からの差分」で足りるものとは性質が違う）。
        let clusters = if is_cos_conversation {
            self.cluster_context()
        } else {
            Vec::new()
        };
        Ok(RunExtras {
            role,
            children,
            available_genres,
            node,
            memory,
            conversation,
            standing_rules,
            organization,
            conversation_addressee,
            work_genre,
            recent_work,
            milestone_review,
            workspace_note,
            profile,
            mode,
            comments,
            interrupt,
            knowledge,
            active_projects,
            clusters,
            // ADR-0052 D2: フォールバックの判断は `dispatch_ready` がする（ここは run ごとの文脈だけ）。
            knowledge_fallback: None,
            session,
            session_diff,
            skills,
            missing_skills,
            // ADR-0072 D9/D21（Phase E2）: WU の run かどうかは `dispatch_ready` が判断し、
            // ここ（`run_extras`）の返り値を上書きする（ここでは常に `None`）。
            work_unit: None,
            continuation_override: None,
            // ADR-0140 D2・付記 session-container: WU と atomic task の worker run だけ `dispatch_ready` が
            // `resolve_continuation_session` で書く。
            continuation_session: None,
            // ADR-0072 D13/D14（Phase E3）: planner run かどうかも `dispatch_ready` が判断し、
            // ここの返り値を上書きする（ここでは常に `None`）。
            execution_planner: None,
            // ADR-0072 D14（Phase E4b 項目3）: 同上、`dispatch_ready` が planner run のときだけ
            // 上書きする（ここでは常に `None`）。
            planner_permission_mode: None,
            artifacts_dir_override: None,
            cargo_target_work_unit: None,
            // ADR-0079 D7（Phase R3a）: 木の節点の worker の run だけ `dispatch_ready` が上書きする。
            decision_requests: false,
            direct_route: None,
            // ADR-0074「R7-11」: 呼び出し元（`dispatch_ready_task`）が spawn の直前に実効の予算を入れる。
            budget: None,
        })
    }

    /// ADR-0059 D6（Phase 99）: `[[clusters]]` の id（決定的な順、昇順）と、接続状態・実効
    /// work_dir（DB の上書き `cluster_settings` > 設定ファイルの `work_dir`）。CoS の対話 run にだけ渡す。
    pub(super) fn cluster_context(&self) -> Vec<task_worker::ClusterContext> {
        let mut ids: Vec<&String> = self.config.clusters.keys().collect();
        ids.sort();
        ids.into_iter()
            .map(|id| {
                let spec = &self.config.clusters[id];
                let work_dir = self
                    .store
                    .cluster_settings_get(id)
                    .ok()
                    .flatten()
                    .and_then(|s| s.work_dir)
                    .or_else(|| {
                        spec.work_dir
                            .as_ref()
                            .map(|p| p.to_string_lossy().into_owned())
                    });
                task_worker::ClusterContext {
                    id: id.clone(),
                    connected: self.cluster_connected.get(id).copied().unwrap_or(false),
                    work_dir,
                }
            })
            .collect()
    }

    /// ADR-0054 Phase 67c: この run が CoS の対話 run になるなら、その現役セッション（あれば）を返す。
    /// `run_extras` の `is_cos_conversation` と同じ判定（対話 run で、担当が `OrgKind::Secretary`）を
    /// `select_provider` より前に行う（sticky 選択がランキングより先に効く必要があるため。ADR-0054 D1
    /// の CoS セッションは `node_id = "cos"` 固定）。対象でなければ `Ok(None)`。
    pub(super) fn cos_conversation_session(
        &self,
        task: &Task,
    ) -> Result<Option<NodeSession>, DispatchError> {
        if !task_core::is_conversation(task) {
            return Ok(None);
        }
        let Some(assignee) = task.assignee.as_deref() else {
            return Ok(None);
        };
        let org = self.store.org_list()?;
        let is_secretary = org
            .iter()
            .find(|n| n.id == assignee)
            .is_some_and(|n| n.kind == OrgKind::Secretary);
        if !is_secretary {
            return Ok(None);
        }
        Ok(self
            .store
            .node_session_active(task_core::COS_ID, SessionKind::Conversation, None)?)
    }

    /// ADR-0054 D1（Phase 67）: `(node_id, kind, project_id)` の継続セッションを決める・作る・引退させる
    /// （store の読み書き。判断そのものは `crate::sessions::decide`、純粋・テスト容易）。対応しない
    /// アダプタでは `(None, vec![])` を返す（このノード・kind は継続セッションを持たない）。
    pub(super) fn resolve_node_session(
        &self,
        node_id: &str,
        kind: SessionKind,
        project_id: Option<ProjectId>,
        adapter_id: &str,
        account: Option<&str>,
        now: OffsetDateTime,
    ) -> Result<(Option<task_worker::protocol::SessionHandle>, Vec<String>), DispatchError> {
        if !crate::sessions::adapter_supports_sessions(adapter_id) {
            return Ok((None, Vec::new()));
        }
        let active = self.store.node_session_active(node_id, kind, project_id)?;
        let action = crate::sessions::decide(
            active.as_ref(),
            adapter_id,
            account,
            self.config.session_rollover_tokens,
            // ADR-0054 D1: resume の失敗（アダプタがセッション不明/拒否を報告した）の明示検出は
            // 今回のスコープには含めない（PROGRESS の未解決事項）。供給側失敗として requeue されるだけ。
            false,
        );
        match action {
            crate::sessions::SessionAction::Resume => {
                let active = active.expect("SessionAction::Resume implies an active session");
                let diff = self.session_diff_since(node_id, active.last_used_at)?;
                Ok((
                    Some(task_worker::protocol::SessionHandle {
                        adapter: adapter_id.to_string(),
                        session_id: active.session_id.clone(),
                        resume: true,
                    }),
                    diff,
                ))
            }
            crate::sessions::SessionAction::Fresh(reason) => {
                if let Some(stale) = &active {
                    // ADR-0054 Phase 67b 追記: 本番の自己修復（P-67b-1）。`claude-code` の
                    // `session_id` が UUID でない行（Phase 67 が ULID を渡していた事故）を retire する
                    // ときは、次の障害調査のためにも必ずログへ残す。
                    if matches!(reason, crate::sessions::FreshReason::InvalidSessionId) {
                        tracing::warn!(
                            node_id,
                            kind = kind.as_str(),
                            adapter = adapter_id,
                            old_session_id = %stale.session_id,
                            "node_sessions.session_id はこのアダプタでは使えない形式（claude-code は UUID が必要）。\
                             retire して新しいセッションを作る（ADR-0054 Phase 67b）"
                        );
                    }
                    self.store
                        .node_session_retire(node_id, kind, project_id, now)?;
                }
                // claude-code は celeris が id を前もって決める（`--session-id`）。codex / acp は
                // アダプタが run の途中で初めて確定させるので、確定するまでは空文字（ADR-0054 D1）。
                // Phase 67b: id の発行は `crate::sessions::new_session_id`（純粋関数、テスト対象）に
                // 切り出した。Phase 67 まで使っていた `ulid::Ulid::new().to_string()`（ULID）は Claude
                // Code CLI 2.1.278 が `--session-id`/`--resume` に要求する UUID 形式ではなく、本番の
                // すべての CoS 対話・部門長レビュー run を壊した（2026-09-21 観測）。
                let session_id = crate::sessions::new_session_id(adapter_id);
                let new_session = NodeSession::new(
                    node_id,
                    kind,
                    project_id,
                    adapter_id,
                    account.map(str::to_string),
                    session_id.clone(),
                    now,
                );
                self.store.node_session_create(&new_session)?;
                let summary = if reason.needs_summary() {
                    self.session_summary(node_id)?
                } else {
                    Vec::new()
                };
                Ok((
                    Some(task_worker::protocol::SessionHandle {
                        adapter: adapter_id.to_string(),
                        session_id,
                        resume: false,
                    }),
                    summary,
                ))
            }
        }
    }

    /// ADR-0054 D1: 継続中セッションの前置きに出す差分（`since` より後に起きたこと。新しい人の発言・
    /// 終端タスクの要約・新しい案件。認可の結果は今回のスコープには含めない — PROGRESS の未解決事項）。
    pub(super) fn session_diff_since(
        &self,
        node_id: &str,
        since: OffsetDateTime,
    ) -> Result<Vec<String>, DispatchError> {
        // `project_id: None` = 絞らない（`message_page` の規約。ADR-0048 D1）。CoS のセッションは
        // どの案件についての発言も同じ 1 本のセッションに乗るため、案件を問わず読む。
        let messages = self.store.message_page(Some(node_id), None, None, 50)?;
        let new_messages: Vec<(OffsetDateTime, task_core::MessageRole, String)> = messages
            .into_iter()
            .map(|m| (m.created_at, m.role, m.text))
            .collect();
        let finished_tasks = self.finished_tasks_since(since)?;
        let new_projects = self.new_projects_since(since)?;
        Ok(crate::sessions::diff_lines(
            &new_messages,
            &finished_tasks,
            &[],
            &new_projects,
            since,
        ))
    }

    /// ADR-0054 D1: 新しいセッションを継ぐときの「これまでの要約」（ADR-0033 D4 の対話履歴の末尾
    /// `CONVERSATION_HISTORY` 件）。
    pub(super) fn session_summary(&self, node_id: &str) -> Result<Vec<String>, DispatchError> {
        let messages = self.store.message_page(
            Some(node_id),
            None,
            None,
            task_ops::conversation::CONVERSATION_HISTORY,
        )?;
        let history: Vec<(task_core::MessageRole, String)> =
            messages.into_iter().map(|m| (m.role, m.text)).collect();
        Ok(crate::sessions::summary_lines(
            &history,
            task_ops::conversation::CONVERSATION_HISTORY,
        ))
    }

    /// ADR-0054 D1: 前回の run 以降に終端になったタスク（支援タスクは除く）の 1 行要約。
    pub(super) fn finished_tasks_since(
        &self,
        since: OffsetDateTime,
    ) -> Result<Vec<(OffsetDateTime, String)>, DispatchError> {
        let filter = ListFilter {
            statuses: vec![Status::Done, Status::Failed, Status::Blocked],
            ..ListFilter::default()
        };
        let page = self
            .store
            .list_page(&filter, ListOrder::UpdatedDesc, None, RECENT_WORK_SCAN)?;
        let mut out = Vec::new();
        for task in page.items {
            if support_kind(&task).is_some() || task.updated_at <= since {
                continue;
            }
            let events = self.store.events_for(task.id)?;
            let outcome = recent_work_outcome(&task, &events).unwrap_or_default();
            out.push((task.updated_at, format!("{} — {outcome}", task.title)));
        }
        Ok(out)
    }

    /// ADR-0054 D1: `since` より後に作られた案件（`proposed`/`active`。人が新しく開いたもの）。
    pub(super) fn new_projects_since(
        &self,
        since: OffsetDateTime,
    ) -> Result<Vec<(OffsetDateTime, String)>, DispatchError> {
        let projects = self.store.project_list()?;
        Ok(projects
            .into_iter()
            .filter(|p| p.created_at > since)
            .map(|p| (p.created_at, p.title))
            .collect())
    }

    /// ADR-0048 D3（Phase 60b）: CoS の対話 run に渡す「進行中の案件」（`proposed` / `active` の案件だけ。
    /// 決定的にストアを読むだけ。LLM も判断も無い）。ADR-0079 D12 / D13（Phase R5a）: 途中目標は凍結したので
    /// CoS には渡さない（`milestones` は常に空。CoS は案件〈方向〉だけを選び、段階は `stages_hint` に書く）。
    pub(super) fn active_projects_context(
        &self,
    ) -> Result<Vec<ActiveProjectContext>, DispatchError> {
        let mut projects = self.store.project_list()?;
        projects.retain(|p| matches!(p.status, ProjectStatus::Proposed | ProjectStatus::Active));
        projects.sort_by_key(|p| p.id);
        let mut out = Vec::new();
        for project in projects.into_iter().take(ACTIVE_PROJECTS_SCAN) {
            out.push(ActiveProjectContext {
                repos: self
                    .store
                    .repo_list(project.id)?
                    .into_iter()
                    .map(|repo| repo.name)
                    .collect(),
                id: project.id.to_string(),
                title: project.title.clone(),
                status: project.status.as_str().to_string(),
                milestones: Vec::new(),
            });
        }
        Ok(out)
    }

    /// ADR-0047 D2（Phase 61）: この run が読める知識の**索引だけ**を組む（純粋に近い: 設定・DB・
    /// `index.json`・ファイル名を読むだけ。本文は入れない）。
    ///
    /// 実効マウント = `[knowledge] default_mounts` ＋ 案件の `projects/<slug>`（自動）
    /// ＋ タスクの明示（Phase 61 では無い。Phase 59 の実効 profile がここに合流する）。
    /// ADR-0046 D1（Phase 59 追記）: `profile_knowledge` は担当ノードの**実効 profile**が継いだ
    /// マウント（`task_core::resolve_profile(..).knowledge`）。組織の和 → 設定の既定 → 案件の順で
    /// `merge_mounts` に渡す（先に出てきたものが前置きの索引で先頭に来る）。
    pub(super) fn knowledge_context(
        &self,
        task: &Task,
        node_id: Option<&str>,
        profile_knowledge: &[task_core::KnowledgeMount],
    ) -> Option<task_worker::protocol::KnowledgeContext> {
        let root = &self.config.knowledge.root;
        // 案件は自動で `projects/<slug>` をマウントする（ADR-0047 D2）。
        let project = task
            .project_id
            .and_then(|id| self.store.project_get(id).ok().flatten());
        let mut project_mounts = Vec::new();
        if let Some(project) = &project {
            // Phase K-1: 案件の slug（`projects.slug`。ADR-0044 D7 追記）。
            let slug = project.kb_slug();
            project_mounts.push(task_core::KnowledgeMount::kb(format!("projects/{slug}")));
        }
        // ADR-0046 D1（Phase 59 追記）: 実効 profile ＋ 設定の既定 ＋ 案件の順で和を取る。
        let mounts = task_core::knowledge::merge_mounts(&[
            profile_knowledge,
            &self.config.knowledge.default_mounts,
            &project_mounts,
        ]);
        if mounts.is_empty() {
            return None;
        }
        let index = if task_ops::knowledge::exists(root) {
            task_ops::knowledge::ensure_index(root).items
        } else {
            Vec::new()
        };
        let mut items: Vec<task_core::KnowledgeItem> = Vec::new();
        for mount in &mounts {
            match mount.kind {
                task_core::MountKind::Kb => {
                    items.extend(
                        index
                            .iter()
                            .filter(|i| task_core::knowledge::mount_matches(mount, i))
                            .cloned(),
                    );
                }
                // `repo` は案件のリポジトリの文書の根のページ（ADR-0043 / ADR-0044 D7）。
                task_core::MountKind::Repo => {
                    if let Some(name) = mount.name.as_deref() {
                        items.extend(self.repo_doc_items(task, name, mount));
                    }
                }
                // `dir` は任意のローカルディレクトリの `*.md`（読み取り）。
                task_core::MountKind::Dir => {
                    if let Some(dir) = mount.path.as_deref() {
                        let label = mount.label();
                        items.extend(
                            task_ops::knowledge::list_pages(dir)
                                .into_iter()
                                .take(200)
                                .map(|rel| task_core::KnowledgeItem {
                                    title: rel.rsplit('/').next().unwrap_or(&rel).to_string(),
                                    path: dir.join(&rel).display().to_string(),
                                    scope: Some(label.clone()),
                                    ..task_core::KnowledgeItem::default()
                                }),
                        );
                    }
                }
                // `memory` はそのノードの手帳（ADR-0033 D3。中身は「覚えていること」の節に既に出ている）。
                task_core::MountKind::Memory => {
                    let node = mount.name.as_deref().or(node_id);
                    if let (Some(dir), Some(node)) = (&self.config.memory_dir, node) {
                        let notes = dir.join(node).join("notes.md");
                        if notes.exists() {
                            items.push(task_core::KnowledgeItem {
                                path: notes.display().to_string(),
                                title: "あなたの手帳（案件をまたぐ記憶）".to_string(),
                                scope: Some(format!("memory:{node}")),
                                ..task_core::KnowledgeItem::default()
                            });
                        }
                    }
                }
            }
        }
        Some(task_worker::protocol::KnowledgeContext {
            mounts,
            index: items,
        })
    }

    /// ADR-0056 D3（Phase 79）: `skills_mounts`（skill 名の一覧）を KB の `skills/<name>/` から解決する
    /// （`SKILL.md` があるかどうかを読むだけ。決定的、LLM は関与しない）。見つかったものは
    /// `RunContext.skills` に乗せる `SkillMount`、見つからなかった名前は 2 つ目の戻り値（呼び出し元が
    /// `status` の進行イベントを 1 行出し、run は落とさない）。
    pub(super) fn skills_context(
        &self,
        mounts: &[String],
        run: task_ops::knowledge::SkillUse,
    ) -> (Vec<task_worker::protocol::SkillMount>, Vec<String>) {
        let root = &self.config.knowledge.root;
        let mut skills = Vec::new();
        let mut missing = Vec::new();
        for name in mounts {
            match task_ops::knowledge::skills_get(root, name) {
                Some(detail) => {
                    if !task_ops::knowledge::skill_applies_to(&detail.skill_md, run) {
                        continue;
                    }
                    let description = task_ops::knowledge::skill_description(&detail.skill_md);
                    let path = root
                        .join(task_core::knowledge::SKILLS_DIR)
                        .join(name)
                        .display()
                        .to_string();
                    skills.push(task_worker::protocol::SkillMount {
                        name: name.clone(),
                        path,
                        description,
                    });
                }
                None => missing.push(name.clone()),
            }
        }
        (skills, missing)
    }

    /// `repo` マウント 1 件分（案件のその名前のリポジトリの文書の根のページ）。
    pub(super) fn repo_doc_items(
        &self,
        task: &Task,
        name: &str,
        mount: &task_core::KnowledgeMount,
    ) -> Vec<task_core::KnowledgeItem> {
        let Some(project_id) = task.project_id else {
            return Vec::new();
        };
        let Ok(repos) = self.store.repo_list(project_id) else {
            return Vec::new();
        };
        let Some(repo) = repos.iter().find(|r| r.name == name) else {
            return Vec::new();
        };
        let task_core::WorkspaceSpec::Local { path, .. } = &repo.location else {
            return Vec::new();
        };
        let (config, _) = task_core::workspace_config::load_or_default(path);
        let docs_root = mount.docs.clone().unwrap_or(config.outputs.docs);
        let branch = task_ops::changes::default_branch(path, repo.default_branch.as_deref());
        let label = mount.label();
        task_ops::docs::list(path, &branch, &docs_root)
            .into_iter()
            .take(200)
            .map(|rel| task_core::KnowledgeItem {
                title: rel.rsplit('/').next().unwrap_or(&rel).to_string(),
                path: rel,
                scope: Some(label.clone()),
                ..task_core::KnowledgeItem::default()
            })
            .collect()
    }

    /// Phase 41（ADR-0038 D1）: レビューの対話 run に渡す「その途中目標のここまで」。
    /// その途中目標に属する仕事（裏方は除く。作られた順に最大 20 件）の title / status / 終端の要約 /
    /// 主な成果物（`answer.md` / `report.md` の先頭 4,000 字）を集める。LLM は使わない（DESIGN 原則 1）。
    pub(super) fn milestone_review_of(
        &self,
        project_id: Option<ProjectId>,
        milestone_id: task_core::MilestoneId,
    ) -> Result<Option<MilestoneReviewContext>, DispatchError> {
        let Some(project_id) = project_id else {
            return Ok(None);
        };
        let Some(milestone) = self
            .store
            .milestone_list(project_id)?
            .into_iter()
            .find(|m| m.id == milestone_id)
        else {
            return Ok(None);
        };
        let filter = ListFilter {
            project_id: Some(project_id),
            ..ListFilter::default()
        };
        let page = self.store.list_page(
            &filter,
            ListOrder::CreatedDesc,
            None,
            MILESTONE_REVIEW_TASK_SCAN,
        )?;
        let mut subjects: Vec<Task> = page
            .items
            .into_iter()
            .filter(|t| t.milestone_id == Some(milestone_id) && support_kind(t).is_none())
            .collect();
        subjects.sort_by_key(|t| (t.created_at, t.id));
        let mut tasks = Vec::new();
        for task in subjects.into_iter().take(MILESTONE_REVIEW_TASK_LIMIT) {
            let events = self.store.events_for(task.id)?;
            tasks.push(MilestoneTaskResult {
                title: task.title.clone(),
                status: task.status,
                outcome: recent_work_outcome(&task, &events),
                artifacts_excerpt: self.milestone_artifacts_excerpt(&task),
            });
        }
        Ok(Some(MilestoneReviewContext {
            milestone: MilestoneBrief {
                id: milestone.id.to_string(),
                title: milestone.title.clone(),
                description: milestone.description.clone(),
                status: milestone.status.as_str().to_string(),
            },
            tasks,
        }))
    }

    /// Phase 41（ADR-0038 D1）: その仕事が残した `answer.md` / `report.md` の先頭 4,000 字
    /// （両方あれば名前を見出しに付けて繋ぎ、全体を 4,000 字で切る。読めなければ空文字）。
    pub(super) fn milestone_artifacts_excerpt(&self, task: &Task) -> String {
        let Some(workspace) = self.task_dir(task) else {
            return String::new();
        };
        let dir = self.artifacts_dir(task, &workspace);
        let mut out = String::new();
        for name in MILESTONE_REVIEW_ARTIFACTS {
            if let Ok(text) = std::fs::read_to_string(dir.join(name)) {
                out.push_str(&format!("#### {name}\n"));
                out.push_str(text.trim_end());
                out.push('\n');
            }
        }
        truncate_chars(&out, MILESTONE_REVIEW_EXCERPT_CHARS)
    }

    /// Phase 33（ADR-0033 D4 追記）: `node_id` の直近の仕事（対話・まとめ・承認・レビューは除く）を
    /// 更新の新しい順に最大 10 件集める。`current_project` があれば、その案件のものを先に、
    /// 残りは他の案件から（`current_project` の中でも他の案件のものでも、それぞれ更新の新しい順は保つ）。
    /// ストアの読み取りだけで、LLM は使わない（DESIGN 原則 1）。
    pub(super) fn recent_work_of(
        &self,
        node_id: &str,
        current_project: Option<ProjectId>,
    ) -> Result<Vec<RecentWork>, DispatchError> {
        let filter = ListFilter {
            assignee: Some(node_id.to_string()),
            ..ListFilter::default()
        };
        let page = self
            .store
            .list_page(&filter, ListOrder::UpdatedDesc, None, RECENT_WORK_SCAN)?;
        let candidates: Vec<Task> = page
            .items
            .into_iter()
            .filter(|t| support_kind(t).is_none())
            .collect();
        let (same_project, other_project): (Vec<Task>, Vec<Task>) = if current_project.is_some() {
            candidates
                .into_iter()
                .partition(|t| t.project_id == current_project)
        } else {
            (Vec::new(), candidates)
        };
        let mut project_titles: HashMap<ProjectId, Option<String>> = HashMap::new();
        let mut out = Vec::with_capacity(RECENT_WORK_LIMIT);
        for task in same_project
            .into_iter()
            .chain(other_project)
            .take(RECENT_WORK_LIMIT)
        {
            let events = self.store.events_for(task.id)?;
            let artifacts = artifact_names_of(&events);
            let outcome = recent_work_outcome(&task, &events);
            let finished_at =
                if matches!(task.status, Status::Done | Status::Failed | Status::Blocked) {
                    Some(rfc3339(task.updated_at))
                } else {
                    None
                };
            let project_title = match task.project_id {
                Some(pid) => project_titles
                    .entry(pid)
                    .or_insert_with(|| self.store.project_get(pid).ok().flatten().map(|p| p.title))
                    .clone(),
                None => None,
            };
            out.push(RecentWork {
                task_id: task.id,
                title: task.title.clone(),
                project_title,
                status: task.status,
                finished_at,
                outcome,
                artifacts,
            });
        }
        Ok(out)
    }
}
