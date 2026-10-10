//! worker 本体（free fn の run_worker と補助）。Dispatcher に依存しない。ADR-0082 の L1。

use super::*;

/// ADR-0043 D3 / D4: `[commands] setup` の 1 コマンドあたりの上限（設定キーにはしない。
/// `cargo fetch` / `pnpm install` が入る想定で、run の予算とは別に取る）。
const SETUP_TIMEOUT: Duration = Duration::from_secs(1800);

/// ADR 2026-10-10-local-disk-growth-paths D1: `CARGO_TARGET_DIR` を与えられなかった run の `WorkerProgress`
/// （`kind = status`）の接頭辞。repo 直下 target に落ちうる run を event で決定的に見分ける。
pub(super) const CARGO_TARGET_NOTE_PREFIX: &str = "cargo-target:";

/// `run_worker` が run ごとに adapter へ施す包み（ADR-0107 D1）。主 adapter と browser fallback
/// 候補の両方に同じ値を使う。
#[derive(Clone, Default)]
pub(super) struct RunAdapterPrep {
    /// ADR-0075 D4 / ADR-0129 (1): 与える env（`CARGO_TARGET_DIR` と `[scratch.cargo]`）。`remove` は常に空。
    /// `None` は `CARGO_TARGET_DIR` を与えない run（コンテナ・Remote・共有キャッシュ無効）。
    pub(super) env: Option<task_worker::scratch::CargoEnv>,
    /// ADR-0098 D6: 後続 task の宣言先。cargo の env の後、コンテナの前に適用する。
    pub(super) followups_env: Option<Vec<(String, String)>>,
    /// ADR-0074 付記 2026-10-05 D3: WU の run の `CELERIS_WU_BASE` / `CELERIS_WU_TARGET`（followups の後、コンテナの前）。
    pub(super) work_unit_env: Option<Vec<(String, String)>>,
    /// ADR 2026-10-07-build-tmp-hygiene D2.1: ローカルの host run の `TMPDIR`・`TMP`・`TEMP`
    /// （`runs/<run_id>/tmp`。WU の env の後、コンテナの前）。
    pub(super) run_tmp_env: Option<Vec<(String, String)>>,
    /// ADR-0043 D3: コンテナで走らせる run のプラン。
    pub(super) container: Option<task_worker::SharedPlan>,
    /// ADR-0072 D14: planner run の `[execution.planner].permission_mode`。
    pub(super) permission_mode: Option<String>,
}

/// ADR-0107（agent-docs/adr/0107-browser-fallback-candidate-preparation.md）D1: 主 adapter と browser
/// fallback 候補の run ごとの準備を 1 か所にまとめる。順序は ADR-0075 の env 設定 →
/// コンテナ（ADR-0043 D3）→ planner の permission mode（ADR-0072 D14）。tier ごとのモデルは
/// 各 adapter（`TieredAdapter`）が同じ `req.task.worker_hint.tier` から run 時に解決する。
/// 戻り値の `bool` は env（`CARGO_TARGET_DIR`）が実際に適用されたかどうか。
pub(super) fn prepare_run_adapter(
    adapter: Arc<dyn WorkerAdapter>,
    prep: &RunAdapterPrep,
    task_id: TaskId,
) -> (Arc<dyn WorkerAdapter>, bool) {
    let (adapter, env_applied) = match &prep.env {
        // ADR-0129 (1): 与える env を重ねるだけ。host から継いだ env（`RUSTC_WRAPPER` / `SCCACHE_*` を含む）は外さない。
        Some(env) => match adapter.with_env(&env.set) {
            Some(wrapped) => (wrapped, true),
            None => {
                tracing::debug!(task_id = %task_id, adapter = %adapter.id(), "adapter does not support with_env; CARGO_TARGET_DIR was not applied (ADR-0066 D1)");
                (adapter, false)
            }
        },
        None => (adapter, false),
    };
    let adapter = match &prep.followups_env {
        Some(env) => match adapter.with_env(env) {
            Some(wrapped) => wrapped,
            None => {
                tracing::debug!(task_id = %task_id, adapter = %adapter.id(), "adapter does not support with_env; CELERIS_FOLLOWUPS_FILE was not applied (ADR-0098 D6)");
                adapter
            }
        },
        None => adapter,
    };
    let adapter = match &prep.work_unit_env {
        Some(env) => match adapter.with_env(env) {
            Some(wrapped) => wrapped,
            None => {
                tracing::debug!(task_id = %task_id, adapter = %adapter.id(), "adapter does not support with_env; CELERIS_WU_BASE / CELERIS_WU_TARGET were not applied (ADR-0074 appendix 2026-10-05)");
                adapter
            }
        },
        None => adapter,
    };
    let adapter = match &prep.run_tmp_env {
        Some(env) => match adapter.with_env(env) {
            Some(wrapped) => wrapped,
            None => {
                tracing::debug!(task_id = %task_id, adapter = %adapter.id(), "adapter does not support with_env; the run TMPDIR was not applied (ADR 2026-10-07-build-tmp-hygiene D2)");
                adapter
            }
        },
        None => adapter,
    };
    // ADR-0043 D3（Phase 56）: コンテナで走らせる run は、ここでアダプタを包んだ複製に差し替える
    // （差し込み点はアダプタ側の `container::wrap` 1 か所）。この経路を持たないアダプタ
    // （`with_container` が `None`）はホストのまま走る。
    let adapter = match &prep.container {
        Some(plan) => match adapter.with_container(Arc::clone(plan)) {
            Some(wrapped) => wrapped,
            None => {
                tracing::warn!(task_id = %task_id, adapter = %adapter.id(), "adapter does not support containers; running on the host");
                adapter
            }
        },
        None => adapter,
    };
    // ADR-0072 D14（Phase E4b 項目3）: planner run（`permission_mode` が `Some`）は
    // `[execution.planner].permission_mode` を実際の CLI 引数として反映する。対応しないアダプタ
    // （`with_permission_mode` が `None` を返す）はアダプタ既定の permission-mode のまま走る
    // （E3 実装時の既定の動作と同じ。実害は無い）。
    let adapter = match &prep.permission_mode {
        Some(mode) if !mode.is_empty() => match adapter.with_permission_mode(mode) {
            Some(wrapped) => wrapped,
            None => {
                tracing::debug!(task_id = %task_id, adapter = %adapter.id(), %mode, "adapter does not support with_permission_mode; planner permission_mode was not applied (ADR-0072 D14)");
                adapter
            }
        },
        _ => adapter,
    };
    (adapter, env_applied)
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_worker(
    store: Arc<dyn TaskStore>,
    adapter: Arc<dyn WorkerAdapter>,
    browser_candidates: Vec<Arc<dyn WorkerAdapter>>,
    task_id: TaskId,
    execution_tier: task_core::Tier,
    dir: PathBuf,
    run_id: &str,
    limits: RunLimits,
    lease: LeaseRenewal,
    remote: Option<SshSettings>,
    worktree: Option<task_worker::TaskWorkspaces>,
    extras: RunExtras,
    roles: Vec<RoleSpec>,
    genres: Vec<GenreSpec>,
    delegation: DelegationLimits,
    account: Option<String>,
    account_book: Option<Arc<StdMutex<AccountBook>>>,
    container: ContainerDecision,
    // ADR-0066 D1（Phase 110b）/ ADR-0075 D3（Phase G1）: ローカルの git worktree のホスト実行に限って
    // `CARGO_TARGET_DIR` を与える（コンテナ・Remote は対象外。`container_plan` が決まってから判断する）。
    // scratch なら `<scratch>/targets/<owner>/target`、無効なら `<build_cache_dir>/cargo/<repo-key>[/wu-<id>]`。
    cargo_target: CargoTargetPlan,
) -> Result<RunOutcome, AdapterError> {
    // リース取得後の状態（running, lease あり）をワーカーに渡す。
    let mut task = store
        .get(task_id)
        .map_err(|e| AdapterError::Other(format!("store: {e}")))?
        .ok_or_else(|| AdapterError::Other("task vanished".into()))?;
    task.worker_hint.tier = execution_tier;
    // ADR-0072 D14（Phase E4b 項目3）: `extras` は後段で複数のフィールドが個別に消費されるので、
    // 使う値だけ先に取り出しておく。
    let planner_permission_mode = extras.planner_permission_mode.clone();
    // ADR-0140 D2: WU の継続 session の key（sink が resume 拒否で retire する）。
    let continuation_key = extras.continuation_session.clone();
    // ADR-0074「R7-11 実装時の明確化」: dispatcher が決めたこの run の実効の予算（planner の `[execution.planner]`、
    // WU の D18 など）をワーカーに渡す写しに戻す（DB の task は変えない）。
    if let Some(budget) = extras.budget {
        task.budget = budget;
    }
    // ADR-0052 D2（Phase 64）: フォールバックした知識整理 run は、DB のタスクではなく**ワーカーに渡す
    // 写し**だけを書き換える（専用アダプタの固定を外し、予算を `max_turns = 8` / `max_wall_secs = 600` に）。
    if let Some(fallback) = &extras.knowledge_fallback {
        task.worker_hint.adapter = None;
        task.budget = fallback.budget;
        tracing::debug!(task_id = %task_id, adapter = %fallback.adapter, "knowledge: running the fallback extraction");
    }
    // ADR-0018 D1/D3: リモート実行のタスクは、クラスタの内容を写しに取り込み、ラッパを置き、その使い方を指示文に足す
    // （DB のタスクは変えない。ワーカーに渡す写しだけ）。ADR-0059 D6: `path` が実効 `work_dir` から解決
    // できなかった（`cluster_of` が絶対・`~` 始まりに直せなかった）ら、worktree 準備を試す前にここで
    // 明確なエラーにする。
    // ADR-0079 R5b-fix2: remote なら run の後に push するため、用意した `SshWorkspace` を控える。
    let mut remote_ws: Option<SshWorkspace> = None;
    let workspace = match &remote {
        Some(settings) => {
            if !remote_dir_is_resolved(&settings.remote_dir) {
                return Err(AdapterError::Other(format!(
                    "cluster {}: no working directory registered for {:?}; register one via \
                     `PUT /clusters/{}/settings` or the clusters screen (ADR-0059 D6)",
                    settings.cluster, settings.remote_dir, settings.cluster
                )));
            }
            let mut effective_settings = settings.clone();
            let mut ws = SshWorkspace::new(&dir, effective_settings.clone());
            let prepared = match ws.prepare(&task).await {
                Ok(p) => p,
                // ADR-0059 D3: 自動の格下げ。`mode` 省略・`repos` 無し（コードを触らない仕事）なら、
                // worktree が切れなくても失敗にせず `shared`（`SyncMode::None`）として同じ run の中で
                // 続行する。明示的に `mode = "worktree"` を選んだタスクは格下げしない（利用者の意図を
                // 尊重し、設定ミスを隠さない）。
                Err(WorkspaceError::NotAGitRepository(reason))
                    if remote_mode_omitted(&task.workspace) && task.repos.is_empty() =>
                {
                    effective_settings.sync = SyncMode::None;
                    ws = SshWorkspace::new(&dir, effective_settings.clone());
                    let downgraded = ws.prepare(&task).await.map_err(|e| {
                        workspace_error_to_adapter(e, "workspace prepare (downgraded to shared)")
                    })?;
                    if let Some(new_workspace) = downgraded_remote_workspace(&task.workspace) {
                        if let Ok(Some(mut fresh)) = store.get(task_id) {
                            fresh.workspace = new_workspace.clone();
                            fresh.updated_at = OffsetDateTime::now_utc();
                            if let Err(e) = store.update_task(
                                &fresh,
                                Event::WorkspaceModeDowngraded {
                                    cluster: effective_settings.cluster.clone(),
                                    path: effective_settings
                                        .remote_dir
                                        .to_string_lossy()
                                        .into_owned(),
                                    reason,
                                },
                            ) {
                                tracing::warn!(task_id = %task_id, error = %e, "could not persist the workspace mode downgrade (ADR-0059 D3)");
                            }
                        }
                        task.workspace = new_workspace;
                    }
                    downgraded
                }
                Err(e) => return Err(workspace_error_to_adapter(e, "workspace prepare")),
            };
            ws.write_remote_exec_helper()
                .await
                .map_err(|e| workspace_error_to_adapter(e, "remote-exec helper"))?;
            task.objective
                .push_str(&remote_exec_instructions(&effective_settings));
            remote_ws = Some(ws);
            prepared
        }
        // ADR-0041 D1 / ADR-0043 D2: ローカルの作業場所（1 つ以上のリポジトリ）を用意し、その
        // 先頭をワーカーのカレントディレクトリにする（`runs/` `inputs/` `artifacts/` は作業ツリーの外の `dir`）。
        None => {
            let mut ws = LocalWorkspace::new(&dir);
            if let Some(wt) = &worktree {
                wt.ensure()
                    .await
                    .map_err(|e| workspace_error_to_adapter(e, "worktree prepare"))?;
                // 目印（`<task_dir>/worktree.json`）: API・CLI はこれを見て「run のログと成果物は
                // 作業ツリーの外にある」と判断する（git を起こさない。worktree を消した後も残す）。
                if let Err(e) =
                    task_ops::workspace::write_marker(&wt.task_dir, &worktree_marker(wt))
                {
                    tracing::warn!(task_id = %task_id, error = %e, "could not write the worktree marker");
                }
                if let Some(cwd) = wt.cwd() {
                    ws = ws.with_work_dir(cwd);
                }
            }
            ws.prepare(&task)
                .await
                .map_err(|e| AdapterError::Other(format!("workspace prepare: {e}")))?
        }
    };
    // ADR-0043 D3（Phase 56）: この run の実行環境。コンテナなら**イメージをここで用意する**
    // （`[container] image` はそのまま、`dockerfile` は内容の sha のタグでビルドしてキャッシュ）。
    // runtime が無い・ビルドが落ちたときは run を始めず、`setup` の失敗と同じ経路で人に聞く。
    let container_plan: Option<task_worker::SharedPlan> = match container {
        ContainerDecision::Host => None,
        ContainerDecision::Unavailable { question } => {
            tracing::warn!(task_id = %task_id, %question, "no container runtime; asking a human");
            return Ok(RunOutcome {
                terminal: Terminal::Question { text: question },
                exit_code: None,
            });
        }
        ContainerDecision::Container(run) => {
            let mut run = *run;
            let log_path = dir.join(task_worker::container::BUILD_LOG);
            let resolved = match task_worker::container::resolve_image(
                &run.image,
                &run.image_default,
                &run.build_root,
            ) {
                Ok(resolved) => resolved,
                Err(e) => {
                    tracing::warn!(task_id = %task_id, error = %e, "cannot resolve the container image");
                    return Ok(RunOutcome {
                        terminal: Terminal::Question {
                            text: task_worker::container::image_question(&run.repo, &e, &log_path),
                        },
                        exit_code: None,
                    });
                }
            };
            match resolved {
                task_worker::container::ResolvedImage::Ready(tag) => run.plan.image = tag,
                task_worker::container::ResolvedImage::Build(request) => {
                    let program = run.plan.program.clone();
                    let exists =
                        move |tag: &str| task_worker::container::image_exists(&program, tag);
                    if let Err(e) = task_worker::container::ensure_image(
                        &run.plan.program,
                        &request,
                        run.build_timeout,
                        &log_path,
                        &exists,
                    )
                    .await
                    {
                        tracing::warn!(task_id = %task_id, error = %e, "container image build failed");
                        return Ok(RunOutcome {
                            terminal: Terminal::Question {
                                text: task_worker::container::image_question(
                                    &run.repo, &e, &log_path,
                                ),
                            },
                            exit_code: None,
                        });
                    }
                    run.plan.image = request.tag;
                }
            }
            tracing::info!(task_id = %task_id, image = %run.plan.image, runtime = run.plan.runtime.as_str(), repo = %run.repo, "running in a container");
            Some(Arc::new(run.plan))
        }
    };
    // ADR-0043 D3 / D4: worktree を作った直後に `[commands] setup` を**一度だけ**流す
    // （記録は `runs/setup.log`。そのファイルがあれば済んでいる）。コンテナのタスクは
    // **コンテナの中で**流す（Phase 56）。落ちたら run を始めず、既存の質問の経路で
    // タスクを `blocked` にして人に聞く。
    if let Some(wt) = &worktree
        && remote.is_none()
        && !wt
            .task_dir
            .join(task_worker::task_repos::SETUP_LOG)
            .exists()
    {
        match task_worker::run_setup_in(
            &wt.repos,
            &wt.task_dir,
            SETUP_TIMEOUT,
            container_plan.as_ref(),
        )
        .await
        {
            Ok(outcome) if outcome.ok => {}
            Ok(outcome) => {
                tracing::warn!(task_id = %task_id, failures = ?outcome.failures, "setup failed; asking a human");
                return Ok(RunOutcome {
                    terminal: Terminal::Question {
                        text: format!(
                            "setup が失敗しました（`.config/celeris/workspace.toml` の `[commands] setup`）: {}。\
                             記録は `{}` にあります。直し方を教えてください（設定を直す／この手順を飛ばす）。",
                            outcome.failures.join(" / "),
                            wt.task_dir
                                .join(task_worker::task_repos::SETUP_LOG)
                                .display()
                        ),
                    },
                    exit_code: None,
                });
            }
            Err(e) => {
                tracing::warn!(task_id = %task_id, error = %e, "could not run setup");
                return Ok(RunOutcome {
                    terminal: Terminal::Question {
                        text: format!(
                            "setup が失敗しました（`.config/celeris/workspace.toml` の `[commands] setup` を流せませんでした）: {e}。\
                             直し方を教えてください（設定を直す／この手順を飛ばす）。"
                        ),
                    },
                    exit_code: None,
                });
            }
        }
    }
    // ADR-0041 D1 / ADR-0043 D2: ワーカーの cwd は先頭のリポジトリ（`workspace` は足回りの親のまま）。
    let work_dir = worktree
        .as_ref()
        .and_then(|wt| wt.cwd())
        .map(|cwd| cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf()));
    // ADR-0036 D1: 成果物ディレクトリはタスクごと（共有 workspace では `.taskd/artifacts/<task_id>/`）。
    // 決めるのはディスパッチャで、アダプタは `req.artifacts_dir` に書くだけ。
    // ADR 2026-10-05 cos-chat-home D4: task に pin された添付を作業ツリーの外へ照合つきで stage する。
    // 読めない store は run を止めず警告だけ（添付が無い run と同じに扱い、読めたとは装わない）。
    let input_attachments = match &extras.input_attachment_source {
        Some(source) => {
            let base = super::input_attachments::stage_base(
                &dir,
                extras.artifacts_dir_override.as_deref(),
            );
            match super::input_attachments::stage_task_input_attachments(
                source,
                &task_id.to_string(),
                &base,
                remote.is_some(),
            ) {
                Ok(manifest) => manifest,
                Err(e) => {
                    tracing::warn!(task_id = %task_id, error = %e, "could not stage pinned task attachments (ADR cos-chat-home D4)");
                    Vec::new()
                }
            }
        }
        None => Vec::new(),
    };
    let artifacts_dir = match &extras.artifacts_dir_override {
        // ADR-0074 D1.2（Phase F2b）: v2 の WU の run は WU ごとの成果物の置き場を使う。
        Some(dir) => {
            let _ = tokio::fs::create_dir_all(dir).await;
            dir.clone()
        }
        None => task_core::artifacts::artifacts_dir_for(&task, &workspace),
    };
    // ADR-0067 D3 / 付記 2026-10-07: run 後に未申告の成果物を拾うための控え（`workspace`/`artifacts_dir` は
    // この後 `req` に移る）。走査の形（全体 `*.md` か `artifacts_dir` の中だけか）は後段で `remote` / `worktree`
    // を見て決める。
    let workspace_for_undeclared_scan = workspace.clone();
    let artifacts_dir_for_undeclared_scan = artifacts_dir.clone();
    if task.kind == TaskKind::Plan {
        // ADR-0007 D1: 前回の run の plan.json を今回の出力と誤読しない。
        let _ = tokio::fs::remove_file(artifacts_dir.join(PLAN_FILE_NAME)).await;
    }
    // ADR-0098 D1/D6: 後続の宣言（`followups.json`）は worker の run だけ（対話 run は `actions`、planner は計画）。
    let followups_enabled =
        extras.conversation_addressee.is_none() && planner_permission_mode.is_none();
    let artifacts_dir_for_followups = artifacts_dir.clone();
    if followups_enabled {
        followups::clear_stale_followups(&artifacts_dir).await;
    }
    let events = store
        .events_for(task_id)
        .map_err(|e| AdapterError::Other(format!("store: {e}")))?;
    let prior_review = to_prior_review(prior_review_from_events(&events));
    // ADR-0044 D2: 対話 run（人への返事だけをする run。Phase 28）にはコメントの書き方を出さない。
    let writes_comments = extras.conversation_addressee.is_none();
    let cargo_target_work_unit = extras.cargo_target_work_unit.clone();
    let cargo_target_fallback = extras.cargo_target_fallback.clone();
    let work_unit_env = extras.work_unit_env.clone();
    let mut req = RunRequest {
        cargo_target_dir: None,
        protocol: PROTOCOL_VERSION,
        task: task.clone(),
        workspace,
        work_dir,
        artifacts_dir,
        context: RunContext {
            browser: None,
            browser_policy: browser_run_policy(store.as_ref(), &task)?,
            prior_review,
            inputs: task.inputs.clone(),
            answers: to_answers(answers_from_events(&events)),
            review: None,
            role: extras.role,
            children: extras.children,
            available_genres: extras.available_genres,
            node: extras.node,
            memory: extras.memory,
            conversation: extras.conversation,
            // ADR-0033 D5（Phase 26）: 担当宛て + 全員向けの永続の認可。
            standing_rules: extras.standing_rules,
            organization: extras.organization,
            // ADR-0059 D6（Phase 99）: CoS の対話 run だけに入る。
            clusters: extras.clusters,
            conversation_addressee: extras.conversation_addressee,
            work_genre: extras.work_genre,
            recent_work: extras.recent_work,
            // Phase 41（ADR-0038 D1）: 途中目標レビューの対話 run だけに入る。
            milestone_review: extras.milestone_review,
            // Phase 43（ADR-0039 D3）: 案件が作業場所を決めている run だけに入る。
            workspace_note: extras.workspace_note,
            knowledge: extras.knowledge,
            // ADR-0044 D2（Phase 53）: コメントの糸と、直前の run を止めた人のコメント。
            comments: extras.comments,
            interrupt: extras.interrupt,
            // ADR-0046 D1 / D4（Phase 59）: 実効 profile と進め方（どちらも既定なら `None`）。
            profile: extras.profile,
            mode: extras.mode,
            // 仕事の run はコメントを書ける。**対話 run は書かせない**（Phase 28 の「返事だけをする」と
            // ぶつかる）。レビュー run は `review.rs` が `RunContext::default()` を使うので既定の false。
            comments_enabled: writes_comments,
            // Phase 38（ADR-0028 追記）: レビュー run（`review.rs` が組む）だけに入る。
            subject_genre: None,
            // ADR-0048 D3（Phase 60b）: CoS の対話 run だけに入る。
            active_projects: extras.active_projects,
            // ADR-0054 D1（Phase 67）: 継続セッション（CoS の対話・部門長のレビュー run だけ）。
            session: extras.session,
            session_diff: extras.session_diff,
            // ADR-0056 D3（Phase 79）: mount された skills（KB に実在したものだけ）。
            skills: extras.skills,
            // ADR-0072 D9（Phase E1/E2）: 予算切れ・yield の続きなら、前の run の checkpoint と
            // これまでの run の 1 行要約。continuation でない run では `None`
            // （プロンプトは D10 の追加分を除きバイト単位で従来どおり）。WU の run では
            // `dispatch_ready` が `runs` 索引から組み立てた文脈（`continuation_override`）を使う
            // （events は WU をまたぐ run_id の区別を持たないため）。
            continuation: extras
                .continuation_override
                .clone()
                .or_else(|| build_continuation_context(&events)),
            // ADR-0072 D9/D21（Phase E2）: 計画のある Task の WU の run にだけ `Some`。
            work_unit: extras.work_unit.clone(),
            // ADR-0072 D13/D14（Phase E3）: task-local な planner run にだけ `Some`。
            execution_planner: extras.execution_planner.clone(),
            decision_requests: extras.decision_requests,
            // ADR-0124 D4: 直行経路の implementation run にだけ `Some`。
            direct_route: extras.direct_route.clone(),
            // ADR 2026-10-06 cos-chat-run-dispatch: task の run は CoS chat run ではない。
            cos_chat: None,
            // 多目的 routing Phase 3: registry を差し込んだ dispatcher の worker/planner run だけ `Some`。
            routing_context_ref: extras.routing_context_ref.clone(),
            // ADR 2026-10-05 cos-chat-home D4: task に pin された添付の入力 manifest。
            input_attachments,
        },
    };
    // ADR-0066 D1（Phase 110b）: ローカルの git worktree のホスト実行にだけ、共有ビルドキャッシュの
    // `CARGO_TARGET_DIR` を与える（コンテナ実行〈`container_plan.is_some()`〉と Remote は対象外）。
    // ADR 2026-10-10-local-disk-growth-paths D1: 先頭が git でなければ dispatcher が決めた代わりの checkout
    // （`repos=[]` の子 task なら祖先の checkout）。どちらも無く `Missing` なら理由を event に残す。
    let local_host = container_plan.is_none() && remote.is_none();
    let repo_for_target = worktree
        .as_ref()
        .and_then(|wt| wt.repos.first())
        .filter(|r| r.is_git() && local_host)
        .cloned()
        .or_else(|| {
            cargo_target_fallback
                .as_ref()
                .and_then(|f| f.repo())
                .filter(|_| local_host)
                .cloned()
        });
    let mut cargo_target_notes: Vec<String> = Vec::new();
    if local_host && !matches!(cargo_target, CargoTargetPlan::None) {
        match &cargo_target_fallback {
            Some(super::workspaces::CargoTargetFallback::Missing { reason }) => {
                tracing::warn!(task_id = %task_id, %reason, "cargo target: CARGO_TARGET_DIR not set (ADR 2026-10-10-local-disk-growth-paths D1)");
                cargo_target_notes.push(format!(
                    "{CARGO_TARGET_NOTE_PREFIX} CARGO_TARGET_DIR not set: {reason}"
                ));
            }
            Some(super::workspaces::CargoTargetFallback::Ancestor { ancestor, repo }) => {
                tracing::info!(task_id = %task_id, %ancestor, checkout = %repo.dir.display(), "cargo target: using the ancestor's checkout (ADR 2026-10-10-local-disk-growth-paths D1)");
            }
            _ => {}
        }
    }
    // ADR-0075 D4 / ADR-0129 (1): scratch なら env は `CARGO_TARGET_DIR` と `[scratch.cargo]`（checks と同じ組み方）。
    // legacy は `CARGO_TARGET_DIR` だけ。sccache 系は Celeris が足しも外しもしない（host の cargo 設定に任せる）。
    let target: Option<(PathBuf, task_worker::scratch::CargoEnv)> = match (
        &cargo_target,
        repo_for_target,
    ) {
        (CargoTargetPlan::None, _) | (_, None) => None,
        // ADR-0075 D3: owner は Task 単位の run なら `task-<id>`、自分の worktree の WU なら `task-<id>/wu-<id>`。
        (
            CargoTargetPlan::Scratch {
                settings,
                candidates,
            },
            Some(repo),
        ) => {
            let owner = match &cargo_target_work_unit {
                Some((id, _)) => task_worker::scratch::Owner::work_unit(task_id.to_string(), id),
                None => task_worker::scratch::Owner::task(task_id.to_string()),
            };
            let (settings, candidates) = (settings.clone(), candidates.clone());
            let key = cargo_target_work_unit.as_ref().map(|(_, k)| k.clone());
            let fallback = (
                settings.pool().target_dir(&owner),
                scratch_cargo_env(&settings, &owner),
            );
            match tokio::task::spawn_blocking(move || {
                let target = allocate_scratch_target(&settings, &candidates, &owner, &repo, key);
                // adopt は owner のパスへ rename するので、`target` は env の `CARGO_TARGET_DIR` と同じ。
                let env = scratch_cargo_env(&settings, &owner);
                (target, env)
            })
            .await
            {
                Ok(t) => Some(t),
                Err(e) => {
                    tracing::warn!(task_id = %task_id, error = %e, "scratch: allocation task failed; using the target path");
                    Some(fallback)
                }
            }
        }
        // ADR-0074 F5-fix（不具合 1）: 並列の WU は `<repo-key>/wu-<id>`（兄弟 WU の別ブランチの
        // 生成物を混ぜない）。Task 単位の run は従来どおり `<repo-key>`。
        (CargoTargetPlan::Legacy(build_cache_dir), Some(repo)) => {
            let dir = match &cargo_target_work_unit {
                Some((id, _)) => task_worker::build_cache::work_unit_cargo_target_dir(
                    build_cache_dir,
                    &repo.source,
                    id,
                ),
                None => task_worker::build_cache::cargo_target_dir(build_cache_dir, &repo.source),
            };
            let env = task_worker::scratch::CargoEnv::set_only(vec![(
                task_worker::build_cache::CARGO_TARGET_DIR_VAR.to_string(),
                dir.display().to_string(),
            )]);
            Some((dir, env))
        }
    };
    // ADR-0098 D6: `celerisctl add` が run の中では後続の宣言になるよう、書き先を env で渡す
    // （コンテナは同じパスで mount されるので、container の包みより前に重ねる）。
    let followups_env = followups_enabled.then(|| {
        let run_db = task_worker::db_guard::installed().map(|g| g.db_path().to_path_buf());
        followups::followups_env(
            task_id,
            run_id,
            &artifacts_dir_for_followups,
            run_db.as_deref(),
        )
    });
    // ADR 2026-10-07-build-tmp-hygiene D2.1: ローカルの host run（Remote・コンテナは範囲外）は run 固有の
    // `runs/<run_id>/tmp` を作って `TMPDIR`・`TMP`・`TEMP` を向ける。D2.2: この関数を抜ける全経路で消す
    // （正常な終端は下の `remove`、`?` の早期 return と future ごとの中断は `Drop`）。作れなければ警告だけで
    // run は従来の環境のまま走らせる。
    let run_tmp = if remote.is_none() && container_plan.is_none() {
        match task_worker::run_tmpdir::RunTmpDir::create(&req.workspace, run_id) {
            Ok(tmp) => Some(tmp),
            Err(e) => {
                tracing::warn!(task_id = %task_id, run_id, error = %e, "run tmpdir: could not create; TMPDIR is left unchanged (ADR 2026-10-07-build-tmp-hygiene D2)");
                None
            }
        }
    } else {
        None
    };
    let prep = RunAdapterPrep {
        env: target.as_ref().map(|(_, env)| env.clone()),
        followups_env,
        work_unit_env,
        run_tmp_env: run_tmp.as_ref().map(|tmp| tmp.env()),
        container: container_plan.clone(),
        permission_mode: planner_permission_mode.clone(),
    };
    let (adapter, env_applied) = prepare_run_adapter(adapter, &prep, task_id);
    // ADR 2026-10-07-build-tmp-hygiene 付記 A1（2026-10-09）: Rust の repo（`Cargo.toml`）で env を重ねられなければ
    // cargo は作業場所に `target/` を作る。黙らせず警告する。
    if !env_applied
        && prep.env.is_some()
        && (worktree
            .as_ref()
            .and_then(|wt| wt.repos.first())
            .is_some_and(|r| r.dir.join("Cargo.toml").is_file())
            || cargo_target_fallback
                .as_ref()
                .is_some_and(|f| f.repo().is_some()))
    {
        tracing::warn!(task_id = %task_id, adapter = %adapter.id(), "adapter does not support with_env; CARGO_TARGET_DIR was not applied to a Rust repository (ADR 2026-10-07-build-tmp-hygiene A1)");
        cargo_target_notes.push(format!(
            "{CARGO_TARGET_NOTE_PREFIX} CARGO_TARGET_DIR not set: adapter {} does not support with_env",
            adapter.id()
        ));
    }
    if env_applied {
        req.cargo_target_dir = target.map(|(dir, _)| dir);
    }
    let browser_candidates: Vec<Arc<dyn WorkerAdapter>> = browser_candidates
        .into_iter()
        .map(|candidate| prepare_run_adapter(candidate, &prep, task_id).0)
        .collect();
    // ADR-0054 D1（Phase 67）: `run_worker` を通る run で継続セッションを持てるのは CoS の対話 run
    // だけ（部門長のレビュー run は `review.rs` の別経路。`run_extras` の `is_cos_conversation` と同じ
    // 判定で `extras.session` が埋まるので、ここでは `req.context.session` の有無だけを見ればよい）。
    // ADR-0140 D2: WU の継続 session（`continuation_key`）を持つ run は CoS の key を持たない。
    let session_key = (req.context.session.is_some() && continuation_key.is_none()).then(|| {
        (
            task_core::COS_ID.to_string(),
            task_core::SessionKind::Conversation,
            None,
        )
    });
    // ADR-0067 D3: `store` は `sink` に move されるので、後段の未申告成果物の登録用に控えておく。
    let store_for_undeclared_scan = Arc::clone(&store);
    // ADR-0098 D3: 後続は daemon の `[[roles]]` / `[[genres]]` で解決する（`roles` / `genres` は `sink` に move される）。
    let roles_for_followups = roles.clone();
    let genres_for_followups = genres.clone();
    let sink = StoreSink {
        auto_leaf: extras.auto_leaf,
        store,
        task_id,
        run_id: run_id.to_string(),
        lease_ttl: lease.ttl,
        renew_every: lease.every,
        last_renew: std::sync::Mutex::new(Instant::now()),
        roles,
        genres,
        delegation,
        delegated_this_run: std::sync::atomic::AtomicUsize::new(0),
        account,
        account_book,
        session_key,
        continuation_key,
    };
    // ADR-0079 付記「R6-1」D6: remote の準備の進行（submodule の初期化など）を run の進行に残す（sink はここで
    // できるので、準備の直後ではなく run の前に書く）。
    if let Some(ws) = &remote_ws {
        drain_remote_progress_notes(ws.take_progress_notes(), &sink);
    }
    // ADR 2026-10-10-local-disk-growth-paths D1: `CARGO_TARGET_DIR` を与えられなかった理由を run の event に残す。
    for note in &cargo_target_notes {
        sink.progress_with(
            note,
            &task_core::ProgressFields::of(task_core::ProgressKind::Status),
        );
    }
    let outcome = if task_core::browser::requests_browser(&req.task.skills)
        && (remote.is_some() || container_plan.is_some())
    {
        Err(AdapterError::Other(
            "browser capability currently requires a local host run".into(),
        ))
    } else if let Err(e) = task_worker::browser_policy::admit(&req) {
        // D2.0: task ∩ the grant read for this run is empty (or invalid): never start the run.
        Err(AdapterError::Other(format!(
            "browser policy rejected: {}",
            e.code()
        )))
    } else {
        let run = task_worker::browser::run_with_candidates(
            adapter,
            browser_candidates,
            req,
            run_id,
            limits,
            &sink,
        );
        if let Some(watch) = &sink.auto_leaf {
            let stopped = || {
                Ok(RunOutcome {
                    terminal: Terminal::Yielded {
                        checkpoint: serde_json::Value::Null,
                        usage: None,
                    },
                    exit_code: None,
                })
            };
            // Keep the future (and its process-group registration) alive until the
            // shared stop path signals descendants and containers. Drop alone only
            // kills the direct child. Finish then saves a mechanical checkpoint.
            tokio::pin!(run);
            let outcome = if watch.exceeded() {
                stopped()
            } else {
                tokio::select! {
                    biased;
                    _ = watch.wake.notified() => {
                        let container_stop = container_plan.as_ref().map(|p|
                            Arc::new(task_worker::ContainerStop::of(p)) as Arc<dyn task_worker::ContainerStopper>);
                        if task_worker::kill_tree_with(run_id, limits.kill_grace, container_stop) {
                            tokio::time::sleep(limits.kill_grace).await;
                        }
                        stopped()
                    },
                    outcome = &mut run => outcome,
                }
            };
            if watch.exceeded() { stopped() } else { outcome }
        } else {
            run.await
        }
    };
    // ADR-0079 R5b-fix2: remote workspace は run が終わるたびに（成否に関わらず）手元の写しをクラスタへ
    // push する（review と次の run の prepare〈`--delete` 付きの pull〉の前）。push が落ちたら印が残り、
    // 次の pull は先に push をやり直すので手元の編集は消えない。
    let outcome = match &remote_ws {
        Some(ws) => push_remote_after_run(ws, &sink, outcome).await,
        None => outcome,
    };
    // ADR 2026-10-07-build-tmp-hygiene D2.2: 成否・timeout・中断に依らず run の一時 dir を消す
    // （`runs/<run_id>/` のログ・`result.json` は残す）。消せなかった分は `run_tmpdir::sweep_stale` が拾い直す。
    if let Some(tmp) = run_tmp {
        let path = tmp.path().to_path_buf();
        if let Err(e) = tmp.remove() {
            tracing::warn!(task_id = %task_id, run_id, path = %path.display(), error = %e, "run tmpdir: cleanup failed (ADR 2026-10-07-build-tmp-hygiene D2)");
        }
    }
    // ADR-0098 D1: run の終わり方に依らず、run がまだ lease を持っていれば宣言した後続を作る
    // （出自 = この run。`absorb_memory` と同じく run の成否とは独立）。
    if followups_enabled {
        followups::absorb_run_followups(
            store_for_undeclared_scan.as_ref(),
            task_id,
            run_id,
            &artifacts_dir_for_followups,
            &roles_for_followups,
            &genres_for_followups,
        );
    }
    // ADR-0067 D3 / ADR-0074 D6.3（Phase F1 (j)）/ ADR-0067 付記 2026-10-07: run の終端が
    // `Done` / `Question` / `Waiting` なら未申告の成果物を登録する（判断待ち・cluster job 待ちでも
    // 人がその時点の成果物を使う）。
    // - git worktree ではない local の所有する作業場所はリポジトリ全体
    //   （`artifacts_dir` の外を含む）から `*.md` を拾う（従来どおり。取りこぼし防止）。
    // - すべての Task で、その run の `artifacts_dir` の中を、人が読む拡張子で走査する。
    //   共有 workspace（親なしの retry を含む）では元 task の markdown を混ぜないため全体を走査しない。
    // 重複は `(path, sha256)` で見る（同じ中身は増えない。中身が変われば新しい版）。
    if outcome
        .as_ref()
        .is_ok_and(|o| crate::undeclared_artifacts::terminal_wants_scan(&o.terminal))
    {
        // この run の間に申告された成果物（`sink.artifact()`）も重複の判定に入れるため、run 後の履歴を読み直す
        // （読めなければ run 前の控え）。
        let after_run = store_for_undeclared_scan
            .events_for(task_id)
            .unwrap_or_else(|_| events.clone());
        let existing =
            crate::undeclared_artifacts::registered_keys(after_run.iter().map(|(_, ev)| ev));
        let mut found = if worktree.is_none()
            && remote.is_none()
            && task_core::artifacts::owns_workspace(&task, &workspace_for_undeclared_scan)
        {
            crate::undeclared_artifacts::scan_undeclared_markdown_artifacts(
                &workspace_for_undeclared_scan,
                &artifacts_dir_for_undeclared_scan,
                &existing,
            )
        } else {
            Vec::new()
        };
        let in_artifacts_dir = crate::undeclared_artifacts::scan_undeclared_artifacts_in_dir(
            &workspace_for_undeclared_scan,
            &artifacts_dir_for_undeclared_scan,
            &existing,
        );
        for artifact in in_artifacts_dir {
            if !found
                .iter()
                .any(|old| old.path == artifact.path && old.sha256 == artifact.sha256)
            {
                found.push(artifact);
            }
        }
        for artifact in found {
            let ev = Event::ArtifactProduced {
                run_id: run_id.to_string(),
                artifact,
            };
            if let Err(e) = store_for_undeclared_scan.append_event(task_id, &ev) {
                tracing::warn!(task_id = %task_id, error = %e, "failed to record an undeclared artifact (ADR-0067 D3 / ADR-0074 D6.3)");
            }
        }
    }
    outcome
}

/// ADR-0079 付記「R6-1」D6: remote workspace の準備（`prepare` / `ensure_worktree`）の途中で溜まった進行の行
/// （R6-3 の submodule の初期化「initialised N submodules in <wt> on cluster <c>」など）を、その run の進行
/// （`WorkerProgress`）に 1 行ずつ残す。取り出した行は消える。書いた行の数を返す。
pub(super) fn drain_remote_progress_notes(
    notes: Vec<String>,
    sink: &dyn task_worker::EventSink,
) -> usize {
    for line in &notes {
        sink.progress(line);
    }
    notes.len()
}

/// ADR-0079 R5b-fix2: ワーカー run の後の push（1 run につき 1 回）。結果は進行（`WorkerProgress`）に
/// 1 行残す。push が落ちたら error 付きの進行を残し、run が成功していても失敗として返す（黙って
/// review に進まない。`Unreachable` は供給側失敗 = attempts を消費しない requeue）。run 自体が既に
/// 失敗していればその失敗をそのまま返す。
pub(super) async fn push_remote_after_run(
    ws: &SshWorkspace,
    sink: &dyn task_worker::EventSink,
    outcome: Result<RunOutcome, AdapterError>,
) -> Result<RunOutcome, AdapterError> {
    if ws.settings().sync == SyncMode::None {
        return outcome;
    }
    let target = format!(
        "{}:{}",
        ws.settings().cluster,
        ws.effective_remote_dir().to_string_lossy()
    );
    match ws.push_after_run().await {
        Ok(()) => {
            sink.progress(&format!(
                "pushed the workspace to cluster {target} after the run"
            ));
            outcome
        }
        Err(e) => {
            let msg = format!(
                "push to cluster {target} after the run failed: {e} (local edits are kept; the next sync pushes before pulling)"
            );
            sink.progress_with(
                &msg,
                &task_core::ProgressFields {
                    error: true,
                    ..Default::default()
                },
            );
            tracing::warn!(cluster = %ws.settings().cluster, error = %e, "push after the run failed (R5b-fix2)");
            match outcome {
                Ok(_) => Err(workspace_error_to_adapter(e, "workspace push after run")),
                Err(original) => Err(original),
            }
        }
    }
}

/// ワーカー run 中のリース延長パラメータ（ADR-0010 D7）。
#[derive(Debug, Clone, Copy)]
pub(super) struct LeaseRenewal {
    /// 延長後の ttl（`idle_timeout + lease_grace`）。
    pub(super) ttl: Duration,
    /// 延長の最小間隔（`lease_grace / 2`）。
    pub(super) every: Duration,
}

/// ADR-0129 (1): scratch の経路（run・daemon が走らせる検査）に与える env。`CARGO_TARGET_DIR` と
/// `[scratch.cargo]`（`CARGO_INCREMENTAL` など）だけで、sccache 系は足さず、継いだ env も外さない（`remove` は空）。
pub(super) fn scratch_cargo_env(
    settings: &task_worker::scratch::ScratchSettings,
    owner: &task_worker::scratch::Owner,
) -> task_worker::scratch::CargoEnv {
    let mut set = task_worker::scratch::target_env(&settings.pool(), owner);
    set.extend(task_worker::scratch::cargo_tuning_env(&settings.cargo));
    task_worker::scratch::CargoEnv::set_only(set)
}

/// ADR 2026-10-05-browser-department-web-live-view D2.0: the task browser policy handed to the
/// worker, narrowed to the task's `requirements.browser.allowed_domains`. The grant is not
/// stored with the task: `run_extras` reads the assignee's profile from the org for every run,
/// so the worker intersects with the grant in force (a shrink applies to the next run).
pub(super) fn browser_run_policy(
    store: &dyn TaskStore,
    task: &Task,
) -> Result<Option<task_core::BrowserTaskPolicy>, AdapterError> {
    let stored = store
        .browser_task_policy_get(task.id)
        .map_err(|e| AdapterError::Other(format!("browser policy: {e}")))?;
    if !task_core::browser::requests_browser(&task.skills) {
        return Ok(stored);
    }
    task_core::browser::task_run_policy(&task.requirements, stored.as_ref())
        .map_err(|e| AdapterError::Other(format!("browser policy rejected: {}", e.code())))
}
