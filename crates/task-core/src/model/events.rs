//! Append-only event payloads and event type names.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use super::{
    ArtifactRef, ProgressFields, ProgressKind, RunMetrics, RunRole, Status, Task, TaskId, Tier,
    ToolPolicyKind, Usage,
};

/// ADR-0079 D4 (4)（Phase R1b）: `Event::Created.origin`。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CreatedOrigin {
    /// 親の計画（plan/3）の kind task の unit から daemon が作った子 task。
    PlanUnit,
    /// ADR-0098 D5（Phase R7-10）: task `task_id` の worker の run `run_id` が宣言した後続
    /// （`<artifacts_dir>/followups.json`）から daemon が作った task。出自は daemon が run に割り当てた
    /// 成果物ディレクトリで決まる（run の自己申告ではない）。
    WorkerRun { task_id: TaskId, run_id: String },
}

/// ADR-0079 付記 R7-5 D1: `Event::WorkUnitChecksFailed` の不合格の検査 1 件。`detail` は判定文そのもの
/// （`cmd=… exit=… expected=… stdout_tail=… stderr_tail=…`、timeout・exec 失敗の文も同じ）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FailedWorkUnitCheck {
    pub cmd: String,
    pub expect_exit: i32,
    pub detail: String,
}

/// ADR 2026-10-22-stalled-auto-replan D1: `Event::AutoReplanRequested.source`（どこの失敗から replan したか）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AutoReplanSource {
    /// 段の統合の検査の失敗（`integration_gives_up`）。
    Integration,
    /// WU の run の失敗・no progress・plan_issue。
    WorkUnit,
}

/// `Event::TargetSweepRan` の実行方式（D1.4: `--dry-run` は何も消さない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TargetSweepMode {
    Apply,
    DryRun,
}

/// 消した項目の理由別の数（D1.2: 古さ・上限・放置された target dir）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TargetSweepByReason {
    pub age: u64,
    pub cap: u64,
    pub stale_target: u64,
}

/// `Event::TargetSweepRan` の root 1 つ分。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TargetSweepRootReport {
    pub root: String,
    pub before_bytes: u64,
    pub after_bytes: u64,
    pub deleted_bytes: u64,
    pub deleted_items: u64,
    pub by_reason: TargetSweepByReason,
}

/// `Event::TargetSweepRan` の skip 1 件（例: `reason = "build_in_progress"`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TargetSweepSkip {
    pub path: String,
    pub reason: String,
}

/// DESIGN §4.3 の `Event`（追記専用）。ADR-0002 D2: `Transitioned` は遷移の
/// *結果* を記録するものであり、`transition()` の入力（`Trigger`）とは別物。
/// `JsonSchema` は ADR-0013 D8: `docs/api/v1/event.schema.json`（`EventRow` 経由）の契約に使う。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// ADR 2026-10-11 stranded branches: durable integration state changed.
    IntegrationDetected {
        repo_id: crate::RepoId,
        branch: String,
        head_sha: String,
        reason: Option<String>,
        actor: String,
    },
    IntegrationMerged {
        repo_id: crate::RepoId,
        branch: String,
        head_sha: String,
        actor: String,
    },
    IntegrationDiscarded {
        repo_id: crate::RepoId,
        branch: String,
        head_sha: String,
        reason: String,
        actor: String,
    },
    IntegrationHeld {
        repo_id: crate::RepoId,
        branch: String,
        head_sha: String,
        reason: String,
        actor: String,
    },
    IntegrationReopened {
        repo_id: crate::RepoId,
        branch: String,
        head_sha: String,
        reason: Option<String>,
        actor: String,
    },
    /// ADR 2026-10-11 stranded-done-branches F2: 遠隔 repo など sweep の対象外になった（`not_applicable`）。
    IntegrationNotApplicable {
        repo_id: crate::RepoId,
        branch: String,
        head_sha: String,
        reason: String,
        actor: String,
    },
    HarnessSelected {
        harness_id: String,
    },
    LlmRouteSelected {
        source_id: String,
        model_id: String,
        account_id: String,
        metadata: serde_json::Value,
    },
    HarnessMigrationUnresolved {
        task_id: String,
        original_value: String,
    },
    /// CoS operation attempt, including rejected attempts. The envelope is immutable.
    CosOperation {
        actor: String,
        thread_id: String,
        run_id: String,
        operation_id: String,
        reason: String,
        policy_version: String,
        state: String,
        target_kind: String,
        target_id: String,
    },
    /// ADR-0118 D3: immutable snapshot of one repository's review attempt.
    ReviewTargetSynced {
        review_run: String,
        repo_id: crate::RepoId,
        target_ref: String,
        target_sha: String,
        before_sha: String,
        reviewed_sha: String,
        merge_candidate_sha: String,
        attempt: u32,
    },
    /// ADR 2026-10-23-required-checks-bound-to-candidate: review が評価した必須検査（リポジトリの
    /// `[commands] check`）の結果を、基底 `target_sha`・候補 `reviewed_sha` に束縛して残す。
    /// 同じ束縛の合格は再 review で再利用し、delivery は束縛が現在の候補・基底と一致する証跡だけを受け取る。
    RequiredChecksEvaluated {
        review_run: String,
        repo_id: crate::RepoId,
        target_sha: String,
        reviewed_sha: String,
        /// 評価した必須検査の定義（cmd を順に連ねた sha256）。
        definition_sha256: String,
        /// この定義を読んだリポジトリ（review が検査の定義を引く先頭のリポジトリ）。
        definition_repo_id: crate::RepoId,
        results: Vec<PhaseCheckResult>,
    },
    /// ADR-0118 D4: a previously reviewed target advanced before integration.
    ReviewTargetAdvanced {
        review_run: String,
        repo_id: crate::RepoId,
        reviewed_sha: String,
        target_sha: String,
        attempt: u32,
    },
    /// ADR-0120 D5: review 前同期の衝突に対し IntegrationRepair WU を起票した（WU の追加・
    /// `ReviewRepair` 遷移と同じ transaction で 1 回）。`attempt` は integration_repair WU の通し番号
    /// （1 始まり）。`conflict_files` は重複を除きパス順。状態・attempts は変えない。
    IntegrationRepairScheduled {
        work_unit_id: String,
        key: String,
        repo_id: crate::RepoId,
        target_ref: String,
        target_sha: String,
        before_sha: String,
        conflict_files: Vec<String>,
        attempt: u32,
    },
    /// ADR-0120 D5: 修復 WU の完了後、最新 target への再同期が成功した（`target_sha` は再同期時の
    /// SHA）。checks/reviewer の合格は意味しない。状態・attempts は変えない。
    IntegrationRepairResolved {
        work_unit_id: String,
        repo_id: crate::RepoId,
        target_sha: String,
        reviewed_sha: String,
        attempt: u32,
    },
    /// ADR-0120 D4/D5: IntegrationRepair を打ち切った（1 回につき 1 件）。`rollback_to_sha` は安全な
    /// rollback が完了したときだけ `before_sha`。`fallback` は未同期 HEAD で review へ進めるときだけ
    /// `true`。状態・attempts は変えない。
    IntegrationRepairExhausted {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        work_unit_id: Option<String>,
        repo_id: crate::RepoId,
        target_sha: String,
        before_sha: String,
        attempt: u32,
        reason: crate::execution::IntegrationRepairExhaustReason,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rollback_to_sha: Option<String>,
        fallback: bool,
    },
    /// Credential-free mapping between one isolated browser session and a worker execution.
    BrowserUpdated {
        browser: crate::BrowserRun,
    },
    /// ADR-0080 D4/D5: browser の人待ち（登録依頼・承認）を開いた。wait は参照と固定の値だけで、秘密を持たない。
    BrowserWaitOpened {
        wait: Box<crate::browser_wait::BrowserWait>,
    },
    /// ADR-0080 D4: browser の wait が解決・終端化した（`code` は固定の語だけ）。
    BrowserWaitResolved {
        wait_id: String,
        reason: crate::browser_wait::BrowserWaitReason,
        state: crate::browser_wait::BrowserWaitState,
        code: String,
        version: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        approval_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        credential_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor_id: Option<String>,
    },
    /// ADR 2026-10-08-browser-prod-enablement D2: browser の前提（適合台帳・task policy）が無いので
    /// task を止めた。`message` は code ごとの固定の人向けの文（path・秘密は載せない）。止まっている間は再追記しない。
    BrowserPrerequisiteBlocked {
        code: crate::browser_prerequisite::BrowserPrerequisiteCode,
        message: String,
    },
    /// ADR 2026-10-08-browser-prod-enablement D2: 前提が揃い、止めていた task を `ready` に戻した。
    /// `code` は止めたときの code。
    BrowserPrerequisiteResumed {
        code: crate::browser_prerequisite::BrowserPrerequisiteCode,
    },
    /// ADR-0090 D3: worker の run が `result.json` の `wait` でクラスタ job の終了待ちに入った（`state = waiting`）。
    /// `cluster_job_waits` の行は同じトランザクションで作る（`cluster_job::apply_event_tx`）。
    ClusterJobWaitStarted {
        wait: Box<crate::cluster_job::ClusterJobWait>,
    },
    /// ADR-0090 D3: poll で job の状態が変わった（変わらない poll では出さない）。
    ClusterJobWaitPolled {
        wait_id: String,
        jobs: Vec<crate::cluster_job::ClusterJobStatus>,
    },
    /// ADR-0090 D3: wait が終わった（`satisfied` = すべての job が終わった / `timed_out` / `cancelled`）。
    ClusterJobWaitFinished {
        wait_id: String,
        state: crate::cluster_job::ClusterJobWaitState,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        jobs: Vec<crate::cluster_job::ClusterJobStatus>,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        detail: String,
    },
    Created {
        task: Box<Task>,
        /// ADR-0079 D4 (4)（Phase R1b）: どの入口から作られたか（今は親の計画の kind task の unit から
        /// daemon が作った子 task だけが `plan_unit` を持つ。それ以外は省略〈従来の JSON のまま〉）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<CreatedOrigin>,
    },
    Transitioned {
        from: Status,
        to: Status,
        reason: String,
    },
    WorkerStarted {
        run_id: String,
        adapter: String,
        model: String,
        /// どのプロバイダ（= アカウント）で実行したか（ADR-0012 D1）。導入前のイベントには無いので任意。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
        /// プール（`account_pool = true`）で選ばれた Claude アカウントの id（ADR-0024 D4）。プールを使わない
        /// プロバイダ・導入前のイベントには無い。`provider`（アダプタ×プロバイダ行）とは別軸。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account: Option<String>,
        /// ADR-0014 D1: `None` はワーカー run、`Some(Reviewer)` は Reviewer run（ワーカー run に `Some(Worker)` は書かない）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<RunRole>,
        /// ADR-0016 D1: run 開始時のタスクの役割名（`Task.role`）。役割の無いタスク・導入前のイベントには無い。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        task_role: Option<String>,
    },
    /// ADR-0048 D2（Phase 60a）: 構造化した進行。`msg` は従来どおり人が読む 1 行で、
    /// `kind` / `tool` / `summary` / `detail` / `truncated` / `error` は**追加のみ**
    /// （付けないワーカー・導入前のイベントは全て `None` / `false` として読める）。
    /// 構築は `Event::worker_progress` / `Event::worker_progress_with` を使う。
    WorkerProgress {
        run_id: String,
        msg: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<ProgressKind>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        truncated: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        error: bool,
    },
    /// ADR 2026-10-07-worker-no-subagents-no-llm-cli D5: worker が subagent の道具・別の LLM CLI・LLM API を
    /// 使おうとしたことの決定的な検出（adapter の進行の写像で見つける。LLM は関与しない）。run は止めない
    /// （警告）。reviewer に `ReviewRequest.policy_violations` として渡る（D6）。
    WorkerPolicyViolation {
        run_id: String,
        kind: ToolPolicyKind,
        /// 道具名（`Bash` / `command_execution` / `Agent` …）。
        tool: String,
        /// 一致した語（道具名・CLI 名・API host）。
        matched: String,
        /// 起動しようとした command（500 文字で切る。subagent 道具なら入力の要約）。
        command: String,
    },
    ArtifactProduced {
        run_id: String,
        artifact: ArtifactRef,
    },
    WorkerFinished {
        run_id: String,
        outcome: String,
        usage: Option<Usage>,
        /// ADR-0014 D1: `WorkerStarted.role` と同じ（`None` はワーカー run）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<RunRole>,
        /// ADR-0061（Phase 104）: harness routing 基盤のメトリクス（wall time・retry 回数）。
        /// `WorkerStarted.adapter`/`model` と同じ `run_id` で突き合わせる。導入前のイベント・
        /// この run の起点時刻を持たない経路（無い）は `None`。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metrics: Option<RunMetrics>,
        /// ADR-0072 D7（Phase E1）: この run の終わり方の構造化した分類。導入前のイベント、または
        /// 分類できなかった run（旧経路の字句判定に頼るしかないもの）は `None`。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        end: Option<crate::execution::RunEnd>,
    },
    ReviewVerdict {
        run_id: String,
        criterion_idx: usize,
        pass: bool,
        reason: String,
    },
    ApprovalRequested,
    ApprovalDecided {
        by: String,
        approved: bool,
        note: Option<String>,
    },
    /// Phase F7（ADR-0033 D5 追記 2026-09-28）: このタスクが終端（`done` / `failed` / `cancelled`）に
    /// なったので、未決の認可の要求（`approvals` 表の行）を celeris が `withdrawn` で閉じた。
    /// `reason` は `"task_terminal"`（終端への遷移と同じトランザクション）か `"reconcile"`
    /// （tick の照合）。状態は変えない（`replay` は無視する）。`ApprovalDecided`（`kind = approval`
    /// タスクの承認・却下）とは別物。
    ApprovalsWithdrawn {
        approval_ids: Vec<crate::approval::ApprovalId>,
        task_status: Status,
        reason: String,
    },
    /// `blocked` のタスクへの人間の回答（ADR-0010 D3, P-10）。`Transitioned{reason:"answer"}` と同一トランザクションで
    /// 追記し、次の run の `context.answers` に載せる。
    Answered {
        question: String,
        answer: String,
    },
    /// ADR-0021 D2: ディスパッチャが人間に出した質問（run の終了ではないので `WorkerFinished` は使わない）。
    /// 状態は変えない（`replay` は無視する）。同じトランザクションの `Transitioned{to: blocked}` と対で記録する。
    QuestionRaised {
        /// この質問のきっかけになった run（親の直近の run）。
        run_id: String,
        text: String,
    },
    /// ADR-0016 D2: 実行中の run が `delegate` で提案し、検証を通って挿入された子タスク。状態は変えない（`replay` は無視する）。
    Delegated {
        run_id: String,
        task_ids: Vec<TaskId>,
    },
    /// ADR-0018 D2: クラスタへの ssh 多重接続が無く、そのクラスタでは実行できない（人のログイン待ち）。
    ClusterUnavailable {
        cluster: String,
        /// `~/.ssh/config` の `Host` 名（`scripts/cluster-login.sh <host>` を案内するため）。第 1 段階の行には無いので任意。
        #[serde(default)]
        host: String,
        reason: String,
    },
    /// ADR-0062 A（Phase 107）: celeris が保持していた ssh master（`ClusterMaster`）のプロセスが、
    /// 明示的な切断（`DELETE /clusters/{id}/connect`）を経ずに自分で終了した。`exit_code` は分かる
    /// ときだけ（シグナルで落ちた場合は `None`）、`stderr_tail` は master の stderr の末尾（最大 300
    /// バイト。ADR-0032 D4 のとおり検証コードはここには出ない）。同じ切断について 1 回だけ記録する
    /// （`Dispatcher` 側で dedupe する。`mark_cluster_unavailable` から呼ぶ）。
    ClusterMasterExited {
        cluster: String,
        exit_code: Option<i32>,
        #[serde(default)]
        stderr_tail: String,
    },
    ProviderThrottled {
        provider: String,
        #[serde(with = "time::serde::rfc3339")]
        #[schemars(with = "String")]
        until: OffsetDateTime,
        /// 供給側失敗の種別（ADR-0013 D9）: `throttled | auth_failed | exhausted | spawn`。
        /// 導入前のイベントには無いので任意。このバリアントを構築している箇所は現状無い
        /// （`grep -rn ProviderThrottled crates` で確認済み）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Phase 31（実機の事故、2026-09-18）: `failed`/`cancelled` を複製してやり直したときの新しいタスクに
    /// 記録する。`from` = 元のタスク。状態は変えない（`Created` が初期状態を与える）。
    Retried {
        from: TaskId,
    },
    /// ADR-0044 D1（Phase 53）: 人がタスクを編集した（`PATCH /tasks/{id}`）。`fields` は変えた項目の名前
    /// （`title` / `priority` / `labels` …。並びは決定的）、`by` は `"human"`。状態は変えない
    /// （`replay` は無視する）。
    Edited {
        fields: Vec<String>,
        by: String,
    },
    /// ADR-0046 D5（Phase 59）: `assignee` が無いタスクの担当を matching が決めた。状態は変えない
    /// （`replay` は無視する）。GUI のタスク画面が「なぜこの担当か」をこの 1 件から出す。
    Assigned {
        /// 決まった担当（`org_nodes.id`）。
        node: String,
        /// タスクの skills とノードの実効 skills の重なりの数。
        score: usize,
        /// 決め手（決定的な文面。LLM は使わない）。
        reason: String,
    },
    /// ADR-0059 D3（Phase 99）: `mode` 省略のリモートタスクが worktree 準備で「git リポジトリでない」
    /// （exit 65）になり、`repos` が無い（コードを触らない仕事）ので `shared` として続行した。
    /// 状態は変えない（`replay` は無視する）。この後 `task.workspace.mode` が `Some(Shared)` に
    /// 書き戻る（`Event::Edited` は積まない。編集は人によるものではないため）。
    WorkspaceModeDowngraded {
        cluster: String,
        /// クラスタ側の作業ディレクトリ（展開前の `path` の文字列表現）。
        path: String,
        /// worktree 準備が失敗した理由（ssh.rs の stderr）。
        reason: String,
    },
    /// ADR-0066 D2（Phase 110b）: 終端になってから `[workspace] prune_after_secs` 経った作業場所から、
    /// ビルド生成物（`target/` 等）を消した。状態は変えない（`replay` は無視する）。
    WorkspacePruned {
        /// 消したパス（作業場所〈`<workspace_root>/<task_id>`〉からの相対。例: `repos/benchfs/target`）。
        removed: Vec<String>,
    },
    /// ADR 2026-10-07-build-tmp-hygiene D1.5: 共有 cargo target の定期掃除（`target_sweep`）を 1 回走らせた記録。
    /// cron 由来の task に追記する。状態は変えない（`replay` は無視する）。
    TargetSweepRan {
        mode: TargetSweepMode,
        roots: Vec<TargetSweepRootReport>,
        /// skip した path と理由（先頭 50 件。総数は `skipped_total`）。
        skipped: Vec<TargetSweepSkip>,
        skipped_total: u64,
        /// lock が取れない profile のせいで上限の 80% まで下がらなかった。
        over_cap_unresolved: bool,
        duration_ms: u64,
    },
    /// ADR-0069 D5（Phase 114）: この run の routing の監査記録（担当・harness・lane・model・features・
    /// 当たった規則・policy の版・エスカレーション）。同じ `run_id` の `WorkerStarted` の直後に 1 件。
    /// 状態は変えない（`replay` は無視する）。
    RoutingDecided {
        run_id: String,
        record: Box<crate::model_policy::RoutingRecord>,
    },
    /// Selector stage transition, recorded with its persisted projection.
    RoutingStageChanged {
        genre: String,
        harness_id: String,
        from: Option<String>,
        to: String,
        reason: String,
        window: serde_json::Value,
        actor: String,
    },
    /// 2026-10-04 多目的 routing ADR §6（Phase 3）: 決定 1 件の特徴 snapshot（本文・secret なし）。wire は
    /// `decision_id, context_version, features, provenance, missing_fields` が event の欄に平たく並ぶ。
    /// 状態は変えない（`replay` は無視する）。
    RoutingFeaturesRecorded {
        #[serde(flatten)]
        record: Box<crate::model_router::feedback::RoutingFeaturesRecord>,
    },
    /// 同 §6: proxy の要求 1 件の trace（run 側 decision を親に持つ。`request_id` で `llm_proxy_requests` と結合）。
    /// 状態は変えない。
    RoutingRequestDecided {
        #[serde(flatten)]
        record: Box<crate::model_router::feedback::RoutingRequestRecord>,
    },
    /// 同 §6: run 1 件の結果 vector と reward（`model_router::feedback::project_run_outcomes` の投影）。
    /// 訂正は新しい `outcome_id` + `supersedes` を追記する（上書きしない）。状態は変えない。
    RoutingOutcomeRecorded {
        #[serde(flatten)]
        outcome: Box<crate::model_router::feedback::RoutingOutcome>,
    },
    /// ADR 2026-10-16-mlsys-measure-task-routing D2: 作成時に task 欄で固定した routing 設定（mode・下段・
    /// 根拠表・オンライン更新）。`task_routing_overrides` の行と同じ transaction で 1 回だけ追記する。
    /// 状態は変えない。`online_update = false` なら `routing_outcome_recorded` を追記しない。
    RoutingOverridePinned {
        routing_override: crate::route_select::RoutingOverride,
    },
    /// 同 §7.1・Phase 4: shadow（decision / execution）1 件の結果。primary の選択・成功/失敗・attempts とは
    /// 別欄で、それらを変えない。本文・prompt・credential は持たない（出力は SHA-256 と tokens だけ）。
    /// 状態は変えない（`replay` は無視する）。
    RoutingShadowRecorded {
        #[serde(flatten)]
        record: Box<crate::routing_shadow_history::ShadowRecord>,
    },
    /// ADR-0072 D5/D8（Phase E1）: run 終了時に daemon が確定させた checkpoint（worker の申告 +
    /// mechanical の合成）。状態は変えない（`replay` の attempts 計算は無視する）。
    CheckpointSaved {
        run_id: String,
        /// E1 では常に `None`（暗黙の WorkUnit）。E2 以降で WorkUnit の id を持つ。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        work_unit_id: Option<String>,
        checkpoint: Box<crate::execution::Checkpoint>,
    },
    /// ADR-0072 D5/D14/D17（Phase E2）: 計画の採用（新規または replan）。`supersedes` は replan
    /// のときの旧 `execution_plans.id`（E2 では常に `None`。replan は E4）。状態は変えない
    /// （`replay` の attempts 計算は無視する）。
    ExecutionPlanned {
        plan_id: String,
        version: u32,
        origin: crate::execution_plan::PlanOrigin,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        supersedes: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        plan: Box<crate::execution_plan::ExecutionPlanSpec>,
    },
    /// ADR-0072 D5/D6（Phase E2）: WorkUnit の状態遷移。`run_id` はこの遷移のきっかけになった run
    /// （無ければ `None`。例: 依存先の失敗による `dependency_failed`）。状態は変えない
    /// （`replay` の attempts 計算は無視する）。
    WorkUnitTransitioned {
        work_unit_id: String,
        key: String,
        from: crate::execution_plan::WorkUnitStatus,
        to: crate::execution_plan::WorkUnitStatus,
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_id: Option<String>,
    },
    /// ADR-0079 付記 R7-5 D1: WU の run が `done` を返したが、その WU の `checks` が不合格だった（daemon が run を
    /// retry / failed にすり替えた理由）。`cwd` は check を実際に走らせた所。その run の `WorkerFinished` と同じ
    /// トランザクションで積む。状態は変えない（`replay` は無視する）。
    WorkUnitChecksFailed {
        run_id: String,
        work_unit_id: String,
        key: String,
        cwd: String,
        failed: Vec<FailedWorkUnitCheck>,
    },
    /// ADR-0079 R5b-fix1: 人の replan（`PlanOrigin::Human`）が done の WorkUnit の spec を上書きした
    /// （状態は `done` のまま。`work_units.spec` だけを新しい版の spec に置き換える）。`plan_id` / `plan_version` は
    /// 上書きした新しい版、`changed_fields` は変わった `WorkUnitSpec` の欄の名前（昇順）。replan と同じ
    /// トランザクションで、その版の `ExecutionPlanned` の前に積む。状態は変えない（`replay` は行の spec の
    /// 書き換えにだけ使う）。
    WorkUnitSpecOverridden {
        work_unit_id: String,
        key: String,
        plan_id: String,
        plan_version: u32,
        #[serde(default)]
        changed_fields: Vec<String>,
    },
    /// ADR-0072 D5/D13（Phase E3）: Complexity Gate の判定（atomic/compound、当たった信号）。
    /// `Task.routing.execution` と同じトランザクションで書く。状態は変えない（`replay` は無視する）。
    ExecutionGated {
        decision: Box<crate::execution_gate::ExecutionGateDecision>,
    },
    /// ADR-0124: 経路選択の監査記録。状態は変えない。
    ExecutionRouted {
        decision: Box<crate::direct_route::RouteDecision>,
    },
    /// ADR-0072「Phase F6 実装時の決定」: 起票済みの Task の実行の形（atomic / compound）を人が後から
    /// 決めた（`POST /tasks/{id}/execution/decompose`、MCP `task_decompose`、retry の `execution`）。
    /// 同じトランザクションで `Task.routing.execution_hint = {mode, explicit: true}` を書き、
    /// `Task.routing.execution`（前の gate の判定）を消す（次の dispatch で gate が `human/explicit` として
    /// 判定し直し、新しい `ExecutionGated` を残す）。`replan = true` は計画を既に持つ Task への
    /// compound の依頼（次の dispatch で replan の planner run になる。ADR-0072 D17 5.）。
    /// 状態は変えない（`replay` は無視する）。
    ExecutionHintSet {
        mode: crate::execution_gate::ExecutionMode,
        /// 変える前の `execution_hint`（無ければ省略）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        previous: Option<crate::execution_gate::ExecutionHintSpec>,
        /// 消した前の gate の判定（`rule_id` と `source` と `shadow` の監査用。無ければ省略）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        previous_decision: Option<Box<crate::execution_gate::ExecutionGateDecision>>,
        /// 誰が決めたか（`"human"`、`"mcp:<client_id>"`）。
        source: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        replan: bool,
    },
    /// ADR-0074 D6.2（Phase F1）: repair WU を起こしたこと（class・起こした場所）を残す。
    /// `execution_metrics::summarize` はこの Event から `repairs_by_class` を組み立てる
    /// （title の接頭辞の復元に頼らない。`unknown` を無くす。D16/D17）。状態は変えない
    /// （`replay` の attempts 計算は無視する）。
    RepairScheduled {
        work_unit_id: String,
        key: String,
        /// `RepairClass::bucket()`（`format`/`lint`/`test_small`/`reviewer_local`/`merge_base`/
        /// `review_timeout`）、または planner が replan で自ら書いた repair WU の `"planner"`。
        class: String,
        origin: crate::execution::RepairOrigin,
    },
    /// ADR-0074 D4.3（Phase F3 quota）: 1 run の quota 消費の推定（決定的。`task_core::quota` の
    /// 純粋関数で決める）。`WorkerFinished` には足さない（struct literal の多数の箇所を避けるため、
    /// ADR-0072 E1/E2 の逸脱節と同じ判断）。状態は変えない（`replay` は無視する）。
    ///
    /// `apportioned` は同じ `run_id` にもう 1 件出ることがある（グループの最後の run が終わって
    /// 按分が確定したとき。**最後の Event が有効**）。
    QuotaEstimated {
        run_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        work_unit_id: Option<String>,
        /// `"claude-oauth"` / `"codex-oauth"` / それ以外（アカウントプールを使わない provider の id。
        /// D4.2 手順 5 の `free` になりうる）。
        source: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account: Option<String>,
        windows: Vec<crate::quota::QuotaWindowUse>,
        weighted_tokens: f64,
        /// `windows` を 1 つに畳み込んだ代表値（`task_core::quota::representative_method`）。
        method: crate::quota::QuotaMethod,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        calibration: Option<crate::quota::QuotaCalibration>,
        weights_version: String,
        /// 参考の定価 USD（`Usage.cost_usd` と同じ値。不明なら `None`）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        list_price_usd: Option<f64>,
    },
    /// ADR-0074 D1.2（Phase F2b）: v2 の WU の run が done になり、daemon が WU の作業ツリーで
    /// 決定的に commit した（`git add -A && git commit -m "wu/<key>: <title>"`。変更が無ければ commit
    /// せず、その時点の HEAD）。`work_units.branch`/`base_commit`/`head_commit` の正本。状態は変えない。
    WorkUnitCommitted {
        work_unit_id: String,
        key: String,
        /// `celeris-wu/<task_id>/<key>`（統合の repair WU のように Task の worktree で走った WU は
        /// Task のブランチ）。
        branch: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base: Option<String>,
        commit: String,
    },
    /// ADR-0074 D1.4（Phase F2b）: 工程の統合が済んだ（葉の WU の merge と検査の再実行が通った）。
    /// 状態は変えない（Task の遷移は同じトランザクションの `Transitioned`）。
    PhaseIntegrated {
        phase: String,
        work_unit_id: String,
        merged: Vec<PhaseMerged>,
        /// 統合後の Task ブランチの HEAD（WU の worktree を持たない並列 1 の工程では空文字列）。
        head: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        checks: Vec<PhaseCheckResult>,
    },
    /// 2026-10-04 統合の検査の進み具合 D1: 段の統合が検査 1 件を始めた。`index` は 0 始まり、`total` はその統合の
    /// 検査の数。`log_path` はその検査の stdout/stderr を逐次書くファイル（daemon の host の絶対パス）。
    /// 状態は変えない（`replay` は無視する）。
    IntegrationCheckStarted {
        work_unit_id: String,
        key: String,
        index: u32,
        total: u32,
        cmd: String,
        log_path: String,
        started_at: String,
    },
    /// 2026-10-04 統合の検査の進み具合 D1: 段の統合の検査 1 件が終わった。`exit` は signal・timeout・起動失敗で
    /// `None`。`duration_ms` は再実行（timeout の 2 倍・merge-base の修復）を含む所要時間。状態は変えない。
    IntegrationCheckFinished {
        work_unit_id: String,
        key: String,
        index: u32,
        total: u32,
        cmd: String,
        pass: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit: Option<i32>,
        #[serde(default)]
        timed_out: bool,
        duration_ms: u64,
    },
    /// 2026-10-04 WU 検査の引き継ぎ D3: 葉の WU の受け入れ検査（`spec.checks`）が検査 1 件を始めた。形は
    /// `IntegrationCheckStarted` と同じで、検査を起こした worker run の `run_id` を足す。状態は変えない。
    WorkUnitCheckStarted {
        work_unit_id: String,
        key: String,
        run_id: String,
        index: u32,
        total: u32,
        cmd: String,
        log_path: String,
        started_at: String,
    },
    /// 2026-10-04 WU 検査の引き継ぎ D3: 葉の WU の受け入れ検査 1 件が終わった（`IntegrationCheckFinished` と同じ形）。
    WorkUnitCheckFinished {
        work_unit_id: String,
        key: String,
        run_id: String,
        index: u32,
        total: u32,
        cmd: String,
        pass: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit: Option<i32>,
        #[serde(default)]
        timed_out: bool,
        duration_ms: u64,
    },
    /// ADR-0040 付記（2026-10-04、WU 検査の引き継ぎ）D1: draining の旧 instance で worker run が終わり、受け入れ
    /// 検査を始めずに手を離した。WU は `running`・lease はその run のまま残り、結果は `runs/<run_id>/result.json`
    /// にある。新しい active instance がこれを見て result.json から確定し直す（検査はそこで走る）。状態は変えない。
    WorkUnitChecksHandedOff {
        work_unit_id: String,
        key: String,
        run_id: String,
    },
    /// ADR-0118 D5: 段階の統合で、子 task のブランチ HEAD が記録済みの `merge_candidate_sha` と違った
    /// （review 後に子のブランチが動いた）ので merge しなかった。子は再 sync → 再 check → 再 review に戻る。
    MergeCandidateStale {
        phase: String,
        work_unit_id: String,
        key: String,
        child_task: TaskId,
        repo_id: crate::RepoId,
        branch: String,
        merge_candidate_sha: String,
        head_sha: String,
        target_sha: String,
    },
    /// ADR 2026-10-22-stalled-auto-replan D1: daemon が失敗から自動 replan（`Continue{Replan}`）を起こした。`key` は
    /// 失敗した WU（統合 WU を含む）、`failure` は失敗の識別子（`task_ops::plan_gate::failure_signature`）。同じ
    /// `(key, failure)` が進展（変更のある `WorkUnitCommitted`）なしに続けば自動 replan をやめる。状態は変えない。
    AutoReplanRequested {
        key: String,
        failure: String,
        source: AutoReplanSource,
    },
    /// ADR-0074 D1.2（Phase F2b）: v2 の計画だが並列 1 に倒した（remote / 書き込み可能な `dir` の
    /// repo / `Shared`）。計画ごとに 1 回だけ残す。状態は変えない。
    WorkUnitsSerialized {
        plan_id: String,
        reason: String,
    },
    /// ADR-0074 D2.1（Phase F3 途中確認）: 計画の採用（新規・replan）、または
    /// `PUT /tasks/{id}/execution/pause-after` の直後に、その時点の `Task.routing.pause_after` を
    /// 工程の key の集合へ解決した結果を残す。同じ `plan_id` に複数出ることがある（**最後の Event が
    /// 有効**。`ExecutionPlanned`/`QuotaEstimated` と同じ規律）。状態は変えない（`replay` は無視する）。
    PausePointsResolved {
        plan_id: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        phases: Vec<String>,
        source: crate::pause::PauseSource,
    },
    /// ADR-0074 D2.3（Phase F3 途中確認）: 停止点の工程の統合の後に、決定的に組み立てた途中報告を
    /// 残す（LLM は使わない）。同じ内容を `artifacts/phase-reports/<n>-<phase>.md` にも書く
    /// （`ArtifactProduced` が別に飛ぶ）。状態は変えない（同じトランザクションの
    /// `Transitioned{reason:"awaiting_human"}` が Task を `blocked` にする）。
    PhaseReported {
        phase: String,
        report: Box<crate::pause::PhaseReport>,
    },
    /// ADR-0074 D3.3（Phase F4a）: CoS の案件計画 run（`mode = "milestones"`）が検証を通り、マイルストーン
    /// （`proposed`）と top-level の draft Task を作った。**`plan_task_id`（この Event が付く Plan
    /// タスク）の events 列がその提案の正本**。状態は変えない（`replay` は無視する）。
    ProjectPlanProposed {
        project_id: crate::org::ProjectId,
        /// 1 から始まる連番。replan（D3.4、F4b）のたびに増える。F4a の初回提案は常に 1。
        version: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        supersedes: Option<u32>,
        /// 提案が承認されたときの計画**全体**（replan の差分でも、差分を当てた後の全体を入れる）。
        plan: Box<crate::project_plan::ProjectPlanSpec>,
        /// この提案で作った途中目標と Task（`plan.milestones` と同じ順）。承認/却下の対象を
        /// 決定的に引くため。replan（F4b）では、既存のもの（id はそのまま）と `add` で作ったものを
        /// 合わせた、承認後の計画全体の対応表。
        milestones: Vec<ProposedMilestone>,
        /// ADR-0074 D3.4（Phase F4b (e)）: replan の差分（`celeris.project-plan-delta/1`）。初回の提案は `None`。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delta: Option<Box<crate::project_plan::ProjectPlanDelta>>,
    },
    /// ADR-0074 D3.3（Phase F4a）: 人の承認/却下
    /// （`POST /projects/{id}/project-plan/{version}/decide`）。状態は変えない（マイルストーン/Task の
    /// 遷移は同じ操作の中の他の呼び出しが行う）。
    ProjectPlanDecided {
        project_id: crate::org::ProjectId,
        version: u32,
        approved: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    // ---- ADR-0079（Phase R1a）: 再帰的な task の木。型と replay の読みだけ（発行は R1b 以降）----
    /// ADR-0079 D4 (4): 親の計画の kind task の unit から子 task を作った（親の events に積む）。
    /// 同じトランザクションで `work_units.child_task_id` を書く（store の派生の書き込み）。
    /// 状態は変えない（`replay` は無視する）。
    ChildTaskCreated {
        /// 親の計画の版（`execution_plans.id`）。
        plan_id: String,
        unit_key: String,
        child_task_id: TaskId,
        /// 子の深さ（task の層数。root = 1。ADR-0079 付記 U-R1）。
        depth: u32,
    },
    /// ADR-0079 D15: 既存の task を root の計画の kind task の unit の子として採用した（root の events に
    /// 積む）。同じトランザクションで `work_units.child_task_id` を書く。状態は変えない。
    ChildAdopted {
        plan_id: String,
        unit_key: String,
        stage: String,
        child_task_id: TaskId,
    },
    /// ADR-0079 D4 (3): unit の gate が planner の宣言と食い違い、daemon が扱いを決めた。
    /// 状態は変えない（Phase R2a で計画の採用の直後に発行する。上げた・下げた unit の形は採用した
    /// 計画の spec に既に入っている）。
    UnitGateOverridden {
        plan_id: String,
        unit_key: String,
        declared: crate::tree::UnitDeclared,
        gate: crate::execution_gate::ExecutionMode,
        action: crate::tree::UnitGateAction,
        /// unit の深さ（= 計画を持つ task の深さ + 1）。
        depth: u32,
        /// その深さの gate の閾値（`task_core::tree::gate_threshold`）。
        threshold: u32,
        /// Phase R2a: unit の view の gate のスコア（`ExecutionGateDecision.score`）。
        #[serde(default)]
        score: i32,
        /// Phase R2a: 判断の理由（規則 id・構造上の理由・leaf の基準の不足など。人が読む 1 行）。
        #[serde(default, skip_serializing_if = "String::is_empty")]
        reason: String,
    },
    /// ADR-0079 D7: 人への決定の要求（出した節点の events に積む）。同じトランザクションで
    /// `decisions` の行を書く（store の派生の書き込み）。状態は変えない（発行は R3a 以降）。
    DecisionRequested {
        decision: Box<crate::decision::DecisionRequest>,
    },
    /// ADR-0079 D7: 決定への回答（revise も同じ Event。最後の回答が有効）。状態は変えない。
    DecisionAnswered {
        id: String,
        option: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
        by: String,
    },
    /// ADR-0079 D7: 決定の取り下げ（計画の replan・subtree の中止など）。状態は変えない。
    DecisionWithdrawn {
        id: String,
        reason: String,
    },
    /// ADR-0079 D8: root の計画に人の承認が要る（決定を含む / `review: human` / 上限に近い）。
    /// 状態は変えない（同じトランザクションの `Transitioned` が止める。発行は R3b）。
    PlanApprovalRequested {
        plan_id: String,
        reasons: Vec<String>,
    },
    /// ADR-0121 D3: 対象案件の root で delivery（main への取り込み）を開始できなかった。状態は変えない
    /// 監査イベント。(task, reason, head) ごとに高々 1 件（`head = null` は未解決の head）。
    DeliverySkipped {
        reason: crate::DeliverySkipReason,
        detail: String,
        #[serde(default)]
        head: Option<String>,
    },
    /// ADR-0131 付記（2026-10-04）: 日次整理の計画を人の承認なしで本番 KB に適用し、変更を 1 commit にして
    /// KB の remote へ push した（試みた）。状態は変えない監査イベント。`commit_sha` は commit できなかったとき
    /// `None`（理由は `commit_error`）。`push` は `pushed`・`no_remote`・`failed`・`skipped`（commit できず
    /// push を省いた）のどれか。push の失敗は apply を失敗にしない（次回の push でまとめて送る）。
    KnowledgeCurationApplied {
        /// 適用日（job の timezone の `YYYY-MM-DD`）。
        date: String,
        merged: u32,
        new: u32,
        deleted: u32,
        fixed: u32,
        #[serde(default)]
        commit_sha: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        commit_error: Option<String>,
        push: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        push_detail: Option<String>,
    },
    /// ADR parallel integration D4: 統合の依頼。状態は変えない。
    // serde の type は `integration_requested`（migration 0047 の部分 index と一致）。
    IntegrationRequested {
        request: Box<crate::integration_request::IntegrationRequest>,
        origin: String,
    },
    /// ADR parallel integration D4: 人が依頼に回答した監査事象。
    // serde の type は `integration_answered`（migration 0047 の部分 index と一致）。
    IntegrationAnswered {
        request_id: String,
        answer: String,
        #[serde(default)]
        note: Option<String>,
    },
    /// ADR 2026-10-06 D4: モデル catalog の変化（追加・消失・復活）。task に属さない監査イベントで、
    /// `model_catalog::catalog_event_task_id()` の疑似 task の列に追記する。状態は変えない。
    ModelCatalogChanged {
        source: String,
        #[serde(default)]
        added: Vec<String>,
        #[serde(default)]
        removed: Vec<String>,
        #[serde(default)]
        restored: Vec<String>,
    },
    /// ADR 2026-10-06 model-role-assignments D1: source × 役割の割り当ての変更（設定・解除）。
    /// `model_id` は変更後（解除なら `None`）、`previous` は変更前（無ければ `None`）。`actor` は呼び手。
    /// `catalog_event_task_id()` の疑似 task の列に追記する。
    ModelRoleAssignmentChanged {
        source: String,
        tier: Tier,
        #[serde(default)]
        model_id: Option<String>,
        #[serde(default)]
        previous: Option<String>,
        actor: String,
    },
    /// ADR 2026-10-07-browser-trusted-devices D5: 信頼できる端末を登録した。秘密も hash も載せない。
    /// `trusted_device::trusted_device_event_task_id()` の疑似 task の列に追記する。状態は変えない。
    TrustedDeviceRegistered {
        device_id: String,
        name: String,
        method: crate::trusted_device::TrustedDeviceMethod,
        actor: String,
        expires_at: i64,
        #[serde(default)]
        absolute_expires_at: Option<i64>,
    },
    /// 同 D5: 登録端末で owner session を作り直した（秘密を回転し期限を延長した）。`expires_at` は延長後。
    TrustedDeviceUsed {
        device_id: String,
        actor: String,
        expires_at: i64,
    },
    /// 同 D5: 端末を失効させた（人の失効か、回転前の秘密の再提示）。
    TrustedDeviceRevoked {
        device_id: String,
        actor: String,
        reason: crate::trusted_device::TrustedDeviceRevokeReason,
    },
    /// 同 D5: 端末の検証・登録を拒否した。`device_id` は実在する id のときだけ（攻撃者が選んだ文字列を書かない）。
    TrustedDeviceRejected {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        device_id: Option<String>,
        actor: String,
        reason: crate::trusted_device::TrustedDeviceRejectReason,
    },
    /// ADR-0079 D10: 木の節点が「走っている / 走れる / 名指しの待ち」のどれでもないまま
    /// `liveness_timeout_secs`（D10 の `stall_secs`）続いた。状態は変えない（Phase R3b で発行。同じ止まり方には
    /// 1 回だけ）。
    StallDetected {
        task_id: TaskId,
        detail: String,
        /// Phase R3b: 分類の理由（`task_core::tree::NodeLiveness.reason`。例 `child_missing`）。
        #[serde(default, skip_serializing_if = "String::is_empty")]
        reason: String,
        /// Phase R3b: 理由なく止まっていると daemon が最初に見た時刻（RFC 3339）。
        #[serde(default, skip_serializing_if = "String::is_empty")]
        since: String,
        /// Phase R3b: 木の中の位置（root からこの節点まで。決定の要求の path と同じ形）。
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        path: Vec<crate::decision::DecisionPathEntry>,
    },
}

/// `Event::ProjectPlanProposed.milestones[]`（ADR-0074 D3.3）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProposedMilestone {
    pub key: String,
    pub milestone_id: crate::org::MilestoneId,
    pub task_id: TaskId,
}

/// `Event::PhaseIntegrated.merged[]`（ADR-0074 D1.4）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PhaseMerged {
    pub key: String,
    /// merge した WU ブランチの HEAD。
    pub commit: String,
    /// 既に Task ブランチに入っていたので飛ばした（冪等なやり直し）。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub skipped: bool,
    /// ADR-0118 D5: 照合した子の review 時の target（親ブランチ）の SHA（記録の無い子・WU は `None`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_sha: Option<String>,
    /// ADR-0118 D5: 子の merge candidate を merge したときの親ブランチの HEAD（merge の直前）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_head: Option<String>,
}

/// `Event::PhaseIntegrated.checks[]`（ADR-0074 D1.4 の 4）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PhaseCheckResult {
    pub cmd: String,
    pub pass: bool,
    pub summary: String,
}

impl Event {
    /// 従来どおりの文字列だけの進行（ADR-0048 D2 の構造化フィールドは付けない）。
    pub fn worker_progress(run_id: impl Into<String>, msg: impl Into<String>) -> Self {
        Event::WorkerProgress {
            run_id: run_id.into(),
            msg: msg.into(),
            kind: None,
            tool: None,
            summary: None,
            detail: None,
            truncated: false,
            error: false,
        }
    }

    /// ADR-0048 D2: 構造化した進行。
    pub fn worker_progress_with(
        run_id: impl Into<String>,
        msg: impl Into<String>,
        fields: ProgressFields,
    ) -> Self {
        Event::WorkerProgress {
            run_id: run_id.into(),
            msg: msg.into(),
            kind: fields.kind,
            tool: fields.tool,
            summary: fields.summary,
            detail: fields.detail,
            truncated: fields.truncated,
            error: fields.error,
        }
    }

    /// ADR-0048 D2: `WorkerProgress` の構造化フィールドを取り出す（他のイベントは `None`）。
    pub fn progress_fields(&self) -> Option<ProgressFields> {
        match self {
            Event::WorkerProgress {
                kind,
                tool,
                summary,
                detail,
                truncated,
                error,
                ..
            } => Some(ProgressFields {
                kind: *kind,
                tool: tool.clone(),
                summary: summary.clone(),
                detail: detail.clone(),
                truncated: *truncated,
                error: *error,
            }),
            _ => None,
        }
    }
}
