use serde::{Deserialize, Serialize};

/// 長さ上限（文字数）。全履歴・全 repo は送らない。
pub const MAX_INSTRUCTION_CHARS: usize = 4000;
pub const MAX_ITEM_CHARS: usize = 400;
pub const MAX_ITEMS: usize = 8;
pub const MAX_FAILURE_CHARS: usize = 800;

/// 分類の入力。構造化済みの事実（genre・check 有無・依存完了・job 終了・残予算）は含めない。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClassifyInput {
    pub instruction: String,
    pub acceptance: Vec<String>,
    pub required_artifacts: Vec<String>,
    pub dependency_summaries: Vec<String>,
    pub recent_failure: Option<String>,
}

const SECRET_PREFIXES: &[&str] = &[
    "sk-",
    "sk_",
    "ghp_",
    "gho_",
    "ghs_",
    "github_pat_",
    "xoxb-",
    "xoxp-",
    "xapp-",
    "AKIA",
    "glpat-",
    "or-v1-",
];
const SECRET_KEY_WORDS: &[&str] = &[
    "key",
    "token",
    "secret",
    "password",
    "passwd",
    "credential",
    "auth",
];
const REDACTED: &str = "[redacted]";

fn trim_punct(w: &str) -> &str {
    w.trim_matches(|c: char| {
        matches!(
            c,
            ',' | ';' | '"' | '\'' | '`' | ')' | '(' | '<' | '>' | '.'
        )
    })
}

fn looks_like_blob(w: &str) -> bool {
    w.len() >= 32
        && w.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '+' | '/' | '='))
        && w.chars().any(|c| c.is_ascii_digit())
        && w.chars().any(|c| c.is_ascii_alphabetic())
        && !w.contains('/')
}

fn is_email(w: &str) -> bool {
    match w.split_once('@') {
        Some((l, r)) => {
            !l.is_empty() && r.contains('.') && !r.starts_with('.') && !r.ends_with('.')
        }
        None => false,
    }
}

fn redact_word(w: &str, prev_bearer: bool) -> Option<String> {
    let t = trim_punct(w);
    if t.is_empty() {
        return None;
    }
    if prev_bearer
        || SECRET_PREFIXES.iter().any(|p| t.starts_with(p))
        || looks_like_blob(t)
        || is_email(t)
    {
        return Some(w.replacen(t, REDACTED, 1));
    }
    // KEY=value / token: value 形式（同じ語内）
    if let Some(i) = t.find(['=', ':']) {
        let (lhs, rhs) = (&t[..i], &t[i + 1..]);
        let l = lhs.to_ascii_lowercase();
        if !rhs.is_empty() && SECRET_KEY_WORDS.iter().any(|k| l.contains(k)) {
            return Some(w.replacen(t, &format!("{lhs}{}{REDACTED}", &t[i..=i]), 1));
        }
    }
    None
}

/// 秘密らしき値・メールアドレスを `[redacted]` に置き換える（行構造は保つ）。
pub fn redact_secrets(text: &str) -> String {
    text.lines()
        .map(|line| {
            let mut prev_bearer = false;
            line.split(' ')
                .map(|w| {
                    let out = redact_word(w, prev_bearer).unwrap_or_else(|| w.to_string());
                    prev_bearer = trim_punct(w).eq_ignore_ascii_case("bearer");
                    out
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn clip(s: &str, max: usize) -> String {
    let r = redact_secrets(s.trim());
    if r.chars().count() <= max {
        r
    } else {
        let mut t: String = r.chars().take(max).collect();
        t.push('…');
        t
    }
}

fn clip_list(v: &[String]) -> Vec<String> {
    v.iter()
        .take(MAX_ITEMS)
        .map(|s| clip(s, MAX_ITEM_CHARS))
        .filter(|s| !s.is_empty())
        .collect()
}

impl ClassifyInput {
    /// 長さ上限で切り詰め、秘密らしき値を除去した写し。送信前に必ず通す。
    pub fn sanitized(&self) -> ClassifyInput {
        ClassifyInput {
            instruction: clip(&self.instruction, MAX_INSTRUCTION_CHARS),
            acceptance: clip_list(&self.acceptance),
            required_artifacts: clip_list(&self.required_artifacts),
            dependency_summaries: clip_list(&self.dependency_summaries),
            recent_failure: self
                .recent_failure
                .as_deref()
                .map(|s| clip(s, MAX_FAILURE_CHARS))
                .filter(|s| !s.is_empty()),
        }
    }

    /// Decisions API の `state`（文字列の配列）。
    pub fn to_state(&self) -> Vec<String> {
        let s = self.sanitized();
        let mut state = vec![format!("instruction: {}", s.instruction)];
        state.extend(s.acceptance.iter().map(|a| format!("acceptance: {a}")));
        state.extend(
            s.required_artifacts
                .iter()
                .map(|a| format!("required_artifact: {a}")),
        );
        state.extend(
            s.dependency_summaries
                .iter()
                .map(|a| format!("dependency_result: {a}")),
        );
        if let Some(f) = &s.recent_failure {
            state.push(format!("recent_failure: {f}"));
        }
        state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_known_secret_shapes() {
        let t = "use sk-abcdef123456 and Authorization: Bearer abc.def mail me@example.com API_KEY=hunter2 \
                 blob 0123456789abcdef0123456789abcdef0123";
        let r = redact_secrets(t);
        for leak in [
            "sk-abcdef",
            "abc.def",
            "me@example.com",
            "hunter2",
            "0123456789abcdef0123",
        ] {
            assert!(!r.contains(leak), "{leak} leaked: {r}");
        }
        assert!(r.contains("API_KEY=[redacted]"));
    }

    #[test]
    fn plain_text_and_paths_survive() {
        let t = "fix crates/task-core/src/wait.rs so tests pass";
        assert_eq!(redact_secrets(t), t);
    }

    #[test]
    fn truncates_and_bounds_lists() {
        let input = ClassifyInput {
            instruction: "あ".repeat(10_000),
            acceptance: (0..20).map(|i| format!("a{i}")).collect(),
            recent_failure: Some("x".repeat(5000)),
            ..Default::default()
        };
        let s = input.sanitized();
        assert_eq!(s.instruction.chars().count(), MAX_INSTRUCTION_CHARS + 1);
        assert_eq!(s.acceptance.len(), MAX_ITEMS);
        assert!(s.recent_failure.unwrap().chars().count() <= MAX_FAILURE_CHARS + 1);
    }

    #[test]
    fn state_has_only_given_fields() {
        let input = ClassifyInput {
            instruction: "do it".into(),
            ..Default::default()
        };
        assert_eq!(input.to_state(), vec!["instruction: do it".to_string()]);
    }
}
