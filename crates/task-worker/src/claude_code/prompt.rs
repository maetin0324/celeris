//! Claude Code と互換 adapter が使う task kind 別プロンプト生成。

use super::*;

/// ADR-0090 D5: planner の leaf の基準に足す 1 段落（数時間かかるクラスタ job の扱い）。
pub const CLUSTER_JOB_PLANNER_GUIDANCE: &str = "**Long cluster jobs (PBS / Slurm, ADR-0090)**: a unit that \
     submits jobs that run for hours is still one unit — its run submits the jobs and ends with a `wait` \
     (`{\"type\":\"wait\",\"kind\":\"cluster_job\",...}` in result.json); celeris polls the scheduler and resumes \
     the same unit as a continuation run when the jobs finish, and that run collects the results. Do not \
     split \"submit\" and \"collect\" into separate units and do not budget the unit for the job's wall time. \
     An acceptance check may require the jobs to have finished successfully (for example \"all PBS jobs are \
     F with Exit_status 0\" verified from the scheduler or the job logs).";

/// ADR-0006 Phase 115 D1（本番障害 01M3915FARENW8M0JM11XVF6W0 / 01M38T8N17MEWPTJQXGX1TNYJD）:
/// `work_dir`（実際の cwd）が `workspace` と異なる run（部署のリポジトリの git worktree で走るタスク）
/// だけ、プロンプトの先頭に「cwd と成果物ディレクトリは別」の注意を 2 行足す。`result_json_instructions`
/// は既に `artifacts_dir` の絶対パスで書く（`RunRequest::artifacts_rel` が `work_dir.is_some()` のとき
/// 絶対パスを返す）が、その絶対パスの指示を読み飛ばして相対 `artifacts/` を書いてしまう事故があったため、
/// 冒頭で明示的に注意する。`work_dir` が無い・`workspace` と同じなら何も足さない（既存の文面は
/// 1 バイトも変わらない）。純粋関数。
pub(crate) fn work_dir_note(
    work_dir: Option<&Path>,
    workspace: &Path,
    artifacts_dir: &Path,
) -> String {
    match work_dir {
        Some(wd) if wd != workspace => format!(
            "cwd は `{}`（リポジトリの worktree）。成果物ディレクトリは `{}`。\
             相対 `artifacts/` はリポジトリの中を指すので使わない。\n\n",
            wd.display(),
            artifacts_dir.display()
        ),
        _ => String::new(),
    }
}

/// タスクからワーカーへのプロンプトを組み立てる（ADR-0006 D2, ADR-0007 D7, 純粋関数）。`run_id` は
/// スキーマ変更を避けてプロンプト文面にのみ埋め込む（旧 P-11。ADR-0006 D2 参照）。`task.kind` で分岐する
/// （`Plan` はプランナー用、`Review` はレビュアー用、それ以外は Phase 4 のワーカー用プロンプト。ADR-0007 D7）。
/// `artifacts` は成果物ディレクトリの workspace 相対表記（`RunRequest::artifacts_rel`。ADR-0036 D3。
/// 単独タスクでは `artifacts` なので、文面は Phase 34 までと 1 バイトも変わらない）。
pub fn build_prompt(task: &Task, context: &RunContext, run_id: &str, artifacts: &str) -> String {
    let mut prompt = build_prompt_inner(task, context, run_id, artifacts);
    if let Some(browser) = &context.browser {
        prompt.push_str(&crate::browser::prompt(browser));
    }
    prompt
}

fn build_prompt_inner(task: &Task, context: &RunContext, run_id: &str, artifacts: &str) -> String {
    // ADR-0072 D14（Phase E3）: task-local な planner run は `task.kind` に依らず（常に `Execute`）、
    // `context.execution_planner` の有無で選ぶ。
    if context.execution_planner.is_some() {
        return build_execution_plan_prompt(task, context, run_id, artifacts);
    }
    match task.kind {
        // ADR-0074 D3.3（Phase F4a (b)）: `MILESTONES_PLAN_LABEL` の印がある Plan タスクは、案件全体の
        // マイルストーン DAG を `project-plan.json` に書く専用のプロンプト（旧来の `plan.json` の
        // 分解は 1 バイトも変えない）。
        // ADR-0074 D3.4（Phase F4b (e)）: 承認済みの計画がある案件の replan は差分
        // （`celeris.project-plan-delta/1`）を書く専用のプロンプト。
        TaskKind::Plan if task_core::is_milestones_replan_task(task) => {
            build_project_replan_prompt(task, context, run_id, artifacts)
        }
        TaskKind::Plan if task_core::is_milestones_plan_task(task) => {
            build_project_plan_prompt(task, context, run_id, artifacts)
        }
        TaskKind::Plan => build_plan_prompt(task, context, run_id, artifacts),
        TaskKind::Review => build_review_prompt(task, context, run_id, artifacts),
        TaskKind::Execute | TaskKind::Approval => {
            build_execute_prompt(task, context, run_id, artifacts)
        }
    }
}

/// 冒頭の共通部分（タイトル・run_id/attempt・前置き・分野・目的）。前置き（役職と brief・永続の認可・記憶・
/// 直近のやり取り・役割の指示文）は `crate::preamble::render` が組む（ADR-0016 D1 / M3, ADR-0033 D4 / D6）。
/// `task.genre` があり、その分野が `context.available_genres` に載っていれば、続けて `## Genre: <id>` と
/// 説明を出す（ADR-0027 D1）。
fn prompt_header(task: &Task, context: &RunContext, run_id: &str, artifacts: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!("# Task: {}\n\n", task.title));
    out.push_str(&format!(
        "(run {run_id}, attempt {} of {})\n\n",
        task.attempts + 1,
        task.budget.max_retries + 1
    ));
    // ADR-0033 D4 / D6（Phase 24）: 役職と brief → 永続の認可 → 記憶 → 直近のやり取り → 役割の指示文。
    // 前置きは `crate::preamble` が 1 か所で組む（`RunContext` が空なら 1 バイトも増えない）。
    out.push_str(&crate::preamble::render(context, artifacts));
    if let Some(genre_id) = &task.genre
        && let Some(genre) = context.available_genres.iter().find(|g| &g.id == genre_id)
    {
        out.push_str(&format!(
            "## Genre: {}\n{}\n\n",
            genre.id, genre.description
        ));
    }
    // ADR-0072 D9/D21（Phase E2）: 計画のある Task の WU の run では、Task 全体の objective では
    // なく WU の objective を出す（`context.work_unit` が無い run は E1 までと 1 バイトも変わらない）。
    match &context.work_unit {
        Some(wu) => {
            out.push_str(&format!("## Objective\n{}\n\n", wu.objective));
            out.push_str(&format!(
                "## このタスク全体の目的（参考）\n{}\n\n",
                wu.task_objective_excerpt
            ));
            if !wu.plan_overview.is_empty() {
                out.push_str("## 計画の中のこの WorkUnit\n");
                for line in &wu.plan_overview {
                    out.push_str(&format!("- {line}\n"));
                }
                out.push('\n');
            }
            if !wu.dependency_summaries.is_empty() {
                out.push_str("## 依存する WorkUnit の完了状況\n");
                for line in &wu.dependency_summaries {
                    out.push_str(&format!("- {line}\n"));
                }
                out.push('\n');
            }
            // ADR-0079 D7（Phase R3a）: この leaf が待っていた人の決定（固定の書式の行。無ければ出さない）。
            if !wu.human_decisions.is_empty() {
                out.push_str(crate::preamble::HUMAN_DECISIONS_HEADING);
                out.push('\n');
                for line in &wu.human_decisions {
                    out.push_str(line);
                    out.push('\n');
                }
                out.push('\n');
            }
            // ADR-0079 付記 R7-5 D3: 直前の run が done を返したのに daemon の check が落ちたときだけ（無ければ空）。
            out.push_str(&previous_check_failures_section(wu, artifacts));
            // ADR-0074 D1.2（Phase F2b）: WU ごとの worktree で走る run だけ（無ければ空）。
            out.push_str(&crate::preamble::work_unit_branch_section(wu));
        }
        None => {
            out.push_str(&format!("## Objective\n{}\n\n", task.objective));
        }
    }
    // ADR-0079 D7（Phase R3a）: 木の節点の worker の run だけ（`false` ならプロンプトは変わらない）。
    if context.decision_requests {
        out.push_str(&crate::preamble::decision_requests_section(artifacts));
    }
    out
}

/// ADR-0079 付記 R7-5 D3: 「前回の run の check の不合格」節。`previous_check_failures` が空なら空文字列
/// （プロンプトは 1 バイトも変わらない）。
fn previous_check_failures_section(
    wu: &crate::protocol::WorkUnitPromptContext,
    artifacts: &str,
) -> String {
    if wu.previous_check_failures.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "## 前回の run の check の不合格\n\
         この WorkUnit の前回の run は done を返したが、celeris が run の後に走らせた次の check が不合格だった\
         （check は計画のもので、この run では変えられない）。作業をやり直す前に、まずこの不合格の原因を確かめよ。\
         成果を直し、同じ場所（下の `cwd`）から同じ check を自分で走らせて期待どおりの exit になるのを確かめてから done を返すこと。\n",
    );
    for line in &wu.previous_check_failures {
        out.push_str(&format!("- {line}\n"));
    }
    out.push_str(&format!(
        "check そのものが誤っていて成果をどう直しても通らない（引数の形が違う、存在しない場所を見ている等）なら、done を返さず \
         `{artifacts}/result.json` に `{{\"yield\": {{\"plan_issue\": \"<どの check がなぜ通らないか、どう直すべきか>\"}}}}` を書いて終えよ\
         （計画の問題の申告として replan になる）。\n\n"
    ));
    out
}

/// `context.available_genres` の一覧を「使える専門家」の箇条書きに描く（ADR-0027 D1, ADR-0028 D2）。
/// `できること` / `渡すもの…返るもの` の行は、それぞれの一覧が空なら出さない（ADR-0028 D1: 3 フィールドとも任意）。
/// 見出しと、末尾の使い方の説明（委譲 or plan.json）は呼び出し側が足す。
fn genre_list_lines(context: &RunContext) -> String {
    let mut out = String::new();
    for g in &context.available_genres {
        out.push_str(&format!("- {}: {}\n", g.id, g.description));
        if !g.capabilities.is_empty() {
            out.push_str(&format!("  できること: {}\n", g.capabilities.join(" / ")));
        }
        if !g.input_artifacts.is_empty() || !g.output_artifacts.is_empty() {
            out.push_str(&format!(
                "  渡すもの: {} → 返るもの: {}\n",
                g.input_artifacts.join(", "),
                g.output_artifacts.join(", ")
            ));
        }
        let roles = if g.roles.is_empty() {
            "-".to_string()
        } else {
            g.roles
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        out.push_str(&format!("  役割: {roles}\n"));
    }
    out
}

/// `context.available_genres` があれば「使える専門家」節を足す（ADR-0027 D1, ADR-0028 D2）。委譲できる run
/// （`build_execute_prompt`）にだけ、この run が子に割り当てられる分野の能力・入出力・役割の選択肢を伝える。
fn available_genres_section(context: &RunContext, artifacts: &str) -> String {
    let mut out = String::new();
    if context.available_genres.is_empty() {
        return out;
    }
    out.push_str("## 使える専門家 (available genres and roles you can delegate to)\n");
    out.push_str(&genre_list_lines(context));
    out.push_str(&format!(
        "\nIf part of this work belongs to a different genre, delegate it with `role` set to one \
         of that genre's roles and `genre` set to its id in `{artifacts}/delegate.json`.\n\n"
    ));
    out
}

/// Plan run 用の「使える専門家」節（ADR-0028 D3）。子タスクの `genre` / `role` を `artifacts/plan.json` で
/// 選べることを伝える点だけが `available_genres_section` と異なる（委譲ではなく分解なので）。
fn available_genres_section_for_plan(context: &RunContext, artifacts: &str) -> String {
    let mut out = String::new();
    if context.available_genres.is_empty() {
        return out;
    }
    out.push_str("## 使える専門家 (available genres and roles you can assign child tasks to)\n");
    out.push_str(&genre_list_lines(context));
    out.push_str(&format!(
        "\nIf a child task belongs to a different genre than this one, set its `genre` (and, one of \
         that genre's roles, its `role`) in `{artifacts}/plan.json`.\n\n"
    ));
    out
}

/// 1 つの分野の「成果物の名前は固定」の箇条書き（Phase 38。`名前: 説明` の `名前` と `説明` に分けて出す）。
fn harness_artifact_lines(genre: &crate::protocol::GenreContext) -> String {
    let mut out = format!(
        "- {}: この分野の担当は**ハーネス**で動く。成果物は次の名前で固定され、担当が別のファイルを書くことはできない。\n",
        genre.id
    );
    for (name, description) in genre.output_artifacts_named() {
        match description {
            Some(d) => out.push_str(&format!("  - `{name}`: {d}\n")),
            None => out.push_str(&format!("  - `{name}`\n")),
        }
    }
    out
}

/// Phase 38（ADR-0028 追記。実機のレビュー不合格から）: **Plan run** に、ハーネスで動く分野
/// （`GenreContext::is_harness`）の成果物の規約を出す。実機で、計画が研究文献調査課（PaperQA2）に
/// 「候補テーマを `candidates.json` にまとめよ」と書き、`artifact_exists: candidates.json` を条件に付けた。
/// `candidates.json`（現 `papers.json`）はハーネスが書く検索コーパスの固定名で、担当は計画が決めた名前の
/// ファイルを書けないため、答えの中身が良かったのにレビュアーが基準どおり不合格にした。
/// ハーネスでない分野（coding 等）しか無い設定では**何も出さない**（従来の文面と 1 バイトも変わらない）。
pub(super) fn harness_artifacts_section_for_plan(context: &RunContext) -> String {
    let harness: Vec<&crate::protocol::GenreContext> = context
        .available_genres
        .iter()
        .filter(|g| g.is_harness() && !g.output_artifacts.is_empty())
        .collect();
    if harness.is_empty() {
        return String::new();
    }
    let mut out = String::from("## ハーネスで動く分野の成果物（名前は固定）\n");
    for genre in harness {
        out.push_str(&harness_artifact_lines(genre));
    }
    out.push_str(
        "受け入れ条件（`artifact_exists`）にはこの名前だけを使うこと。上に無い名前のファイルを要求しても、\n\
         その担当は書けない（条件は celeris が落とし、警告が残る）。**内容の要求は `objective` に書き、\n\
         レビュアー条件（`{\"type\":\"reviewer\"}`）で判定させること**（「X を Y に書け」ではなく\n\
         「答えに X を含めよ」）。\n\n",
    );
    out
}

/// ADR-0033 D4（Phase 24）: 組織図（id / name / brief / genre）。分解・委譲できる run にだけ渡り、
/// 「どの課に何を振るか」を `assignee` で決めさせる。空なら何も出さない（Phase 23 までと同じ出力）。
fn organization_section(context: &RunContext) -> String {
    let mut out = String::new();
    if context.organization.is_empty() {
        return out;
    }
    out.push_str("## 組織図 (who you can assign work to)\n");
    for n in &context.organization {
        let kind = match n.kind {
            task_core::OrgKind::Secretary => "秘書",
            task_core::OrgKind::Department => "部",
            task_core::OrgKind::Section => "課",
        };
        let parent = n.parent_id.as_deref().unwrap_or("-");
        let genre = n.genre.as_deref().unwrap_or("-");
        out.push_str(&format!(
            "- {} [{kind}] {} (親: {parent}, 分野: {genre})",
            n.id, n.name
        ));
        if !n.brief.is_empty() {
            out.push_str(&format!(" — {}", n.brief));
        }
        out.push('\n');
    }
    out.push('\n');
    out
}

/// `context.children` があれば「集約 run」節を足す（ADR-0016 D3 / M4）。
fn children_section(context: &RunContext, artifacts: &str) -> String {
    let mut out = String::new();
    if context.children.is_empty() {
        return out;
    }
    out.push_str("## Delegated child tasks (this is the aggregate run)\n");
    for c in &context.children {
        let role = c.role.as_deref().unwrap_or("-");
        let status = serde_json::to_string(&c.status).unwrap_or_default();
        let status = status.trim_matches('"');
        let outcome = c.outcome.as_deref().unwrap_or("-");
        let workspace = c
            .workspace
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "-".to_string());
        let artifacts = if c.artifacts.is_empty() {
            "-".to_string()
        } else {
            c.artifacts
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>()
                .join(",")
        };
        // ADR-0041 D1: 子が worktree で作業したときだけブランチを足す（無い子の文面は従来どおり）。
        let branch = match &c.branch {
            Some(b) => format!(" branch={b}"),
            None => String::new(),
        };
        out.push_str(&format!(
            "- {} [{role}] status={status} outcome={outcome} workspace={workspace}{branch} artifacts={artifacts}\n",
            c.title
        ));
    }
    out.push_str(&format!(
        "\nSummarize the results of the delegated child tasks in `{artifacts}/summary.md`. \
         The reviewer will check that `{artifacts}/summary.md` exists.\n\n"
    ));
    out
}

/// ADR-0046 D2 / D4 / D5（Phase 59）: 計画には**人選をさせない**。
///
/// Phase 58 までは「上の組織図を見て子タスクごとに `assignee` を必ず書け」と指示していたが、
/// ADR-0046 D5 で担当は決定的な matching が決めるようになった。代わりに、子タスクごとに
/// **`harness` / `skills` / `mode`** を宣言させる（それが matching の入力になる）。
/// 組織図を渡していない run（Phase 23 までの構成）では何も出さない。
fn assignee_instructions_for_plan(context: &RunContext) -> String {
    if context.organization.is_empty() {
        return String::new();
    }
    "## 誰がやるか (ADR-0046 D5)\n\
     **`assignee`（担当）は書くな。組織図から人を選ぶ必要は無い。** 誰がやるかは celeris が決定的に決める\n\
     （必要な skill と harness の重なりで選ぶ）。代わりに、子タスクごとに次の 3 つを書け:\n\
     - `harness`: その仕事の実行契約の id（下の「使える分野」から選ぶ）\n\
     - `skills`: その仕事に**必要な能力タグ**の配列（例 `[\"rust\",\"sqlite\"]`）。小文字・`[a-z0-9._-]`、最大 12 個。\n\
     \u{3000}分からなければ空でよい（その harness を既定に持つ担当に回る）\n\
     - `mode`: `prototype`（動くことを最短で示す）/ `production`（既定。テストと lint を通す）/ \
     `research`（主張に出典か計測を付ける）\n\
     - `repos` と、仕事の制約（検証方法・戻せるか・失敗したときの損失）は objective と acceptance に書け。\n\
     **担当（`assignee`）とモデル（`tier`）は選ぶな。** 書いても使われない（ADR-0069: 担当は matching、\n\
     lane は仕事の性質から celeris が決定的に決める）。\n\n"
        .to_string()
}

/// 同じことを委譲（`artifacts/delegate.json`）側にも書く（ADR-0033 D4）。
/// 部をまたぐ委譲は秘書の認可が要るので、それも伝える（SPEC §3.1）。
fn assignee_instructions_for_delegation(context: &RunContext) -> String {
    if context.organization.is_empty() {
        return String::new();
    }
    "委譲する子には goal（title / objective）・受け入れ条件・`genre`（harness）を書け。**担当（`assignee`）と\n\
     モデル（`tier`）は選ぶな**。書いても使われない（ADR-0069: 担当は celeris が skills と harness から決定的に\n\
     選び、lane は仕事の性質から決める）。上の組織図は「どんな担当がいるか」を知るためだけに使え。\n\n"
        .to_string()
}

/// `context.prior_review` があれば「前回の判定」として列挙する（ADR-0006 D2）。
fn prior_review_section(context: &RunContext) -> String {
    let mut out = String::new();
    if !context.prior_review.is_empty() {
        out.push_str("## Previous attempt's review result (this is a retry)\n");
        for pr in &context.prior_review {
            let verdict = if pr.pass { "pass" } else { "fail" };
            out.push_str(&format!(
                "- criterion {}: {verdict} ({})\n",
                pr.criterion, pr.reason
            ));
        }
        out.push('\n');
    }
    out
}

/// `context.answers` があれば「以前の質問への人間の回答」節として列挙する（ADR-0010 D3, P-10）。
/// Review では `ReviewRequest.answers` を別の節で出す。
fn answers_section(context: &RunContext) -> String {
    let mut out = String::new();
    if !context.answers.is_empty() {
        out.push_str("## Answers from a human to your earlier questions\n");
        for Answer { question, answer } in &context.answers {
            out.push_str(&format!("- Q: {question}\n  A: {answer}\n"));
        }
        out.push('\n');
    }
    out
}

/// 結果ファイル（`<artifacts_dir>/result.json`）の書式指示（ADR-0006 D3。全 kind 共通。ADR-0036 D3）。
fn result_json_instructions(artifacts: &str) -> String {
    format!(
        "Always write `{artifacts}/result.json` (create the `{artifacts}/` directory if it does not exist \
         yet) as a single JSON object of the form `{{\"summary\": \"<what you did>\", \"evidence\": []}}`. \
         `evidence` may be left empty; if you fill it, each element must be an object of the form \
         `{{\"criterion\": <index>, \"command\": \"<what you ran>\", \"exit\": <code>, \"stdout_tail\": \"...\"}}` \
         (plain strings are not accepted; `command`, `exit` and `stdout_tail` may be omitted for a criterion that \
         did not involve running a command). \
         If you cannot proceed and need a decision from a human, instead write \
         `{{\"question\": \"<your question>\"}}` to `{artifacts}/result.json` and stop there. This is a \
         non-interactive run: you cannot ask a question any other way, and no one will read your final \
         chat message directly.\n"
    )
}

/// ADR-0072 D10（Phase E1）: 予算の予告と rolling checkpoint の指示。coding 系の execute run
/// （対話は除く。D10）にだけ足す。claude-code は `--max-turns` で turn の上限を実際に強制するが、
/// codex/acp/aider は強制しない（目安として出す。§7 U1 と同じ「分類できなければ安全側」の考え方）。
/// **決定的**（壁時計の現在時刻は埋め込まない。DESIGN 原則 1 / 同じ入力から同じプロンプトを保つため。
/// 「開始時刻」は `WorkerStarted` イベントに残るので、ここに書かなくても run の記録からは追える）。
fn budget_preamble(task: &Task, artifacts: &str) -> String {
    format!(
        "## 予算 (budget)\n\
         この run の上限の目安: 最大 {} turn（claude-code はこれを `--max-turns` で実際に強制する。\
         他の harness では目安）/ 最大 {} 秒（壁時計はどの harness でも celeris が強制する）。\n\
         残りが約 20% になったら、または context が長くなってきたと感じたら、作業を区切りのよい所で \
         止め、`{artifacts}/checkpoint.json` を最新の状態にしてから `{artifacts}/result.json` に \
         `{{\"yield\": {{\"completed\": [...], \"remaining\": [...], \"next_action\": \"...\"}}}}` \
         （フィールドは全て任意、`checkpoint.json` と同じ形）を書いて終了せよ（予算切れで打ち切られる \
         より、区切って自分から止まる方が良い。これは失敗ではなく、続きは新しい session が checkpoint \
         から引き継ぐ）。\n\
         意味のある区切り（1 つの小目標の完了・方針の決定・テストの実行）のたびに \
         `{artifacts}/checkpoint.json` を次の形で上書きせよ（無ければ新規に作る。全欄が任意）:\n\
         `{{\"completed\":[\"...\"],\"remaining\":[\"...\"],\
         \"decisions\":[{{\"what\":\"...\",\"why\":\"...\"}}],\
         \"files_changed\":[{{\"path\":\"...\",\"change\":\"modified\"}}],\
         \"tests_run\":[{{\"command\":\"...\",\"exit\":0}}],\
         \"known_failures\":[{{\"what\":\"...\"}}],\"next_action\":\"...\"}}`\n\n",
        task.budget.max_turns, task.budget.max_wall_secs
    )
}

/// 実行中の委譲の方法（ADR-0016 D2 / M8, ADR-0027 D1）。
fn delegation_instructions(artifacts: &str) -> String {
    format!(
        "If you want to delegate part of this work to another agent, write `{artifacts}/delegate.json` \
         (create the `{artifacts}/` directory if it does not exist yet) as a single JSON object of the form \
         `{{\"tasks\":[{{\"title\":\"...\",\"objective\":\"...\",\"acceptance\":[{{\"text\":\"...\",\
         \"check\":{{\"type\":\"command\",\"cmd\":\"...\",\"expect_exit\":0}}}}],\"role\":\"<optional>\",\
         \"genre\":\"<optional>\",\
         \"depends_on\":[<index into this array, or an existing task id>]}}]}}`. \
         Do not choose an assignee or a model tier: celeris assigns the owner deterministically and picks the \
         model lane from the task's nature (ADR-0069). Describe the work, its acceptance checks, and constraints instead.\n\
         `check` may also be \
         `{{\"type\":\"artifact_exists\",\"name\":\"...\"}}`, `{{\"type\":\"reviewer\"}}`, or `{{\"type\":\"human\"}}`. \
         celeris will validate this after this run ends and insert whatever proposals pass validation as child \
         tasks (how many are accepted per run is limited by configuration; any rejected proposal has its \
         reason recorded as an event you cannot see, but a human can). A task cannot list its own parent or \
         itself in `depends_on`. This task will not be considered done until any children you delegated have \
         finished.\n"
    )
}

/// ADR-0098 D6（Phase R7-10）: 親の完了を止めない独立した後続 task の起票の方法。run の中から DB には書けない
/// （ADR-0095）ので、`followups.json` か run の中の `celerisctl add`（daemon の DB に向けたとき宣言になる）。
fn followups_instructions(artifacts: &str) -> String {
    format!(
        "If a human asked you to file follow-up tasks (independent work that should not block this task), \
         run `celerisctl add --title ... --objective ... --check-cmd ...` inside this run, or append \
         `{{\"tasks\":[<POST /tasks body>]}}` to `{artifacts}/followups.json`. You cannot write the database \
         directly from a run. celeris creates them as drafts when this run ends, in this task's project with its \
         repositories; do not set a project, parent, assignee or workspace.\n"
    )
}

/// ADR-0039 D3: 案件が作業場所を決めている run にだけ、委譲の指示に「子は同じ作業場所を継ぐ」を足す。
/// 決めていない案件では空文字列（Phase 42 までと 1 バイトも変わらない）。
pub(super) fn delegate_workspace_instruction(context: &RunContext) -> String {
    if context.workspace_note.is_none() {
        return String::new();
    }
    "Children you delegate inherit this project's workspace (the location described above), so do not \
     tell them to `ssh` into another host and edit files there. Only if a child must work somewhere else \
     (a different repository or cluster), give it a `workspace` of \
     `{\"kind\":\"local\",\"path\":\"...\"}` or `{\"kind\":\"remote\",\"cluster\":\"...\",\"path\":\"...\"}`.\n"
        .to_string()
}

/// ADR-0039 D3: 計画 run（`build_plan_prompt`）用。子タスクが継ぐ作業場所と、`plan.json` の `workspace` の
/// 使いどころを伝える。案件が作業場所を決めていない run では空文字列（従来どおりの文面）。
pub(super) fn workspace_section_for_plan(context: &RunContext) -> String {
    let Some(note) = &context.workspace_note else {
        return String::new();
    };
    format!(
        "## 子タスクの作業場所 (the workspace child tasks inherit)\n\
         {note}\n\
         コードを扱う仕事はこの場所で行われ、分解した子タスクはこの作業場所をそのまま継ぐ。担当に \
         `ssh` でリモートの作業ツリーへ直接書かせてはいけない（同期は celeris が行う）。**別の場所**\
         （別のリポジトリ・別のクラスタ）で作業させたい子にだけ、`workspace` に \
         `{{\"kind\":\"local\",\"path\":\"...\"}}` か \
         `{{\"kind\":\"remote\",\"cluster\":\"...\",\"path\":\"...\"}}` を書け。\n\n"
    )
}

/// `Execute`（および `Approval`）用プロンプト（ADR-0006 D2。既存のワーカー用プロンプトのまま）。
fn build_execute_prompt(
    task: &Task,
    context: &RunContext,
    run_id: &str,
    artifacts: &str,
) -> String {
    let mut out = prompt_header(task, context, run_id, artifacts);
    // ADR-0072 D9（Phase E1）: 続きの実行（continuation）の節。`context.continuation` が無い run の
    // 出力はここで 1 バイトも増えない。
    out.push_str(&crate::preamble::continuation_section(context));
    // ADR-0072 D10（Phase E1）: 予算の予告と rolling checkpoint の指示。対話 run には出さない
    // （D10: 対話・レビュー・計画・研究 harness は除く）。
    if context.conversation_addressee.is_none() {
        out.push_str(&budget_preamble(task, artifacts));
    }
    // ADR-0072 D9（Phase E2）: 計画のある Task の WU の run では、`## Acceptance criteria` は
    // この WU の `done_when` にし、Task の受け入れ条件は「最終レビューで確かめる」参考として別に出す
    // （`context.work_unit` が無い run は E1 までと 1 バイトも変わらない）。
    if let Some(wu) = &context.work_unit {
        out.push_str("## Acceptance criteria (this WorkUnit)\n");
        if wu.done_when.is_empty() {
            out.push_str(
                "(no explicit done_when was given for this WorkUnit; use the objective above)\n",
            );
        }
        for (i, c) in wu.done_when.iter().enumerate() {
            out.push_str(&format!("{i}. {c}\n"));
        }
        out.push('\n');
        out.push_str("## 最終レビューで確かめる Task の受け入れ条件（参考）\n");
        for (i, c) in task.acceptance.iter().enumerate() {
            out.push_str(&format!("{}. {}\n", i, c.text));
        }
        out.push('\n');
    } else {
        out.push_str("## Acceptance criteria\n");
        for (i, c) in task.acceptance.iter().enumerate() {
            let detail = match &c.check {
                Check::Command { cmd, expect_exit } => format!(
                    " (a reviewer will independently re-run `{cmd}` in this directory afterwards and \
                     requires exit code {expect_exit}; your own claim of success is not trusted)"
                ),
                Check::ArtifactExists { name } => {
                    format!(" (a reviewer will check that the file `{artifacts}/{name}` exists)")
                }
                Check::KnowledgePage { path } => {
                    format!(" (a human will check the knowledge base page `{path}`)")
                }
                Check::Reviewer | Check::Human => String::new(),
            };
            out.push_str(&format!("{}. {}{}\n", i, c.text, detail));
        }
        out.push('\n');
    }
    // ADR-0124 D4: 直行経路の run だけ。`context.direct_route` が無い run は 1 バイトも増えない。
    out.push_str(&direct_route_section(context));
    out.push_str(&prior_review_section(context));
    out.push_str(&answers_section(context));
    out.push_str(&children_section(context, artifacts));
    out.push_str(&organization_section(context));
    out.push_str(&available_genres_section(context, artifacts));
    out.push_str(&assignee_instructions_for_delegation(context));
    out.push_str("## Instructions\n");
    out.push_str("Work in the current directory (it is a dedicated workspace for this task). ");
    // ADR-0054 D2 / Phase 98 追記: 対話 run（`conversation_addressee` が Some）は結果ファイルを書いて
    // 返事だけをする（`preamble::conversation_instructions` の `actions` 経由でしか仕事を作れない）。
    // 対話 run の一部（CoS）は read-only サンドボックスで `artifacts/delegate.json` を書けないため、
    // 汎用の delegate.json 段落をここで出すと「委譲ファイルを作成できない」と誤って諦める
    // （実機障害 2026-09-22 00:18 UTC、task 01M337NT3QT1FR1G6WHS9G6NDA）。対話でない run の文面は
    // 1 バイトも変えない。
    if context.conversation_addressee.is_some() {
        out.push_str(
            "この run は返事だけを書く。仕事は返事の `actions` で作る（ファイルは書けない）。\n",
        );
    } else {
        out.push_str(&delegation_instructions(artifacts));
        out.push_str(&delegate_workspace_instruction(context));
        out.push_str(&followups_instructions(artifacts));
    }
    out.push_str("When you are done:\n");
    out.push_str(&result_json_instructions(artifacts));
    out
}

/// ADR-0124 D4: planner を挟まない直行経路の節（文面はここ 1 箇所。codex・acp・aider も
/// `build_prompt` を共有するので同じ節が出る）。`context.direct_route` が `None` なら空文字列。
pub(crate) fn direct_route_section(context: &RunContext) -> String {
    let Some(route) = &context.direct_route else {
        return String::new();
    };
    let mut out = String::from(
        "## 直行経路（planner なし）\n\
         この task は決定的な条件で分解不要と判定され、planner を挟まずこの 1 run で実装する。\n\
         - 調査 → 編集 → テスト → 局所修正を、この run の中で完結させる。\n\
         - 上の acceptance の command check を自分で実行し、落ちたら同じ run の中で直して再実行する。\n\
         - run が終わると celeris が同じ command check を決定的に再実行し、その後に最終レビューが入る。\n\
         - 範囲が想定より大きいと分かったら、無理に広げず result.json の summary にそう書く\n  \
         （分解は人が decompose で指示する。委譲の書式は従来どおり使える）。\n",
    );
    if !route.reasons.is_empty() {
        out.push_str(&format!("判定の根拠（{}）:\n", route.policy_version));
        for reason in &route.reasons {
            out.push_str(&format!("- {reason}\n"));
        }
    }
    out.push('\n');
    out
}

/// `Plan` kind 用プロンプト（DESIGN §5.6, ADR-0007 D7）。目標を独立に検証可能な受け入れ条件を持つ
/// 子タスク群に分解させ、`artifacts/plan.json` に `PlanOutput` を書かせる。
fn build_plan_prompt(task: &Task, context: &RunContext, run_id: &str, artifacts: &str) -> String {
    let mut out = prompt_header(task, context, run_id, artifacts);
    out.push_str(
        "## Instructions\n\
         Decompose this goal into a set of child tasks, each with an independently verifiable \
         acceptance criterion or criteria (DESIGN §5.6: \"目標を、独立に検証可能な受け入れ条件を持つ \
         子タスク群に分解せよ\"). Prefer 3 to 6 child tasks when the size of the goal makes that \
         reasonable (DESIGN §6 Phase 5 acceptance criteria); use fewer or more only if the goal \
         clearly requires it.\n\n",
    );
    out.push_str(&format!(
        "Write your decomposition to `{artifacts}/plan.json` (create the `{artifacts}/` directory if it \
         does not exist yet) as a single JSON object of exactly this shape:\n\
         ```json\n\
         {{\"tasks\":[{{\"title\":\"...\",\"objective\":\"...\",\
         \"acceptance\":[{{\"text\":\"...\",\"check\":{{\"type\":\"command\",\"cmd\":\"...\",\
         \"expect_exit\":0}}}}],\"depends_on\":[<index into this same tasks array>],\
         \"kind\":\"execute\"|\"plan\" (omit for \"execute\"),\
         \"harness\":\"<harness id>\", \"skills\":[\"<skill tag>\"], \
         \"mode\":\"prototype\"|\"production\"|\"research\" (optional), \
         \"repos\":[\"<repo name>\"] (optional)}}]}}\n\
         ```\n\
         Do not write `assignee` or `tier`: the owner and the model lane are decided by celeris (ADR-0069).\n\
         `check` may also be `{{\"type\":\"artifact_exists\",\"name\":\"...\"}}`, \
         `{{\"type\":\"knowledge_page\",\"path\":\"...\"}}` (a page in the knowledge base), \
         `{{\"type\":\"reviewer\"}}`, or `{{\"type\":\"human\"}}`. Unknown fields are rejected, so do not \
         add any field not shown above. `depends_on` indices refer to positions within this same \
         `tasks` array and must form a DAG (no self-reference, no cycles). Every child task must have \
         at least one `acceptance` entry. If a child has a `human` check, its `acceptance` must also \
         include an `artifact_exists` or `knowledge_page` check pointing at the human-readable material \
         (ADR-0067: it must live in registered artifacts or the knowledge base, not in the target \
         repository's tracked files, so a human can see it from the GUI). `kind:\"plan\"` children are \
         only allowed while the total decomposition depth stays within {} (DESIGN §5.6 \"分解の深さは \
         上限 3\"; this plan itself already counts toward that limit). Any `command` check will later be \
         re-run for real inside the child task's own working directory by an independent reviewer, so \
         do not fabricate a command whose result you have not actually observed.\n\n",
        task_core::plan::MAX_PLAN_DEPTH
    ));
    let schema = serde_json::to_string(&task_core::plan::schema_value())
        .unwrap_or_else(|_| "{}".to_string());
    out.push_str(&format!(
        "### Schema for the `{artifacts}/plan.json` object\n```json\n"
    ));
    out.push_str(&schema);
    out.push_str("\n```\n\n");
    out.push_str(&prior_review_section(context));
    out.push_str(&answers_section(context));
    // ADR-0046 D5（Phase 59）: 計画に組織図は渡さない（人選をさせない）。代わりに harness / skills /
    // mode を宣言させ、担当は決定的な matching が決める。
    out.push_str(&assignee_instructions_for_plan(context));
    out.push_str(&available_genres_section_for_plan(context, artifacts));
    out.push_str(&workspace_section_for_plan(context));
    out.push_str(&harness_artifacts_section_for_plan(context));
    out.push_str(&result_json_instructions(artifacts));
    out
}

/// ADR-0074 D3.3（Phase F4a (b)）: 案件レベルの計画（マイルストーン Task の DAG）を
/// `{artifacts}/project-plan.json`（`celeris.project-plan/1`）に書かせるプロンプト。旧来の
/// `build_plan_prompt`（`plan.json`、単発の分解）とは別物 — ここでは**案件全体**の途中目標の並びを
/// 設計させる（SPEC §7「途中目標は予め大まかに決めておいて、適宜再設計する」）。
fn build_project_plan_prompt(
    task: &Task,
    context: &RunContext,
    run_id: &str,
    artifacts: &str,
) -> String {
    let mut out = prompt_header(task, context, run_id, artifacts);
    out.push_str(
        "## Instructions\n\
         Design this project's milestones as a DAG: break the request into 1 to 12 milestones, each a \
         top-level, independently reportable unit of progress (SPEC §7: \"途中目標は予め大まかに決めておいて、\
         適宜再設計する\"). A milestone is not a single small task — it is a stage of the project a human \
         will explicitly approve or reject once it is reached (ADR-0038: ok / discuss / ng). Order them with \
         `depends_on` (a DAG, not necessarily a single chain): independent milestones may run in parallel, \
         while a milestone that needs another milestone's result depends on it.\n\n",
    );
    out.push_str(&format!(
        "Write your plan to `{artifacts}/project-plan.json` (create the `{artifacts}/` directory if it \
         does not exist yet) as a single JSON object of exactly this shape:\n\
         ```json\n\
         {{\"schema\":\"celeris.project-plan/1\",\"rationale\":\"...\",\
         \"milestones\":[{{\"key\":\"<[a-z0-9-]{{1,32}}, unique>\",\"title\":\"...\",\"objective\":\"...\",\
         \"reach_criteria\":\"what must be shown for a human to call this milestone reached\",\
         \"acceptance\":[{{\"text\":\"...\",\"check\":{{\"type\":\"human\"}}}}],\
         \"depends_on\":[\"<key of another milestone in this array>\"],\
         \"genre\":\"<optional>\",\"skills\":[\"<skill tag>\"],\"repos\":[\"<repo name>\"]}}]}}\n\
         ```\n\
         Do not write `assignee`, `tier`, `model` or `lane`: the owner and the model lane are decided by \
         celeris (ADR-0069). `check` may also be `{{\"type\":\"artifact_exists\",\"name\":\"...\"}}`, \
         `{{\"type\":\"knowledge_page\",\"path\":\"...\"}}`, `{{\"type\":\"reviewer\"}}`, or \
         `{{\"type\":\"command\",\"cmd\":\"...\",\"expect_exit\":0}}`. Unknown fields are rejected. Every \
         milestone must have at least one `acceptance` entry, and if it includes a `human` check, the \
         `acceptance` must also include an `artifact_exists` or `knowledge_page` check pointing at the \
         human-readable material (ADR-0067: it must live in registered artifacts or the knowledge base, not \
         in the target repository's tracked files). `depends_on` values must be keys of other milestones in \
         this same array and must form a DAG (no self-reference, no cycles). At most 12 milestones.\n\n",
    ));
    let schema = serde_json::to_string(&task_core::project_plan::schema_value())
        .unwrap_or_else(|_| "{}".to_string());
    out.push_str(&format!(
        "### Schema for the `{artifacts}/project-plan.json` object\n```json\n"
    ));
    out.push_str(&schema);
    out.push_str("\n```\n\n");
    out.push_str(&prior_review_section(context));
    out.push_str(&answers_section(context));
    out.push_str(&available_genres_section_for_project_plan(context));
    out.push_str(&workspace_section_for_plan(context));
    out.push_str(&result_json_instructions(artifacts));
    out
}

/// ADR-0074 D3.4（Phase F4b (e)）: 案件の replan。目的（objective）に現行の計画（`base_version` と各
/// マイルストーンの key・状態・変更できるか）が載っているので、それに対する**差分**を
/// `{artifacts}/project-plan.json`（`celeris.project-plan-delta/1`）に書かせる。
fn build_project_replan_prompt(
    task: &Task,
    context: &RunContext,
    run_id: &str,
    artifacts: &str,
) -> String {
    let mut out = prompt_header(task, context, run_id, artifacts);
    out.push_str(
        "## Instructions\n\
         This project already has an approved milestone plan (a DAG of milestones; the objective above lists \
         it with its `base_version`, each milestone's key, state and whether it can still be changed). \
         Revise the plan by writing a **delta** against it — do not rewrite the whole plan. The current plan \
         keeps running until a human approves your delta, so only change what the reason for this replan \
         requires.\n\n\
         Rules: `modify` and `remove` may only name milestones that have not been dispatched yet (marked \
         変更可). A milestone that is running or finished cannot be modified or removed; if it must stop, \
         list it in `cancel` explicitly (a running one will be cancelled on approval; a finished one cannot \
         be cancelled). A key may appear in only one of `modify` / `remove` / `cancel`. New milestones go to \
         `add` with keys that do not collide with any existing key. After the delta, every `depends_on` must \
         name a milestone that is still in the plan (so when you remove or cancel a milestone, modify the \
         not-yet-dispatched milestones that depended on it), the plan must stay a DAG, and it must keep 1 to \
         12 milestones.\n\n",
    );
    out.push_str(&format!(
        "Write the delta to `{artifacts}/project-plan.json` (create the `{artifacts}/` directory if it does \
         not exist yet) as a single JSON object of exactly this shape:\n\
         ```json\n\
         {{\"schema\":\"celeris.project-plan-delta/1\",\"base_version\":<the base_version above>,\
         \"rationale\":\"why the plan changes\",\
         \"add\":[<milestone objects in the celeris.project-plan/1 shape>],\
         \"modify\":[{{\"key\":\"<existing key>\",\"title\":\"<only the fields you change>\"}}],\
         \"remove\":[\"<existing key>\"],\"cancel\":[\"<existing key>\"]}}\n\
         ```\n\
         Do not write `assignee`, `tier`, `model` or `lane` (ADR-0069). Unknown fields are rejected. Every \
         added milestone needs at least one `acceptance` entry, and a `human` check needs an \
         `artifact_exists` or `knowledge_page` check next to it (ADR-0067).\n\n",
    ));
    let schema = serde_json::to_string(&task_core::project_plan::delta_schema_value())
        .unwrap_or_else(|_| "{}".to_string());
    out.push_str(&format!(
        "### Schema for the `{artifacts}/project-plan.json` delta object\n```json\n"
    ));
    out.push_str(&schema);
    out.push_str("\n```\n\n");
    out.push_str(&prior_review_section(context));
    out.push_str(&answers_section(context));
    out.push_str(&available_genres_section_for_project_plan(context));
    out.push_str(&workspace_section_for_plan(context));
    out.push_str(&result_json_instructions(artifacts));
    out
}

/// ADR-0074 D3.3（Phase F4a (b)）: 案件計画専用の「使える専門家」節。`available_genres_section_for_plan`
/// と違い、milestone spec には `role` が無いので `genre` だけを宣言させる（子タスクではなくマイルストーン
/// の話であることも明示する）。
fn available_genres_section_for_project_plan(context: &RunContext) -> String {
    if context.available_genres.is_empty() {
        return String::new();
    }
    let mut out =
        String::from("## 使える専門家 (available genres a milestone can be tagged with)\n");
    out.push_str(&genre_list_lines(context));
    out.push_str(
        "\nIf a milestone's work belongs to a particular genre, set its `genre` in \
         `project-plan.json`. Do not set `role` (milestones have none; the owning Task's role, if any, is \
         decided when it is dispatched).\n\n",
    );
    out
}

/// ADR-0072 D14（Phase E3）: task-local な planner run 用プロンプト。Complexity Gate が compound と
/// 判定した Task を WorkUnit の集合に分解させ、`artifacts/execution-plan.json` に
/// `ExecutionPlanSpec`（`celeris.execution-plan/1`）を書かせる。この run 自身は普通のワーカー run と
/// 同じ `result.json` の契約（`summary`/`evidence`、または `question`）で終わる。計画の検証・採用・
/// 不正なら 1 回だけ再試行・それでも駄目なら atomic に倒す判断は daemon（`task-dispatch`）が行う
/// （このプロンプトの役目ではない）。
fn build_execution_plan_prompt(
    task: &Task,
    context: &RunContext,
    run_id: &str,
    artifacts: &str,
) -> String {
    let mut out = prompt_header(task, context, run_id, artifacts);
    out.push_str(
        "## Instructions\n\
         This task was judged too large or too broad for a single session (ADR-0072 Complexity Gate). \
         Decompose it into a small number of WorkUnits, each of which fits comfortably in one context \
         window (investigation, design, implementation, tests, release — whatever split makes sense for \
         this goal). Prefer the smallest number of WorkUnits that keeps each one focused; do not split \
         further than the goal actually requires. You are planning, not doing the work yourself: do not \
         write code or run the actual implementation in this run.\n\n",
    );
    // ADR-0074 D1.1（Phase F2b）: `[execution] parallel = true` のときだけ v2 を書かせる。
    let parallel = context
        .execution_planner
        .as_ref()
        .is_some_and(|p| p.parallel);
    let planner_schema = if parallel {
        task_core::EXECUTION_PLAN_SCHEMA_V2
    } else {
        task_core::EXECUTION_PLAN_SCHEMA
    };
    let max_work_units = context
        .execution_planner
        .as_ref()
        .map(|p| p.max_work_units)
        .unwrap_or(8);
    let work_unit_max_turns = context
        .execution_planner
        .as_ref()
        .map(|p| p.work_unit_max_turns)
        .unwrap_or(80);
    let work_unit_max_wall_secs = context
        .execution_planner
        .as_ref()
        .map(|p| p.work_unit_max_wall_secs)
        .unwrap_or(3600);
    let default_max_turns = context
        .execution_planner
        .as_ref()
        .map(|p| p.default_max_turns)
        .unwrap_or(30);
    let default_max_wall_secs = context
        .execution_planner
        .as_ref()
        .map(|p| p.default_max_wall_secs)
        .unwrap_or(1800);
    let tree = context
        .execution_planner
        .as_ref()
        .and_then(|p| p.tree.as_ref());
    if let (Some(planner), Some(tree)) = (context.execution_planner.as_ref(), tree) {
        // ADR-0079 D4 (2)（Phase R2b）: 木の節点の planner は /3（段階・unit = leaf | 子 task・決定）を書く。
        out.push_str(&tree_plan_shape_section(planner, tree, artifacts));
    } else {
        out.push_str(&format!(
        "Write your plan to `{artifacts}/execution-plan.json` (create the `{artifacts}/` directory if it \
         does not exist yet) as a single JSON object of exactly this shape:\n\
         ```json\n\
         {{\"schema\":\"{planner_schema}\",\"rationale\":\"...\",\"work_units\":[{{\"key\":\"survey\",\
         \"kind\":\"investigate\"|\"design\"|\"implement\"|\"test\"|\"release\"|\"repair\"|\"other\",\
         \"title\":\"...\",\"objective\":\"...\",\"depends_on\":[\"<key of another work unit>\"],\
         \"done_when\":[\"...\"],\"checks\":[{{\"cmd\":\"...\",\"expect_exit\":0}}],\
         \"context\":{{\"paths\":[\"...\"],\"from_work_units\":[\"<key>\"],\"knowledge\":[\"...\"]}},\
         \"harness\":\"<genre id, or omit to inherit this task's genre>\",\
         \"features\":{{\"judgment\":\"low\"|\"medium\"|\"high\",\"ambiguity\":\"low\"|\"medium\"|\"high\",\
         \"verifiability\":\"low\"|\"medium\"|\"high\",\"reversibility\":\"low\"|\"medium\"|\"high\",\
         \"consequence\":\"low\"|\"medium\"|\"high\"}} (see below),\
         \"budget\":{{\"max_turns\":40,\"max_wall_secs\":1800}} (optional),\
         \"outputs\":[\"...\"]}}]}}\n\
         ```\n\
         Do not write `assignee`, `tier`, `model`, or `lane`: the owner never changes (WorkUnits inherit \
         this task's owner) and the model lane is decided by celeris from each WorkUnit's nature \
         (ADR-0069, ADR-0074). Unknown fields are rejected, so do not add any field not shown above. \
         `key` must match `[a-z0-9-]{{1,32}}` and be unique within this plan. `depends_on` refers to \
         other `key`s in this same array and must form a DAG (no cycles). `checks` may only be \
         `{{\"cmd\":\"...\",\"expect_exit\":0}}` (a deterministic command check; do not fabricate one \
         you have not actually run — it will really be executed later). Give every mechanically \
         checkable WorkUnit an executable `checks` entry: it raises that WorkUnit's verifiability and \
         lets it route to a cheaper lane instead of defaulting to this task's own lane. `harness`, if \
         set, must be one of the genre ids listed below (available genres); an unset `harness` inherits \
         this task's own genre. Plan at most {max_work_units} WorkUnits (a plan with more will be \
         rejected and this run retried once). If you write a `budget` for a WorkUnit, `max_turns` will \
         be capped at {work_unit_max_turns} and `max_wall_secs` at {work_unit_max_wall_secs}; if you \
         omit it, the default is max({default_max_turns}, task budget) turns and \
         max({default_max_wall_secs}, task budget) seconds.\n\n"
    ));
    }
    // Phase F5-fix3: 検証が使う上限をすべて、計画の形の説明のすぐ後に出す。前の試行が拒否されていれば、
    // その理由も（同じ間違いを繰り返させない。dogfood 4 回目は 2 回とも `too many checks: 8 > 6`）。
    if let Some(planner) = &context.execution_planner {
        out.push_str(&plan_limits_section(planner));
        // ADR-0079 R7-2/R7-10: check の書き方（本番で check 自体が誤って落ちた形）。
        out.push_str(PLANNER_CHECK_GUIDANCE);
        // ADR-0095 付記 D-d: 本番 host の操作は人が実行する手順として書く。
        out.push_str(PRODUCTION_HOST_PLANNER_GUIDANCE);
        out.push_str(&previous_attempt_errors_section(planner, artifacts));
    }
    let schema = serde_json::to_string(&task_core::execution_plan::schema_value())
        .unwrap_or_else(|_| "{}".to_string());
    out.push_str(&format!(
        "### Schema for the `{artifacts}/execution-plan.json` object\n```json\n"
    ));
    out.push_str(&schema);
    out.push_str("\n```\n\n");
    out.push_str(&work_unit_features_section());
    if let Some(planner) = context
        .execution_planner
        .as_ref()
        .filter(|p| p.parallel && p.tree.is_none())
    {
        out.push_str(&parallel_phases_section(planner.max_phases));
    }
    if let Some(planner) = &context.execution_planner {
        out.push_str(&format!(
            "## Why this task was judged compound (Complexity Gate, rule `{}`, score {})\n",
            planner.gate_rule_id, planner.gate_score
        ));
        if planner.gate_signals.is_empty() {
            out.push_str(
                "(no scored signals — a human or the CoS marked this task compound directly)\n\n",
            );
        } else {
            for line in &planner.gate_signals {
                out.push_str(&format!("- {line}\n"));
            }
            out.push('\n');
        }
    }
    // ADR-0072 D17（Phase E4b 項目1）: replan のときだけ、今の計画・WU の状態・起こした理由を足す
    // （`replan = false` の run は 1 バイトも変わらない）。
    if let Some(planner) = context.execution_planner.as_ref().filter(|p| p.replan) {
        match &planner.tree {
            Some(tree) => out.push_str(&tree_replan_context_section(planner, tree)),
            None => out.push_str(&replan_context_section(planner)),
        }
    }
    if !context.available_genres.is_empty() {
        out.push_str("## Available genres (valid values for a WorkUnit's `harness`)\n");
        out.push_str(&genre_list_lines(context));
        out.push('\n');
    }
    out.push_str(&prior_review_section(context));
    out.push_str(&answers_section(context));
    out.push_str(&result_json_instructions(artifacts));
    out
}

/// Phase F5-fix3: planner context の上限を `ExecutionLimits` に戻す。0（古い request で欄が無い）は
/// `ExecutionLimits::default()` の値に倒す。
fn planner_limits(
    planner: &crate::protocol::ExecutionPlannerContext,
) -> task_core::ExecutionLimits {
    let d = task_core::ExecutionLimits::default();
    let or = |v: usize, dv: usize| if v == 0 { dv } else { v };
    task_core::ExecutionLimits {
        max_work_units: or(planner.max_work_units, d.max_work_units),
        work_unit_max_turns: if planner.work_unit_max_turns == 0 {
            d.work_unit_max_turns
        } else {
            planner.work_unit_max_turns
        },
        work_unit_max_wall_secs: if planner.work_unit_max_wall_secs == 0 {
            d.work_unit_max_wall_secs
        } else {
            planner.work_unit_max_wall_secs
        },
        max_rationale_chars: or(planner.max_rationale_chars, d.max_rationale_chars),
        max_title_chars: or(planner.max_title_chars, d.max_title_chars),
        max_objective_chars: or(planner.max_objective_chars, d.max_objective_chars),
        max_done_when_items: or(planner.max_done_when_items, d.max_done_when_items),
        max_done_when_chars: or(planner.max_done_when_chars, d.max_done_when_chars),
        max_checks: or(planner.max_checks, d.max_checks),
        max_plan_json_bytes: or(planner.max_plan_json_bytes, d.max_plan_json_bytes),
        // ADR-0079 R7-2: /3 の planner の `max_plan_json_bytes` は dispatcher が /3 の上限で埋める。
        max_plan_json_bytes_v3: or(planner.max_plan_json_bytes, d.max_plan_json_bytes_v3),
        // `max_work_units` は dispatcher が v1/v2 に応じて選んだ値（`max_work_units_v2` を含む）。
        max_work_units_v2: or(planner.max_work_units, d.max_work_units_v2),
        max_phases: or(planner.max_phases, d.max_phases),
        max_children: or(planner.max_children, d.max_children),
        // ADR-0079 D4 (2)（Phase R2b）: /3 の planner には検証と同じ木の上限を渡す（`context.tree`）。
        tree: match &planner.tree {
            Some(t) => task_core::TreeLimits {
                enabled: true,
                max_depth: t.max_depth,
                max_units_per_stage: t.max_units_per_stage,
                max_stages: t.max_stages,
                max_child_tasks_per_plan: t.max_child_tasks_per_plan,
                max_parallel_child_tasks: t.max_parallel_child_tasks,
                max_open_decisions_per_plan: t.max_decisions_per_plan,
                ..d.tree
            },
            None => d.tree,
        },
    }
}

/// Phase F5-fix3: 計画の上限（`task_core::execution_plan::validate` が拒否する条件）を全部並べる。
/// 値は run の `ExecutionPlannerContext`（dispatcher が実際に検証に使う `ExecutionLimits` から埋める）。
fn plan_limits_section(planner: &crate::protocol::ExecutionPlannerContext) -> String {
    let l = planner_limits(planner);
    let mut out = String::from(
        "### Plan limits (celeris validates the plan against these; a plan that exceeds ANY of them \
         is rejected as a whole)\n",
    );
    if planner.tree.is_some() {
        // ADR-0079 D3（Phase R2b）: /3 の計画の上限（検証で拒否）。
        out.push_str(&format!(
            "- `stages`: 1 to {} stages. At most {} units per stage (leaves + child tasks; units already done, \
             `adopt` units, and celeris-added integration steps and repairs do not count). At most {} units with `\"kind\":\"task\"` that will \
             create a child (units already done and `adopt` units do not count). At most {} `decisions` \
             (plan-level and unit-level together).\n",
            l.tree.max_stages,
            l.tree.max_units_per_stage,
            l.tree.max_child_tasks_per_plan,
            l.tree.max_open_decisions_per_plan
        ));
    } else {
        out.push_str(&format!(
            "- `work_units`: 1 to {} WorkUnits (celeris-added integration steps and repairs do not count).\n",
            l.max_work_units
        ));
    }
    if planner.parallel && planner.tree.is_none() {
        out.push_str(&format!(
            "- `phases`: 1 to {} phases. `children`: at most {} child tasks.\n",
            l.max_phases, l.max_children
        ));
    }
    out.push_str(&format!(
        "- Per WorkUnit: `title` at most {} characters; `objective` at most {} characters; at most {} \
         `done_when` items, each at most {} characters; **at most {} `checks`**. If more commands need to \
         run, combine related ones into a single check (e.g. `{{\"cmd\":\"cargo fmt --all -- --check && \
         cargo clippy --workspace -- -D warnings\",\"expect_exit\":0}}`) or move them into `done_when`.\n",
        l.max_title_chars,
        l.max_objective_chars,
        l.max_done_when_items,
        l.max_done_when_chars,
        l.max_checks
    ));
    out.push_str(&format!(
        "- `rationale` at most {} characters; the whole plan JSON at most {} bytes.\n",
        l.max_rationale_chars,
        if planner.tree.is_some() {
            l.max_plan_json_bytes_v3
        } else {
            l.max_plan_json_bytes
        }
    ));
    if planner.tree.is_some() {
        out.push_str(&format!(
            "- A leaf `budget` above `max_turns` {} or `max_wall_secs` {} is **rejected** (not capped): such a \
             unit does not fit one run, so declare it as a child task or split it.\n",
            l.work_unit_max_turns, l.work_unit_max_wall_secs
        ));
    } else {
        out.push_str(&format!(
            "- A WorkUnit `budget` is not rejected but capped: `max_turns` at {}, `max_wall_secs` at {}.\n",
            l.work_unit_max_turns, l.work_unit_max_wall_secs
        ));
    }
    if planner.replan && planner.tree.is_none() {
        out.push_str(
            "- When replanning with a diff, the limits apply to the resulting plan after the diff is \
             applied (including the WorkUnits that carry over unchanged).\n",
        );
    }
    out.push_str(
        "Count these yourself before you finish: celeris does not trim or merge anything for you.\n\n",
    );
    out
}

/// Phase F5-fix3: 前の planner run の計画が拒否された理由（同じ計画の回の中）。拒否された計画のファイルは
/// dispatcher が `execution-plan.rejected.json` に移してある（そのまま再提出させない）。
fn previous_attempt_errors_section(
    planner: &crate::protocol::ExecutionPlannerContext,
    artifacts: &str,
) -> String {
    if planner.previous_attempt_errors.is_empty() {
        return String::new();
    }
    let mut out = String::from("### Your previous plan was REJECTED\n");
    out.push_str(
        "A previous planner run for this same plan wrote a plan that celeris rejected with these \
         validation error(s):\n",
    );
    for e in &planner.previous_attempt_errors {
        out.push_str(&format!("- {e}\n"));
    }
    out.push_str(&format!(
        "\nThe rejected file was moved to `{artifacts}/execution-plan.rejected.json`. Do NOT resubmit it \
         unchanged and do not assume any plan file you find is valid: start from it if useful, fix \
         exactly the problems listed above (keep everything else as it was), check the result against \
         the plan limits above, and write the corrected plan to `{artifacts}/execution-plan.json`.\n\n"
    ));
    out
}

/// ADR-0079 D4 (2) / D7 / D12（Phase R2b）: 木の節点の planner の計画の形（`celeris.execution-plan/3`）、木の中の
/// 位置（深さ・残りの深さ・祖先）、leaf の基準、決定の要求の書き方、木の上限の残り、人の段階の名指し。
/// `context.execution_planner.tree` があるときだけ、/1・/2 の形の説明の代わりに出す。
fn tree_plan_shape_section(
    planner: &crate::protocol::ExecutionPlannerContext,
    tree: &crate::protocol::TreePlannerContext,
    artifacts: &str,
) -> String {
    let schema = task_core::EXECUTION_PLAN_SCHEMA_V3;
    let l = planner_limits(planner);
    let mut out = String::new();
    out.push_str(&format!(
        "### Plan shape: `{schema}` (recursive task tree, ADR-0079)\n\
         Write your plan to `{artifacts}/execution-plan.json` (create the `{artifacts}/` directory if it \
         does not exist yet) as a single JSON object of exactly this shape:\n\
         ```json\n\
         {{\"schema\":\"{schema}\",\"rationale\":\"...\",\
         \"stages\":[{{\"key\":\"build\",\"kind\":\"investigate\"|\"design\"|\"implement\"|\"test\"|\"release\"|\"other\",\
         \"title\":\"...\",\"review\":\"none\"|\"human\" (optional)}}],\
         \"units\":[\
         {{\"key\":\"api\",\"stage\":\"build\",\"kind\":\"investigate\"|\"design\"|\"implement\"|\"test\"|\"release\"|\"other\",\
         \"title\":\"...\",\"objective\":\"...\",\"depends_on\":[\"<unit key>\"],\"needs_decisions\":[\"<decision key>\"],\
         \"done_when\":[\"...\"],\"checks\":[{{\"cmd\":\"...\",\"expect_exit\":0}}],\
         \"context\":{{\"repo\":\"<one repository>\",\"paths\":[\"...\"],\"from_work_units\":[\"<key>\"],\"knowledge\":[\"...\"]}},\
         \"harness\":\"<genre id, or omit>\",\"features\":{{...}},\
         \"budget\":{{\"max_turns\":40,\"max_wall_secs\":1800}} (optional),\"outputs\":[\"...\"]}},\
         {{\"key\":\"part-b\",\"stage\":\"build\",\"kind\":\"task\",\"title\":\"...\",\"objective\":\"...\",\
         \"acceptance\":[{{\"text\":\"...\",\"check\":{{\"type\":\"command\",\"cmd\":\"...\",\"expect_exit\":0}}}}],\
         \"depends_on\":[\"<unit key>\"],\"needs_decisions\":[\"<decision key>\"],\"genre\":\"<optional>\",\
         \"skills\":[\"<skill tag>\"],\"repos\":[\"<subset of this task's repositories>\"],\
         \"gate\":\"compound\"|\"atomic\" (optional),\"features\":{{...}}}}],\
         \"decisions\":[{{\"key\":\"h1\",\"question\":\"...\",\
         \"options\":[{{\"key\":\"a\",\"label\":\"...\",\"consequence\":\"...\"}},{{\"key\":\"b\",\"label\":\"...\"}}],\
         \"recommended\":\"a\",\"cost_of_reversal\":\"low\"|\"medium\"|\"high\",\"cost_note\":\"...\",\
         \"needed_before\":[\"<unit key>\"|\"stage:<stage key>\"]}}]}}\n\
         ```\n\
         A unit is either a **leaf** (any `kind` except `task`: one WorkUnit, finished by one run) or a \
         **child task** (`\"kind\":\"task\"`: its own task with its own acceptance, final review, gate and — if \
         needed — its own plan). Do not write `phases`, `work_units` or `children` (those are the older \
         shapes), and do not write `assignee`, `tier`, `model`, `lane` or `adopt`. Unknown fields are \
         rejected. Keys (stages, units, decisions, options) match `[a-z0-9-]{{1,32}}`; unit keys are unique \
         in the plan and must not start with `integrate-` (celeris adds one integration step per stage and \
         merges every leaf branch and child-task branch of the stage into this task's branch).\n\
         Stages run in array order. Units of the same stage may run in parallel; a unit may depend on at \
         most one unit of its own stage, and `depends_on` may point to the same or an earlier stage only. \
         `\"review\":\"human\"` pauses after that stage for a human check; use it only when a human really \
         has to look before the next stage.\n\n"
    ));

    out.push_str("#### Where this task sits in the tree\n");
    out.push_str(&format!(
        "- Depth: {} (task levels; the root task is depth 1). Max depth: {}. **Remaining depth: {}**.\n",
        tree.depth, tree.max_depth, tree.remaining_depth
    ));
    if tree.remaining_depth >= 1 {
        out.push_str(
            "- You may declare units with `\"kind\":\"task\"` (child tasks). A child task plans itself by \
             default; write `\"gate\":\"atomic\"` on the unit when the child should run as a single run.\n",
        );
    } else {
        out.push_str(
            "- Remaining depth is 0: **do NOT write any unit with `\"kind\":\"task\"`** (such a plan is \
             rejected). Every unit must be a leaf. If a piece of work cannot fit in a leaf, raise a decision \
             for it (see below) instead of forcing it into a leaf.\n",
        );
    }
    if !tree.ancestors.is_empty() {
        out.push_str("- Ancestors (root first):\n");
        for (i, a) in tree.ancestors.iter().enumerate() {
            let stage = a
                .stage
                .as_deref()
                .map(|s| format!(" (stage `{s}`)"))
                .unwrap_or_default();
            out.push_str(&format!(
                "  - depth {}: \"{}\"{stage} — {}\n",
                i + 1,
                a.title,
                a.objective_excerpt.replace('\n', " ")
            ));
        }
    }
    out.push('\n');

    out.push_str(&format!(
        "#### Leaf criteria (ADR-0079 D4) — every leaf must meet ALL three\n\
         (a) **Bounded**: it finishes in one run — `budget.max_turns` ≤ {} and `budget.max_wall_secs` ≤ {} \
         (a larger budget is rejected, not capped). Continuation is a safety net, not a plan.\n\
         (b) **One area, one repository**: `context.repo` names at most one repository and `context.paths` \
         stays within one subtree.\n\
         (c) **At least one command check**: `checks` has at least one deterministic command \
         (`{{\"cmd\":\"...\",\"expect_exit\":0}}`) that really verifies the result. \
         否定の grep（`! grep …`）を check に書くときは、自分が書く説明文や ADR の本文に当たらないか確かめる\
         （自己言及で落ちた実例あり）。\n\
         Declare a unit as a child task (`\"kind\":\"task\"`) when it fails any of these, when it is a \
         deliverable that should be accepted and reviewed on its own, when it needs a human acceptance or a \
         decision (`needs_decisions`), or when it needs another department's skills. A child-task unit needs \
         `acceptance` (at least one criterion) and must NOT have `checks`, `budget`, `harness` or \
         `context.paths` (the child decides those itself); its `repos` must be a subset of this task's \
         repositories.\n\
         A child-task unit may set `gate` (leaves must not): `\"gate\":\"compound\"` — the child writes its own \
         plan and splits the work (this is the default when `gate` is omitted: choosing `\"kind\":\"task\"` \
         means the child needs its own plan). `\"gate\":\"atomic\"` — the child runs as a single node without \
         a plan (one worker run and its own final review); write it when you want a child task that is \
         accepted on its own but small enough for one run. celeris does not override an explicit gate.\n\
         **A unit that cannot fit in a leaf must be declared as a child task{} or raised as a decision — never \
         squeezed into a leaf.** celeris re-gates every leaf when it adopts the plan (a leaf that is too \
         large becomes a child task) and records any disagreement with your declaration.\n\
         {}\n\n",
        l.work_unit_max_turns,
        l.work_unit_max_wall_secs,
        if tree.remaining_depth >= 1 {
            ""
        } else {
            " (not possible at this depth)"
        },
        CLUSTER_JOB_PLANNER_GUIDANCE,
    ));

    out.push_str(&format!(
        "#### Decisions for a human (`decisions`, ADR-0079 D7)\n\
         Raise a decision only when the work needs a choice that you cannot make from the objective, the \
         acceptance criteria and the repository (for example: which external service to depend on, which of \
         two incompatible designs the human wants, whether to spend a scarce resource). Do not ask about \
         things you can decide yourself. Each decision has 2 to 5 `options` (with `key` and `label`, \
         optionally `consequence`), a `recommended` option key, `cost_of_reversal` (`low` | `medium` | \
         `high`, how expensive it is to change the answer later) and a non-empty `needed_before`: the unit \
         keys or `stage:<key>` that must wait for the answer. Units that wait list the decision key in their \
         `needs_decisions`. Only what waits for an answer stops; every other unit proceeds. At most {} \
         decisions per plan. You may also write `decisions` inside a unit (the unit is then added to \
         `needed_before`).\n\n",
        l.tree.max_open_decisions_per_plan
    ));

    out.push_str("#### Remaining limits of this tree\n");
    out.push_str(&format!(
        "- This plan: at most {} stages, {} units per stage, {} child-task units, {} decisions (a plan \
         beyond these is rejected).\n",
        l.tree.max_stages,
        l.tree.max_units_per_stage,
        l.tree.max_child_tasks_per_plan,
        l.tree.max_open_decisions_per_plan
    ));
    let tokens = tree
        .tokens_left
        .map(|t| format!(", tokens left: {t}"))
        .unwrap_or_default();
    out.push_str(&format!(
        "- Whole tree (all tasks under the root): leaves left: {}, runs left: {}, replans left: {} (this \
         task: {}){tokens}, open decisions left: {}. Work beyond these limits is stopped and a human is \
         asked, so plan within them: prefer fewer, well-bounded units.\n",
        tree.leaves_left,
        tree.runs_left,
        tree.replans_left,
        tree.node_replans_left,
        tree.open_decisions_left
    ));
    out.push_str(&format!(
        "- At most {} child tasks of this task run at the same time (the rest wait; nothing is dropped).\n\n",
        tree.max_parallel_child_tasks
    ));

    if !tree.stages_hint.is_empty() {
        out.push_str("#### Stages the human named (stages_hint, ADR-0079 D12)\n");
        for h in &tree.stages_hint {
            if h.scope.is_empty() {
                out.push_str(&format!("- \"{}\"\n", h.title));
            } else {
                out.push_str(&format!("- \"{}\": {}\n", h.title, h.scope));
            }
        }
        out.push_str(
            "Use these as the guide for your stages (their names and scope, in this order). They are input, \
             not a required structure: keep the whole scope the human asked for and do not narrow it.\n\n",
        );
    }
    out
}

/// ADR-0079 D9（Phase R2b）: /3 の replan の節。差分（`execution-plan-delta/1`）は /2 の形しか持たないので、/3 は
/// 計画の全体を書かせる（done の unit は daemon が採用した spec のまま持ち越す）。子 task の失敗は理由・checkpoint の
/// 要約つきで `replan_reason` と unit の要約に入っている。
fn tree_replan_context_section(
    planner: &crate::protocol::ExecutionPlannerContext,
    _tree: &crate::protocol::TreePlannerContext,
) -> String {
    let mut out = String::from("## You are REPLANNING an existing execution plan\n");
    out.push_str(&format!(
        "This is not the first plan for this task: a previous plan already ran, and something about it \
         needs to change. Write the whole new plan in the `{}` shape above (the diff schema is not \
         available for this shape). The plan limits apply to the resulting plan including the units that \
         carry over.\n\n",
        task_core::EXECUTION_PLAN_SCHEMA_V3
    ));
    if let Some(version) = planner.current_plan_version {
        out.push_str(&format!("Current (superseded) plan version: v{version}.\n"));
    }
    out.push_str(&format!(
        "Why this replan was triggered: {}\n\n",
        if planner.replan_reason.is_empty() {
            "(not recorded)"
        } else {
            planner.replan_reason.as_str()
        }
    ));
    if !planner.work_unit_summaries.is_empty() {
        out.push_str(
            "### Units in the current plan (status and, for child tasks, the child's outcome)\n",
        );
        for line in &planner.work_unit_summaries {
            out.push_str(&format!("- {line}\n"));
        }
        out.push('\n');
    }
    if !planner.preserve_done_keys.is_empty() {
        out.push_str(&format!(
            "These units are already done and carry over unchanged (celeris restores their adopted spec; you \
             may omit them): {}. The only thing you may change in a done unit is its `checks`: write the unit \
             with a new non-empty `checks` list to replace a check that cannot hold after the stage \
             integration (e.g. one that assumes a merge commit). The done unit is not re-run; its checks are \
             re-run at the stage integration. Every other field is restored.\n\n",
            planner.preserve_done_keys.join(", ")
        ));
    }
    out.push_str(
        "For a child-task unit whose child **failed** you may: keep the same unit key (celeris then creates a \
         new child task from that unit — the next attempt; fix its objective or acceptance so the retry can \
         succeed, using the failure reason above), split it into several units, drop it, or raise a decision \
         if a human has to choose. The failed child's branch and history are kept. A unit whose child is \
         still running keeps its child. A key that belonged to a unit you removed earlier cannot be reused.\n\n",
    );
    out
}

/// ADR-0074 D1.1（Phase F2b）: v2（工程と並列の WU）の書き方。`[execution] parallel = true` の planner
/// run にだけ足す（v1 のプロンプトは 1 バイトも変わらない）。
fn parallel_phases_section(max_phases: usize) -> String {
    let max_phases = if max_phases == 0 { 5 } else { max_phases };
    let mut out = String::from("### Phases and parallel WorkUnits (`celeris.execution-plan/2`)\n");
    out.push_str(&format!(
        "Group the WorkUnits into ordered phases. Add a top-level `\"phases\":[{{\"key\":\"build\",\
         \"kind\":\"implement\",\"title\":\"...\"}}, ...]` array (1 to {max_phases} phases; the array \
         order is the execution order; `key` matches `[a-z0-9-]{{1,32}}`) and give every WorkUnit a \
         `\"phase\":\"<phase key>\"`. Rules (a plan that breaks them is rejected):\n\
         - WorkUnits in the **same phase may run in parallel**, each in its own git worktree and branch. \
         Put independent pieces of work (different files/modules) in the same phase; celeris merges \
         each phase's branches deterministically at the end of the phase and re-runs the checks.\n\
         - Within a phase a WorkUnit may depend on **at most one** other WorkUnit of the same phase \
         (it then starts from that WorkUnit's branch). If it needs two or more, put it in a later phase.\n\
         - `depends_on` may point to WorkUnits in the same phase or an earlier phase, never a later one.\n\
         - Do not use keys starting with `integrate-` and do not write `\"kind\":\"integrate\"` \
         (celeris adds one integration step per phase itself).\n\n"
    ));
    // ADR-0074 D3.7（Phase F4b (f)）: 子 Task の提案（`children`）の書き方。
    out.push_str(
        "#### Child tasks (`children`, optional)\n\
         Only when part of this goal is really a **separate deliverable** — it needs a different \
         department's skills, a different repository, or a human wants to approve it on its own — propose \
         it as a child task instead of a WorkUnit: add a top-level `\"children\":[{\"key\":\"<[a-z0-9-]{1,32}>\",\
         \"title\":\"...\",\"objective\":\"...\",\"acceptance\":[{\"text\":\"...\",\"check\":{\"type\":\"command\",\
         \"cmd\":\"...\",\"expect_exit\":0}}],\"genre\":\"<optional>\",\"skills\":[\"<skill tag>\"],\
         \"features\":{...},\"depends_on\":[\"<another child key>\"]}]` array (at most 8). Do not write \
         `assignee`, `tier` or `model` (celeris picks the owner). A WorkUnit that needs a child's result \
         waits for it with `\"depends_on\":[\"child:<key>\"]`. celeris creates the children through the same \
         checks as delegation when it adopts the plan; a child in another department is first confirmed \
         with the secretary. Do not split a WorkUnit that is merely too large into children — make it \
         smaller WorkUnits instead. Otherwise leave `\"children\"` out.\n\n",
    );
    out.push_str(
        "Example: `{\"schema\":\"celeris.execution-plan/2\",\"rationale\":\"...\",\"phases\":[\
         {\"key\":\"build\",\"kind\":\"implement\",\"title\":\"core pieces\"},\
         {\"key\":\"verify\",\"kind\":\"test\",\"title\":\"end-to-end\"}],\"work_units\":[\
         {\"key\":\"api\",\"phase\":\"build\",...},{\"key\":\"store\",\"phase\":\"build\",...},\
         {\"key\":\"e2e\",\"phase\":\"verify\",\"depends_on\":[\"api\",\"store\"],...}]}`.\n\n",
    );
    out
}

/// ADR-0074 D5.1（Phase F1）: WU ごとの `features`（`TaskFeatureHints` の軸）の説明と例。planner
/// プロンプトの JSON 例の直後に足す（E6-1: 例に無く、planner は書かなかった。書かせても
/// `.ok()` で黙って捨てていた。今は `execution_plan::validate` が読めない `features` を拒否する）。
fn work_unit_features_section() -> String {
    let mut out = String::from(
        "### `features` (per-WorkUnit routing hints, ADR-0074 D5.1)\n\
         Write at least these five axes for every WorkUnit so celeris can route it to the cheapest \
         lane that still fits (a WorkUnit with no `features` just inherits this task's own lane, which \
         wastes budget on mechanical WorkUnits). Each axis is `\"low\"`, `\"medium\"`, or `\"high\"`. Do \
         not write `lane`, `tier`, `assignee`, or `model` here — celeris decides the lane from these \
         axes plus whether `checks` is set.\n\n",
    );
    out.push_str(
        "- `judgment`: how much open-ended judgment or design taste this WorkUnit needs. `low` = \
         mechanical (rename, update a price table, apply a known fix); `high` = real design or \
         investigation conclusions.\n\
         - `ambiguity`: how underspecified the goal is. `low` = the objective and `done_when`/`checks` \
         fully pin down the outcome; `high` = the WorkUnit has to make judgment calls about what \
         \"done\" even means.\n\
         - `verifiability`: whether the result can be checked deterministically. `high` = an executable \
         `checks` command decides pass/fail; `low` = only a human can tell.\n\
         - `reversibility`: how easy a mistake is to undo. `high` = changes live in this task's own \
         worktree/branch; `low` = touches production, other people's data, or something sent externally.\n\
         - `consequence`: how bad a mistake would be. `low` for routine internal changes; `high` for \
         anything security-, billing-, or data-loss-adjacent.\n\n",
    );
    out.push_str(
        "Example: updating a static price table with a `cargo test` check that confirms the new \
         values is `{\"judgment\":\"low\",\"ambiguity\":\"low\",\"verifiability\":\"high\",\
         \"reversibility\":\"high\",\"consequence\":\"low\"}` (mechanical, checkable, safe to redo — \
         routes to the cheap lane). Designing a new routing policy with no executable check is \
         `{\"judgment\":\"high\",\"ambiguity\":\"high\",\"verifiability\":\"low\",\
         \"reversibility\":\"high\",\"consequence\":\"medium\"}` (routes to the frontier lane, capped by \
         this task's own lane).\n\n",
    );
    out
}

/// ADR-0072 D17（Phase E4b 項目1）: replan run のプロンプトに足す節。今の計画の版・WU ごとの状態・
/// 起こした理由・保持すべき `done` の WU の key を出す。`v2` の出力は `done` の WU を変えてはいけない、
/// と明示する（D14 の検証がこれを拒否することも書く）。
fn replan_context_section(planner: &crate::protocol::ExecutionPlannerContext) -> String {
    let mut out = String::from("## You are REPLANNING an existing execution plan\n");
    out.push_str(
        "This is not the first plan for this task: a previous plan already ran, and something \
         about it needs to change.\n\n",
    );
    let delta_schema = task_core::execution_plan::EXECUTION_PLAN_DELTA_SCHEMA;
    let base_version = planner.current_plan_version.unwrap_or(1);
    out.push_str(&format!(
        "Prefer writing a DIFF instead of the full plan (ADR-0074 D5.3): write \
         `{{\"schema\":\"{delta_schema}\",\"base_version\":{base_version},\"rationale\":\"...\",\
         \"add\":[<new WorkUnit specs, same shape as above>],\
         \"modify\":[{{\"key\":\"<existing key>\", ...only the fields you are changing...}}],\
         \"remove\":[\"<key>\", ...]}}` to `{{artifacts}}/execution-plan.json`. Do NOT restate \
         WorkUnits you are not changing — they carry over automatically, including every WorkUnit \
         that is already done (you cannot change a done WorkUnit's spec except its `checks`; see below). If a \
         diff genuinely cannot express what you need, you may instead write the full \
         `\"schema\":\"{}\"` plan shape shown above (still subject to the done-WorkUnit rule).\n\n",
        task_core::EXECUTION_PLAN_SCHEMA
    ));
    if let Some(version) = planner.current_plan_version {
        out.push_str(&format!("Current (superseded) plan version: v{version}.\n"));
    }
    out.push_str(&format!(
        "Why this replan was triggered: {}\n\n",
        if planner.replan_reason.is_empty() {
            "(not recorded)"
        } else {
            planner.replan_reason.as_str()
        }
    ));
    if !planner.work_unit_summaries.is_empty() {
        out.push_str("### WorkUnits in the current plan\n");
        for line in &planner.work_unit_summaries {
            out.push_str(&format!("- {line}\n"));
        }
        out.push('\n');
    }
    if planner.preserve_done_keys.is_empty() {
        out.push_str(
            "No WorkUnit in the current plan is done yet, so you may replace all of them.\n\n",
        );
    } else {
        out.push_str(&format!(
            "IMPORTANT: these WorkUnit keys are already done and MUST appear unchanged (same \
             `key`, same spec — objective, depends_on, done_when, context, harness, budget, \
             outputs) in your new plan; do not edit, rename, remove, or reorder them. A plan that \
             changes a done WorkUnit will be rejected: {}. The one exception is `checks`: you may \
             replace a done WorkUnit's `checks` (in a diff: `\"modify\":[{{\"key\":\"<done key>\",\
             \"checks\":[...]}}]`) when a check cannot hold after the phase integration (e.g. one \
             that assumes a merge commit). The done WorkUnit is not re-run; its checks are re-run \
             at the phase integration.\n\n",
            planner.preserve_done_keys.join(", ")
        ));
    }
    out
}

/// Phase 38（ADR-0028 追記）: レビュー対象のタスクがハーネスで動く分野なら、レビュアーにも成果物の規約を
/// 渡す（`context.subject_genre`。ディスパッチャが決定的に入れる）。ハーネスでない分野・分野が無いタスクの
/// レビューでは何も出さない（従来の文面と 1 バイトも変わらない）。
pub(super) fn harness_artifacts_section_for_review(context: &RunContext) -> String {
    let Some(genre) = context.subject_genre.as_ref().filter(|g| g.is_harness()) else {
        return String::new();
    };
    let names = genre.output_artifacts_named();
    let Some(&(answer, _)) = names.first() else {
        return String::new();
    };
    let mut out = String::from("## この担当の成果物（名前は固定。判定はこの前提で行う）\n");
    out.push_str(&harness_artifact_lines(genre));
    out.push_str(&format!(
        "上のどれかに「答え」があり、それ以外は道具の記録である（例: `papers.json` は検索したコーパスで\n\
         あって答えではない。答えは `{answer}`）。テーマ候補・考察・結論といった**内容は `{answer}` の中で\n\
         判定せよ**。受け入れ条件が上に無いファイル名を求めていても、その担当にはそれを書く手段が無いので、\n\
         ファイル名の不一致だけを理由に不合格にはせず、要求された**内容**が `{answer}` にあるかで判定せよ。\n\n"
    ));
    out
}

/// `Review` kind 用プロンプト（DESIGN §5.7, ADR-0007 D5/D7）。対象タスクの成果物を読み取り専用で
/// 検証し `artifacts/review.json` に判定を書かせる。
fn build_review_prompt(task: &Task, context: &RunContext, run_id: &str, artifacts: &str) -> String {
    let mut out = prompt_header(task, context, run_id, artifacts);
    out.push_str(
        "## Instructions\n\
         You are a reviewer independently verifying another worker's output. You must not modify any \
         files in this directory — this is a read-only inspection of the working directory. If in \
         doubt about whether a criterion is actually satisfied, set `pass` to false and explain why: a \
         false pass is worse than a false fail (completion is decided by review, not by the worker's \
         own claim).\n\n",
    );
    out.push_str("## Acceptance criteria of the task under review\n");
    for (i, c) in task.acceptance.iter().enumerate() {
        out.push_str(&format!("{}. {}\n", i, c.text));
    }
    out.push('\n');
    out.push_str(&harness_artifacts_section_for_review(context));
    match &context.review {
        Some(review) => {
            if !review.decisions.is_empty() || !review.answers.is_empty() {
                out.push_str("## Human decisions and answers (authoritative)\n\
                    Human decisions and answers take precedence when interpreting the acceptance criteria. \
                    If a human decision changed the scope, location, or method, judge against that decided scope. \
                    Do not fail a criterion solely because work added or changed under a human decision, including \
                    expanded scope, differs from the criterion's strict wording. Changes beyond the human decision \
                    must still be judged against the criterion.\n");
                for decision in &review.decisions {
                    out.push_str(&format!(
                        "- decision {} (task {}): {} => {} ({})",
                        decision.key,
                        decision.task_id,
                        decision.question,
                        decision.option,
                        decision.option_label
                    ));
                    if let Some(note) = &decision.note {
                        out.push_str(&format!("; note: {note}"));
                    }
                    out.push('\n');
                }
                for answer in &review.answers {
                    out.push_str(&format!(
                        "- Q: {}\n  A: {}\n",
                        answer.question, answer.answer
                    ));
                }
                out.push('\n');
            }
            if !review.checks.is_empty() {
                out.push_str("## Deterministic checks already executed by celeris (authoritative)\n\
                    These results were produced by celeris in this workspace, not self-reported by the worker; \
                    they take precedence over the worker's summary and evidence. To fail a criterion for a reason \
                    that contradicts a passing check (for example, claiming the same command exited 101), run \
                    the command yourself now and include your command and relevant output in the verdict reason. \
                    Do not rely only on the worker's record. If you cannot rerun it because of sandbox limits or \
                    cost, treat the passing check as authoritative.\n");
                for check in &review.checks {
                    let criterion = check.criterion.map_or_else(
                        || "implicit criterion".to_string(),
                        |index| format!("criterion {index}"),
                    );
                    out.push_str(&format!(
                        "- {criterion}, {}: pass={}",
                        check.kind, check.pass
                    ));
                    if let Some(cmd) = &check.cmd {
                        out.push_str(&format!(" command `{cmd}`"));
                    }
                    out.push_str(&format!(" reason: {}\n", check.reason));
                }
                out.push('\n');
            }
            out.push_str(&format!(
                "## Worker's self-reported summary (not to be trusted blindly)\n{}\n\n",
                review.summary
            ));
            out.push_str("## Criteria you must judge in this run\n");
            for idx in &review.criteria {
                out.push_str(&format!("- criterion {idx}\n"));
            }
            out.push('\n');
            out.push_str("## Evidence self-reported by the worker (not to be trusted blindly)\n");
            if review.evidence.is_empty() {
                out.push_str("(none reported)\n");
            }
            for e in &review.evidence {
                // ADR-0012 D3: command / exit / stdout_tail は任意。
                let command = e
                    .command
                    .as_deref()
                    .map(|c| format!(" command `{c}`"))
                    .unwrap_or_default();
                let exit = e.exit.map(|x| format!(" exit={x}")).unwrap_or_default();
                let tail = e
                    .stdout_tail
                    .as_deref()
                    .map(|t| format!(" stdout_tail={t:?}"))
                    .unwrap_or_default();
                out.push_str(&format!(
                    "- criterion {}:{command}{exit}{tail}\n",
                    e.criterion
                ));
            }
            out.push('\n');
        }
        None => {
            out.push_str("## Review context\nno review context\n\n");
        }
    }
    out.push_str("## Input artifacts produced by the run under review\n");
    if context.inputs.is_empty() {
        out.push_str("(none)\n");
    }
    for a in &context.inputs {
        out.push_str(&format!(
            "- {} at `{}` (sha256={})\n",
            a.name, a.path, a.sha256
        ));
    }
    out.push('\n');
    out.push_str(&format!(
        "## Result\n\
         Write your verdicts to `{artifacts}/review.json` (create the `{artifacts}/` directory if it does \
         not exist yet) as a single JSON object of exactly this shape: \
         `{{\"verdicts\":[{{\"criterion\":<index>,\"pass\":<bool>,\"reason\":\"...\"}}]}}`. You must write \
         exactly one verdict for each criterion listed under \"Criteria you must judge in this run\" \
         above.\n\n"
    ));
    out.push_str(&result_json_instructions(artifacts));
    out
}

/// ADR-0079 R7-2: planner の「check の書き方」。本番（2026-09-29/30）で unit の成果ではなく check そのものが
/// 誤って落ちた形を 1 規則 1 文で並べる（/1・/2・/3 の planner に共通。上限の節の直後）。
pub const PLANNER_CHECK_GUIDANCE: &str = "### check の書き方 (how to write `checks` and command acceptance)\n\
     - A \"no out-of-scope diff\" check must exclude the paths the unit is allowed to write as records: \
     `docs/PROGRESS.md`, `docs/progress/`, and every path this plan itself says the unit may write.\n\
     - Do not pass extra positional arguments to `pnpm -C <dir> test` or `cargo test` unless the package \
     script accepts them (`pnpm -C web test scripts/ e2e/support/` handed directories to `node --test` and \
     failed).\n\
     - Pin the package manager: write `corepack pnpm@<version from package.json packageManager> -C <dir> ...` \
     instead of bare `pnpm` (the host pnpm may differ and fail with ERR_PNPM_BAD_PM_VERSION).\n\
     - Compare against `$(git merge-base HEAD main)` (e.g. `git diff --quiet $(git merge-base HEAD main) -- \
     <paths>`) or the unit's recorded base, never a hard-coded main sha, because main moves during the task.\n\
     - A negated grep (`! grep ...`) must not match text the unit itself writes (its own ADR, notes or \
     comments explaining the rule); this self-reference has failed real checks.\n\
     - A check runs where the unit's worker starts: its own unit's worktree, or the task's directory \
     (where `artifacts/` is) when the task has no git worktree. It may only use files that exist there: a check \
     that runs a script another unit creates belongs to a unit that `depends_on` the creating unit.\n\
     - When a check runs a script the unit itself creates, write the exact invocation (the arguments the check \
     passes) in the unit's objective so the unit writes the script to accept that form (a check passed a URL to \
     a script that took `[LAN_IP] [PORT]` and failed on every run although the work was done).\n\
     - Checks run with `/bin/sh` (dash), so do not use bash-only syntax such as `${s:0:12}`, `[[ ]]`, or arrays.\n\
     - An out-of-scope diff check is compared with sibling units during stage integration, so exclude every \
     unit's allowed paths in that stage, not only this unit's paths.\n\
     - Keep each leaf small enough for one run, and do not pack implementation work into a recording or close-out leaf.\n\
     - For a unit or task changing only `web/` or `docs/`, replace mandatory `cargo test --workspace` with a \
     check that `crates/` has no diff (for example `git diff --quiet $(git merge-base HEAD main) -- crates/`); \
     leave Cargo checks to the daemon's workspace check.\n\
     - Include the planned ADR and recording locations from the start in acceptance criteria and diff-check path scopes.\n\
     - Do not run CPU-burning load scripts (busy loops, stress-ng, parallel cargo load) in checks or \
     acceptance; reproduce timing bugs deterministically (paused or injected clock, event waits, \
     SIGSTOP/SIGCONT, test-only delay hooks; see docs/testing.md).\n\n";
/// ADR-0095 付記 D-d: 本番 host の操作は人が実行する手順として書く（planner 指示。worker 前置きの
/// `production_host_note` と対になる — 計画段階でも最初から試みさせない）。
pub const PRODUCTION_HOST_PLANNER_GUIDANCE: &str = "### 本番 host の操作 (production host changes)\n\
     本番 host の操作は人が実行する手順として書く: `systemctl --user` / `systemd-run` / `~/.config/systemd` / \
     `~/.local/celeris/releases` / `/local` / `/local/celeris/state/releases` / `~/.config/celeris` を変更する \
     WorkUnit を計画しない。本番の daemon の \
     再起動・差し替えが要るときは、人が実行する手順（コマンドと確認方法）を成果物に書く WorkUnit を置き、\
     実行そのものは `decisions` か `needs_decisions` の人の check に回す（ADR-0095 付記 D-d）。\n\n";
