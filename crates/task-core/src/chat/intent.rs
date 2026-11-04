//! ADR 2026-11-04-project-controller §A4（付記 A4）: CoS thread への人の発言の「型」の推定。
//!
//! 相談（consult）・run 内の短い読取（read）・task への委譲（delegate）の 3 つ。人は型を選ばない。
//! daemon がこの関数で決めた型と規則 id を prompt の入力の横に hint として置き、LLM は文脈で覆せる。
//! 状態・認可・予算には効かない（起票の可否は `/cos/operations` の検査が決める）。純粋関数で、LLM も I/O も無い。

/// 推定した型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CosIntent {
    /// 判断・相談・推薦。task を作らずに返事する。
    Consult,
    /// 直近の状態の比較・集計・確認。run の中の短い読取で返事する。
    Read,
    /// コードの修正・実装・長い作業。task を起票し、review と成果統合の経路に渡す。
    Delegate,
}

impl CosIntent {
    pub fn as_str(self) -> &'static str {
        match self {
            CosIntent::Consult => "consult",
            CosIntent::Read => "read",
            CosIntent::Delegate => "delegate",
        }
    }

    /// prompt に出す日本語の名前。
    pub fn label(self) -> &'static str {
        match self {
            CosIntent::Consult => "相談（task を作らずに答える）",
            CosIntent::Read => "読取（run の中で読んで答える）",
            CosIntent::Delegate => "委譲（task を起票し、完了をこの thread に返す）",
        }
    }
}

/// 推定の結果。`rule_id` はどの規則が当たったか（監査・試験で読む）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CosIntentHint {
    pub intent: CosIntent,
    pub rule_id: &'static str,
}

/// 変更・実装を頼む語。読取の語より先に見る（「比較して直して」は委譲）。
const DELEGATE_WORDS: &[&str] = &[
    "修正して",
    "直して",
    "なおして",
    "実装して",
    "作って",
    "作成して",
    "追加して",
    "足して",
    "削除して",
    "消して",
    "変更して",
    "書き換えて",
    "リファクタ",
    "fix ",
    "implement",
];

/// 読んで答える語。
const READ_WORDS: &[&str] = &[
    "比較して",
    "比べて",
    "直近",
    "集計",
    "数えて",
    "件数",
    "一覧",
    "見せて",
    "調べて",
    "確認して",
    "読んで",
    "教えて",
    "list ",
    "compare",
];

/// 判断・相談の語。どれにも当たらなければ相談として扱う（task を作らないのが安全側）。
const CONSULT_WORDS: &[&str] = &[
    "どうするべき",
    "どうすべき",
    "どうしたら",
    "どう思う",
    "べき？",
    "べきか",
    "相談",
    "おすすめ",
    "どれがいい",
    "優先",
    "should",
];

/// 発言の型を推定する。
pub fn classify(text: &str) -> CosIntentHint {
    let lower = format!("{} ", text.to_lowercase());
    let hit = |words: &[&str]| words.iter().any(|w| lower.contains(w));
    if hit(DELEGATE_WORDS) {
        return CosIntentHint {
            intent: CosIntent::Delegate,
            rule_id: "intent/delegate-verb",
        };
    }
    if hit(READ_WORDS) {
        return CosIntentHint {
            intent: CosIntent::Read,
            rule_id: "intent/read-verb",
        };
    }
    if hit(CONSULT_WORDS) {
        return CosIntentHint {
            intent: CosIntent::Consult,
            rule_id: "intent/consult-question",
        };
    }
    CosIntentHint {
        intent: CosIntent::Consult,
        rule_id: "intent/consult-default",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn general_cos_consult_intent_classifies_the_three_acceptance_utterances() {
        let cases = [
            (
                "どうするべき？",
                CosIntent::Consult,
                "intent/consult-question",
            ),
            ("直近 3 件を比較して", CosIntent::Read, "intent/read-verb"),
            (
                "これを修正して",
                CosIntent::Delegate,
                "intent/delegate-verb",
            ),
            ("おはよう", CosIntent::Consult, "intent/consult-default"),
            (
                "直近の失敗を比較して直して",
                CosIntent::Delegate,
                "intent/delegate-verb",
            ),
            (
                "Please implement the parser",
                CosIntent::Delegate,
                "intent/delegate-verb",
            ),
        ];
        for (text, intent, rule) in cases {
            let hint = classify(text);
            assert_eq!((hint.intent, hint.rule_id), (intent, rule), "{text}");
        }
    }
}
