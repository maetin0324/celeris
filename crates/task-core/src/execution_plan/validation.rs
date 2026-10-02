//! Plan validation, limits, and deterministic topological ordering.

use super::*;

// ---------------------------------------------------------------------------
// D14: 検証
// ---------------------------------------------------------------------------

/// D18: 検証・丸めに使う上限（既定値は ADR-0072 D18 の表）。
/// ADR-0074 D5.3（Phase F1）: 計画のサイズ上限（§4）を足す。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutionLimits {
    pub max_work_units: usize,
    pub work_unit_max_turns: u32,
    pub work_unit_max_wall_secs: u64,
    /// `rationale` の文字数上限（既定 1,500）。
    pub max_rationale_chars: usize,
    /// WU の `title` の文字数上限（既定 120）。
    pub max_title_chars: usize,
    /// WU の `objective` の文字数上限（既定 2,000）。
    pub max_objective_chars: usize,
    /// WU の `done_when` の件数上限（既定 8）。
    pub max_done_when_items: usize,
    /// WU の `done_when` の 1 件あたりの文字数上限（既定 300）。
    pub max_done_when_chars: usize,
    /// WU の `checks` の件数上限（既定 6）。
    pub max_checks: usize,
    /// 計画の JSON 全体の大きさの上限（バイト、既定 24 KiB）。/1・/2 の検証が見る。
    pub max_plan_json_bytes: usize,
    /// ADR-0079 R7-2: `celeris.execution-plan/3` の JSON 全体の大きさの上限（バイト、既定 64 KiB）。/3 は段階・
    /// 決定・done の unit の持ち越しを 1 つの JSON に書くので /1・/2 の 24 KiB では足りない（本番 24815 > 24576）。
    pub max_plan_json_bytes_v3: usize,
    /// ADR-0074 §4（Phase F2）: `celeris.execution-plan/2` の `work_units` の件数上限（既定 10。
    /// 統合 WU・repair は数えない）。`max_work_units` は v1 専用のまま（既定 8。§4 の表）。
    pub max_work_units_v2: usize,
    /// ADR-0074 §4（Phase F2）: `phases` の件数上限（既定 5）。
    pub max_phases: usize,
    /// ADR-0074 D3.7（Phase F4b (f)）: `children` の件数上限（既定 8 = 委譲の 1 run あたりの上限）。
    pub max_children: usize,
    /// ADR-0079 D3（Phase R1a）: `[execution.tree]` の上限。`celeris.execution-plan/3` の検証だけが
    /// 見る（/1・/2 の検証・丸めは 1 バイトも変えない）。
    pub tree: crate::tree::TreeLimits,
}

impl ExecutionLimits {
    /// replay（採用済みの計画のトポロジカル順の復元）用: 形の検査だけをし、上限では拒否しない。
    pub fn permissive() -> Self {
        ExecutionLimits {
            max_children: 8,
            max_work_units: usize::MAX,
            work_unit_max_turns: u32::MAX,
            work_unit_max_wall_secs: u64::MAX,
            max_rationale_chars: usize::MAX,
            max_title_chars: usize::MAX,
            max_objective_chars: usize::MAX,
            max_done_when_items: usize::MAX,
            max_done_when_chars: usize::MAX,
            max_checks: usize::MAX,
            max_plan_json_bytes: usize::MAX,
            max_plan_json_bytes_v3: usize::MAX,
            max_work_units_v2: usize::MAX,
            max_phases: usize::MAX,
            tree: crate::tree::TreeLimits::permissive(),
        }
    }
}

impl Default for ExecutionLimits {
    fn default() -> Self {
        ExecutionLimits {
            max_work_units: 8,
            work_unit_max_turns: 80,
            work_unit_max_wall_secs: 3600,
            max_rationale_chars: 1_500,
            max_title_chars: 120,
            max_objective_chars: 2_000,
            max_done_when_items: 8,
            max_done_when_chars: 300,
            max_checks: 6,
            max_plan_json_bytes: 24 * 1024,
            max_plan_json_bytes_v3: 64 * 1024,
            max_work_units_v2: 10,
            max_phases: 5,
            max_children: 8,
            tree: crate::tree::TreeLimits::default(),
        }
    }
}

/// D14: 検証エラー（すべて拒否理由。1 回だけ再試行し、それでも駄目なら atomic に倒す。呼び出し側の責務）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanValidationError {
    /// `schema` 欄が `celeris.execution-plan/1` でも `/2`（ADR-0074 D1.1、Phase F2）でもない。
    WrongSchema {
        found: String,
    },
    NoWorkUnits,
    TooManyWorkUnits {
        count: usize,
        max: usize,
    },
    InvalidKey {
        key: String,
    },
    DuplicateKey {
        key: String,
    },
    UnknownDependency {
        key: String,
        depends_on: String,
    },
    CyclicDependency {
        cycle: Vec<String>,
    },
    /// title の正規化一致、または objective のトークン Jaccard ≥ 0.9。
    DuplicateWorkUnit {
        a: String,
        b: String,
        reason: String,
    },
    /// replan（D14）: done の WU の key/spec が変わっている。E2 では replan を発行しないので、
    /// 呼び出し側が既存の done WU を渡したときだけ検査する。
    DoneWorkUnitChanged {
        key: String,
    },
    /// ADR-0079 R5b-fix1: 人の replan（origin human）は done の WU の spec を上書きできるが、構造の欄
    /// （`kind` / `phase`〈/3 の段階〉/ `depends_on`）は変えられない。`field` は変わった欄の名前。
    DoneWorkUnitStructureChanged {
        key: String,
        field: &'static str,
    },
    /// ADR-0074 D5.1（Phase F1）: WU の `features` が `TaskFeatureHints` として読めない
    /// （`deny_unknown_fields` を含む型として不正）。
    InvalidFeatures {
        key: String,
        detail: String,
    },
    InvalidWritePaths {
        key: String,
        detail: String,
    },
    /// ADR-0074 D5.3（Phase F1）: `rationale` が上限を超える。
    RationaleTooLong {
        len: usize,
        max: usize,
    },
    /// ADR-0074 D5.3（Phase F1）: WU の `title` が上限を超える。
    TitleTooLong {
        key: String,
        len: usize,
        max: usize,
    },
    /// ADR-0074 D5.3（Phase F1）: WU の `objective` が上限を超える。
    ObjectiveTooLong {
        key: String,
        len: usize,
        max: usize,
    },
    /// ADR-0074 D5.3（Phase F1）: WU の `done_when` の件数が上限を超える。
    TooManyDoneWhen {
        key: String,
        count: usize,
        max: usize,
    },
    /// ADR-0074 D5.3（Phase F1）: WU の `done_when` の 1 件が上限を超える。
    DoneWhenItemTooLong {
        key: String,
        index: usize,
        len: usize,
        max: usize,
    },
    /// ADR-0074 D5.3（Phase F1）: WU の `checks` の件数が上限を超える。
    TooManyChecks {
        key: String,
        count: usize,
        max: usize,
    },
    /// ADR-0074 D5.3（Phase F1）: 計画の JSON 全体が上限を超える。
    PlanTooLarge {
        bytes: usize,
        max: usize,
    },
    /// ADR-0074 D1.1（Phase F2）: `celeris.execution-plan/1` に `phases` が書かれている
    /// （v1 は工程を持たない。1 バイトも変えない）。
    PhasesNotAllowedInV1,
    /// ADR-0074 D1.1（Phase F2）: `celeris.execution-plan/2` に `phases` が 1 つも無い。
    NoPhases,
    /// ADR-0074 §4（Phase F2）: `phases` の件数が上限を超える。
    TooManyPhases {
        count: usize,
        max: usize,
    },
    /// ADR-0074 D1.1（Phase F2）: 工程の `key` が `[a-z0-9-]{1,32}` に合わない。
    InvalidPhaseKey {
        key: String,
    },
    /// ADR-0074 D1.1（Phase F2）: 工程の `key` が重複している。
    DuplicatePhaseKey {
        key: String,
    },
    /// ADR-0074 D1.1（Phase F2）: `celeris.execution-plan/1` の WorkUnit に `phase` が書かれている
    /// （v1 は工程を持たない）。
    WorkUnitPhaseNotAllowedInV1 {
        key: String,
    },
    /// ADR-0074 D1.1（Phase F2）: `celeris.execution-plan/2` の WorkUnit に `phase` が無い（必須）。
    WorkUnitMissingPhase {
        key: String,
    },
    /// ADR-0074 D1.1（Phase F2）: WorkUnit の `phase` が `phases` に無い key を指している。
    UnknownWorkUnitPhase {
        key: String,
        phase: String,
    },
    /// ADR-0074 D1.1（Phase F2）: 依存先が自分より後の工程にある（拒否。D1.1 の規則 1）。
    DependencyInLaterPhase {
        key: String,
        depends_on: String,
    },
    /// ADR-0074 D1.1（Phase F2）: 同じ工程の中の依存が 2 つ以上ある（規則 2。鎖か木のみ許す）。
    TooManyIntraPhaseDependencies {
        key: String,
        phase: String,
    },
    /// ADR-0074 D3.7: `children` は `celeris.execution-plan/2` だけ（v1 では空でなければならない）。
    NonEmptyChildren,
    /// ADR-0074 D3.7（Phase F4b (f)）: 子の件数・key・依存・受け入れ条件。
    TooManyChildren {
        count: usize,
        max: usize,
    },
    InvalidChildKey {
        key: String,
    },
    DuplicateChildKey {
        key: String,
    },
    UnknownChildDependency {
        key: String,
        depends_on: String,
    },
    CyclicChildDependency {
        cycle: Vec<String>,
    },
    ChildNoAcceptance {
        key: String,
    },
    ChildInvalidAcceptance {
        key: String,
        detail: String,
    },
    /// ADR-0074 D1.4（Phase F2）: `kind = integrate` は daemon が足す system WU 専用の予約語で、
    /// 計画（planner・人）が自分の WorkUnit にこの kind を書くことはできない。
    ReservedKind {
        key: String,
    },
    /// ADR-0074 D1.4（Phase F2b）: `integrate-` で始まる key は daemon が足す統合 WU
    /// （`integrate-<phase>`）の予約語。
    ReservedKey {
        key: String,
    },
    // ---- ADR-0079（Phase R1a）: /1・/2 に /3 の欄・語彙が書かれている ----
    /// `stages` / `units` / `decisions` は /3 だけ。
    V3FieldNotAllowed {
        field: &'static str,
    },
    /// `kind = task` は /3 だけ。
    TaskKindRequiresV3 {
        key: String,
    },
    // ---- ADR-0079 D2 / D3 / D4（Phase R1a）: /3 の検証 ----
    /// `[execution.tree] enabled = false` なので /3 は採用できない。
    TreeDisabled,
    /// /3 に /2 の欄（`phases` / `work_units` / `children`）が書かれている。
    V2FieldInV3 {
        field: &'static str,
    },
    NoStages,
    TooManyStages {
        count: usize,
        max: usize,
    },
    InvalidStageKey {
        key: String,
    },
    DuplicateStageKey {
        key: String,
    },
    /// 段階の kind に `task` / `integrate` は使えない。
    InvalidStageKind {
        key: String,
    },
    NoUnits,
    UnknownUnitStage {
        key: String,
        stage: String,
    },
    TooManyUnitsInStage {
        stage: String,
        count: usize,
        max: usize,
    },
    TooManyChildTasks {
        count: usize,
        max: usize,
    },
    /// ADR-0079 付記「R7-3」D3: 退役した（superseded / cancelled の）行の key の再利用。`stage` は段階の統合 WU の key
    /// `integrate-<stage>` が退役した統合 WU と重なった（前の版で消した段階の key を戻した）ときの段階。
    RetiredKeyReused {
        key: String,
        stage: Option<String>,
    },
    /// ADR-0079 付記「R7-12」D3: daemon の足した生きた WU（配送 / 最終レビュー / 統合の repair WU）の key を計画に書いた。
    DaemonAddedKeyReused {
        key: String,
    },
    /// この深さの task の計画は kind task の unit を持てない（U-R1: `depth < max_depth` のときだけ）。
    ChildTaskTooDeep {
        key: String,
        depth: u32,
        max_depth: u32,
    },
    /// /2 の `child:<key>` は /3 では書けない（kind task の unit を依存先にする）。
    ChildDependencyNotAllowed {
        key: String,
        depends_on: String,
    },
    TaskUnitNoAcceptance {
        key: String,
    },
    TaskUnitInvalidAcceptance {
        key: String,
        detail: String,
    },
    /// kind task の unit に `checks` / `budget` / `harness` / `context.paths` が書かれている。
    TaskUnitFieldNotAllowed {
        key: String,
        field: &'static str,
    },
    /// leaf に kind task 専用の欄（`acceptance` / `genre` / `skills` / `repos` / `adopt` / `gate`）が書かれている。
    LeafFieldNotAllowed {
        key: String,
        field: &'static str,
    },
    /// D4 (2) (c): leaf に機械的な検査が無い。
    LeafWithoutChecks {
        key: String,
    },
    /// D4 (2) (b): leaf の `context.repo` が 2 つ以上。
    LeafMultipleRepos {
        key: String,
        count: usize,
    },
    /// D4 (2) (a): leaf の予算が 1 run の上限を超える（/3 は丸めずに拒否する）。
    LeafBudgetOverLimit {
        key: String,
        detail: String,
    },
    /// D2 / D15: `adopt` は人の計画（origin human）だけ。
    AdoptNotAllowed {
        key: String,
    },
    /// D7: 決定の形の誤り。
    InvalidDecision {
        detail: String,
    },
    DuplicateDecisionKey {
        key: String,
    },
    /// D7: `needed_before` が計画に無い unit / 段階を指している。
    UnknownNeededBefore {
        key: String,
        target: String,
    },
    /// D2: `needs_decisions` が計画に無い決定を指している。
    UnknownNeedsDecision {
        key: String,
        decision: String,
    },
    /// D3 / D7: 計画あたりの未回答の決定の上限。
    TooManyDecisions {
        count: usize,
        max: usize,
    },
}

impl std::fmt::Display for PlanValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanValidationError::WrongSchema { found } => {
                write!(
                    f,
                    "schema must be {EXECUTION_PLAN_SCHEMA}, {EXECUTION_PLAN_SCHEMA_V2} or {EXECUTION_PLAN_SCHEMA_V3}, found {found}"
                )
            }
            PlanValidationError::NoWorkUnits => write!(f, "work_units must not be empty"),
            PlanValidationError::TooManyWorkUnits { count, max } => {
                write!(f, "too many work_units: {count} > {max}")
            }
            PlanValidationError::InvalidKey { key } => {
                write!(
                    f,
                    "invalid work unit key: {key:?} (must match [a-z0-9-]{{1,32}})"
                )
            }
            PlanValidationError::DuplicateKey { key } => {
                write!(f, "duplicate work unit key: {key}")
            }
            PlanValidationError::UnknownDependency { key, depends_on } => {
                write!(f, "work unit {key} depends on unknown key {depends_on}")
            }
            PlanValidationError::CyclicDependency { cycle } => {
                write!(f, "cyclic dependency: {}", cycle.join(" -> "))
            }
            PlanValidationError::DuplicateWorkUnit { a, b, reason } => {
                write!(f, "work units {a} and {b} look like duplicates ({reason})")
            }
            PlanValidationError::DoneWorkUnitChanged { key } => {
                write!(
                    f,
                    "done work unit {key} must not change on replan（done の WU は差分に書かない・全体形式なら旧版のまま写す。{DAEMON_ADDED_HINT}。planner が done の WU で直せるのは `checks` だけ〈段階の統合で再実行される。ADR-0079 R7-3〉。それ以外の欄の誤りを直す必要があるなら、planner は直さずに質問で人に伝える: 人は `PUT /tasks/{{id}}/execution-plan`〈origin human の replan〉で done の WU の spec を上書きできる〈ADR-0079 R5b-fix1〉）"
                )
            }
            PlanValidationError::DoneWorkUnitStructureChanged { key, field } => {
                write!(
                    f,
                    "done work unit {key}: {field} must not change on replan (a human replan may override the spec of a done work unit, but not its kind / phase / stage / depends_on, and may not remove it; ADR-0079 R5b-fix1)"
                )
            }
            PlanValidationError::InvalidFeatures { key, detail } => {
                write!(
                    f,
                    "work unit {key}: features must parse as TaskFeatureHints: {detail}"
                )
            }
            PlanValidationError::InvalidWritePaths { key, detail } => {
                write!(f, "work unit {key}: expected_write_paths: {detail}")
            }
            PlanValidationError::RationaleTooLong { len, max } => {
                write!(f, "rationale is too long: {len} > {max} characters")
            }
            PlanValidationError::TitleTooLong { key, len, max } => {
                write!(
                    f,
                    "work unit {key}: title is too long: {len} > {max} characters"
                )
            }
            PlanValidationError::ObjectiveTooLong { key, len, max } => {
                write!(
                    f,
                    "work unit {key}: objective is too long: {len} > {max} characters"
                )
            }
            PlanValidationError::TooManyDoneWhen { key, count, max } => {
                write!(
                    f,
                    "work unit {key}: too many done_when items: {count} > {max}"
                )
            }
            PlanValidationError::DoneWhenItemTooLong {
                key,
                index,
                len,
                max,
            } => {
                write!(
                    f,
                    "work unit {key}: done_when[{index}] is too long: {len} > {max} characters"
                )
            }
            PlanValidationError::TooManyChecks { key, count, max } => {
                write!(f, "work unit {key}: too many checks: {count} > {max}")
            }
            PlanValidationError::PlanTooLarge { bytes, max } => {
                // ADR-0079 R7-2: planner が何を削ればよいかを添える（拒否の文は次の試行の planner に渡る）。
                write!(
                    f,
                    "execution plan JSON is too large: {bytes} > {max} bytes \
                     (objective は要点だけにし、詳細は artifacts / 知識ベースのパスで参照してください)"
                )
            }
            PlanValidationError::PhasesNotAllowedInV1 => {
                write!(f, "phases must be empty in {EXECUTION_PLAN_SCHEMA}")
            }
            PlanValidationError::NoPhases => {
                write!(f, "phases must not be empty in {EXECUTION_PLAN_SCHEMA_V2}")
            }
            PlanValidationError::TooManyPhases { count, max } => {
                write!(f, "too many phases: {count} > {max}")
            }
            PlanValidationError::InvalidPhaseKey { key } => {
                write!(
                    f,
                    "invalid phase key: {key:?} (must match [a-z0-9-]{{1,32}})"
                )
            }
            PlanValidationError::DuplicatePhaseKey { key } => {
                write!(f, "duplicate phase key: {key}")
            }
            PlanValidationError::WorkUnitPhaseNotAllowedInV1 { key } => {
                write!(
                    f,
                    "work unit {key}: phase must not be set in {EXECUTION_PLAN_SCHEMA}"
                )
            }
            PlanValidationError::WorkUnitMissingPhase { key } => {
                write!(
                    f,
                    "work unit {key}: phase is required in {EXECUTION_PLAN_SCHEMA_V2}"
                )
            }
            PlanValidationError::UnknownWorkUnitPhase { key, phase } => {
                write!(f, "work unit {key}: unknown phase {phase:?}")
            }
            PlanValidationError::DependencyInLaterPhase { key, depends_on } => {
                write!(
                    f,
                    "work unit {key}: depends on {depends_on}, which is in a later phase"
                )
            }
            PlanValidationError::TooManyIntraPhaseDependencies { key, phase } => {
                write!(
                    f,
                    "work unit {key}: depends on more than one work unit within phase {phase} \
                     (at most one intra-phase dependency is allowed)"
                )
            }
            PlanValidationError::NonEmptyChildren => {
                write!(
                    f,
                    "children are only allowed in {EXECUTION_PLAN_SCHEMA_V2} (must be empty in v1)"
                )
            }
            PlanValidationError::TooManyChildren { count, max } => {
                write!(f, "too many children: {count} > {max}")
            }
            PlanValidationError::InvalidChildKey { key } => {
                write!(
                    f,
                    "invalid child key: {key:?} (must match [a-z0-9-]{{1,32}})"
                )
            }
            PlanValidationError::DuplicateChildKey { key } => {
                write!(f, "duplicate child key: {key}")
            }
            PlanValidationError::UnknownChildDependency { key, depends_on } => {
                write!(f, "child {key} depends on unknown child {depends_on}")
            }
            PlanValidationError::CyclicChildDependency { cycle } => {
                write!(f, "cyclic child dependency: {}", cycle.join(" -> "))
            }
            PlanValidationError::ChildNoAcceptance { key } => {
                write!(f, "child {key}: acceptance must not be empty")
            }
            PlanValidationError::ChildInvalidAcceptance { key, detail } => {
                write!(f, "child {key}: {detail}")
            }
            PlanValidationError::ReservedKind { key } => {
                write!(
                    f,
                    "work unit {key}: kind \"integrate\" is reserved for daemon-created integration work units（{DAEMON_ADDED_HINT}）"
                )
            }
            PlanValidationError::ReservedKey { key } => {
                write!(
                    f,
                    "work unit {key}: keys starting with \"{INTEGRATE_KEY_PREFIX}\" are reserved for daemon-created integration work units（{DAEMON_ADDED_HINT}）"
                )
            }
            PlanValidationError::V3FieldNotAllowed { field } => write!(
                f,
                "{field} is only allowed in {EXECUTION_PLAN_SCHEMA_V3} (must be empty in v1/v2)"
            ),
            PlanValidationError::TaskKindRequiresV3 { key } => write!(
                f,
                "work unit {key}: kind \"task\" (a child task unit) is only allowed in {EXECUTION_PLAN_SCHEMA_V3}"
            ),
            PlanValidationError::TreeDisabled => write!(
                f,
                "{EXECUTION_PLAN_SCHEMA_V3} (recursive task decomposition, ADR-0079) is disabled: \
                 set [execution.tree] enabled = true to adopt it, or write a {EXECUTION_PLAN_SCHEMA_V2} plan"
            ),
            PlanValidationError::V2FieldInV3 { field } => {
                let hint = match *field {
                    "children" => "use units with kind \"task\" instead",
                    "phases" => "use stages instead",
                    _ => "use units instead",
                };
                write!(
                    f,
                    "{field} is not allowed in {EXECUTION_PLAN_SCHEMA_V3} ({hint})"
                )
            }
            PlanValidationError::NoStages => {
                write!(f, "stages must not be empty in {EXECUTION_PLAN_SCHEMA_V3}")
            }
            PlanValidationError::TooManyStages { count, max } => {
                write!(f, "too many stages: {count} > {max}")
            }
            PlanValidationError::InvalidStageKey { key } => write!(
                f,
                "invalid stage key: {key:?} (must match [a-z0-9-]{{1,32}})"
            ),
            PlanValidationError::DuplicateStageKey { key } => {
                write!(f, "duplicate stage key: {key}")
            }
            PlanValidationError::InvalidStageKind { key } => write!(
                f,
                "stage {key}: kind \"task\" / \"integrate\" cannot be used for a stage"
            ),
            PlanValidationError::NoUnits => {
                write!(f, "units must not be empty in {EXECUTION_PLAN_SCHEMA_V3}")
            }
            PlanValidationError::UnknownUnitStage { key, stage } => {
                write!(f, "unit {key}: unknown stage {stage:?}")
            }
            PlanValidationError::TooManyUnitsInStage { stage, count, max } => write!(
                f,
                "stage {stage}: too many units: {count} > {max} (leaf + task; units already done and adopt units do not count; split the stage or group units into a child task)"
            ),
            PlanValidationError::TooManyChildTasks { count, max } => {
                write!(f, "too many units with kind \"task\": {count} > {max}")
            }
            PlanValidationError::RetiredKeyReused { key, stage: None } => write!(
                f,
                "work unit key {key:?} was used by a superseded work unit and cannot be reused; choose a new key（superseded の unit の key は再利用できない。新しい key を選ぶこと。ADR-0079 R7-3）"
            ),
            PlanValidationError::RetiredKeyReused {
                key,
                stage: Some(stage),
            } => write!(
                f,
                "stage key {stage:?} was used by a stage removed in an earlier plan version (its integration work unit {key} is superseded) and cannot be reused; choose a new stage key（前の版で消した段階の key は再利用できない。新しい段階の key を選ぶこと。ADR-0079 R7-3）"
            ),
            PlanValidationError::DaemonAddedKeyReused { key } => write!(
                f,
                "work unit key {key:?} belongs to a work unit the daemon added (a delivery / final-review / integration repair work unit, not in the plan); do not write it — the daemon carries it over（{key} は daemon が足した WU〈計画に無い repair WU〉なので計画に書かない。daemon が持ち越す。ADR-0079 R7-12）"
            ),
            PlanValidationError::ChildTaskTooDeep {
                key,
                depth,
                max_depth,
            } => write!(
                f,
                "unit {key}: a task at depth {depth} cannot have child task units (max_depth = {max_depth} task levels); make it a leaf"
            ),
            PlanValidationError::ChildDependencyNotAllowed { key, depends_on } => write!(
                f,
                "unit {key}: depends_on {depends_on:?} uses the {EXECUTION_PLAN_SCHEMA_V2} \"{CHILD_DEP_PREFIX}\" prefix; depend on the key of a unit with kind \"task\" instead"
            ),
            PlanValidationError::TaskUnitNoAcceptance { key } => write!(
                f,
                "unit {key}: a unit with kind \"task\" must have at least one acceptance criterion"
            ),
            PlanValidationError::TaskUnitInvalidAcceptance { key, detail } => {
                write!(f, "unit {key}: {detail}")
            }
            PlanValidationError::TaskUnitFieldNotAllowed { key, field } => write!(
                f,
                "unit {key}: a unit with kind \"task\" must not set {field} (the child task decides it)"
            ),
            PlanValidationError::LeafFieldNotAllowed { key, field } => write!(
                f,
                "unit {key}: {field} is only allowed on a unit with kind \"task\""
            ),
            PlanValidationError::LeafWithoutChecks { key } => write!(
                f,
                "unit {key}: a leaf must have at least one mechanical check (make it a unit with kind \"task\" or add checks)"
            ),
            PlanValidationError::LeafMultipleRepos { key, count } => write!(
                f,
                "unit {key}: a leaf works in at most one repository, found {count} in context.repo (make it a unit with kind \"task\" or split it)"
            ),
            PlanValidationError::LeafBudgetOverLimit { key, detail } => write!(
                f,
                "unit {key}: {detail} exceeds what one run can do (make it a unit with kind \"task\" or shrink it)"
            ),
            PlanValidationError::AdoptNotAllowed { key } => write!(
                f,
                "unit {key}: adopt is only allowed in a plan written by a human (origin human) (a done unit copied verbatim from the previous version may keep its adopt)"
            ),
            PlanValidationError::InvalidDecision { detail } => write!(f, "{detail}"),
            PlanValidationError::DuplicateDecisionKey { key } => {
                write!(f, "duplicate decision key: {key}")
            }
            PlanValidationError::UnknownNeededBefore { key, target } => write!(
                f,
                "decision {key}: needed_before {target:?} is neither a unit key nor stage:<key> of this plan"
            ),
            PlanValidationError::UnknownNeedsDecision { key, decision } => {
                write!(
                    f,
                    "unit {key}: needs_decisions names unknown decision {decision}"
                )
            }
            PlanValidationError::TooManyDecisions { count, max } => {
                write!(f, "too many decisions in one plan: {count} > {max}")
            }
        }
    }
}

impl std::error::Error for PlanValidationError {}

fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 32
        && key
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn normalize_title(title: &str) -> String {
    title
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect()
}

fn token_set(text: &str) -> BTreeSet<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

fn jaccard(a: &BTreeSet<String>, b: &BTreeSet<String>) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let intersection = a.intersection(b).count();
    let union = a.union(b).count();
    if union == 0 {
        0.0
    } else {
        intersection as f64 / union as f64
    }
}

/// D14: 検証を通った計画（budget を D18 の上限に丸めた写しと、丸めた記録）。
#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedPlan {
    pub spec: ExecutionPlanSpec,
    /// 丸めたことの記録（`work unit <key>: max_turns 120 -> 80` のような 1 行ずつ）。
    pub rounding_notes: Vec<String>,
    /// トポロジカル順（`work_units` の index）。`work_units.seq` に使う。
    pub topological_order: Vec<usize>,
}

/// ADR-0079 R5b-fix1: /3 の unit が、前の版の done の unit をそのまま写したものか（内部の形
/// 〈[`PlanUnitSpec::to_work_unit_spec`]〉が done の spec と一致する）。
/// ADR-0079 付記「R7-3」D1: `checks` だけが違う写しも done の写しとして扱う（planner の replan は done の unit の
/// `checks` を書き換えられる）。
fn is_done_carry_over(unit: &PlanUnitSpec, done_work_units: &[(String, WorkUnitSpec)]) -> bool {
    done_work_units
        .iter()
        .any(|(k, s)| *k == unit.key && same_except_checks(s, &unit.to_work_unit_spec()))
}

/// ADR-0079 付記「R7-3」D1: 2 つの WU の spec が `checks` の他は同じか。
pub fn same_except_checks(a: &WorkUnitSpec, b: &WorkUnitSpec) -> bool {
    let mut b = b.clone();
    b.checks = a.checks.clone();
    *a == b
}

/// replan の done の不変条件（D14 / D17）。done の WU は新しい計画に同じ key で残り、spec も変わらない。
///
/// ADR-0079 R5b-fix1: 人の replan（`origin == Human`）だけは done の WU の spec を上書きできる（人がその仕事は
/// 済んだと言い、記録した spec〈典型的には `checks` のコマンド〉を直す）。ただし消すこと（`DoneWorkUnitChanged`）と、
/// 構造の欄（`kind` / `phase` = /3 の段階 / `depends_on`）を変えること（`DoneWorkUnitStructureChanged`）は人でも拒む。
/// ADR-0079 付記「R7-3」D1: planner の計画は done の WU の **`checks` だけ**を書き換えられる（段階の統合で再実行される
/// check。done の葉の check が統合で成り立たない形〈`HEAD^2` など〉だと、直せないまま同じ check で落ち続けた）。
/// repair / fixture の計画は従来どおり完全一致だけ。
fn done_carry_over_errors(
    work_units: &[WorkUnitSpec],
    done_work_units: &[(String, WorkUnitSpec)],
    origin: PlanOrigin,
) -> Vec<PlanValidationError> {
    let by_key: BTreeMap<&str, &WorkUnitSpec> =
        work_units.iter().map(|w| (w.key.as_str(), w)).collect();
    let mut errors = Vec::new();
    for (key, done_spec) in done_work_units {
        let Some(new_spec) = by_key.get(key.as_str()) else {
            errors.push(PlanValidationError::DoneWorkUnitChanged { key: key.clone() });
            continue;
        };
        if *new_spec == done_spec {
            continue;
        }
        if origin == PlanOrigin::Planner && same_except_checks(done_spec, new_spec) {
            continue;
        }
        if origin != PlanOrigin::Human {
            errors.push(PlanValidationError::DoneWorkUnitChanged { key: key.clone() });
            continue;
        }
        for (field, same) in [
            ("kind", new_spec.kind == done_spec.kind),
            ("phase", new_spec.phase == done_spec.phase),
            ("depends_on", new_spec.depends_on == done_spec.depends_on),
        ] {
            if !same {
                errors.push(PlanValidationError::DoneWorkUnitStructureChanged {
                    key: key.clone(),
                    field,
                });
            }
        }
    }
    errors
}

/// ADR-0079 R5b-fix1: 検証を通った計画（`spec`。/3 は内部の形に写して比べる）で、spec が done の spec から
/// 変わった done の WU（人の replan の上書き）と、変わった欄の名前（`WorkUnitSpec` の JSON の最上位の key、
/// 昇順）。planner の replan では `checks` だけを変えた done の WU（R7-3 D1。検証が他の欄の変更を許さない）。
pub fn done_work_unit_overrides(
    spec: &ExecutionPlanSpec,
    done_work_units: &[(String, WorkUnitSpec)],
) -> Vec<(String, WorkUnitSpec, Vec<String>)> {
    let internal = internal_view(spec);
    let mut out = Vec::new();
    for (key, done_spec) in done_work_units {
        let Some(new_spec) = internal.work_units.iter().find(|w| &w.key == key) else {
            continue;
        };
        if new_spec == done_spec {
            continue;
        }
        let as_map = |w: &WorkUnitSpec| match serde_json::to_value(w) {
            Ok(serde_json::Value::Object(m)) => m,
            _ => serde_json::Map::new(),
        };
        let (old, new) = (as_map(done_spec), as_map(new_spec));
        let fields: BTreeSet<&String> = old.keys().chain(new.keys()).collect();
        let changed: Vec<String> = fields
            .into_iter()
            .filter(|f| old.get(*f) != new.get(*f))
            .cloned()
            .collect();
        out.push((key.clone(), new_spec.clone(), changed));
    }
    out
}

/// D14: 計画を検証する。`done_work_units`（replan で持ち越す既存の done WU の `(key, spec)`）が
/// 空でなければ、それらの key と spec が新しい計画でも変わっていないことを確かめる（E2 では常に空。
/// replan は E4）。
pub fn validate(
    spec: &ExecutionPlanSpec,
    limits: ExecutionLimits,
    done_work_units: &[(String, WorkUnitSpec)],
) -> Result<ValidatedPlan, Vec<PlanValidationError>> {
    validate_with(spec, limits, done_work_units, PlanContext::default())
}

/// ADR-0079（Phase R1a）: 計画を書いた者と、計画を持つ task の木の中の深さ。/3 の検証だけが見る
/// （`adopt` は origin human だけ、kind task の unit は `depth < max_depth` のときだけ）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanContext {
    pub origin: PlanOrigin,
    /// task の層数で数えた深さ（root = 1。`task_core::tree::depth_of`）。
    pub depth: u32,
}

impl Default for PlanContext {
    fn default() -> Self {
        PlanContext {
            origin: PlanOrigin::Planner,
            depth: 1,
        }
    }
}

/// [`validate`] に計画の出どころと深さを添えたもの。/1・/2 は done の不変条件でだけ `ctx.origin` を見る
/// （ADR-0079 R5b-fix1: 人の replan は done の WU の spec を上書きできる。done が空なら [`validate`] と同じ結果）。
pub fn validate_with(
    spec: &ExecutionPlanSpec,
    limits: ExecutionLimits,
    done_work_units: &[(String, WorkUnitSpec)],
    ctx: PlanContext,
) -> Result<ValidatedPlan, Vec<PlanValidationError>> {
    if spec.schema == EXECUTION_PLAN_SCHEMA_V3 {
        return validate_v3(spec, limits, done_work_units, ctx);
    }
    let mut errors = Vec::new();

    // ADR-0074 D1.1（Phase F2）: `schema` の値で v1 / v2 を分ける。どちらでもなければ
    // `WrongSchema` を出すが、それ以外の検査（v1 の形として）は続けて行う（複数のエラーを
    // 一度に返す既存の流儀。E2 のまま）。
    let is_v2 = spec.schema == EXECUTION_PLAN_SCHEMA_V2;
    if spec.schema != EXECUTION_PLAN_SCHEMA && !is_v2 {
        errors.push(PlanValidationError::WrongSchema {
            found: spec.schema.clone(),
        });
    }
    if spec.work_units.is_empty() {
        errors.push(PlanValidationError::NoWorkUnits);
    }
    let max_work_units = if is_v2 {
        limits.max_work_units_v2
    } else {
        limits.max_work_units
    };
    if spec.work_units.len() > max_work_units {
        errors.push(PlanValidationError::TooManyWorkUnits {
            count: spec.work_units.len(),
            max: max_work_units,
        });
    }

    // ADR-0074 D3.7（Phase F4b (f)）: `children` は v2 だけ。件数・key・子同士の依存（循環なし）・
    // 受け入れ条件（1 件以上、human には deliverable）。
    if !is_v2 && !spec.children.is_empty() {
        errors.push(PlanValidationError::NonEmptyChildren);
    }
    let child_keys: BTreeSet<&str> = spec.children.iter().map(|c| c.key.as_str()).collect();
    if is_v2 {
        errors.extend(validate_children(&spec.children, limits));
    }

    // ADR-0074 D1.1（Phase F2）: `phases`（v2 のみ）。
    if is_v2 {
        if spec.phases.is_empty() {
            errors.push(PlanValidationError::NoPhases);
        }
        if spec.phases.len() > limits.max_phases {
            errors.push(PlanValidationError::TooManyPhases {
                count: spec.phases.len(),
                max: limits.max_phases,
            });
        }
        let mut seen_phase_keys: BTreeSet<&str> = BTreeSet::new();
        for p in &spec.phases {
            if !valid_key(&p.key) {
                errors.push(PlanValidationError::InvalidPhaseKey { key: p.key.clone() });
                continue;
            }
            if !seen_phase_keys.insert(p.key.as_str()) {
                errors.push(PlanValidationError::DuplicatePhaseKey { key: p.key.clone() });
            }
        }
    } else if !spec.phases.is_empty() {
        errors.push(PlanValidationError::PhasesNotAllowedInV1);
    }

    // ADR-0074 D1.4（Phase F2）: `kind = integrate` は system WU 専用の予約語。
    for wu in &spec.work_units {
        if wu.kind == WorkUnitKind::Integrate {
            errors.push(PlanValidationError::ReservedKind {
                key: wu.key.clone(),
            });
        }
        // ADR-0074 D1.4（Phase F2b）: `integrate-<phase>` の key も予約（v2 のみ。v1 は不変）。
        if spec.schema == EXECUTION_PLAN_SCHEMA_V2 && wu.key.starts_with(INTEGRATE_KEY_PREFIX) {
            errors.push(PlanValidationError::ReservedKey {
                key: wu.key.clone(),
            });
        }
        // ADR-0079（Phase R1a）: `kind = task`（子 task の unit）は /3 だけ。
        if wu.kind == WorkUnitKind::Task {
            errors.push(PlanValidationError::TaskKindRequiresV3 {
                key: wu.key.clone(),
            });
        }
    }
    // ADR-0079（Phase R1a）: /3 の欄は /1・/2 では空（`serde(default)` で読めるが採用しない）。
    for (field, present) in [
        ("stages", !spec.stages.is_empty()),
        ("units", !spec.units.is_empty()),
        ("decisions", !spec.decisions.is_empty()),
    ] {
        if present {
            errors.push(PlanValidationError::V3FieldNotAllowed { field });
        }
    }

    // ADR-0074 D1.1（Phase F2）: WorkUnit の `phase` は v2 で必須・既知の工程を指す、v1 で禁止。
    let phase_index: BTreeMap<&str, usize> = spec
        .phases
        .iter()
        .enumerate()
        .map(|(i, p)| (p.key.as_str(), i))
        .collect();
    for wu in &spec.work_units {
        if is_v2 {
            match &wu.phase {
                None => errors.push(PlanValidationError::WorkUnitMissingPhase {
                    key: wu.key.clone(),
                }),
                Some(phase) if !phase_index.contains_key(phase.as_str()) => {
                    errors.push(PlanValidationError::UnknownWorkUnitPhase {
                        key: wu.key.clone(),
                        phase: phase.clone(),
                    });
                }
                Some(_) => {}
            }
        } else if wu.phase.is_some() {
            errors.push(PlanValidationError::WorkUnitPhaseNotAllowedInV1 {
                key: wu.key.clone(),
            });
        }
    }

    let mut seen_keys: BTreeSet<&str> = BTreeSet::new();
    for wu in &spec.work_units {
        if !valid_key(&wu.key) {
            errors.push(PlanValidationError::InvalidKey {
                key: wu.key.clone(),
            });
            continue;
        }
        if !seen_keys.insert(wu.key.as_str()) {
            errors.push(PlanValidationError::DuplicateKey {
                key: wu.key.clone(),
            });
        }
    }

    let known_keys: BTreeSet<&str> = spec.work_units.iter().map(|w| w.key.as_str()).collect();
    for wu in &spec.work_units {
        for dep in &wu.depends_on {
            // ADR-0074 D3.7（Phase F4b (f)）: `child:<key>` は子 Task への依存（v2 の既知の子だけ）。
            if let Some(child) = dep.strip_prefix(CHILD_DEP_PREFIX) {
                if !is_v2 || !child_keys.contains(child) {
                    errors.push(PlanValidationError::UnknownDependency {
                        key: wu.key.clone(),
                        depends_on: dep.clone(),
                    });
                }
                continue;
            }
            if !known_keys.contains(dep.as_str()) {
                errors.push(PlanValidationError::UnknownDependency {
                    key: wu.key.clone(),
                    depends_on: dep.clone(),
                });
            }
        }
    }

    // ADR-0074 D1.1（Phase F2）: v2 の依存の規則 1・2（既知の key・既知の phase を持つ WU 同士に
    // 限る。上の 2 つの検査で既にエラーが出ている key/phase は静かに飛ばす。二重にエラーを
    // 積まないため）。
    if is_v2 {
        let wu_phase_of: BTreeMap<&str, usize> = spec
            .work_units
            .iter()
            .filter_map(|w| {
                w.phase
                    .as_deref()
                    .and_then(|p| phase_index.get(p))
                    .map(|&idx| (w.key.as_str(), idx))
            })
            .collect();
        for wu in &spec.work_units {
            let Some(&own_phase_idx) = wu_phase_of.get(wu.key.as_str()) else {
                continue;
            };
            let mut intra_phase_deps = 0usize;
            for dep in &wu.depends_on {
                let Some(&dep_phase_idx) = wu_phase_of.get(dep.as_str()) else {
                    continue;
                };
                match dep_phase_idx.cmp(&own_phase_idx) {
                    std::cmp::Ordering::Greater => {
                        errors.push(PlanValidationError::DependencyInLaterPhase {
                            key: wu.key.clone(),
                            depends_on: dep.clone(),
                        });
                    }
                    std::cmp::Ordering::Equal => intra_phase_deps += 1,
                    std::cmp::Ordering::Less => {}
                }
            }
            if intra_phase_deps > 1 {
                errors.push(PlanValidationError::TooManyIntraPhaseDependencies {
                    key: wu.key.clone(),
                    phase: wu.phase.clone().unwrap_or_default(),
                });
            }
        }
    }

    // ADR-0074 D5.1（Phase F1）: `features` は書かれていれば `TaskFeatureHints` として読めなければ
    // ならない（黙って捨てない）。5 軸のうちいくつか欠けているだけなら拒否しない
    // （`decide_for_work_unit` が Task から推定した値のまま補う）。
    for wu in &spec.work_units {
        if let Some(features) = &wu.features
            && let Err(e) =
                serde_json::from_value::<crate::model_policy::TaskFeatureHints>(features.clone())
        {
            errors.push(PlanValidationError::InvalidFeatures {
                key: wu.key.clone(),
                detail: e.to_string(),
            });
        }
    }

    // ADR-0074 D5.3（Phase F1）: 計画のサイズ上限（§4）。
    if spec.rationale.chars().count() > limits.max_rationale_chars {
        errors.push(PlanValidationError::RationaleTooLong {
            len: spec.rationale.chars().count(),
            max: limits.max_rationale_chars,
        });
    }
    for wu in &spec.work_units {
        let title_len = wu.title.chars().count();
        if title_len > limits.max_title_chars {
            errors.push(PlanValidationError::TitleTooLong {
                key: wu.key.clone(),
                len: title_len,
                max: limits.max_title_chars,
            });
        }
        let objective_len = wu.objective.chars().count();
        if objective_len > limits.max_objective_chars {
            errors.push(PlanValidationError::ObjectiveTooLong {
                key: wu.key.clone(),
                len: objective_len,
                max: limits.max_objective_chars,
            });
        }
        if wu.done_when.len() > limits.max_done_when_items {
            errors.push(PlanValidationError::TooManyDoneWhen {
                key: wu.key.clone(),
                count: wu.done_when.len(),
                max: limits.max_done_when_items,
            });
        }
        for (index, item) in wu.done_when.iter().enumerate() {
            let len = item.chars().count();
            if len > limits.max_done_when_chars {
                errors.push(PlanValidationError::DoneWhenItemTooLong {
                    key: wu.key.clone(),
                    index,
                    len,
                    max: limits.max_done_when_chars,
                });
            }
        }
        if wu.checks.len() > limits.max_checks {
            errors.push(PlanValidationError::TooManyChecks {
                key: wu.key.clone(),
                count: wu.checks.len(),
                max: limits.max_checks,
            });
        }
    }
    let plan_bytes = serde_json::to_vec(spec).map(|v| v.len()).unwrap_or(0);
    if plan_bytes > limits.max_plan_json_bytes {
        errors.push(PlanValidationError::PlanTooLarge {
            bytes: plan_bytes,
            max: limits.max_plan_json_bytes,
        });
    }

    // トポロジカルソート（循環の検出も兼ねる。Kahn's algorithm、決定的に `(phase, key)` 昇順で
    // tie-break。D1.1）。
    let mut topological_order = Vec::new();
    if errors.is_empty() {
        match topo_sort(&spec.work_units, &phase_index) {
            Ok(order) => topological_order = order,
            Err(cycle) => errors.push(PlanValidationError::CyclicDependency { cycle }),
        }
    }

    // 重複の検出: title を正規化して一致、または objective のトークン Jaccard >= 0.9。
    for i in 0..spec.work_units.len() {
        for j in (i + 1)..spec.work_units.len() {
            let a = &spec.work_units[i];
            let b = &spec.work_units[j];
            if normalize_title(&a.title) == normalize_title(&b.title) && !a.title.is_empty() {
                errors.push(PlanValidationError::DuplicateWorkUnit {
                    a: a.key.clone(),
                    b: b.key.clone(),
                    reason: "identical normalized title".to_string(),
                });
                continue;
            }
            let sim = jaccard(&token_set(&a.objective), &token_set(&b.objective));
            if sim >= 0.9 {
                errors.push(PlanValidationError::DuplicateWorkUnit {
                    a: a.key.clone(),
                    b: b.key.clone(),
                    reason: format!("objective token overlap {sim:.2}"),
                });
            }
        }
    }

    // replan の不変条件: done の WU の key/spec は変わらない（ADR-0079 R5b-fix1: 人の replan は spec の
    // 上書きだけを許す。[`done_carry_over_errors`]）。
    errors.extend(done_carry_over_errors(
        &spec.work_units,
        done_work_units,
        ctx.origin,
    ));

    if !errors.is_empty() {
        return Err(errors);
    }

    // D18: budget を丸める（丸めたことを記録する）。
    let mut rounded = spec.clone();
    let mut rounding_notes = Vec::new();
    for wu in &mut rounded.work_units {
        if let Some(budget) = &mut wu.budget {
            if let Some(turns) = budget.max_turns
                && turns > limits.work_unit_max_turns
            {
                rounding_notes.push(format!(
                    "work unit {}: max_turns {} -> {}",
                    wu.key, turns, limits.work_unit_max_turns
                ));
                budget.max_turns = Some(limits.work_unit_max_turns);
            }
            if let Some(wall) = budget.max_wall_secs
                && wall > limits.work_unit_max_wall_secs
            {
                rounding_notes.push(format!(
                    "work unit {}: max_wall_secs {} -> {}",
                    wu.key, wall, limits.work_unit_max_wall_secs
                ));
                budget.max_wall_secs = Some(limits.work_unit_max_wall_secs);
            }
        }
    }

    Ok(ValidatedPlan {
        spec: rounded,
        rounding_notes,
        topological_order,
    })
}

/// ADR-0074 D3.7（Phase F4b (f)）: `children` の検証（v2 のみ呼ぶ）。
fn validate_children(
    children: &[ExecutionChildSpec],
    limits: ExecutionLimits,
) -> Vec<PlanValidationError> {
    let mut errors = Vec::new();
    if children.len() > limits.max_children {
        errors.push(PlanValidationError::TooManyChildren {
            count: children.len(),
            max: limits.max_children,
        });
    }
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for c in children {
        if !valid_key(&c.key) {
            errors.push(PlanValidationError::InvalidChildKey { key: c.key.clone() });
            continue;
        }
        if !seen.insert(c.key.as_str()) {
            errors.push(PlanValidationError::DuplicateChildKey { key: c.key.clone() });
        }
    }
    let known: BTreeSet<&str> = children.iter().map(|c| c.key.as_str()).collect();
    for c in children {
        for dep in &c.depends_on {
            if !known.contains(dep.as_str()) || dep == &c.key {
                errors.push(PlanValidationError::UnknownChildDependency {
                    key: c.key.clone(),
                    depends_on: dep.clone(),
                });
            }
        }
        if c.acceptance.is_empty() {
            errors.push(PlanValidationError::ChildNoAcceptance { key: c.key.clone() });
        } else if let Err(detail) =
            crate::model::validate_human_checks_have_deliverable(&c.acceptance)
        {
            errors.push(PlanValidationError::ChildInvalidAcceptance {
                key: c.key.clone(),
                detail,
            });
        }
    }
    if errors.is_empty() {
        // 子同士の循環（Kahn 法。`topo_sort` を WU と同じ形で使う）。
        let as_units: Vec<WorkUnitSpec> = children
            .iter()
            .map(|c| WorkUnitSpec {
                key: c.key.clone(),
                kind: WorkUnitKind::Implement,
                title: c.title.clone(),
                objective: c.objective.clone(),
                depends_on: c.depends_on.clone(),
                done_when: Vec::new(),
                checks: Vec::new(),
                context: WorkUnitContext::default(),
                harness: None,
                features: None,
                budget: None,
                outputs: Vec::new(),
                phase: None,
            })
            .collect();
        if let Err(cycle) = topo_sort(&as_units, &BTreeMap::new()) {
            errors.push(PlanValidationError::CyclicChildDependency { cycle });
        }
    }
    errors
}

/// ADR-0079 D2 / D3 / D4 (2)（Phase R1a）: `celeris.execution-plan/3` の検証（純粋関数）。
///
/// - `[execution.tree] enabled = false` なら `TreeDisabled` だけを返す（他は見ない）。
/// - 段階: 1..=`max_stages`、key の形・一意、kind に `task` / `integrate` を使わない。
/// - unit: key の形・一意・`integrate-` の予約、段階は既知、段階あたり `max_units_per_stage`、
///   kind task は計画あたり `max_child_tasks_per_plan` で `depth < max_depth` のときだけ。
/// - kind task: `acceptance` 1 件以上（human には成果物）、`checks` / `budget` / `harness` /
///   `context.paths` を持たない、`adopt` は origin human だけ。
/// - leaf: kind task 専用の欄を持たない、`checks` 1 本以上、`context.repo` は高々 1、予算は 1 run の
///   上限以内（/3 は丸めずに拒否）。
/// - 依存: /2 の規則（既知の key、同じか前の段階、同じ段階の中は高々 1 つ）、`child:` は拒否、循環なし。
/// - 決定: 形（D7）、key の一意、`needed_before` は既知の unit か `stage:<key>`、計画あたり
///   `max_open_decisions_per_plan`。`needs_decisions` は既知の決定。
/// - /2 と同じ: `features`・サイズの上限・重複の検出・replan の done の不変条件。
fn validate_v3(
    spec: &ExecutionPlanSpec,
    limits: ExecutionLimits,
    done_work_units: &[(String, WorkUnitSpec)],
    ctx: PlanContext,
) -> Result<ValidatedPlan, Vec<PlanValidationError>> {
    let tree = limits.tree;
    if !tree.enabled {
        return Err(vec![PlanValidationError::TreeDisabled]);
    }
    let mut errors = Vec::new();

    for (field, present) in [
        ("phases", !spec.phases.is_empty()),
        ("work_units", !spec.work_units.is_empty()),
        ("children", !spec.children.is_empty()),
    ] {
        if present {
            errors.push(PlanValidationError::V2FieldInV3 { field });
        }
    }

    // 段階。
    if spec.stages.is_empty() {
        errors.push(PlanValidationError::NoStages);
    }
    if spec.stages.len() > tree.max_stages {
        errors.push(PlanValidationError::TooManyStages {
            count: spec.stages.len(),
            max: tree.max_stages,
        });
    }
    let mut seen_stage_keys: BTreeSet<&str> = BTreeSet::new();
    for s in &spec.stages {
        if !valid_key(&s.key) {
            errors.push(PlanValidationError::InvalidStageKey { key: s.key.clone() });
            continue;
        }
        if !seen_stage_keys.insert(s.key.as_str()) {
            errors.push(PlanValidationError::DuplicateStageKey { key: s.key.clone() });
        }
        if matches!(s.kind, WorkUnitKind::Task | WorkUnitKind::Integrate) {
            errors.push(PlanValidationError::InvalidStageKind { key: s.key.clone() });
        }
    }
    let stage_index: BTreeMap<&str, usize> = spec
        .stages
        .iter()
        .enumerate()
        .map(|(i, s)| (s.key.as_str(), i))
        .collect();

    // unit の key・kind・段階。
    if spec.units.is_empty() {
        errors.push(PlanValidationError::NoUnits);
    }
    let mut seen_keys: BTreeSet<&str> = BTreeSet::new();
    for u in &spec.units {
        if !valid_key(&u.key) {
            errors.push(PlanValidationError::InvalidKey { key: u.key.clone() });
            continue;
        }
        if !seen_keys.insert(u.key.as_str()) {
            errors.push(PlanValidationError::DuplicateKey { key: u.key.clone() });
        }
        if u.key.starts_with(INTEGRATE_KEY_PREFIX) {
            errors.push(PlanValidationError::ReservedKey { key: u.key.clone() });
        }
        if u.kind == WorkUnitKind::Integrate {
            errors.push(PlanValidationError::ReservedKind { key: u.key.clone() });
        }
        if !stage_index.contains_key(u.stage.as_str()) {
            errors.push(PlanValidationError::UnknownUnitStage {
                key: u.key.clone(),
                stage: u.stage.clone(),
            });
        }
    }
    // ADR-0079 付記「R7-3」D5: 段階あたりの unit は生きた unit だけを数える（replan で持ち越す done の unit と、run を
    // 費やさない `adopt` の unit を除く。R7-2 の `max_child_tasks_per_plan` と同じ考え方）。failed / running の行を
    // 持ち越す unit は数える（D6）。
    let done_keys: BTreeSet<&str> = done_work_units.iter().map(|(k, _)| k.as_str()).collect();
    for s in &spec.stages {
        let count = spec
            .units
            .iter()
            .filter(|u| {
                u.stage == s.key && u.adopt.is_none() && !done_keys.contains(u.key.as_str())
            })
            .count();
        if count > tree.max_units_per_stage {
            errors.push(PlanValidationError::TooManyUnitsInStage {
                stage: s.key.clone(),
                count,
                max: tree.max_units_per_stage,
            });
        }
    }
    // ADR-0079 D15（Phase R5b-prep）: `adopt` の unit は子を作らないので数えない。
    // ADR-0079 R7-2: replan で持ち越す done の unit（`done_work_units`）も数えない（子はもう終わっていて新しい子を
    // 作らない。R6-1 D5 の承認の材料と同じ数え方）。長く走る root が done の kind task を溜めると新しい unit を
    // 足せなくなっていた（本番「7 > 6」）。done でない行を持ち越す unit（failed の子を作り直す・走っている子）は数える。
    let task_units = spec
        .units
        .iter()
        .filter(|u| u.creates_child() && !done_keys.contains(u.key.as_str()))
        .count();
    if task_units > tree.max_child_tasks_per_plan {
        errors.push(PlanValidationError::TooManyChildTasks {
            count: task_units,
            max: tree.max_child_tasks_per_plan,
        });
    }

    // kind task / leaf の欄。
    for u in &spec.units {
        let key = u.key.clone();
        if u.is_task() {
            if !crate::tree::can_have_child_tasks(ctx.depth, tree.max_depth) {
                errors.push(PlanValidationError::ChildTaskTooDeep {
                    key: key.clone(),
                    depth: ctx.depth,
                    max_depth: tree.max_depth,
                });
            }
            if u.acceptance.is_empty() {
                errors.push(PlanValidationError::TaskUnitNoAcceptance { key: key.clone() });
            } else if let Err(detail) =
                crate::model::validate_human_checks_have_deliverable(&u.acceptance)
            {
                errors.push(PlanValidationError::TaskUnitInvalidAcceptance {
                    key: key.clone(),
                    detail,
                });
            }
            for (field, present) in [
                ("checks", !u.checks.is_empty()),
                ("budget", u.budget.is_some()),
                ("harness", u.harness.is_some()),
                ("context.paths", !u.context.paths.is_empty()),
            ] {
                if present {
                    errors.push(PlanValidationError::TaskUnitFieldNotAllowed {
                        key: key.clone(),
                        field,
                    });
                }
            }
            // ADR-0079 R5b-fix1: planner の replan は、前の版から done の unit をそのまま写す（done の不変条件）
            // ので、その unit が持つ `adopt` も残ってよい（採用は済んでいて、replan は done の行に触れない）。
            // done の写しでない unit の新しい `adopt` は従来どおり人の計画だけ。
            if u.adopt.is_some()
                && ctx.origin != PlanOrigin::Human
                && !is_done_carry_over(u, done_work_units)
            {
                errors.push(PlanValidationError::AdoptNotAllowed { key: key.clone() });
            }
        } else {
            for (field, present) in [
                ("acceptance", !u.acceptance.is_empty()),
                ("genre", u.genre.is_some()),
                ("skills", !u.skills.is_empty()),
                ("repos", !u.repos.is_empty()),
                ("adopt", u.adopt.is_some()),
                ("gate", u.gate.is_some()),
            ] {
                if present {
                    errors.push(PlanValidationError::LeafFieldNotAllowed {
                        key: key.clone(),
                        field,
                    });
                }
            }
            if u.checks.is_empty() {
                errors.push(PlanValidationError::LeafWithoutChecks { key: key.clone() });
            }
            let repo_count = u
                .context
                .repo
                .as_ref()
                .map(RepoSelector::count)
                .unwrap_or(0);
            if repo_count > 1 {
                errors.push(PlanValidationError::LeafMultipleRepos {
                    key: key.clone(),
                    count: repo_count,
                });
            }
            if let Some(budget) = u.budget {
                if let Some(turns) = budget.max_turns
                    && turns > limits.work_unit_max_turns
                {
                    errors.push(PlanValidationError::LeafBudgetOverLimit {
                        key: key.clone(),
                        detail: format!(
                            "budget.max_turns {turns} > {}",
                            limits.work_unit_max_turns
                        ),
                    });
                }
                if let Some(wall) = budget.max_wall_secs
                    && wall > limits.work_unit_max_wall_secs
                {
                    errors.push(PlanValidationError::LeafBudgetOverLimit {
                        key: key.clone(),
                        detail: format!(
                            "budget.max_wall_secs {wall} > {}",
                            limits.work_unit_max_wall_secs
                        ),
                    });
                }
            }
        }
    }

    // 依存（/2 の規則 + `child:` の拒否）。
    let known_keys: BTreeSet<&str> = spec.units.iter().map(|u| u.key.as_str()).collect();
    let unit_stage_of: BTreeMap<&str, usize> = spec
        .units
        .iter()
        .filter_map(|u| {
            stage_index
                .get(u.stage.as_str())
                .map(|&i| (u.key.as_str(), i))
        })
        .collect();
    for u in &spec.units {
        let mut intra_stage_deps = 0usize;
        for dep in &u.depends_on {
            if dep.starts_with(CHILD_DEP_PREFIX) {
                errors.push(PlanValidationError::ChildDependencyNotAllowed {
                    key: u.key.clone(),
                    depends_on: dep.clone(),
                });
                continue;
            }
            if !known_keys.contains(dep.as_str()) {
                errors.push(PlanValidationError::UnknownDependency {
                    key: u.key.clone(),
                    depends_on: dep.clone(),
                });
                continue;
            }
            let (Some(&own), Some(&other)) = (
                unit_stage_of.get(u.key.as_str()),
                unit_stage_of.get(dep.as_str()),
            ) else {
                continue;
            };
            match other.cmp(&own) {
                std::cmp::Ordering::Greater => {
                    errors.push(PlanValidationError::DependencyInLaterPhase {
                        key: u.key.clone(),
                        depends_on: dep.clone(),
                    })
                }
                std::cmp::Ordering::Equal => intra_stage_deps += 1,
                std::cmp::Ordering::Less => {}
            }
        }
        if intra_stage_deps > 1 {
            errors.push(PlanValidationError::TooManyIntraPhaseDependencies {
                key: u.key.clone(),
                phase: u.stage.clone(),
            });
        }
    }

    // 決定（D7）。
    let decisions = normalized_decisions(spec);
    let mut decision_keys: BTreeSet<&str> = BTreeSet::new();
    for d in &decisions {
        for e in crate::decision::validate_shape(d) {
            errors.push(PlanValidationError::InvalidDecision {
                detail: e.to_string(),
            });
        }
        if !decision_keys.insert(d.key.as_str()) {
            errors.push(PlanValidationError::DuplicateDecisionKey { key: d.key.clone() });
        }
        for target in &d.needed_before {
            let known = match target.strip_prefix(crate::decision::NEEDED_BEFORE_STAGE_PREFIX) {
                Some(stage) => stage_index.contains_key(stage),
                None => known_keys.contains(target.as_str()),
            };
            if !known {
                errors.push(PlanValidationError::UnknownNeededBefore {
                    key: d.key.clone(),
                    target: target.clone(),
                });
            }
        }
    }
    if decisions.len() > tree.max_open_decisions_per_plan {
        errors.push(PlanValidationError::TooManyDecisions {
            count: decisions.len(),
            max: tree.max_open_decisions_per_plan,
        });
    }
    for u in &spec.units {
        for d in &u.needs_decisions {
            if !decision_keys.contains(d.as_str()) {
                errors.push(PlanValidationError::UnknownNeedsDecision {
                    key: u.key.clone(),
                    decision: d.clone(),
                });
            }
        }
    }

    // /2 と同じ検査（`features`・サイズ・重複・replan の不変条件）は /2 の形に写して行う。
    let internal = internal_view(spec);
    // ADR-0130 D1: `expected_write_paths` は /3 の unit だけが持つ任意欄。
    for unit in &spec.units {
        if let Some(paths) = &unit.expected_write_paths
            && let Err(detail) = crate::write_set::normalize_write_paths(paths)
        {
            errors.push(PlanValidationError::InvalidWritePaths {
                key: unit.key.clone(),
                detail,
            });
        }
    }
    for wu in &internal.work_units {
        if let Some(features) = &wu.features
            && let Err(e) =
                serde_json::from_value::<crate::model_policy::TaskFeatureHints>(features.clone())
        {
            errors.push(PlanValidationError::InvalidFeatures {
                key: wu.key.clone(),
                detail: e.to_string(),
            });
        }
    }
    let rationale_len = spec.rationale.chars().count();
    if rationale_len > limits.max_rationale_chars {
        errors.push(PlanValidationError::RationaleTooLong {
            len: rationale_len,
            max: limits.max_rationale_chars,
        });
    }
    for wu in &internal.work_units {
        let title_len = wu.title.chars().count();
        if title_len > limits.max_title_chars {
            errors.push(PlanValidationError::TitleTooLong {
                key: wu.key.clone(),
                len: title_len,
                max: limits.max_title_chars,
            });
        }
        let objective_len = wu.objective.chars().count();
        if objective_len > limits.max_objective_chars {
            errors.push(PlanValidationError::ObjectiveTooLong {
                key: wu.key.clone(),
                len: objective_len,
                max: limits.max_objective_chars,
            });
        }
        if wu.done_when.len() > limits.max_done_when_items {
            errors.push(PlanValidationError::TooManyDoneWhen {
                key: wu.key.clone(),
                count: wu.done_when.len(),
                max: limits.max_done_when_items,
            });
        }
        for (index, item) in wu.done_when.iter().enumerate() {
            let len = item.chars().count();
            if len > limits.max_done_when_chars {
                errors.push(PlanValidationError::DoneWhenItemTooLong {
                    key: wu.key.clone(),
                    index,
                    len,
                    max: limits.max_done_when_chars,
                });
            }
        }
        if wu.checks.len() > limits.max_checks {
            errors.push(PlanValidationError::TooManyChecks {
                key: wu.key.clone(),
                count: wu.checks.len(),
                max: limits.max_checks,
            });
        }
    }
    // ADR-0079 R7-2: /3 は `max_plan_json_bytes_v3`（既定 64 KiB）で測る。
    let plan_bytes = serde_json::to_vec(spec).map(|v| v.len()).unwrap_or(0);
    if plan_bytes > limits.max_plan_json_bytes_v3 {
        errors.push(PlanValidationError::PlanTooLarge {
            bytes: plan_bytes,
            max: limits.max_plan_json_bytes_v3,
        });
    }

    let mut topological_order = Vec::new();
    if errors.is_empty() {
        match topo_sort(&internal.work_units, &stage_index) {
            Ok(order) => topological_order = order,
            Err(cycle) => errors.push(PlanValidationError::CyclicDependency { cycle }),
        }
    }

    for i in 0..internal.work_units.len() {
        for j in (i + 1)..internal.work_units.len() {
            let a = &internal.work_units[i];
            let b = &internal.work_units[j];
            if normalize_title(&a.title) == normalize_title(&b.title) && !a.title.is_empty() {
                errors.push(PlanValidationError::DuplicateWorkUnit {
                    a: a.key.clone(),
                    b: b.key.clone(),
                    reason: "identical normalized title".to_string(),
                });
                continue;
            }
            let sim = jaccard(&token_set(&a.objective), &token_set(&b.objective));
            if sim >= 0.9 {
                errors.push(PlanValidationError::DuplicateWorkUnit {
                    a: a.key.clone(),
                    b: b.key.clone(),
                    reason: format!("objective token overlap {sim:.2}"),
                });
            }
        }
    }

    errors.extend(done_carry_over_errors(
        &internal.work_units,
        done_work_units,
        ctx.origin,
    ));

    if !errors.is_empty() {
        return Err(errors);
    }
    let mut normalized = spec.clone();
    for unit in &mut normalized.units {
        if let Some(paths) = &unit.expected_write_paths
            && let Ok(paths) = crate::write_set::normalize_write_paths(paths)
        {
            unit.expected_write_paths = Some(paths);
        }
    }
    Ok(ValidatedPlan {
        spec: normalized,
        rounding_notes: Vec::new(),
        topological_order,
    })
}

/// Kahn's algorithm。`Ok` はトポロジカル順（`work_units` の index。同順位は
/// `(phase_rank, key)` 昇順で tie-break）、`Err` は見つかった循環（key の列）。
///
/// ADR-0074 D1.1（Phase F2）: `phase_rank` は工程の key → `phases` での出現順（v2）。v1 は
/// 常に空の map を渡す（すべて rank 0 のまま。挙動は従来と 1 バイトも変わらない）。v2 では
/// 工程をまたぐ依存は必ず前の工程 → 後の工程（`validate` が別途保証する）なので、この
/// tie-break は「依存の無い WU 同士がどちらの工程にいても後の工程が先に選ばれない」ことだけを
/// 保証する（表示・`seq` の並びのため。scheduler 自体は `phase` 列で絞り込む。D1.3）。
fn topo_sort(
    work_units: &[WorkUnitSpec],
    phase_rank: &BTreeMap<&str, usize>,
) -> Result<Vec<usize>, Vec<String>> {
    let index_of: BTreeMap<&str, usize> = work_units
        .iter()
        .enumerate()
        .map(|(i, w)| (w.key.as_str(), i))
        .collect();
    let mut in_degree: Vec<usize> = vec![0; work_units.len()];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); work_units.len()];
    for (i, wu) in work_units.iter().enumerate() {
        for dep in &wu.depends_on {
            if let Some(&dep_i) = index_of.get(dep.as_str()) {
                dependents[dep_i].push(i);
                in_degree[i] += 1;
            }
        }
    }
    let rank_of = |wu: &WorkUnitSpec| -> usize {
        wu.phase
            .as_deref()
            .and_then(|p| phase_rank.get(p).copied())
            .unwrap_or(0)
    };
    // 決定的に (phase_rank, key) 昇順で並べた候補集合を都度取り出す。
    let mut ready: BTreeSet<(usize, &str, usize)> = work_units
        .iter()
        .enumerate()
        .filter(|(i, _)| in_degree[*i] == 0)
        .map(|(i, w)| (rank_of(w), w.key.as_str(), i))
        .collect();
    let mut order = Vec::new();
    while let Some((_, _, i)) = ready.iter().next().copied() {
        ready.remove(&(rank_of(&work_units[i]), work_units[i].key.as_str(), i));
        order.push(i);
        for &dep in &dependents[i] {
            in_degree[dep] -= 1;
            if in_degree[dep] == 0 {
                ready.insert((rank_of(&work_units[dep]), work_units[dep].key.as_str(), dep));
            }
        }
    }
    if order.len() == work_units.len() {
        Ok(order)
    } else {
        let cycle: Vec<String> = (0..work_units.len())
            .filter(|i| !order.contains(i))
            .map(|i| work_units[i].key.clone())
            .collect();
        Err(cycle)
    }
}
