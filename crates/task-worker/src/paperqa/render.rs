//! 答えの引用判定と `report.md` の各節・`research.json` の組み立て（ADR-0035 D2・ADR-0063 D1）。
//! run の段（`run_paperqa`）から切り離した決定的な整形だけを置く。

use super::*;

/// 答えがこの候補を引用しているか（ADR-0035 D2。**決定的**。合わなければ `false`）。
///
/// PaperQA2 は `parsing.use_doc_details = false`（ADR-0027 の設定）だと**ファイル名**から引用の鍵を作るので、
/// 次のどれかが答えの中に現れれば引用とみなす: DOI / arXiv id（版番号なし）/ corpus のファイル名（拡張子なし）/
/// `著者姓+年`（`brinkmann2020`）/ 著者姓と年の両方 / 正規化したタイトル。
pub fn answer_cites(answer: &str, candidate: &Candidate) -> bool {
    let lower = answer.to_ascii_lowercase();
    let alnum = normalize_alnum(answer);

    let doi = normalize_doi(&candidate.doi);
    if !doi.is_empty() && lower.contains(&doi) {
        return true;
    }
    let arxiv = strip_arxiv_version(&candidate.arxiv_id.to_ascii_lowercase());
    if arxiv.len() >= 6 && lower.contains(&arxiv) {
        return true;
    }
    if !candidate.file.is_empty() {
        let stem = candidate.file.trim_end_matches(".pdf").to_ascii_lowercase();
        if stem.len() >= 4 && lower.contains(&stem) {
            return true;
        }
    }
    let surname = first_author_surname(&candidate.authors);
    if let (false, Some(year)) = (surname.is_empty(), candidate.year) {
        let year = year.to_string();
        if alnum.contains(&format!("{surname}{year}")) {
            return true;
        }
        if lower.contains(&surname) && lower.contains(&year) {
            return true;
        }
    }
    let title = normalize_alnum(&candidate.title);
    if title.len() >= 12 && alnum.contains(&title) {
        return true;
    }
    false
}

pub(super) fn normalize_alnum(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase()
}

pub(super) fn normalize_doi(doi: &str) -> String {
    let mut text = doi.trim().to_ascii_lowercase();
    for prefix in ["https://doi.org/", "http://doi.org/", "doi:"] {
        if let Some(rest) = text.strip_prefix(prefix) {
            text = rest.to_string();
        }
    }
    text.trim_matches('/').to_string()
}

pub(super) fn strip_arxiv_version(id: &str) -> String {
    match id.rfind('v') {
        Some(pos) if id[pos + 1..].chars().all(|c| c.is_ascii_digit()) && pos + 1 < id.len() => {
            id[..pos].to_string()
        }
        _ => id.to_string(),
    }
}

pub(super) fn first_author_surname(authors: &[String]) -> String {
    let Some(first) = authors.first() else {
        return String::new();
    };
    first
        .split_whitespace()
        .next_back()
        .map(|s| {
            s.chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .collect::<String>()
                .to_ascii_lowercase()
        })
        .unwrap_or_default()
}

/// `artifacts/answer.md` の末尾に足す `## 出典`（ADR-0035 D4）。引用されたものを先に、
/// `[n] 著者 (年). タイトル. venue. URL` の形で並べる（欠けている項目は飛ばす）。
pub(super) fn render_sources_section(candidates: &[(Candidate, bool)]) -> String {
    let mut ordered: Vec<&(Candidate, bool)> =
        candidates.iter().filter(|(_, cited)| *cited).collect();
    ordered.extend(candidates.iter().filter(|(_, cited)| !*cited));

    let mut out = String::from("\n\n## 出典\n\n");
    if ordered.is_empty() {
        out.push_str("(取得できた文献なし)\n");
        return out;
    }
    for (index, (candidate, cited)) in ordered.iter().enumerate() {
        let mut parts: Vec<String> = Vec::new();
        let authors = match candidate.authors.len() {
            0 => String::new(),
            1..=3 => candidate.authors.join(", "),
            _ => format!("{} et al.", candidate.authors[0]),
        };
        if !authors.is_empty() {
            parts.push(match candidate.year {
                Some(year) => format!("{authors} ({year})"),
                None => authors,
            });
        } else if let Some(year) = candidate.year {
            parts.push(format!("({year})"));
        }
        if !candidate.title.is_empty() {
            parts.push(candidate.title.clone());
        }
        if !candidate.venue.is_empty() {
            parts.push(candidate.venue.clone());
        }
        let url = if !candidate.url.is_empty() {
            &candidate.url
        } else {
            &candidate.pdf_url
        };
        if !url.is_empty() {
            parts.push(url.clone());
        }
        let marker = match (*cited, candidate.abstract_only) {
            (true, true) => " (引用・アブストのみ)",
            (true, false) => " (引用)",
            (false, _) => "",
        };
        out.push_str(&format!("[{}] {}{}\n", index + 1, parts.join(". "), marker));
    }
    out
}

/// ADR-0063 Phase 109d C4: `answer.md`/`report.md` の先頭に置く「# 対象別の整理」
/// （対象×観点の表 + 対象ごとの節 + 総括）。対象が 1 件も取れていなければ空文字列
/// （呼び出し側はフォールバックの単一の答えをそのまま先頭に置く）。
///
/// ADR-0063 Phase 109g A: 総括の節の見出しは、比較先（`comparison_target`）が分かっていれば
/// 「## <比較先> との比較分類」、無ければ従来どおり「## 総括」。
pub(super) fn render_target_sections(
    answers: &[AskAnswer],
    table: &str,
    comparison_target: Option<&str>,
) -> String {
    if !answers.iter().any(|a| a.target.is_some()) {
        return String::new();
    }
    let mut out = String::from("# 対象別の整理\n\n");
    if !table.trim().is_empty() {
        out.push_str(table.trim_end());
        out.push_str("\n\n");
    }
    for answer in answers.iter().filter(|a| a.target.is_some()) {
        out.push_str(&format!(
            "### {}\n\n",
            answer.target.as_deref().unwrap_or_default()
        ));
        if answer.answer.trim().is_empty() {
            out.push_str("(回答なし");
            if let Some(err) = &answer.error {
                out.push_str(&format!(": {err}"));
            }
            out.push_str(")\n\n");
        } else {
            out.push_str(answer.answer.trim());
            out.push_str("\n\n");
        }
    }
    if let Some(summary) = answers.iter().find(|a| a.id == "summary") {
        match comparison_target {
            Some(target) if !target.trim().is_empty() => {
                out.push_str(&format!("## {target} との比較分類\n\n"));
            }
            _ => out.push_str("## 総括\n\n"),
        }
        if summary.answer.trim().is_empty() {
            out.push_str("(回答なし");
            if let Some(err) = &summary.error {
                out.push_str(&format!(": {err}"));
            }
            out.push_str(")\n\n");
        } else {
            out.push_str(summary.answer.trim());
            out.push_str("\n\n");
        }
    }
    out
}

/// ADR-0063 Phase 109d C4: 「## 引用された文献（contexts）」節。`docname`（無ければ `dockey`）で
/// 重複排除し、その文献がどの問い（`AskAnswer::id`）で使われたかを列挙する。
pub(super) fn render_contexts_section(answers: &[AskAnswer]) -> String {
    let mut by_key: BTreeMap<String, (String, String, Vec<String>)> = BTreeMap::new();
    for answer in answers {
        for ctx in &answer.contexts {
            let key = if !ctx.docname.is_empty() {
                ctx.docname.clone()
            } else {
                ctx.dockey.clone()
            };
            if key.is_empty() {
                continue;
            }
            let entry = by_key
                .entry(key)
                .or_insert_with(|| (ctx.docname.clone(), ctx.citation.clone(), Vec::new()));
            if !entry.2.contains(&answer.id) {
                entry.2.push(answer.id.clone());
            }
        }
    }
    if by_key.is_empty() {
        return String::new();
    }
    let mut out = String::from("\n\n## 引用された文献（contexts）\n\n");
    for (key, (docname, citation, ids)) in &by_key {
        let name = if !docname.is_empty() { docname } else { key };
        let label = if !citation.trim().is_empty() {
            citation.trim()
        } else {
            name.as_str()
        };
        out.push_str(&format!("- {name}: {label}（{}）\n", ids.join(", ")));
    }
    out
}

/// `answer.md` に足す「一次情報（実装）」節（ADR-0063 D1）。目的文中の GitHub / GitLab の URL は
/// PaperQA の corpus には入れず（論文ではないため）、参照節に載せるだけ。
pub(super) fn render_primary_sources_section(urls: &[String]) -> String {
    if urls.is_empty() {
        return String::new();
    }
    let mut out = String::from("\n\n## 一次情報（実装）\n\n");
    for url in urls {
        out.push_str(&format!("- {url}\n"));
    }
    out
}

/// ADR-0063 D1: `research.json` の `evidence`（`insufficient_is_error` の値に関わらず、acquire が
/// 動いた run では必ず書く）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(super) struct EvidenceSummary {
    pub(super) cited: u32,
    pub(super) cited_fulltext: u32,
    pub(super) cited_abstract_only: u32,
    /// ADR-0063 Phase 109d C2: `cited` のうち、`ask()` が実際に使った証拠（`PQASession.contexts` の
    /// `docname`/`dockey`）と突き合わせられた件数（本文一致だけで数えたものは含まない）。
    pub(super) cited_from_contexts: u32,
    pub(super) min_cited: u32,
    pub(super) insufficient: bool,
}

/// `evidence_gate` の結果。`error` が `Some` なら run は `Terminal::Error{retryable: true}`
/// （検索が 0 件、または `insufficient_is_error = true` で閾値未達）。`summary` は常に埋まる
/// （`research.json` に書く。ADR-0063 D1）。
pub(super) struct EvidenceGateResult {
    pub(super) error: Option<String>,
    pub(super) summary: EvidenceSummary,
}

/// `artifacts/research.json` を書く（ADR-0063 D1。取得の段が動いた run では常に書く）。
/// ADR-0063 Phase 109c A: 目的文から取れた対象・観点も残す（監査・reviewer の参考用）。
/// ADR-0063 Phase 109h: `dropped_targets`/`comparison_page`/`comparison_context_chars`
/// （`ask_output.json` からそのまま写す。観測性）も残す。
#[allow(clippy::too_many_arguments)]
pub(super) async fn write_research_json(
    path: &Path,
    summary: &EvidenceSummary,
    targets: &[String],
    aspects: &[String],
    dropped_targets: &[String],
    comparison_page: Option<&str>,
    comparison_context_chars: u32,
    run_id: &str,
) {
    let value = serde_json::json!({
        "evidence": summary,
        "targets": targets,
        "aspects": aspects,
        "dropped_targets": dropped_targets,
        "comparison_page": comparison_page,
        "comparison_context_chars": comparison_context_chars,
    });
    match serde_json::to_string_pretty(&value) {
        Ok(text) => {
            if let Err(e) = tokio::fs::write(path, format!("{text}\n")).await {
                warn!("run {run_id}: could not write artifacts/research.json: {e}");
            }
        }
        Err(e) => warn!("run {run_id}: could not serialize artifacts/research.json: {e}"),
    }
}

/// `answer.md` に足す「証拠の質」節（ADR-0063 D1: 本文 / アブストのみの内訳を必ず書く）。
/// ADR-0063 Phase 109h: `dropped_targets` が非空なら「max_asks の制限で問えなかった」対象を書く
/// （総括〈比較分類〉の問いを必ず残すため後ろから詰められた対象。観測性）。
pub(super) fn render_evidence_section(
    summary: &EvidenceSummary,
    dropped_targets: &[String],
) -> String {
    let mut out = String::from("\n\n## 証拠の質\n\n");
    out.push_str(&format!(
        "- 引用された出典: {} 件（本文からの引用: {} 件、アブストラクトのみ: {} 件、\
         うち PaperQA の証拠〈contexts〉との突き合わせ: {} 件）\n",
        summary.cited,
        summary.cited_fulltext,
        summary.cited_abstract_only,
        summary.cited_from_contexts
    ));
    if summary.insufficient {
        out.push_str(&format!(
            "- 証拠不足（cited={} < min {}）。代替案: Web 調査（`web-research` / Local Deep Research）に \
             切り替えるか、人が著者版 PDF の URL を与えてください。\n",
            summary.cited, summary.min_cited
        ));
    }
    if !dropped_targets.is_empty() {
        out.push_str(&format!(
            "- 対象 {} 件は max_asks の制限で問えなかった: {}\n",
            dropped_targets.len(),
            dropped_targets.join("、")
        ));
    }
    out
}
