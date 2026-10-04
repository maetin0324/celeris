//! ADR-0047 付記（2026-10-04）H1〜H3: `sources` の人の印の意味と判定（純粋。I/O 無し）。
//!
//! - [`SOURCE_HUMAN_AUTHORED`] / front matter の `author: human` — 人が書いた・直接編集したページ（保護）
//! - [`SOURCE_HUMAN_INSTRUCTION`] — 人の指示・発言に由来する事実（run が書く。保護しない）
//! - [`SOURCE_HUMAN_LEGACY`]（単独）/ 単数形 `source: human` — 移行前の未判別（安全側に保護）

use super::{front_matter, yaml_list};

/// 人が書いた・人が直接編集したページの印（`sources` の要素）。
pub const SOURCE_HUMAN_AUTHORED: &str = "human:authored";
/// 人の指示・発言に由来する事実の印（`sources` の要素。同じ `sources` に `task:<id>` を添える）。
pub const SOURCE_HUMAN_INSTRUCTION: &str = "human:instruction";
/// 旧形の `human`（意味が混ざっていた。移行で判別できなかったものだけが残る）。
pub const SOURCE_HUMAN_LEGACY: &str = "human";

/// front matter の生の行（`---` の間）。閉じていなければ空。
fn front_lines(raw: &str) -> Vec<&str> {
    let body = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    let Some(rest) = body
        .strip_prefix("---\n")
        .or_else(|| body.strip_prefix("---\r\n"))
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in rest.lines() {
        let text = line.trim_end_matches('\r');
        if text.trim_end() == "---" || text.trim_end() == "..." {
            return out;
        }
        out.push(text);
    }
    Vec::new()
}

fn key_is(line: &str, key: &str, value: &str) -> bool {
    line.split_once(':').is_some_and(|(k, v)| {
        k.trim().eq_ignore_ascii_case(key)
            && v.trim()
                .trim_matches(['"', '\''])
                .eq_ignore_ascii_case(value)
    })
}

/// 人が書いたページの印があるか（`sources` の `human:authored` か front matter の `author: human`）。
pub fn human_authored(raw: &str) -> bool {
    front_matter(raw)
        .0
        .sources
        .iter()
        .any(|s| s == SOURCE_HUMAN_AUTHORED)
        || front_lines(raw)
            .iter()
            .any(|line| key_is(line, "author", "human"))
}

/// 移行前の未判別の印（`sources` の単独 `human` か単数形 `source: human`）があるか。
pub fn legacy_human(raw: &str) -> bool {
    front_matter(raw)
        .0
        .sources
        .iter()
        .any(|s| s == SOURCE_HUMAN_LEGACY)
        || front_lines(raw)
            .iter()
            .any(|line| key_is(line, "source", "human"))
}

/// ADR-0047 付記 H3: 自動の整理（削除・統合・大幅書き換え）から守るページか。
/// `user/` 配下、人が書いた印、未判別の旧形。`human:instruction` だけでは守らない。
pub fn protected_page(path: &str, raw: &str) -> bool {
    path.starts_with("user/") || human_authored(raw) || legacy_human(raw)
}

/// ADR-0047 付記 H2: run（知識整理 run・`record`・GC）が書く候補の出典を正す。
///
/// - `human` と `human:authored` は `human:instruction` に置き換える（run は人が書いた印を作れない）。
///   ただし対象の既存ページの `sources`（`existing`）に同じ印があれば残す（出典を保つ `update` で保護を落とさない）
/// - `human:instruction` があり `task_id` が分かれば `task:<id>` を添える
/// - 空白を除き、重複を落とす（順序は保つ）
pub fn normalize_agent_sources(
    sources: &[String],
    existing: &[String],
    task_id: Option<&str>,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for source in sources {
        let source = source.trim();
        if source.is_empty() {
            continue;
        }
        let kept = if (source == SOURCE_HUMAN_LEGACY || source == SOURCE_HUMAN_AUTHORED)
            && !existing.iter().any(|e| e == source)
        {
            SOURCE_HUMAN_INSTRUCTION
        } else {
            source
        };
        if !out.iter().any(|s| s == kept) {
            out.push(kept.to_string());
        }
    }
    if let Some(task_id) = task_id.map(str::trim).filter(|t| !t.is_empty())
        && out.iter().any(|s| s == SOURCE_HUMAN_INSTRUCTION)
    {
        let task = format!("task:{task_id}");
        if !out.contains(&task) {
            out.push(task);
        }
    }
    out
}

/// front matter の `sources` だけを `sources` で置き換えた原文を返す（他の行・本文は 1 バイトも変えない）。
/// `sources` が無ければ閉じの `---` の直前に足す。front matter が無い・閉じていなければ `None`。
pub fn replace_sources(raw: &str, sources: &[String]) -> Option<String> {
    let prefix = "---\n";
    let rest = raw.strip_prefix(prefix)?;
    let rendered = format!("sources: [{}]\n", yaml_list(sources));
    let mut head = String::new();
    let mut in_sources = false;
    let mut replaced = false;
    let mut consumed = 0usize;
    let mut closed = false;
    for line in rest.split_inclusive('\n') {
        let text = line.trim_end_matches(['\n', '\r']);
        if text.trim_end() == "---" || text.trim_end() == "..." {
            closed = true;
            break;
        }
        consumed += line.len();
        if in_sources && text.trim_start().starts_with("- ") {
            continue;
        }
        in_sources = false;
        if text
            .split_once(':')
            .is_some_and(|(k, _)| k.trim().eq_ignore_ascii_case("sources") && !k.starts_with(' '))
        {
            in_sources = true;
            if !replaced {
                head.push_str(&rendered);
                replaced = true;
            }
            continue;
        }
        head.push_str(line);
    }
    if !closed {
        return None;
    }
    if !replaced {
        head.push_str(&rendered);
    }
    Some(format!("{prefix}{head}{}", &rest[consumed..]))
}
