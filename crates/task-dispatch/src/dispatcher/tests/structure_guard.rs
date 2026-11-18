//! 試験用の切替を本体の struct に増やさないことの検査（ADR 2026-11-11 dispatcher-state-ownership D6）。
//! source を読み、struct の本体にある `#[cfg(...)]` のうち `test` を含む属性を数える（field の名前は見ない）。

/// `#[cfg(test)]` の field を 1 個も持たない owner の struct（型名と、その型を定義する source）。
/// 状態を `Dispatcher` から移した owner の unit は、自分の struct をこの一覧に足す。
const OWNER_STRUCTS: &[(&str, &str)] = &[
    ("ClusterRuntime", include_str!("../../cluster_runtime.rs")),
    (
        "HostMaintenance<C>",
        include_str!("../../host_maintenance.rs"),
    ),
    ("InventoryRuns", include_str!("../../host_maintenance.rs")),
    ("RoutingRuntime", include_str!("../../routing_runtime.rs")),
    ("QuotaBook", include_str!("../../routing_runtime/quota.rs")),
    ("RunSupervisor<S>", include_str!("../../run_supervisor.rs")),
    (
        "ExecutionCoordination<S, C>",
        include_str!("../../execution_coordination.rs"),
    ),
    (
        "ProcessTable",
        include_str!("../../execution_coordination.rs"),
    ),
    (
        "LivenessWatch",
        include_str!("../../execution_coordination.rs"),
    ),
    (
        "ProjectOrchestration<L>",
        include_str!("../../project_orchestration.rs"),
    ),
    (
        "StagnationWatch",
        include_str!("../../project_orchestration.rs"),
    ),
    ("ProbePorts", include_str!("../../probe_ports.rs")),
];

/// `src` の `struct <name> {` の本体（次の行頭の `}` まで）の行のうち、`test` を条件に含む `#[cfg(...)]` 属性の数。
/// 空白の入れ方（`#[cfg( test )]`）や `any(test, …)`・`all(test, …)` も数える。doc comment の中の語は数えない。
fn cfg_test_attributes(src: &str, name: &str) -> usize {
    let head = format!("struct {name} {{");
    let start = src
        .lines()
        .position(|l| {
            let t = l.trim_start();
            !t.starts_with("//") && t.contains(&head)
        })
        .unwrap_or_else(|| panic!("struct {name} is defined"));
    src.lines()
        .skip(start + 1)
        .take_while(|l| !l.starts_with('}'))
        .map(|l| l.split_whitespace().collect::<String>())
        .filter(|t| t.starts_with("#[cfg("))
        .filter(|t| {
            let cond = t.split_once(']').map_or(t.as_str(), |(cond, _)| cond);
            cond.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .any(|word| word == "test")
        })
        .count()
}

/// 不変条件（D6）: `Dispatcher` の `#[cfg(test)]` の field は `test_hooks`（`TestHooks`）の 1 個だけで、試験用の
/// 時計・遅延・停止点はすべて `TestHooks` に置く。
/// 既存の試験で検出できない理由: 試験は field の振る舞い（時計を進める）だけを見るので、
/// `Dispatcher` に `#[cfg(test)]` の field を 1 個足しても、名前が `test_` で始まらなくても、すべて通る。
#[test]
fn structure_guard_dispatcher_has_one_test_field() {
    let src = include_str!("../../dispatcher.rs");
    let count = cfg_test_attributes(src, "Dispatcher");
    assert!(
        count <= 1,
        "Dispatcher has {count} #[cfg(test)] fields; put test switches in tests::hooks::TestHooks \
         (ADR 2026-11-11 dispatcher-state-ownership D6)"
    );
}

/// 不変条件（D6）: 状態を `Dispatcher` から移した owner の struct は `#[cfg(test)]` の field を 1 個も持たない。
/// 試験の切替は `Dispatcher` の入口が `TestHooks` から読んで owner の method の引数に渡す。
/// 既存の試験で検出できない理由: owner の振る舞いの試験は field の有無を見ず、owner ごとの structure_guard は
/// `#[cfg(test)]` で始まる行しか見ない（`#[cfg( test )]`・`#[cfg(any(test, …))]` を見逃す）。
#[test]
fn structure_guard_owner_structs_have_no_test_fields() {
    for (name, src) in OWNER_STRUCTS {
        assert_eq!(
            cfg_test_attributes(src, name),
            0,
            "{name} has a #[cfg(test)] field; read the switch from Dispatcher's TestHooks and pass it \
             as an argument (ADR 2026-11-11 dispatcher-state-ownership D6)"
        );
    }
}

/// 不変条件: 上の 2 本の数え方が、名前に関わらず `test` を条件に含む属性を数え、doc comment や他の struct を数えない。
/// 既存の試験で検出できない理由: 数え方が壊れて常に 0 を返すと、上の 2 本は何も検出せずに通る。
#[test]
fn structure_guard_counts_cfg_test_attributes_by_attribute() {
    let src = "\
/// #[cfg(test)] in a doc comment
pub struct Sample {
    a: u32,
    #[cfg(test)]
    anything: u32,
    #[cfg( test )]
    spaced: u32,
    #[cfg(any(test, feature = \"x\"))]
    any_test: u32,
    #[cfg(feature = \"testing\")]
    not_test: u32,
    #[cfg(test)] inline: u32,
}
pub struct Other {
    #[cfg(test)]
    other: u32,
}
";
    assert_eq!(cfg_test_attributes(src, "Sample"), 4);
    assert_eq!(cfg_test_attributes(src, "Other"), 1);
}

/// 共有の状態（store・設定の全体・`Dispatcher`・`CosChatLaunch`）に届く経路の token
/// （ADR 2026-11-18 dispatcher-store-access-pilot「証明」3）。`(返す名, 詰めた source で探す字面, 後ろも語の境目か)`。
/// `CosChatLaunch` は後ろの境目を見ないので `CosChatLaunchConfig` も拾う。`Dispatcher` は語として探すので
/// `DispatchError` を拾わない。
const SHARED_STATE_TOKENS: &[(&str, &str, bool)] = &[
    ("self.store", "self.store", true),
    ("self.config", "self.config", true),
    ("SqliteStore", "SqliteStore", true),
    ("Dispatcher", "Dispatcher", true),
    ("CosChatLaunch", "CosChatLaunch", false),
    ("DispatchConfig", "DispatchConfig", true),
    ("usesuper::*", "use super::*", false),
];

/// `src` が共有の状態に触る経路の token（[`SHARED_STATE_TOKENS`] の名、見つかった順ではなく一覧の順）。
/// `//` から行末（doc comment を含む）を除き、空白を詰めた source を探す。識別子の文字に挟まれた空白だけは
/// 1 個残す（`impl Dispatcher` の語の境目を保つ）ので、rustfmt が割った `self\n    .config` は `self.config` になり、
/// `self.store_path`・`let store_path` は `self.store` にならない。cos-region・measure の guard もこの関数を使う。
fn shared_state_access(src: &str) -> Vec<&'static str> {
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    let mut code = String::with_capacity(src.len());
    let mut gap = false;
    for line in src.lines() {
        for c in line.find("//").map_or(line, |i| &line[..i]).chars() {
            if c.is_whitespace() {
                gap = true;
                continue;
            }
            if gap && is_ident(c) && code.ends_with(is_ident) {
                code.push(' ');
            }
            gap = false;
            code.push(c);
        }
        gap = true;
    }
    SHARED_STATE_TOKENS
        .iter()
        .filter(|(_, pat, whole_word)| {
            code.match_indices(pat).any(|(i, _)| {
                let before = code[..i].chars().next_back().is_some_and(is_ident);
                let after = code[i + pat.len()..].chars().next().is_some_and(is_ident);
                !before && !(*whole_word && after)
            })
        })
        .map(|(name, _, _)| *name)
        .collect()
}

/// 不変条件（ADR 2026-11-18 dispatcher-store-access-pilot D1）: WU 領域の判断の file は共有の状態に触る経路を持たない。
/// store・設定の読み書きは入口（`dispatch_run.rs`・`review_spawn.rs`・`work_units.rs`）が行い、この 4 file の関数には値を渡す。
/// 既存の試験で検出できない理由: 振る舞いの試験（`write_set_gate_*`・`write_set_record_*`・`target_sync`・`tree_decisions`・`cex_*`）は
/// どの file が store に触るかを見ないので、対象 file に `impl Dispatcher` と `self.store` を戻しても全部通る。
#[test]
fn store_access_guard_work_units() {
    let files = [
        ("behind_target.rs", include_str!("../behind_target.rs")),
        ("auto_leaf.rs", include_str!("../auto_leaf.rs")),
        ("write_set_gate.rs", include_str!("../write_set_gate.rs")),
        (
            "write_set_record.rs",
            include_str!("../write_set_record.rs"),
        ),
    ];
    for (file, src) in files {
        let found = shared_state_access(src);
        assert!(
            found.is_empty(),
            "dispatcher/{file} reaches shared state through {found:?}; read and write the store and the \
             config in the entry file and pass values to this file's functions \
             (ADR 2026-11-18 dispatcher-store-access-pilot D1)"
        );
    }
}

/// 不変条件: [`shared_state_access`] が ADR の 8 つの違反（と残りの token の `DispatchConfig`・`CosChatLaunchConfig`）を
/// 各々検出し、comment の中の語・`DispatchError`・`store_path` のような似た語・明示の `use super::<名>` を検出しない。
/// 既存の試験で検出できない理由: 検査関数が壊れて常に空を返すと、`store_access_guard_work_units`（と cos-region・measure の
/// guard）は何も検出せずに通る。違反を戻した対象 file は repo に無いので、検出の側はこの試験でしか確かめられない。
#[test]
fn store_access_guard_detects_each_violation() {
    let violations = [
        ("self.store.task_get(id)", "self.store"),
        ("self\n    .config.max_wall_secs", "self.config"),
        ("fn f(s: &SqliteStore) {}", "SqliteStore"),
        ("fn f(d: &mut Dispatcher) {}", "Dispatcher"),
        ("impl Dispatcher {", "Dispatcher"),
        ("use super::*;", "usesuper::*"),
        ("let s = &self.store;", "self.store"),
        ("fn f(l: &mut CosChatLaunch) {}", "CosChatLaunch"),
        ("fn f(c: &DispatchConfig) {}", "DispatchConfig"),
        ("fn f(c: CosChatLaunchConfig) {}", "CosChatLaunch"),
    ];
    for (src, token) in violations {
        assert_eq!(shared_state_access(src), vec![token], "{src:?}");
    }
    let clean = "\
//! The entry reads self.store and calls impl Dispatcher { .. }.
/// Takes what `Dispatcher` read from SqliteStore and DispatchConfig; no `use super::*`.
fn f(e: DispatchError) -> Result<(), DispatchError> { // self.config, CosChatLaunch
    let store_path = self.store_path.join(\"x\");
    let configured = config.max_wall_secs + self.configured;
    use super::rfc3339;
    Err(e)
}
";
    assert_eq!(shared_state_access(clean), Vec::<&str>::new());
}
