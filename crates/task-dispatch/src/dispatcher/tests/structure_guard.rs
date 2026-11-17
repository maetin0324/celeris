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
