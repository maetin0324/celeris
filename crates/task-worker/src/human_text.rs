//! 人が読む文面の規則（ADR 2026-10-27-yomiyasu-human-text D2）。
//!
//! 日本語の推敲 skill yomiyasu（`config/skills/yomiyasu`、upstream v2.4.0）の要点を、Celeris の
//! 報告・質問・返事に要る分だけ短くしたもの。skill の本体（SKILL.md だけで約 121 KB）は run に
//! 載せない。同じ文を worker の結果の指示（summary・question）と CoS の Core に入れ、決定の要求には
//! 選択肢の label の規則を足す。文を変えたら `cos_policy_version`（`[cos.triage] policy_version`）を上げ、
//! `python3 scripts/dev/check-human-text.py` を通す。

/// worker と CoS に共通の、人が読む文面の規則。
pub const HUMAN_TEXT_RULES: &str = "## 人が読む文面（yomiyasu の要点）\n\
     人が読む文（summary・question・decisions の問いと選択肢・報告・返事）は、日本語の推敲 skill yomiyasu の要点に沿って書く。\n\
     - 結論と、人が決めること・やることを最初に書く。\n\
     - 主語と述語をはっきり書く（誰が何をしたか、誰が何をするか）。\n\
     - 比喩や「手触り」「解像度」「地図を描く」のような意味のぼやけた語を使わない。\n\
     - 数値・条件・task ID・コマンド・次の手を具体的に書く。太字・見出し・箇条書きは要る所だけに使う。\n\
     - 意味・条件・数値・未確認の事項を変えない。確かめていないことは「未確認」と書き、書いていないことを足さない。\n\n";

/// 決定の要求（`decisions`）の問いと選択肢に足す規則。
pub const DECISION_TEXT_RULES: &str = "問い（`question`）は「何を決めるか」と、決めないと何が止まるかを 1〜2 文で書く。\
     選択肢の `label` は、選んだら何が起きるかが分かる短い句にする（「A案」だけにしない）。\n\n";
