use super::*;

#[test]
fn extracts_a_slash_separated_list_of_proper_nouns() {
    let objective = "CHFS/FINCHFS/GekkoFS/UnifyFS/BeeOND のデプロイモデルを比較調査する。";
    assert_eq!(
        research_targets(objective),
        vec!["CHFS", "FINCHFS", "GekkoFS", "UnifyFS", "BeeOND"]
    );
}

#[test]
fn drops_a_trailing_etc_suffix_since_it_is_not_ascii() {
    let objective = "CHFS、FINCHFS、GekkoFS など、主要な ad-hoc HPC ファイルシステムを調べる。";
    assert_eq!(
        research_targets(objective),
        vec!["CHFS", "FINCHFS", "GekkoFS"]
    );
}

#[test]
fn accepts_a_mixed_separator_style() {
    let objective = "Lustre・BeeGFS・WekaFS の一次情報を確認する。";
    assert_eq!(
        research_targets(objective),
        vec!["Lustre", "BeeGFS", "WekaFS"]
    );
}

#[test]
fn does_not_mistake_a_url_path_for_a_target_list() {
    // 実測の事故（Phase 109c）: `github.com/otatebe/chfs` の `/` がパス区切りなのに
    // 対象の列挙（`com`/`otatebe`/`chfs`）と誤認されていた。1 件しか対象が無い目的文なので
    // 空（列挙にならない）。
    let objective = "CHFS（https://github.com/otatebe/chfs）を調べる";
    assert!(
        research_targets(objective).is_empty(),
        "{:?}",
        research_targets(objective)
    );
}

#[test]
fn a_real_target_list_survives_next_to_a_url() {
    let objective =
        "CHFS/FINCHFS を、それぞれの GitHub（https://github.com/otatebe/chfs）も見て調べる";
    assert_eq!(research_targets(objective), vec!["CHFS", "FINCHFS"]);
}

#[test]
fn returns_empty_without_a_delimited_list() {
    assert!(research_targets("BenchFS の設計方針を調べる。").is_empty());
    assert!(research_targets("").is_empty());
}

#[test]
fn a_single_short_token_pair_like_io_is_not_mistaken_for_a_target_list() {
    // `I/O` の `I`/`O` は 1 文字なので連なりとして数えない。
    assert!(research_targets("非同期 I/O ランタイムを調べる。").is_empty());
}

#[test]
fn keeps_the_first_occurrence_when_two_lists_tie_in_length() {
    // 先に出てくる鎖（長さ 2）が採用される。
    let objective = "AA/BB を先に見て、その後 CC/DD も見る。";
    assert_eq!(research_targets(objective), vec!["AA", "BB"]);
}

#[test]
fn aspects_prefer_a_parenthesized_list_in_the_objective() {
    let objective = "CHFS/FINCHFS のデプロイモデルを比較する（server/client 配置、cache 方式）。";
    assert_eq!(
        research_aspects(objective),
        vec!["server/client 配置", "cache 方式"]
    );
}

#[test]
fn aspects_fall_back_to_the_default_list_without_parens() {
    assert_eq!(
        research_aspects("CHFS/FINCHFS を比較する。"),
        DEFAULT_ASPECTS
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
    );
}

#[test]
fn comparison_target_finds_an_ascii_identifier_before_and_compare() {
    assert_eq!(
        comparison_target("CHFS/FINCHFS を BenchFS と比較する"),
        Some("BenchFS".to_string())
    );
    assert_eq!(
        comparison_target("CHFS/FINCHFS を BenchFS との比較で調べる"),
        Some("BenchFS".to_string())
    );
}

#[test]
fn comparison_target_is_none_without_a_compare_phrase() {
    assert!(comparison_target("CHFS/FINCHFS を調べる").is_none());
    assert!(comparison_target("").is_none());
}

/// ADR-0063 Phase 109g A: 知識ベースに比較先のページが無いときの最後の拠り所（目的文中の比較の
/// 言い回しを含む 1 文）。
#[test]
fn comparison_target_paragraph_returns_the_sentence_around_the_compare_phrase() {
    let objective = "CHFS/FINCHFS の学術文献を調査する。BenchFSとの比較が『公平比較可能』か『背景比較のみ』かを\
分類すること。既存knowledgeの設計と対比できるよう根拠付きで書くこと。";
    assert_eq!(
        comparison_target_paragraph(objective),
        Some("BenchFSとの比較が『公平比較可能』か『背景比較のみ』かを分類すること。".to_string())
    );
}

#[test]
fn comparison_target_paragraph_is_none_without_a_compare_phrase() {
    assert!(comparison_target_paragraph("CHFS/FINCHFS を調べる。").is_none());
    assert!(comparison_target_paragraph("").is_none());
}

#[test]
fn aspects_ignore_a_single_item_parenthesized_note() {
    // 括弧の中身が 1 件だけなら列挙とみなさず既定リストに落ちる。
    assert_eq!(
        research_aspects("CHFS/FINCHFS を比較する（詳細は省略）。"),
        DEFAULT_ASPECTS
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
    );
}

/// Phase 109f: 実際に本番で使われた目的文（先頭段落）。`## 方針（人の指定、2026-09-23）` を
/// 足す前後で targets/aspects が変わらないことを、この定数を共有する 2 つのテストで確認する。
const REAL_OBJECTIVE_FIRST_PARAGRAPH: &str = "CHFS/FINCHFS/GekkoFS/UnifyFS/BeeOND/BeeGFS-on-demand(必要ならDAOS/Lustre)、Mochi-Margo-Mercury、UCX、io_uring・RDMA統合、low-overhead RPC・非同期runtime分野の近年の学術文献を調査する。各システムの目的・semantics・deployment model・server/core利用・data pathを整理し、BenchFSとの比較が『公平比較可能』か『背景比較のみ』かを分類すること。既存knowledge(projects/benchfs/architecture-overview.md)のPluvio/Locusta等の設計と対比できるよう根拠付きで書くこと。";

const REAL_OBJECTIVE_WITH_HUMAN_APPENDED_SECTION: &str = "CHFS/FINCHFS/GekkoFS/UnifyFS/BeeOND/BeeGFS-on-demand(必要ならDAOS/Lustre)、Mochi-Margo-Mercury、UCX、io_uring・RDMA統合、low-overhead RPC・非同期runtime分野の近年の学術文献を調査する。各システムの目的・semantics・deployment model・server/core利用・data pathを整理し、BenchFSとの比較が『公平比較可能』か『背景比較のみ』かを分類すること。既存knowledge(projects/benchfs/architecture-overview.md)のPluvio/Locusta等の設計と対比できるよう根拠付きで書くこと。\n\n## 方針（人の指定、2026-09-23）\n本文が有料で取得できない論文は、アブストラクト（OpenAlex / arXiv / Semantic Scholar のメタデータ）まで確認できれば妥協し、「本文未取得・アブストのみ」と明記する。";

fn expected_real_objective_targets() -> Vec<String> {
    [
        "CHFS",
        "FINCHFS",
        "GekkoFS",
        "UnifyFS",
        "BeeOND",
        "BeeGFS-on-demand",
        "Mochi-Margo-Mercury",
        "UCX",
        "io_uring",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn expected_real_objective_aspects() -> Vec<String> {
    [
        "目的",
        "semantics",
        "deployment model",
        "server/core利用",
        "data path",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

#[test]
fn real_objective_targets_exclude_the_optional_paren_and_the_glued_rdma_word() {
    // 実際の事故（Phase 109f、run 01M37A3JMMFXVY30SWD4EZ01JN）: `DAOS`/`Lustre` は
    // 「(必要なら…)」の中なので対象から除く。`RDMA` は `RDMA統合` という複合語の一部で
    // 単独の対象ではないので除く（`io_uring` は残る）。`Pluvio`/`Locusta`（既存 knowledge との対比
    // の対象。目的文の対象列とは別物）は最初の段落の最長の鎖ではないので入らない。
    assert_eq!(
        research_targets(REAL_OBJECTIVE_FIRST_PARAGRAPH),
        expected_real_objective_targets()
    );
}

#[test]
fn real_objective_aspects_come_from_the_enumerate_pattern_not_the_optional_paren() {
    assert_eq!(
        research_aspects(REAL_OBJECTIVE_FIRST_PARAGRAPH),
        expected_real_objective_aspects()
    );
}

#[test]
fn a_human_appended_heading_with_a_date_does_not_change_targets_or_aspects() {
    // 実際の事故（Phase 109f）: 目的文の末尾に人が足した「## 方針（人の指定、2026-09-23）」の
    // 括弧内が観点の列挙と誤認され、`research.json.aspects` が `["人の指定", "2026-09-23"]` に
    // なった。最初の段落だけを見るようにしたので、この節が増えても targets/aspects は変わらない。
    assert_eq!(
        research_targets(REAL_OBJECTIVE_WITH_HUMAN_APPENDED_SECTION),
        expected_real_objective_targets()
    );
    assert_eq!(
        research_aspects(REAL_OBJECTIVE_WITH_HUMAN_APPENDED_SECTION),
        expected_real_objective_aspects()
    );
}

#[test]
fn an_explicit_marker_form_for_both_targets_and_aspects_takes_priority() {
    let objective = "対象: Alpha/Beta（観点: latency、throughput、cost）を調べる。";
    assert_eq!(
        research_targets(objective),
        vec!["Alpha".to_string(), "Beta".to_string()]
    );
    assert_eq!(
        research_aspects(objective),
        vec![
            "latency".to_string(),
            "throughput".to_string(),
            "cost".to_string()
        ]
    );
}

#[test]
fn aspects_fall_back_to_default_when_only_a_date_and_a_meta_word_are_offered() {
    // 妥当性チェック単体の確認: 日付と付記の語だけの括弧書きは 2 件に満たない扱いになり、
    // 既定リストに落ちる。
    let objective = "CHFS/FINCHFS を比較する（人の指定、2026-09-23）。";
    assert_eq!(
        research_aspects(objective),
        DEFAULT_ASPECTS
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
    );
}

/// Phase 109f 追記（親からの追加観測、run `01M37AZ129EMB93N50MZ132S8K`）: 「対象:」の明示形でも、
/// `/` の前後に空白が入る書き方（`preamble.rs` が例示する `CHFS / FINCHFS / …` の形そのもの）だと
/// 区切りとして繋がらず、対象の列挙が空扱いになって一般走査にフォールバックし、代わりに観点の
/// 括弧の中の `server/core` がそれっぽい鎖として拾われ `targets = ["model", "server", "core"]` に
/// なっていた。観点も「観点: 目的」とラベルが残っていた。
const PRODUCTION_MARKER_LINE: &str = "対象: CHFS / FINCHFS / GekkoFS / UnifyFS / BeeOND / \
Mochi-Margo-Mercury / UCX / io_uring（観点: 目的、file semantics、deployment model、\
server/core 利用、data path、BenchFS との比較分類）";

fn expected_production_marker_targets() -> Vec<String> {
    [
        "CHFS",
        "FINCHFS",
        "GekkoFS",
        "UnifyFS",
        "BeeOND",
        "Mochi-Margo-Mercury",
        "UCX",
        "io_uring",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn expected_production_marker_aspects() -> Vec<String> {
    [
        "目的",
        "file semantics",
        "deployment model",
        "server/core 利用",
        "data path",
        "BenchFS との比較分類",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

#[test]
fn production_marker_line_with_spaces_around_slashes_is_parsed_correctly() {
    assert_eq!(
        research_targets(PRODUCTION_MARKER_LINE),
        expected_production_marker_targets()
    );
    assert_eq!(
        research_aspects(PRODUCTION_MARKER_LINE),
        expected_production_marker_aspects()
    );
}

#[test]
fn production_marker_line_is_unaffected_by_an_appended_human_section() {
    let objective = format!(
        "{PRODUCTION_MARKER_LINE}\n\n## 方針（人の指定、2026-09-23）\n本文が有料で取得できない\
論文は、アブストラクト（OpenAlex / arXiv / Semantic Scholar のメタデータ）まで確認できれば妥協し、\
「本文未取得・アブストのみ」と明記する。"
    );
    assert_eq!(
        research_targets(&objective),
        expected_production_marker_targets()
    );
    assert_eq!(
        research_aspects(&objective),
        expected_production_marker_aspects()
    );
}
