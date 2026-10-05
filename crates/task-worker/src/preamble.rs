//! プロンプトの前置きを 1 か所で組む（ADR-0033 D4 / D6 / D5、Phase 24 / 26）。
//!
//! 「人」らしさは**注入される記憶と brief** で作る（ADR-0033 D6）。ハーネスのプロセスは相変わらず
//! ステートレスで、状態はファイルと DB にある（ADR-0001 D2 原則 2）。ここは純粋関数だけで、I/O も LLM も無い。
//!
//! 並び（ADR-0033 D4 / Phase 24 の指示。Phase 30 で 1 の直後に「仕事で使う道具」を追加。
//! Phase 33 で 3 の直後に「あなたの直近の仕事」を追加。Phase 53 / ADR-0044 D2 で**先頭に**
//! 「コメント」を追加）:
//! 0. コメント（`context.comments` / `context.interrupt`。ADR-0044 D2: 人がコメントで run を止めたら、
//!    次の run の**先頭**に「**人からの割り込み**: …」として出す。続けてコメントの糸を最新 20 件）
//! 1. 役職と brief（`context.node`）＋ 対話 run で担当が自分の仕事の分野を持つときは「仕事で使う道具」
//!    （`context.work_genre`。Phase 30: 対話は常に対話用分野で走るが、その人が自分の得意分野を知って
//!    答えられるように 1 行足す）
//! 2. 永続の認可（`context.standing_rules`。SPEC §3.6「永続の認可は文字で記録してエージェントに注入する」。
//!    Phase 26 が埋める: 担当宛て + 全員向け）
//! 3. 記憶（`context.memory`）
//! 4. あなたの直近の仕事（`context.recent_work`。Phase 33: 実機で対話 run の担当が自分の直近の失敗を
//!    知らずに「対象タスク ID が必要です」と聞き返した事故の再発防止。対話 run にだけ出す）
//! 5. 途中目標のここまでの結果（`context.milestone_review`。Phase 41 / ADR-0038 D1: 途中目標レビューの
//!    対話 run にだけ出す。その途中目標と、属する仕事の終わり方・成果物の抜粋。以下は 1 つずつ繰り下がる）
//! 5. 直近のやり取り（`context.conversation`）
//! 6. 役割の指示文（`context.role`。ADR-0016 D1 からある既存の節）
//! 7. 記憶の書き方の指示（記憶が有効な run にだけ）と、コメントの書き方
//!    （`context.comments_enabled` の run にだけ。ADR-0044 D2）
//! 8. 対話専用の指示（`context.conversation_addressee`。Phase 28: 対話 run は返事だけをする。
//!    末尾に足す。ADR-0033 D4 追記。Phase 33 で「自分の直近の仕事を先に見ること」を一文追加）
//!
//! 検索ハーネス（`local-deep-research`）はこの前置きを**一切使わない**（ADR-0029 / ADR-0033 D6:
//! 検索に渡す問いを濁さないため。Phase 27 の監査 M-2）。
//!
//! `RunContext` が既定値（Phase 23 までの中身しか無い）のときの出力は、Phase 23 の
//! `claude_code::prompt_header` が出していた文字列と**バイト単位で同じ**になる（既存テストがそれを見る）。

use task_core::MessageRole;

use crate::protocol::{ConversationAddressee, MilestoneReviewContext, RunContext};

/// ADR-0079 D7（Phase R3a）: leaf の前置きの「人の決定」節の見出し（子 task の objective の末尾と同じ）。
pub const HUMAN_DECISIONS_HEADING: &str = task_core::decision::DECISIONS_HEADING;

/// ADR-0079 D7（Phase R3a）: 木の節点の worker の run（`context.decision_requests`）にだけ出す、
/// `result.json` の `decisions` で人への決定の要求を出す方法。
pub fn decision_requests_section(artifacts: &str) -> String {
    format!(
        "## 人への決定の要求（ADR-0079 D7）\n\
         人が選ぶべき点（方式・範囲・後戻りしにくい選択）に行き当たったら、自由文の質問（`question`）で止まる代わりに、\
         `{artifacts}/result.json` に `decisions` の配列を書けます（1 件 = \
         `{{\"key\": \"<a-z0-9->\", \"question\": \"…\", \"options\": [{{\"key\": \"…\", \"label\": \"…\"}}, …], \
         \"recommended\": \"<option key>\", \"cost_of_reversal\": \"low|medium|high\", \"needed_before\": [\"self\" | \"<unit key>\" | \"stage:<key>\"]}}`、\
         選択肢は 2〜5 件）。`needed_before: [\"self\"]` はこの仕事を人の答えが出るまで止めます（答えは次の run の前置きに入ります）。\
         他の unit を指せばその unit だけが待ち、この run の完了は妨げません。数が多すぎると 1 件に束ねられます。\n\n"
    )
}

/// 前置き（役割の指示文を含む）。`claude-code` / `codex` / `acp` / `paperqa` が使う。
/// `artifacts` は成果物ディレクトリの workspace 相対表記（`RunRequest::artifacts_rel`。ADR-0036 D3。
/// 単独タスクでは `artifacts` なので出力は Phase 34 までとバイト単位で同じ）。
/// ADR-0074 D1.2（Phase F2b）: v2 の WU の run が WU ごとの worktree で走るときの節（作業ブランチに
/// commit してよい・push しない・並行する他の WU のファイルに触らない）。`branch` が無い run では空
/// （v1・並列 1・atomic のプロンプトは 1 バイトも変わらない）。
pub fn work_unit_branch_section(wu: &crate::protocol::WorkUnitPromptContext) -> String {
    let Some(branch) = &wu.branch else {
        return String::new();
    };
    let mut out = String::from("## 作業ブランチ（並列の WorkUnit）\n");
    out.push_str(&format!(
        "- この WorkUnit は専用のブランチ `{branch}` の作業ツリーで走っています。変更はこのブランチに commit してかまいません（push はしないこと）。終わった時点で残っている変更は celeris が commit します。\n"
    ));
    out.push_str(
        "- 同じ工程の他の WorkUnit が別の作業ツリーで並行して走っています。**他の WorkUnit が担当するファイルには触らないでください**（自分の Objective の範囲だけを変更する）。工程の最後に celeris が各ブランチを決定的に merge します。\n",
    );
    // ADR-0074 付記 2026-10-05 D3: 範囲 check の基点・統合先。worker も同じ check を自分で流せるように。
    out.push_str(
        "- この run の環境には `CELERIS_WU_BASE`（この WorkUnit の base commit）と `CELERIS_WU_TARGET`（統合先のブランチ）が入っています。範囲 check（`\"scope\":true`）は同じ変数を使うので、done を返す前に同じコマンドを自分で走らせて範囲外の path が無いことを確かめてください。\n",
    );
    if !wu.parallel_siblings.is_empty() {
        out.push_str("- 並行しうる WorkUnit:\n");
        for s in &wu.parallel_siblings {
            out.push_str(&format!("  - {s}\n"));
        }
    }
    out.push('\n');
    out
}

pub fn render(context: &RunContext, artifacts: &str) -> String {
    // ADR-0044 D2（Phase 53）: コメントは**前置きの先頭**（人が割り込んだら最初に目に入る）。
    // コメントが 1 件も無ければ何も出さないので、Phase 52 までの出力とバイト単位で同じ。
    let mut out = comments_section(context);
    out.push_str(&person_sections(context));
    // ADR-0054 D1（Phase 67）: 継続セッションの差分、または新規セッションの前のセッションの要約。
    // `context.session_diff` が空の run の前置きは Phase 66 までと 1 バイトも変わらない。
    out.push_str(&session_diff_section(context));
    // ADR-0046 D1 / D4（Phase 59）: 実効 profile（能力・方針・道具・知識・ハーネス）と進め方。
    // どちらも `None` / 空なら節ごと出さないので、Phase 58 までの出力とバイト単位で同じ。
    // 61（ADR-0047 の知識の索引）は `knowledge_section` を別に足す。ここには入れない。
    out.push_str(&profile_section(context));
    out.push_str(&mode_section(context));
    out.push_str(&organization_section(context));
    // ADR-0048 D3（Phase 60b）: CoS の対話 run にだけ、進行中の案件とその途中目標。
    // `context.active_projects` が空の run（CoS 以外）の前置きは Phase 60a までとバイト単位で同じ。
    out.push_str(&active_projects_section(context));
    // ADR-0059 D6（Phase 99）: CoS の対話 run にだけ、クラスタの一覧（id・接続状態・実効 work_dir）。
    // `context.clusters` が空の run の前置きは Phase 98 までと 1 バイトも変わらない。
    out.push_str(&clusters_section(context));
    out.push_str(&workspace_section(context));
    // ADR-0047 D2（Phase 61）: マウントされた知識の**索引だけ**（本文は入れない。道具で読む）。
    // `context.knowledge` が無い run の前置きは Phase 60 までと 1 バイトも変わらない。
    if let Some(knowledge) = &context.knowledge {
        out.push_str(&knowledge_section(&knowledge.mounts, &knowledge.index));
    }
    out.push_str(&role_section(context));
    out.push_str(&deliverables_placement_note());
    out.push_str(&production_host_note());
    out.push_str(&memory_instructions(context, artifacts));
    // ADR-0044 D2（Phase 53）: コメントの書き方（`comments_enabled` の run にだけ）。
    out.push_str(&comment_instructions(context));
    out.push_str(&conversation_instructions(context));
    // Phase 41（ADR-0038 D1）: 途中目標レビューの対話 run には、対話の指示のさらに後ろに
    // 「結果 → 達成の可否 → 次の提案」の指示を足す（対話の指示は消さない）。
    if let Some(review) = &context.milestone_review {
        out.push_str(&milestone_review_instructions(review));
    }
    out
}

/// ADR-0072 D9（Phase E1）: 「続きの実行（continuation）」の節。`context.continuation` が `None`
/// （継続でない run）なら空文字列を返し、呼び出し側の出力は 1 バイトも増えない（(c) の要件）。
/// `execute` run（`build_execute_prompt`）から呼ばれる（Plan/Review には無い）。
pub fn continuation_section(context: &RunContext) -> String {
    let Some(cont) = &context.continuation else {
        return String::new();
    };
    let mut out = format!("## 続きの実行（Run #{}）\n", cont.run_seq);
    out.push_str(&format!(
        "これは新しい session です。前の Run の会話は引き継がれていません。前の Run は{}で\
         終わりました。下の checkpoint と作業ツリーの現状から再開してください。完了済みの作業は\
         やり直さないこと。最初に `git status` と `git diff --stat` で現状を確かめてください。\n",
        cont.previous_end
    ));
    out.push_str(&format!(
        "### checkpoint（Run #{} の終わり）\n",
        cont.run_seq.saturating_sub(1)
    ));
    out.push_str(&checkpoint_bullets(&cont.checkpoint));
    if !cont.prior_runs.is_empty() {
        out.push_str("### これまでの Run（1 行ずつ）\n");
        for line in &cont.prior_runs {
            out.push_str(&format!("- {line}\n"));
        }
    }
    if let Some(jobs) = &cont.cluster_jobs {
        out.push_str(&cluster_jobs_section(jobs));
    }
    out.push('\n');
    out
}

/// ADR-0090 D2: 前の run が待ったクラスタ job の最終状態の節（continuation の前置きの中）。
fn cluster_jobs_section(jobs: &crate::protocol::ClusterJobsContinuation) -> String {
    let outcome = match jobs.state.as_str() {
        "satisfied" => "すべての job が終わりました",
        "timed_out" => {
            "待ちの上限に達しました（まだ終わっていない job があります。人の回答が下にあればそれに従うこと）"
        }
        "cancelled" => "待ちは取り消されました（job 自体は取り消していません）",
        other => other,
    };
    let mut out = format!(
        "### クラスタ job の結果（前の Run が待った job）\n\
         前の Run はクラスタ `{cluster}` の {scheduler} job の終了を待ちました。結果: {outcome}。\n",
        cluster = jobs.cluster,
        scheduler = jobs.scheduler.to_uppercase(),
    );
    if !jobs.summary.is_empty() {
        out.push_str(&format!("前の Run の要約: {}\n", jobs.summary));
    }
    for line in &jobs.jobs {
        out.push_str(&format!("- {line}\n"));
    }
    out.push_str(
        "この Run で結果（出力ファイル・ログ）を回収し、受け入れ条件を確かめてから完了を申告すること。\
         Exit_status が 0 でない job があれば、ログで原因を確かめてから直す・投げ直す（投げ直したら再び \
         `wait` で終える）こと。\n",
    );
    out
}

/// `checkpoint`（`task_core::Checkpoint` と同じ形の生の JSON）から、人が読める箇条書きを作る。
/// 未知の欄・型が合わない欄は黙って飛ばす（寛容に読む。D9 は「表示できる範囲で見せる」の精神）。
fn checkpoint_bullets(cp: &serde_json::Value) -> String {
    fn str_array(v: &serde_json::Value, key: &str) -> Vec<String> {
        v.get(key)
            .and_then(|x| x.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|i| i.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }
    fn field_list(v: &serde_json::Value, key: &str, field: &str) -> Vec<String> {
        v.get(key)
            .and_then(|x| x.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|i| i.get(field).and_then(|f| f.as_str()).map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }
    fn join_or_none(items: &[String]) -> String {
        if items.is_empty() {
            "（なし）".to_string()
        } else {
            items.join("; ")
        }
    }

    let mut out = String::new();
    out.push_str(&format!(
        "- 完了: {}\n",
        join_or_none(&str_array(cp, "completed"))
    ));
    out.push_str(&format!(
        "- 残り: {}\n",
        join_or_none(&str_array(cp, "remaining"))
    ));
    let decisions = field_list(cp, "decisions", "what");
    if !decisions.is_empty() {
        out.push_str(&format!("- 決めたこと: {}\n", decisions.join("; ")));
    }
    let files = field_list(cp, "files_changed", "path");
    if !files.is_empty() {
        out.push_str(&format!("- 変えたファイル: {}\n", files.join(", ")));
    }
    let tests = field_list(cp, "tests_run", "command");
    if !tests.is_empty() {
        out.push_str(&format!("- 実行したテスト: {}\n", tests.join("; ")));
    }
    let known_failures = field_list(cp, "known_failures", "what");
    if !known_failures.is_empty() {
        out.push_str(&format!("- 既知の失敗: {}\n", known_failures.join("; ")));
    }
    let next_action = cp.get("next_action").and_then(|v| v.as_str()).unwrap_or("");
    out.push_str(&format!("- 次の一手: {next_action}\n"));
    out
}

/// ADR-0044 D2（Phase 53）: 「コメント」の節。**前置きの先頭**に出す。
///
/// - 人のコメントで直前の run を止めた（`context.interrupt`）ときは、その本文を
///   「**人からの割り込み**: …」として**いちばん先**に置く（ADR-0044 D2 の表）。
///   割り込みの後に人がさらに書き足していれば、その**最新の**人のコメントがここに出る
///   （どちらも人が読ませたい文なので先頭に出してよい、という判断。`interrupting_comment`）。
/// - 続けてコメントの糸（最新 20 件、古い順）を出す。
/// - コメントが 1 件も無く割り込みも無ければ、この節ごと出さない（既存の出力を変えない）。
fn comments_section(context: &RunContext) -> String {
    if context.interrupt.is_none() && context.comments.is_empty() {
        return String::new();
    }
    let mut out = String::from("## コメント (comments on this task)\n");
    if let Some(interrupt) = &context.interrupt {
        out.push_str(&format!(
            "**人からの割り込み**: {}\n（この run はこのコメントで止められた。まずこれに応えること）\n",
            interrupt.trim()
        ));
    }
    for comment in &context.comments {
        let who = match comment.author_kind {
            task_core::CommentAuthorKind::Human => "人".to_string(),
            task_core::CommentAuthorKind::Node => match &comment.author {
                Some(id) => id.clone(),
                None => "担当".to_string(),
            },
            task_core::CommentAuthorKind::System => "celeris".to_string(),
        };
        out.push_str(&format!(
            "- [{}] {}: {}\n",
            comment.at,
            who,
            one_line(&comment.body)
        ));
    }
    out.push('\n');
    out
}

/// ADR-0044 D2（Phase 53）: ワーカーへの指示（コメントの書き方）。`comments_enabled` の run にだけ出す。
fn comment_instructions(context: &RunContext) -> String {
    if !context.comments_enabled {
        return String::new();
    }
    "## コメント (how to leave a note on this task)\n\
     短い進捗や判断の記録はコメントに書け（`{\"type\":\"comment\",\"body\":\"…\"}` を 1 行出す）。\
     `progress` と違ってコメントは**残り**、人にも次の run にも見える。長い成果は成果物に書くこと。\n\n"
        .to_string()
}

/// 1〜5（役職と brief → 永続の認可 → 記憶 → あなたの直近の仕事 → 直近のやり取り）。
fn person_sections(context: &RunContext) -> String {
    let mut out = String::new();
    if let Some(node) = &context.node {
        out.push_str(&format!("## あなた: {} ({})\n", node.name, node.id));
        if !node.brief.is_empty() {
            out.push_str(&node.brief);
            out.push('\n');
        }
        // Phase 30（ADR-0033 D4 追記）: 対話は常に対話用分野で走るが、担当ノード自身の仕事の分野が
        // あれば「仕事で使う道具」を 1 行足す（その人が自分の得意分野を知って答えられるように）。
        if let Some(genre) = &context.work_genre {
            out.push_str(&format!(
                "あなたの仕事で使う道具（分野）: {}",
                genre.description
            ));
            if !genre.capabilities.is_empty() {
                out.push_str(&format!(
                    "（できること: {}）",
                    genre.capabilities.join("、")
                ));
            }
            out.push('\n');
        }
        out.push('\n');
    }
    if !context.standing_rules.is_empty() {
        out.push_str("## 永続の認可（人が『今後ずっと』と決めたこと）\n");
        for rule in &context.standing_rules {
            out.push_str(&format!("- {rule}\n"));
        }
        out.push('\n');
    }
    if let Some(memory) = &context.memory
        && (!memory.notes.is_empty() || !memory.project.is_empty())
    {
        out.push_str("## 覚えていること (your long-term memory)\n");
        if !memory.notes.is_empty() {
            out.push_str("### 案件をまたぐ記憶\n");
            out.push_str(memory.notes.trim_end());
            out.push_str("\n\n");
        }
        if !memory.project.is_empty() {
            out.push_str("### この案件について\n");
            out.push_str(memory.project.trim_end());
            out.push_str("\n\n");
        }
    }
    if !context.recent_work.is_empty() {
        out.push_str("## あなたの直近の仕事\n");
        for w in &context.recent_work {
            out.push_str(&format!(
                "- [{}] {}",
                status_label(w.status),
                one_line(&w.title)
            ));
            if let Some(project_title) = &w.project_title {
                out.push_str(&format!("（案件: {}）", one_line(project_title)));
            }
            if let Some(outcome) = &w.outcome {
                out.push_str(&format!(": {}", one_line(outcome)));
            }
            if !w.artifacts.is_empty() {
                out.push_str(&format!(" 成果物: {}", w.artifacts.join(", ")));
            }
            out.push('\n');
        }
        out.push('\n');
    }
    out.push_str(&milestone_review_section(context));
    if !context.conversation.is_empty() {
        out.push_str("## 直近のやり取り (this is a continuing conversation)\n");
        for turn in &context.conversation {
            let who = match turn.role {
                MessageRole::User => "人",
                MessageRole::Node => "あなた",
            };
            out.push_str(&format!("- {who}: {}\n", one_line(&turn.text)));
        }
        out.push('\n');
    }
    out
}

/// ADR-0054 D1（Phase 67）: 「前回の run 以降」の節。継続中（`session.resume = true`）なら差分
/// （新しい人の発言・dispatch したタスクの終端と要約・認可の結果・新しい案件）、新規セッション
/// （rollover・アカウント変更・resume 失敗の後を含む）なら前のセッションの要約（ADR-0033 D4 の
/// 対話履歴の末尾 20 件）。どちらの文面を渡すかはディスパッチャが決める（このモジュールは並べるだけ）。
/// `context.session_diff` が空なら何も出さない（Phase 66 までと同じ出力）。
fn session_diff_section(context: &RunContext) -> String {
    if context.session_diff.is_empty() {
        return String::new();
    }
    let mut out = String::from("## 前回の run 以降 (since your last turn in this session)\n");
    for line in &context.session_diff {
        out.push_str(&format!("- {}\n", one_line(line)));
    }
    out.push('\n');
    out
}

/// ADR-0046 D1（Phase 59）: 「あなたの実効 profile」の節。根→葉で継いだ結果（`EffectiveProfile`）を
/// そのまま箇条書きにする。`context.profile` が `None` なら**何も出さない**（Phase 58 までと同じ出力）。
///
/// ADR-0046 D8: 道具は「使ってよいものの一覧」と「ここに無いものは使うな」を必ず書く。
pub fn profile_section(context: &RunContext) -> String {
    let Some(profile) = &context.profile else {
        return String::new();
    };
    if profile.skills.is_empty()
        && profile.policy.is_empty()
        && profile.tools.is_empty()
        && profile.deny_tools.is_empty()
        && profile.knowledge.is_empty()
        && profile.harnesses_allowed.is_empty()
        && profile.run.is_none()
        && profile.tier.is_none()
    {
        return String::new();
    }
    let mut out = String::from("## あなたの実効 profile (inherited from the org tree)\n");
    if !profile.chain.is_empty() {
        out.push_str(&format!("継承: {}\n", profile.chain.join(" > ")));
    }
    if !profile.skills.is_empty() {
        out.push_str(&format!("能力（skills）: {}\n", profile.skills.join(", ")));
    }
    if !profile.harnesses_allowed.is_empty() {
        out.push_str(&format!(
            "受けられるハーネス: {}{}\n",
            profile.harnesses_allowed.join(", "),
            match profile.harness_default.as_deref() {
                Some(d) => format!("（既定 {d}）"),
                None => String::new(),
            }
        ));
    }
    if let Some(tier) = profile.tier {
        let allowed = if profile.allowed_tiers.is_empty() {
            String::new()
        } else {
            format!(
                "（許可: {}）",
                profile
                    .allowed_tiers
                    .iter()
                    .map(tier_label)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        out.push_str(&format!("モデルの段: {}{allowed}\n", tier_label(&tier)));
    }
    if let Some(run) = profile.run {
        out.push_str(&format!(
            "実行場所: {}\n",
            match run {
                task_core::ProfileRun::Host => "host（この計算機の上）",
                task_core::ProfileRun::Container => "container（コンテナの中）",
            }
        ));
    }
    if !profile.knowledge.is_empty() {
        out.push_str(&format!(
            "使える知識: {}\n",
            profile
                .knowledge
                .iter()
                .map(knowledge_label)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    // ADR-0046 D8: 道具は許可制。ここに無いものは使わせない。
    if profile.tools.is_empty() {
        out.push_str(
            "使ってよい外部の道具: **無し**。gh / tavily / exa / docker / クラスタへの ssh は\
             このタスクでは使うな（必要なら人に聞け）。\n",
        );
    } else {
        out.push_str(&format!(
            "使ってよい外部の道具: {}\n",
            profile.tools.join(", ")
        ));
        out.push_str(
            "ここに挙がっていない外部の道具（gh / tavily / exa / docker / クラスタへの ssh）は使うな。\n",
        );
    }
    if !profile.deny_tools.is_empty() {
        out.push_str(&format!(
            "**禁止された道具**: {}\n",
            profile.deny_tools.join(", ")
        ));
    }
    if !profile.policy.is_empty() {
        out.push_str("組織の方針（根から順に。上ほど強い）:\n");
        for line in &profile.policy {
            out.push_str(&format!("- {}\n", one_line(line)));
        }
    }
    out.push('\n');
    out
}

fn tier_label(tier: &task_core::Tier) -> &'static str {
    match tier {
        task_core::Tier::Frontier => "frontier",
        task_core::Tier::Standard => "standard",
        task_core::Tier::Cheap => "cheap",
    }
}

/// `KnowledgeMount::label`（ADR-0047 D2 の `kb:<scope>` / `repo:<name>` / `dir:<path>` / `memory[:<node>]`）
/// に、`repo` の文書ディレクトリ（`docs`。既定と違うときだけ）を添える（Phase 59 追記）。
fn knowledge_label(mount: &task_core::KnowledgeMount) -> String {
    let mut out = mount.label();
    if let Some(docs) = mount.docs.as_deref().filter(|d| !d.is_empty()) {
        out.push_str(&format!("（docs: {docs}）"));
    }
    out
}

/// ADR-0046 D4（Phase 59）: 「この仕事の進め方」の節。`context.mode` が `None`（= 既定の
/// `production`）なら**何も出さない**（Phase 58 までと同じ出力）。
pub fn mode_section(context: &RunContext) -> String {
    let Some(mode) = context.mode else {
        return String::new();
    };
    let (name, rules): (&str, &[&str]) = match mode {
        task_core::TaskMode::Prototype => (
            "prototype（試作）",
            &[
                "動くことを最短で示す。テストは動作確認の最小限でよい。",
                "捨てる前提で書く。作り込むな。",
                "結論と次の一手を summary に書く。",
                "レビューは明示の受け入れ条件だけ。リポジトリの検査コマンド（check）は使わない。",
            ],
        ),
        task_core::TaskMode::Production => (
            "production（本番）",
            &[
                "既存のテストと lint を通す。",
                "変更は小さく、理由をコミットに書く。",
                "レビューは受け入れ条件 ＋ リポジトリの検査コマンド（check）。",
            ],
        ),
        task_core::TaskMode::Research => (
            "research（研究）",
            &[
                "主張には出典か計測を付ける。",
                "数値は再現手順と一緒に書く。",
                "採らなかった案と理由も残す。",
                "結果に出典（`sources`）か計測の記録が無ければ不合格になる。",
            ],
        ),
    };
    let mut out = format!("## この仕事の進め方 (mode: {name})\n");
    for rule in rules {
        out.push_str(&format!("- {rule}\n"));
    }
    out.push('\n');
    out
}

/// ADR-0046 D6（Phase 59）: CoS（根ノード）の対話 run にだけ出す「組織の一覧」。
/// 誰が何をできるか（id / 名前 / skills / harnesses）を見せるが、**人選はしない**
/// （担当は D5 の matching が決定的に決める）。それ以外の run では何も出さない。
pub fn organization_section(context: &RunContext) -> String {
    if context.conversation_addressee != Some(ConversationAddressee::Secretary)
        || context.organization.is_empty()
    {
        return String::new();
    }
    let mut out = String::from("## 組織 (who is in the org)\n");
    for node in &context.organization {
        out.push_str(&format!("- `{}` {}", node.id, node.name));
        if !node.skills.is_empty() {
            out.push_str(&format!(" — skills: {}", node.skills.join(", ")));
        }
        if !node.harnesses.is_empty() {
            out.push_str(&format!(" / harnesses: {}", node.harnesses.join(", ")));
        }
        // Phase 98（ADR-0046 D8）: 道具（特に `cluster:<id>`）を 1 語ずつ添える。CoS がクラスタ作業を
        // どのノードに `create_task` で流せばよいかを前置きから判断できるようにする。
        if !node.tools.is_empty() {
            out.push_str(&format!(" / 道具: {}", node.tools.join(", ")));
        }
        out.push('\n');
    }
    out.push_str(
        "担当は celeris が決める（必要な skills と harness をタスクに書けば、そこから決定的に選ばれる）。\
         あなたが名指しで人を選ぶ必要はない。\n\n",
    );
    out
}

/// ADR-0048 D3（Phase 60b）: CoS の対話 run にだけ出す「進行中の案件」の節（id / 題名 / 状態と登録済み repos）。
/// `actions` の `create_task.project` を選ぶ材料。ADR-0079 D12 / D13（Phase R5a）: 途中目標は凍結したので出さない
/// （CoS は案件〈方向〉だけを選ぶ）。
fn active_projects_section(context: &RunContext) -> String {
    if context.conversation_addressee != Some(ConversationAddressee::Secretary)
        || context.active_projects.is_empty()
    {
        return String::new();
    }
    let mut out = String::from("## 進行中の案件 (active projects)\n");
    for project in &context.active_projects {
        out.push_str(&format!(
            "- `{}` {}（{}）\n",
            project.id, project.title, project.status
        ));
        if !project.repos.is_empty() {
            out.push_str(&format!(
                "  - 登録済み repos: {}（使用時の project: `{}`）\n",
                project
                    .repos
                    .iter()
                    .map(|name| format!("`{name}`"))
                    .collect::<Vec<_>>()
                    .join(", "),
                project.id
            ));
        }
    }
    out.push('\n');
    out
}

/// ADR-0059 D6（Phase 99）: CoS の対話 run にだけ出す「クラスタ」の節（id・接続状態・実効
/// work_dir）。`create_task.workspace` の `path` をどう組むか（登録済みの work_dir を使うか、
/// 未登録なら省略するか）の材料。
fn clusters_section(context: &RunContext) -> String {
    if context.conversation_addressee != Some(ConversationAddressee::Secretary)
        || context.clusters.is_empty()
    {
        return String::new();
    }
    let mut out = String::from("## クラスタ (clusters)\n");
    for cluster in &context.clusters {
        let connected = if cluster.connected {
            "接続中"
        } else {
            "未接続"
        };
        match &cluster.work_dir {
            Some(work_dir) => {
                out.push_str(&format!(
                    "- `{}`（{connected}） 作業ディレクトリ: `{work_dir}`\n",
                    cluster.id
                ));
            }
            None => {
                out.push_str(&format!(
                    "- `{}`（{connected}） 作業ディレクトリ: 未登録（`path` は省略し、\
                     人に登録を頼む）\n",
                    cluster.id
                ));
            }
        }
    }
    out.push('\n');
    out
}

/// ADR-0039 D3: 案件の作業場所から、前置きに出す 1 行を組む（純粋関数。ディスパッチャがこれを
/// `RunContext::workspace_note` に入れる）。`Remote` は ADR-0018 D1 の「クラスタ側が正、手元は写し」を書く。
pub fn workspace_note(spec: &task_core::WorkspaceSpec) -> String {
    match spec {
        task_core::WorkspaceSpec::Local { path, .. } => {
            format!("この案件のコードは `{}` にある。", path.display())
        }
        task_core::WorkspaceSpec::Remote { cluster, path, .. } => format!(
            "この案件のコードはクラスタ {cluster} の `{}` にある。いまのカレントディレクトリはその写しで、\
             celeris が run の前後で同期する。",
            path.display()
        ),
    }
}

/// ADR-0041 D1: タスクごとの worktree を前置きに書く 1 行（純粋関数。ディスパッチャが
/// `workspace_note` の後ろに足す）。「このブランチにコミットせよ」までをここに書く。
pub fn worktree_note(
    repo: &std::path::Path,
    dir: &std::path::Path,
    branch: &str,
    base_sha12: &str,
    base_kind: &str,
) -> String {
    format!(
        "作業ツリー `{}`（`{}` の worktree）、ブランチ `{branch}`、base `{base_sha12}`（{base_kind}）。\
         このブランチにコミットせよ。`main` に直接コミットするな。`git checkout` でブランチを変えるな。",
        dir.display(),
        repo.display()
    )
}

/// ADR-0043 D2 / D8: 前置きの「作業場所」に出すリポジトリ 1 件（純粋なデータ。ディスパッチャが組む）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoNote {
    /// 案件の中での名前（`project_repos.name`）。
    pub name: String,
    /// タスクの作業場所の中での相対パス（`repos/<name>/`）。
    pub dir: String,
    /// git の worktree か（偽なら「ディレクトリ。読み書き可。git ではない」）。
    pub git: bool,
    /// worktree のブランチ（git のときだけ）。
    pub branch: Option<String>,
    /// base の短縮 sha（git のときだけ）。
    pub base: Option<String>,
    /// base をどこから取ったか（`main` / `current` / `head`。git のときだけ）。
    pub base_kind: Option<String>,
    /// `workspace.toml` の `[workspace] description`（無ければ出さない）。
    pub description: Option<String>,
    /// `workspace.toml` の `[commands] check`（無ければ出さない）。
    pub check: Vec<String>,
    /// `workspace.toml` の `[outputs] docs`（既定 `docs`）。
    pub docs: String,
    /// `workspace.toml` の `[outputs] deliverables`（既定 `.`）。
    pub deliverables: String,
}

/// ADR-0066 D1（Phase 110b）: `[workspace] shared_build_cache` が有効で git のリポジトリがあるときだけ、
/// `repos_note` の後ろに足す 1 行（ディスパッチャが `shared_build_cache` の設定を見て呼ぶかどうかを決める。
/// ここは文面だけの純粋関数）。ADR-0075 D7（Phase G1）: target は Celeris が渡すローカルの scratch
/// （owner ごと）になったので、文言を「自分で決めない・`/tmp` と worktree 直下に置かない」にした。
/// ADR-0129 (1): sccache は Celeris から外し、compiler wrapper は host の cargo 設定に任せる。
pub fn shared_build_cache_note() -> &'static str {
    "`target/` は Celeris が渡した `CARGO_TARGET_DIR`（ローカルの scratch）を使う。`CARGO_TARGET_DIR` を\
     自分で決めない。`/tmp` と worktree 直下に target を置かない。\n\
     `RUSTC_WRAPPER` 等のコンパイラ wrapper は host の cargo 設定に任せる（Celeris は設定しない）。\
     `CARGO_TARGET_DIR`・`CARGO_INCREMENTAL`・`CARGO_PROFILE_DEV_DEBUG` は渡された値のまま使う。\n"
}

/// ADR-0043 D2 / D8: タスクが複数のリポジトリを持つときの「作業場所」の本文（純粋関数）。
/// ディスパッチャが ADR-0039 D3 の `workspace_note` の代わりにこれを入れる。
///
/// 出る順は `repos` の順（先頭がカレントディレクトリ）。`repos` が空なら空文字列。
pub fn repos_note(repos: &[RepoNote]) -> String {
    let Some(first) = repos.first() else {
        return String::new();
    };
    let mut out = String::new();
    out.push_str("この案件のリポジトリのうち、このタスクが使うものは次のとおり:\n");
    for repo in repos {
        out.push_str(&format!("- `{}` → `{}`", repo.name, repo.dir));
        if repo.git {
            let branch = repo.branch.as_deref().unwrap_or("");
            let base = repo.base.as_deref().unwrap_or("");
            let base_kind = repo.base_kind.as_deref().unwrap_or("");
            out.push_str(&format!(
                "（worktree、ブランチ `{branch}`、base `{base}`（{base_kind}））"
            ));
        } else {
            out.push_str("（ディレクトリ。読み書き可。git ではない）");
        }
        if let Some(description) = repo.description.as_deref().filter(|d| !d.trim().is_empty()) {
            out.push_str(&format!(" — {}", description.trim()));
        }
        out.push('\n');
    }
    out.push_str(&format!(
        "カレントディレクトリは `{}`。編集はこの作業場所の中だけで行い、元のリポジトリには直接書くな。\n",
        first.dir
    ));
    if repos.iter().any(|r| r.git) {
        out.push_str(
            "git のリポジトリでは celeris が用意したブランチにコミットせよ。`main` に直接コミットするな。\
             `git checkout` でブランチを変えるな。\n",
        );
    }
    for repo in repos.iter().filter(|r| !r.check.is_empty()) {
        out.push_str(&format!(
            "`{}` のこのリポジトリの検査コマンド: {}\n",
            repo.name,
            repo.check
                .iter()
                .map(|c| format!("`{c}`"))
                .collect::<Vec<_>>()
                .join(" / ")
        ));
    }
    // ADR-0043 D8: 成果物は案件のリポジトリの中。`artifacts/` は中間物だけ。
    out.push_str(&format!(
        "コード以外の成果物（図・表・原稿）は `{}`、文書は `{}` の下に置け。`artifacts/` は run の中間物・\
         ログ・機械向けの `result.json` だけで、人が読む成果物を置く場所ではない。\n",
        join_repo_path(&first.dir, &first.deliverables),
        join_repo_path(&first.dir, &first.docs)
    ));
    // ADR-0044 D7（Phase 57）: 文書の書き方（正本は git。人は GUI の「文書」タブで同じファイルを読む）。
    let docs_dir = join_repo_path(&first.dir, &first.docs);
    let docs_dir = if docs_dir.ends_with('/') {
        docs_dir
    } else {
        format!("{docs_dir}/")
    };
    out.push_str(&format!(
        "文書は `{docs_dir}` に Markdown で書く（題名は 1 行目の `# `。タスクとの紐付けは front matter の \
         `tasks: [<このタスクの id>]`）。既定のブランチに直接コミットせず、上のブランチに置け（人が取り込む）。\n"
    ));
    out
}

/// ADR-0043 D2: 計画 run に渡す「この案件のリポジトリ」1 件（純粋なデータ。ディスパッチャが組む）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectRepoNote {
    pub name: String,
    /// `git` / `dir`。
    pub kind: String,
    /// 置き場（`~/workspace/benchfs` / `pegasus:/work/...`）。
    pub location: String,
    /// `workspace.toml` の `[workspace] description`（無ければ出さない）。
    pub description: Option<String>,
    /// 案件の主なリポジトリ（成果物と文書の既定の置き場）。
    pub is_primary: bool,
}

/// ADR-0043 D2: 計画 run の前置きに出す「この案件のリポジトリ」の一覧（純粋関数）。
/// 子タスクはこの**名前**を `repos` に書く。空なら空文字列。
pub fn project_repos_note(repos: &[ProjectRepoNote]) -> String {
    if repos.is_empty() {
        return String::new();
    }
    let mut out = String::from("この案件のリポジトリ:\n");
    for repo in repos {
        out.push_str(&format!("- `{}`（{}", repo.name, repo.kind));
        if repo.is_primary {
            out.push_str("、主なリポジトリ");
        }
        out.push_str(&format!("）: {}", repo.location));
        if let Some(description) = repo.description.as_deref().filter(|d| !d.trim().is_empty()) {
            out.push_str(&format!(" — {}", description.trim()));
        }
        out.push('\n');
    }
    out.push_str("子タスクが使うリポジトリは `repos` に**この名前で**書く（例 `\"repos\": [\"");
    out.push_str(&repos[0].name);
    out.push_str(
        "\"]`）。書かなければ主なリポジトリを継ぐ。ここに無い名前を書くと計画は差し戻される。\n",
    );
    out
}

/// `repos/benchfs/` と `docs` → `repos/benchfs/docs`（`.` はリポジトリのルートそのもの）。
fn join_repo_path(dir: &str, rel: &str) -> String {
    let base = dir.trim_end_matches('/');
    let rel = rel.trim().trim_start_matches("./").trim_end_matches('/');
    if rel.is_empty() || rel == "." {
        format!("{base}/")
    } else {
        format!("{base}/{rel}")
    }
}

/// 作業場所の節（ADR-0039 D3）。**案件が作業場所を決めている run にだけ**出す。
/// 実機の事故（2026-09-18）: 空の workspace に置かれた子タスクが、自分で `ssh` してリモートの
/// 作業ツリーに直接書いた。SPEC §3.7 追記「手元で編集してリモートで検証」をここで明示する。
fn workspace_section(context: &RunContext) -> String {
    let Some(note) = &context.workspace_note else {
        return String::new();
    };
    format!(
        "## 作業場所 (where this project's code lives)\n\
         {note}\n\
         編集はこの run の作業ディレクトリ（カレントディレクトリ）で行うこと。**別のホストの作業ツリーへ \
         `ssh` で直接書き込んではいけない**（同期は celeris が行う。検証・計測だけをリモートで実行する。\
         SPEC §3.7「手元で編集してリモートで検証」）。\n\n"
    )
}

/// ADR-0047 D2 / D3（Phase 61）: 知識の節（**索引だけ**。本文は入れない）。
///
/// `mounts` は実効マウント（ADR-0047 D2: 組織の和 ＋ 案件 ＋ タスクの明示）、`index` はそのマウントで
/// 読めるページの索引（[`task_core::knowledge::mount_matches`] で振り分ける）。純粋関数で、
/// マウントが 1 つも無い・索引が空なら**空文字列**（前置きは 1 バイトも変わらない）。
///
/// 出るもの: マウントの一覧 → D3 の使い方 → マウントごとの `path` / `title` / `tags`
/// （全体で最大 [`task_core::knowledge::MAX_PREAMBLE_ITEMS`] 件。溢れた分は件数だけ出す）。
pub fn knowledge_section(
    mounts: &[task_core::KnowledgeMount],
    index: &[task_core::KnowledgeItem],
) -> String {
    if mounts.is_empty() {
        return String::new();
    }
    let mut out = String::from("## 知識 (knowledge base — 索引だけ。本文は道具で読む)\n");
    out.push_str(&format!(
        "あなたが読める知識: {}。\n",
        mounts
            .iter()
            .map(|m| format!("`{}`", m.label()))
            .collect::<Vec<_>>()
            .join("、")
    ));
    // ADR-0047 D3 の案内文。
    out.push_str(
        "知識は `celerisctl knowledge search <語> [--scope …]` で探し、`celerisctl knowledge get <path>` で読む。\
         将来も使える事実を得たら `celerisctl knowledge record --title … --scope … --source task:<このタスクの id>` \
         で候補に入れる（一時的な情報・雑談・推測は入れない。出典を付ける）。候補は人が確認してから正本に入る。\n",
    );
    let mut shown = 0usize;
    let mut sections = String::new();
    for mount in mounts {
        let items: Vec<&task_core::KnowledgeItem> = index
            .iter()
            .filter(|item| task_core::knowledge::mount_matches(mount, item))
            .collect();
        if items.is_empty() {
            continue;
        }
        sections.push_str(&format!("### {}\n", mount.label()));
        for (n, item) in items.iter().enumerate() {
            if shown >= task_core::knowledge::MAX_PREAMBLE_ITEMS {
                sections.push_str(&format!(
                    "- （ほか {} 件。`search` で探す）\n",
                    items.len() - n
                ));
                break;
            }
            sections.push_str(&format!("- `{}` — {}", item.path, one_line(&item.title)));
            if !item.tags.is_empty() {
                sections.push_str(&format!("（{}）", item.tags.join("、")));
            }
            sections.push('\n');
            shown += 1;
        }
    }
    if sections.is_empty() {
        // マウントはあるが 1 件も読めるものが無い（KB がまだ無い・空）。索引の節は出さない。
        return String::new();
    }
    out.push_str(&sections);
    out.push('\n');
    out
}

/// 5. 役割の指示文（ADR-0016 D1 / M3。Phase 23 までと同じ文面）。
fn role_section(context: &RunContext) -> String {
    let mut out = String::new();
    if let Some(role) = &context.role {
        out.push_str(&format!("## Role: {}\n", role.id));
        if !role.instructions.is_empty() {
            out.push_str(&role.instructions);
            out.push('\n');
        }
        out.push('\n');
    }
    out
}

/// ADR-0067 D1: 人が読む成果物の置き場の規則（全 run 共通、短く固定）。本番事故
/// （BenchFS 案件のタスクが判断材料をリポジトリの `docs/` に書き、GUI から見えなかった）の再発防止。
pub(crate) fn deliverables_placement_note() -> String {
    "## 成果物の置き場所 (where human deliverables live)\n\
     調査報告・framing 案・比較表・提案書など**人が読んで判断する材料**は、成果物ディレクトリ\
     （`artifacts/`）か知識ベース（`projects/<project>/…` のページ）に置いてください。対象リポジトリの\
     追跡ファイル（`docs/` を含む）には Celeris 自身の判断過程・候補案・決定パケットを置かないこと\
     （そのリポジトリに書いてよいのはそのリポジトリ自身の成果 — コード・テスト・決定後の本文など）。\n\n"
        .to_string()
}

/// ADR-0095 付記 D-d: 本番 host の操作は人が実行する手順として書く（D-a・D-b で止まる操作を
/// 最初から試みさせないための事前の指示。常に出る — context に関わらない）。
pub(crate) fn production_host_note() -> String {
    "## 本番 host の操作 (production host changes)\n\
     本番 host の操作は人が実行する手順として書く（`systemctl --user`・`systemd-run`・\
     `~/.config/systemd`・`~/.local/celeris/releases`・`/local`・`/local/celeris/state/releases`・\
     `~/.config/celeris` の変更、daemon の\
     再起動・差し替え、本番 DB への書き込みはしない）。必要なら、人が実行する手順（コマンドと\
     確認方法）を成果物に書き、計画では人の決定（decisions）または人の check を置く（ADR-0095 \
     付記 D-d）。\n\n"
        .to_string()
}

/// 6. 記憶の書き方（ADR-0033 D6）。記憶が有効な run（`context.memory` がある）にだけ出す。
fn memory_instructions(context: &RunContext, artifacts: &str) -> String {
    if context.memory.is_none() {
        return String::new();
    }
    format!(
        "## 覚えておくこと (how to write to your memory)\n\
         覚えておくべきこと（クラスタの使い方、人の好み、直近の相談）は `{artifacts}/result.json` の \
         `memory.notes` に、この案件だけの事は `memory.project` に、短い箇条書きの文字列の配列で返せ: \
         `{{\"summary\": \"…\", \"evidence\": [], \"memory\": {{\"notes\": [\"…\"], \"project\": [\"…\"]}}}}`。\
         覚えることが無ければ `memory` は書かなくてよい（空の配列でもよい）。ここに書いたものだけが次の run に \
         引き継がれる（この会話の他の部分は残らない）。\n\n"
    )
}

/// 節 7: 対話専用の指示（Phase 28 / ADR-0033 D4 追記）。実機で秘書が「返事の代わりに仕事を始めた」
/// （委譲・多ターンの調査・最終試行での代筆）ため、対話 run には**返事だけをする**ことを明示する。
/// 秘書宛てには SPEC §7 の (a)〜(d)、それ以外のノード宛てには「聞かれたことに答える」に文面を分ける。
/// 対話でない run（`conversation_addressee` が `None`）では何も出さない。
fn conversation_instructions(context: &RunContext) -> String {
    match context.conversation_addressee {
        Some(ConversationAddressee::Secretary) => {
            format!(
                "## CoS の対話と仕事の開始 (conversation and authorized work)\n\
                 質問には簡潔に答え、実行・修正・改善の依頼には下記 actions で実際の仕事を作ってください。\
                 明示された通常の修正と検証は既に依頼された作業です。方針を毎回再承認させないでください。\
                 この短い対話 run では大きな調査や実装を直接せず、実行担当へ委譲します。\
                 **1 つの依頼は 1 つの `create_task`**（root task）です。依頼の大きさや段階の数をあなたが判断して\
                 分けたり、範囲を狭めたりしないでください（大きさは Complexity Gate、段階への分解は planner が\
                 決めます。ADR-0079）。元の依頼が修正なら objective と acceptance に実装・検証・成果の引き渡しまで含め、\
                 調査や改善案だけに縮小しないでください。調査はその仕事の途中の手順です。\
                 調査だけを依頼された場合は調査まで。中間報告を依頼全体の完了と呼ばないでください。\
                 未完了なら不足と継続中の仕事を示してください。\
                 質問は実行に不可欠な情報の不足、依頼範囲の拡大、未許可の破壊的操作などの場合だけです。\
                 既存の仕事と結果、承認済みの範囲を確認し、同じ調査や承認要求を繰り返さないでください。\
                 クラスタ（pegasus / sirius / fern03 など）でコマンドを実行する・状態を見る・ジョブを流す\
                 依頼は、自分で実行しないでください。『ssh が禁止されている』『read-only』を理由に断らないで\
                 ください（ADR-0046 D8 のとおりこの対話 run 自身には道具が無いのが正常で、それは断る理由に\
                 なりません）。`cluster:<id>` を tools に持つノードの仕事として `create_task` を書き、\
                 `workspace` に `{{\"kind\":\"remote\",\"cluster\":\"<id>\",\"path\":\"<クラスタ側の作業\
                 ディレクトリ>\"}}` を入れてください。path は案件の作業場所、無ければ人の依頼文にある場所。\
                 コマンドを実行する・状態を見る・ジョブを流すだけでコードの diff を作らない仕事は、\
                 `workspace` に `\"mode\":\"shared\"` も付けてください（worktree を切らず、そのまま実行\
                 します）。この場合の `path` は下の「クラスタ」に出ている実効の作業ディレクトリを使い、\
                 `~` は使わないでください（HPC のホームは作業用ではないのが普通です）。指したいクラスタの\
                 作業ディレクトリが「未登録」なら `path` を省略し（celeris が登録され次第その場所を使い\
                 ます）、返事で人に「cluster-hpc へ依頼したが、<id> クラスタの作業ディレクトリが未登録\
                 なのでクラスタ画面で登録してほしい」のように一言添えてください。\
                 remote の仕事は `cluster:<id>` を持つノード（クラスタ一覧の道具欄）に流してください。\
                 持たないノードに名指しで流すと検証で落ちます。調査・執筆・分析の仕事（web-research /\
                 literature-research / scientific-writing / experiment-data など、`cluster:<id>` を\
                 持たないノード）は remote にしないでください。クラスタで実行する仕事だけを\
                 `cluster:<id>` を持つノードに `workspace.mode: shared` で流してください。案件の作業場所が\
                 remote でも、担当に道具が無ければ celeris が local に落とします（ADR-0062 B）。\n\n\
                 {}",
                actions_instructions()
            )
        }
        Some(ConversationAddressee::Other) => {
            "## これは対話です (this is a conversation, not a work order)\n\
             この返事では作業を始めないでください。委譲・実装・調査は、人が方針と途中目標を承認してから \
             始まります。聞かれたことに答え、必要なら次にやりたいことを書いてください。\
             ファイルの作成や大きな探索は不要です。\
             自分の直近の仕事とその結果は上に書いてある。人に聞き返す前に、まずそれを見て答えること。\n\n"
                .to_string()
        }
        None => String::new(),
    }
}

/// ADR-0048 D3（Phase 60b）: CoS の対話にだけ足す「動く」経路の説明（結果ファイルの宣言的な `actions`
/// を taskd が決定的に実行する。指示文はこれを説明するだけで、実行そのものはコードの仕事）。
fn actions_instructions() -> String {
    "## 人からの頼みを動かす (declaring actions)\n\
     人の発言から具体的な仕事や案件が要りそうなら、返事の `summary` とは別に、結果ファイルに \
     `\"actions\": [...]` を宣言してください（taskd が決定的に実行します。あなた自身がタスクを \
     作ったり道具を使ったりはしません）。\n\
     - `{\"type\": \"create_task\", \"title\": \"…\", \"objective\": \"…\", \"acceptance\": [\"…\"], \
     \"harness\": \"coding\", \"skills\": [\"rust\"], \"mode\": \"prototype\", \"repos\": [], \
     \"project\": \"<案件の id か null>\", \
     \"stages_hint\": [{\"title\": \"Phase 1\", \"scope\": \"…\"}], \
     \"workspace\": {\"kind\":\"remote\",\"cluster\":\"<id>\",\"path\":\"<作業ディレクトリ>\",\
     \"mode\":\"shared\"}}`\
     （`workspace` は省略可。クラスタでの仕事だけ入れる。\
     ローカルなら `{\"kind\":\"local\",\"path\":\"…\"}`）\n\
     `workspace.mode` はコマンドを実行するだけでコードの diff を作らない仕事のとき `\"shared\"` にします\
     （worktree を切らず、`path` にそのまま cd して実行します。ADR-0059）。コードを直す仕事では付けません\
     （省略時は worktree）。`create_task.mode`（下）とは別のフィールドです。\n\
     - `{\"type\": \"propose_project\", \"title\": \"…\", \"request\": \"…\", \"repos\": [\"/abs/path\"]}`\n\
     - `{\"type\": \"ask_human\", \"text\": \"…\"}`\n\
     **あなた（CoS）は goal / harness / skills / mode / repos / 制約を定義し、担当（`assignee`）とモデル（`tier`）は\
     選びません**（ADR-0069）。担当は celeris が skills と harness から決定的に選び、モデルの lane は仕事の性質から\
     決めます。仕事の性質を伝えたいときは任意の `\"features\": {\"judgment\": \"high\", \"verifiability\": \"low\"}` \
     （各軸 low / medium / high。judgment, ambiguity, verifiability, reversibility, consequence, context_size, \
     tool_intensity, expected_length, cross_cutting）を書けます。人が発言で `@<担当 id>` や `tier:<lane>` と明示した\
     ときだけ、その値を `assignee` / `tier` に写してください（celeris は人の発言を確かめてから従います）。\n\
     **1 つの依頼は 1 つの `create_task`（root task）**にしてください（ADR-0079）。依頼の大きさ・段階の数は\
     あなたが判断しません（Complexity Gate と planner の仕事です）。依頼の範囲を狭めないでください: 人が\
     「Phase 1〜4」と言ったら、`objective` と `acceptance` に Phase 1〜4 のすべてを書きます（「設計と Phase 1」に\
     縮めない）。人が段階を名指ししたときだけ、その名前と範囲を任意の \
     `\"stages_hint\": [{\"title\": \"Phase 1\", \"scope\": \"…\"}]` にそのまま写してください（planner への入力で、\
     構造の強制ではありません）。人が名指ししていない段階を作って書かないでください。\n\
     互いに独立な依頼（「A を直して、ついでに無関係な B も」）は `create_task` を 2 つにします（task 同士の依存は\
     書けません）。一方が他方に依存するなら 1 つの `create_task` にまとめます（依存は task の中の段階で表します）。\n\
     人が段階ごとの確認を頼んだときだけ、任意で `\"pause_after\": {\"mode\": \"each_phase\"}`\
     （特定の段階だけなら `{\"mode\": \"after\", \"phases\": [\"design\"]}`）を付けてください\
     （その段階の後で止まり、人が続ける / replan / 取り下げを選びます。ADR-0074）。\n\
     `create_task.mode` は進め方で、prototype / production / research のいずれかです。通常実装は `mode: \"production\"` とし、mode に standard（tier の名前）は書かないでください。\n\
     `create_task.repos` は案件内の登録名です。指定するときは必ず所属する案件の ID を `project` に書き、\
     上の登録済み repos から選んでください。`project: null` と非空の `repos` の組み合わせは禁止です。\
     既存のコードを直す依頼は、そのリポジトリが登録された既存案件に紐づけます。\
     案件に属さない仕事は `project: null, repos: []`。判断できないときは推測せず質問してください。\n\
     `project` は既存の案件（仕事の方向）から選んでください。`propose_project` は人が新しい方向（案件）を\
     名指ししたときだけです。途中目標は作りません（途中目標は root task の段階で表します。ADR-0079）。\
     判断に必要な情報が欠けるときは `ask_human`。通常の実装判断は担当に任せます。案件が分かっていれば `project` \
     を書いてください（担当は書かなくても celeris が skills と harness から決定的に選びます）。\
     検証に落ちた action（知らない harness / repos / 案件など）は実行されず、理由が人に見えます。\n\
     人の確認が要る `acceptance`（`\"human\"`）を書くときは、必ず `artifact_exists` か\
     `knowledge_page`（知識ベースのページ参照）の条件も添えてください。人が読む決定材料は登録済みの\
     artifacts か知識ベースのページに置き（GUI から見える場所）、対象リポジトリの `docs/` などの\
     追跡ファイルには置きません（ADR-0067）。\n\
     `web/` や `docs/` だけを変える task の acceptance では `cargo test --workspace` を必須にせず、\
     `crates/` に差分が無いことの検査に置き換えてください（Cargo の workspace check は daemon 側で行います）。\
     acceptance の範囲指定（差分範囲など）には、計画が要求する ADR や記録（`agent-docs/adr/`、\
     `agent-docs/progress/`）の置き場所を最初から含めてください（ADR-0079 R7-10。置き場所は ADR-0128）。\
     調査系（`literature` / `web-research`）の `create_task` を書くときは、`objective` の 1 行目を \
     **`対象: <対象1> / <対象2> / …（観点: <観点1>、<観点2>、…）`** の明示形にしてください \
     （例: `対象: CHFS / FINCHFS / GekkoFS / UnifyFS / BeeOND（観点: server/client 配置、\
     cache/direct I/O、file semantics、replication）`。区切りは `/` でも `、`/`,` でも構いません）。\
     対象が 5 を超えても \
     依頼が 1 つなら `create_task` は 1 つのままにし、`objective` に全対象を書いてください（対象ごとの分割は \
     planner が段階と単位で行います。ADR-0079 D12）。受け入れ条件は、対象ごとに分ける \
     か、レビュアー条件（`acceptance` のうち `check` が reviewer のもの）に「**対象ごとに**、指定の観点 \
     が一次情報（または文献）に基づいて整理されている。確認できない観点は『未確認』と明記されていれば \
     不合格の理由にしない」という一文を含めてください（ADR-0063 D3、Phase 109c D で具体化）。1 件の欠落 \
     で全体を落とさないためです。成果物の存在確認（`check: \"artifact_exists\"`）は `report.md` を \
     使ってください（`literature`/`web-research` のどちらのハーネスも `report.md` を書きます。\
     ADR-0063 Phase 109b A3）。観点に比較先との比較分類（例: 「BenchFS との比較分類」）を含める場合、\
     それは文献検索ではなく比較先の設計条件との照合による判断で、必ず『公平比較可能』か『背景比較のみ』\
     のどちらかを確度付きで出させてください。分類の欠落は不合格の理由になります（ADR-0063 Phase 109g）。\n\n"
        .to_string()
}

/// 節 4.5: 途中目標のここまでの結果（Phase 41 / ADR-0038 D1）。レビューの対話 run にだけ出す。
/// 中身は決定的に集めたものをそのまま並べるだけ（要約は run の仕事）。
fn milestone_review_section(context: &RunContext) -> String {
    let Some(review) = &context.milestone_review else {
        return String::new();
    };
    let mut out = String::new();
    out.push_str(&format!(
        "## 途中目標『{}』のここまで ({})\n",
        review.milestone.title, review.milestone.status
    ));
    if !review.milestone.description.is_empty() {
        out.push_str(&format!("{}\n", one_line(&review.milestone.description)));
    }
    for task in &review.tasks {
        out.push_str(&format!(
            "\n### [{}] {}\n",
            status_label(task.status),
            one_line(&task.title)
        ));
        if let Some(outcome) = &task.outcome {
            out.push_str(&format!("要約: {}\n", one_line(outcome)));
        }
        if !task.artifacts_excerpt.is_empty() {
            out.push_str("成果物の抜粋:\n");
            out.push_str(task.artifacts_excerpt.trim_end());
            out.push('\n');
        }
    }
    out.push('\n');
    out
}

/// 節 7 の追記（Phase 41 / ADR-0038 D1）: 途中目標レビューの対話 run にだけ足す指示。
/// 「結果 → 達成の可否 → 次の提案 → 判断を仰ぎたい点」を書かせ、次の途中目標は結果ファイルの
/// `milestone_proposal` にも書かせる（celeris はそこだけを決定的に読む）。
fn milestone_review_instructions(review: &MilestoneReviewContext) -> String {
    format!(
        "## 途中目標の判定をお願いする返事です (milestone review)\n\
         途中目標『{}』の仕事が止まりました。人に向けて、数十秒で読める分量で次を書いてください: \
         (a) この途中目標までで**得られた結果**の要約（数字・候補・出典）(b) 達成と言えるか\
         （言えないなら何が足りないか）(c) **次の途中目標の提案**（1 つ。題名と説明）\
         (d) 判断を仰ぎたい点。\
         次の途中目標は結果ファイルの `milestone_proposal` にも \
         `{{\"milestone_proposal\": {{\"title\": \"…\", \"description\": \"…\"}}}}` の形で書いてください\
         （人が「ok」を押すと、これが次の途中目標になります）。達成の可否を決めるのは人です。\n\n",
        review.milestone.title
    )
}

/// やり取りの 1 行化（前置きの箇条書きを崩さないため。中身は削らない）。
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `context.recent_work[].status` の表示名（`Status` の `snake_case` 表現。`task_core::model::Status` の
/// `#[serde(rename_all = "snake_case")]` と同じ）。
fn status_label(status: task_core::Status) -> &'static str {
    use task_core::Status::*;
    match status {
        Draft => "draft",
        Ready => "ready",
        Running => "running",
        Blocked => "blocked",
        Reviewing => "reviewing",
        Done => "done",
        Failed => "failed",
        Cancelled => "cancelled",
    }
}

/// F5-fix5: claude-code の run は headless（`claude -p`）で、turn を終えた時点で run が終わる。本番の
/// gate WU（タスク 01M3JXB3DHVBWKWKPW04DTG6SJ / run 01M3KF2HFMHPJR7YEB5HMT38MQ）は `cargo test --workspace` を
/// Bash の `run_in_background` で走らせ、「完了の通知を待つ」と書いて turn を終え、その command は殺された。
/// claude-code アダプタはこれを `--append-system-prompt` で**全ての** run（worker / planner / reviewer /
/// 対話）に渡す（役割ごとの指示文〈config.toml〉には置かない。プロンプト本文〈`prompt.txt`〉も変えない）。
/// 決定的な定数（run ごとの値を埋め込まない）。
pub const HEADLESS_RUN_NOTE: &str = "\
## headless 実行（celeris）
この run は celeris が `claude -p`（非対話・headless）で起動している。人は見ていない。\
あなたが turn を終えた（道具を呼ばずに返答を終えた）時点で run は終わり、プロセスは終了する。\
続きの turn は来ない。\n\
- background task を使わない: Bash の `run_in_background`、Agent / Task の background 実行、\
Monitor・ScheduleWakeup・Cron など「後で通知が来る」「後で起こす」仕組みは、turn を終えた瞬間に殺され、\
通知は二度と届かない（celeris はこの run の background task を無効にしている）。\n\
- 長い command（`cargo test --workspace`、`cargo clippy`、ビルド、GUI の test など）も foreground で実行し、\
終わるまで待って exit code と出力を確かめる。Bash の `timeout` 引数（ミリ秒）を明示して長めに取る\
（既定の 2 分では打ち切られる。上限はこの run の壁時計）。\n\
- 「完了の通知を待つ」「終わったら続ける」と書いて turn を終えてはいけない。turn を終えるのは、\
成果物と `result.json`（または `yield`）を書き終えたときだけ。\n";

#[cfg(test)]
mod tests;
