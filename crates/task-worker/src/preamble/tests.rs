use super::*;
use crate::protocol::{ConversationTurn, GenreContext, MemoryContext, NodeContext, RoleContext};
use task_core::Status;

fn full_context() -> RunContext {
    RunContext {
        node: Some(NodeContext {
            id: "research-survey".into(),
            name: "関連研究調査課".into(),
            brief: "関連研究を洗い、先行研究との差分を言語化する。".into(),
        }),
        standing_rules: vec!["pegasus のジョブは常に 1 ノードで始めてよい".into()],
        memory: Some(MemoryContext {
            notes: "- 2026-09-10: pegasus は pjsub で投げる".into(),
            project: "- 2026-09-16: Pluvio は非同期ランタイム基盤".into(),
        }),
        conversation: vec![
            ConversationTurn {
                role: MessageRole::User,
                text: "先週の続きを\nお願い".into(),
            },
            ConversationTurn {
                role: MessageRole::Node,
                text: "承知しました".into(),
            },
        ],
        role: Some(RoleContext {
            id: "literature-reader".into(),
            instructions: "あなたは精読担当。".into(),
        }),
        ..RunContext::default()
    }
}

/// ADR-0033 D4 / Phase 24: 並びは 役職と brief → 永続の認可 → 記憶 → 直近のやり取り → 役割の指示文。
#[test]
fn the_sections_come_in_the_order_the_adr_asks_for() {
    let out = render(&full_context(), "artifacts");
    let at = |needle: &str| {
        out.find(needle)
            .unwrap_or_else(|| panic!("missing {needle:?} in:\n{out}"))
    };
    assert!(at("## あなた: 関連研究調査課 (research-survey)") < at("## 永続の認可"));
    assert!(at("## 永続の認可") < at("## 覚えていること"));
    assert!(at("## 覚えていること") < at("## 直近のやり取り"));
    assert!(at("## 直近のやり取り") < at("## Role: literature-reader"));
    assert!(at("## Role: literature-reader") < at("## 覚えておくこと"));
    // 中身
    assert!(out.contains("関連研究を洗い"));
    assert!(out.contains("- pegasus のジョブは常に 1 ノードで始めてよい"));
    assert!(out.contains("### 案件をまたぐ記憶"));
    assert!(out.contains("### この案件について"));
    assert!(out.contains("- 人: 先週の続きを お願い"), "{out}");
    assert!(out.contains("- あなた: 承知しました"));
    assert!(out.contains("memory.notes"));
}

/// Phase 30（ADR-0033 D4 追記）: 対話は常に対話用分野で走るが、担当ノード自身の仕事の分野が
/// あれば「仕事で使う道具」を役職と brief の直後に 1 行足す（実機の事故の再発防止: 関連研究調査課
/// ＝検索ハーネスに話しかけても、検索ハーネスの run にはしない。その人に自分の分野を知らせるだけ）。
#[test]
fn a_work_genre_is_shown_right_after_the_brief_when_present() {
    let context = RunContext {
        work_genre: Some(GenreContext {
            id: "web-research".into(),
            description: "web 検索で先行研究を洗う".into(),
            capabilities: vec!["web 検索".into(), "証拠の収集".into()],
            ..GenreContext::default()
        }),
        ..full_context()
    };
    let out = render(&context, "artifacts");
    let at = |needle: &str| {
        out.find(needle)
            .unwrap_or_else(|| panic!("missing {needle:?} in:\n{out}"))
    };
    assert!(at("## あなた: 関連研究調査課 (research-survey)") < at("あなたの仕事で使う道具"));
    assert!(at("あなたの仕事で使う道具") < at("## 永続の認可"));
    assert!(
        out.contains("あなたの仕事で使う道具（分野）: web 検索で先行研究を洗う（できること: web 検索、証拠の収集）"),
        "{out}"
    );

    // 担当が自分の仕事の分野を持たない（対話用分野のみで走る）ときは何も足さない。
    let without = RunContext {
        work_genre: None,
        ..full_context()
    };
    let out = render(&without, "artifacts");
    assert!(!out.contains("あなたの仕事で使う道具"), "{out}");

    // `context.node` が無ければ、`work_genre` があっても出さない（役職の節そのものが無いため）。
    let no_node = RunContext {
        node: None,
        work_genre: Some(GenreContext {
            id: "coding".into(),
            description: "d".into(),
            ..GenreContext::default()
        }),
        ..RunContext::default()
    };
    assert!(!render(&no_node, "artifacts").contains("あなたの仕事で使う道具"));
}

/// 空の `RunContext` では前置きは「成果物の置き場所」の節だけ（ADR-0067 D1。Phase 23〜110 の出力は
/// 空文字だったが、この節だけは context に関わらず常に出る）。
#[test]
fn an_empty_context_renders_only_the_deliverables_placement_note() {
    assert_eq!(
        render(&RunContext::default(), "artifacts"),
        deliverables_placement_note()
    );
}

/// ADR-0044 D2（Phase 53）: コメントの節は**前置きの先頭**。人の割り込みがいちばん先に来て、
/// その後にコメントの糸（古い順）が並ぶ。`comments_enabled` の run には書き方の指示も付く。
/// コメントが 1 件も無い run の前置きは Phase 52 までと 1 バイトも変わらない。
#[test]
fn comments_come_first_and_the_interruption_is_the_very_first_line() {
    let context = RunContext {
        interrupt: Some("方針を変えたい。まず設計を書いて".into()),
        comments: vec![
            crate::protocol::CommentContext {
                author_kind: task_core::CommentAuthorKind::Node,
                author: Some("impl".into()),
                body: "ビルドは通った\n（続き）".into(),
                at: "2026-09-19T01:00:00Z".into(),
            },
            crate::protocol::CommentContext {
                author_kind: task_core::CommentAuthorKind::Human,
                author: None,
                body: "方針を変えたい。まず設計を書いて".into(),
                at: "2026-09-19T02:00:00Z".into(),
            },
        ],
        comments_enabled: true,
        ..full_context()
    };
    let out = render(&context, "artifacts");
    assert!(
        out.starts_with("## コメント (comments on this task)\n"),
        "{out}"
    );
    let interrupt_at = out.find("**人からの割り込み**").expect("interrupt line");
    let thread_at = out
        .find("- [2026-09-19T01:00:00Z] impl:")
        .expect("thread line");
    assert!(interrupt_at < thread_at, "割り込みが糸より先: {out}");
    // 複数行の本文は 1 行に畳む（他の節と同じ規則）。
    assert!(
        out.contains("- [2026-09-19T01:00:00Z] impl: ビルドは通った （続き）"),
        "{out}"
    );
    assert!(
        out.contains("- [2026-09-19T02:00:00Z] 人: 方針を変えたい。まず設計を書いて"),
        "{out}"
    );
    // 役職の節はコメントの後ろ。
    assert!(
        out.find("## あなた:").expect("node section") > interrupt_at,
        "{out}"
    );
    // 書き方の指示（ADR-0044 D2）。
    assert!(
        out.contains("短い進捗や判断の記録はコメントに書け"),
        "{out}"
    );
    assert!(out.contains(r#"{"type":"comment","body":"…"}"#), "{out}");

    // コメントが無ければ節ごと出ない（`comments_enabled` だけなら指示だけ）。
    let quiet = RunContext {
        comments_enabled: true,
        ..full_context()
    };
    let quiet_out = render(&quiet, "artifacts");
    assert!(
        !quiet_out.contains("## コメント (comments on this task)"),
        "{quiet_out}"
    );
    assert!(
        quiet_out.contains("短い進捗や判断の記録はコメントに書け"),
        "{quiet_out}"
    );
    let silent = render(&full_context(), "artifacts");
    assert!(!silent.contains("コメント"), "{silent}");
}

/// 役割だけがあるときは、Phase 23 の `prompt_header` と同じ `## Role:` 節だけを出す。
#[test]
fn a_role_only_context_renders_exactly_the_old_role_section() {
    let with_instructions = RunContext {
        role: Some(RoleContext {
            id: "lead".into(),
            instructions: "You coordinate.".into(),
        }),
        ..RunContext::default()
    };
    assert_eq!(
        render(&with_instructions, "artifacts"),
        format!(
            "## Role: lead\nYou coordinate.\n\n{}",
            deliverables_placement_note()
        )
    );
    let bare = RunContext {
        role: Some(RoleContext {
            id: "lead".into(),
            instructions: String::new(),
        }),
        ..RunContext::default()
    };
    assert_eq!(
        render(&bare, "artifacts"),
        format!("## Role: lead\n\n{}", deliverables_placement_note())
    );
}

/// Phase 28（ADR-0033 D4 追記）: 対話 run にだけ、末尾に「返事だけをする」指示が付く。
/// 秘書宛ては (a)〜(d)、それ以外は「聞かれたことに答える」。通常タスクの前置きは 1 バイトも変わらない。
#[test]
fn conversation_runs_get_a_reply_only_instruction_appended_at_the_end() {
    let ordinary = full_context();
    let ordinary_out = render(&ordinary, "artifacts");
    assert!(!ordinary_out.contains("これは対話です"), "{ordinary_out}");

    let secretary = RunContext {
        conversation_addressee: Some(ConversationAddressee::Secretary),
        ..ordinary.clone()
    };
    let out = render(&secretary, "artifacts");
    assert!(
        out.starts_with(&ordinary_out),
        "対話の指示は末尾に足すだけ: {out}"
    );
    assert!(!out.contains("この返事では作業を始めないでください"));
    assert!(out.contains("実装・検証・成果の引き渡し"));
    assert!(out.contains("通常の修正と検証は既に依頼された作業"));
    assert!(out.contains("既存の仕事と結果、承認済みの範囲"));

    let other = RunContext {
        conversation_addressee: Some(ConversationAddressee::Other),
        ..RunContext::default()
    };
    let out = render(&other, "artifacts");
    assert!(out.contains("聞かれたことに答え"));
    assert!(!out.contains("(a) 理解の確認"), "{out}");
    assert!(
        out.contains("自分の直近の仕事とその結果は上に書いてある"),
        "{out}"
    );

    // 対話でない run（既定値の `None`）では何も足さない。
    assert_eq!(
        render(&RunContext::default(), "artifacts"),
        deliverables_placement_note()
    );
}

/// Phase 98（ADR-0018、実機障害 2026-09-22）: CoS の対話にだけ「クラスタ作業は自分でやらず
/// `create_task` で組織に流す」規則が付き、「ssh 禁止・read-only」を理由に断らないよう明示する。
/// CoS 以外の対話・通常の run には出ない。
#[test]
fn secretary_instructions_tell_cos_to_route_cluster_work_via_create_task() {
    let secretary = RunContext {
        conversation_addressee: Some(ConversationAddressee::Secretary),
        ..RunContext::default()
    };
    let out = render(&secretary, "artifacts");
    assert!(out.contains("自分で実行しないでください"), "{out}");
    assert!(
        out.contains("『ssh が禁止されている』『read-only』を理由に断らないで"),
        "{out}"
    );
    assert!(out.contains("cluster:<id>"), "{out}");
    assert!(
        out.contains(
            r#""workspace": {"kind":"remote","cluster":"<id>","path":"<作業ディレクトリ>","mode":"shared"}"#
        ),
        "{out}"
    );
    // ADR-0059 D6: `~` を既定にしない・未登録なら `path` を省略して人に登録を頼む規則が入る。
    assert!(out.contains("`~` は使わないでください"), "{out}");
    assert!(out.contains("path` を省略し"), "{out}");
    // ADR-0062 B（Phase 107）: 持たないノードに流すと検証で落ちる、調査・執筆系は remote にしない。
    assert!(out.contains("検証で落ちます"), "{out}");
    assert!(out.contains("web-research"), "{out}");
    assert!(out.contains("celeris が local に落とします"), "{out}");

    let other = RunContext {
        conversation_addressee: Some(ConversationAddressee::Other),
        ..RunContext::default()
    };
    let out = render(&other, "artifacts");
    assert!(!out.contains("cluster:<id>"), "{out}");

    assert_eq!(
        render(&RunContext::default(), "artifacts"),
        deliverables_placement_note()
    );
}

/// Phase 98（ADR-0046 D8）: CoS 宛ての「組織」一覧に、各ノードの tools（`cluster:<id>` を含む）が
/// 1 語ずつ添う。CoS がどのノードにクラスタ作業を流せばよいかを前置きから判断できるようにする。
#[test]
fn organization_section_shows_each_nodes_tools() {
    let secretary = RunContext {
        conversation_addressee: Some(ConversationAddressee::Secretary),
        organization: vec![
            crate::protocol::OrgNodeContext {
                id: "cluster-hpc".into(),
                name: "Cluster & HPC Operations".into(),
                kind: task_core::OrgKind::Section,
                parent_id: Some("operations".into()),
                brief: String::new(),
                genre: None,
                skills: vec!["slurm".into()],
                harnesses: vec!["coding".into()],
                tools: vec![
                    "cluster:pegasus".into(),
                    "cluster:sirius".into(),
                    "cluster:fern03".into(),
                ],
            },
            crate::protocol::OrgNodeContext {
                id: "cos".into(),
                name: "Chief of Staff".into(),
                kind: task_core::OrgKind::Secretary,
                parent_id: None,
                brief: String::new(),
                genre: None,
                skills: Vec::new(),
                harnesses: vec!["conversation".into()],
                tools: Vec::new(),
            },
        ],
        ..RunContext::default()
    };
    let out = render(&secretary, "artifacts");
    assert!(out.contains("## 組織"), "{out}");
    assert!(
        out.contains(
            "- `cluster-hpc` Cluster & HPC Operations — skills: slurm / harnesses: coding / 道具: cluster:pegasus, cluster:sirius, cluster:fern03"
        ),
        "{out}"
    );
    // 道具が無いノードは「道具:」を出さない（従来どおり）。
    assert!(
        out.contains("- `cos` Chief of Staff / harnesses: conversation\n"),
        "{out}"
    );
}

/// ADR-0059 D6（Phase 99）: CoS 宛てに「クラスタ」の節が付き、実効 work_dir の有無で文面が変わる
/// （登録済みならそのパス、未登録なら `path` を省略して人に登録を頼む案内）。CoS 以外・
/// `context.clusters` が空の run には出ない。
#[test]
fn clusters_section_shows_the_effective_work_dir_or_that_it_is_unregistered() {
    let secretary = RunContext {
        conversation_addressee: Some(ConversationAddressee::Secretary),
        clusters: vec![
            crate::protocol::ClusterContext {
                id: "pegasus".into(),
                connected: true,
                work_dir: Some("/work/NBB/rmaeda".into()),
            },
            crate::protocol::ClusterContext {
                id: "sirius".into(),
                connected: false,
                work_dir: None,
            },
        ],
        ..RunContext::default()
    };
    let out = render(&secretary, "artifacts");
    assert!(out.contains("## クラスタ"), "{out}");
    assert!(
        out.contains("- `pegasus`（接続中） 作業ディレクトリ: `/work/NBB/rmaeda`"),
        "{out}"
    );
    assert!(
        out.contains("- `sirius`（未接続） 作業ディレクトリ: 未登録"),
        "{out}"
    );

    let other = RunContext {
        conversation_addressee: Some(ConversationAddressee::Other),
        clusters: vec![crate::protocol::ClusterContext {
            id: "pegasus".into(),
            connected: true,
            work_dir: Some("/work/NBB/rmaeda".into()),
        }],
        ..RunContext::default()
    };
    assert!(!render(&other, "artifacts").contains("## クラスタ"));

    // 空なら Phase 98 までと 1 バイトも変わらない（節ごと出ない）。
    let empty = RunContext {
        conversation_addressee: Some(ConversationAddressee::Secretary),
        ..RunContext::default()
    };
    assert!(!render(&empty, "artifacts").contains("## クラスタ"));

    // Phase 99b（ADR-0059 追記）: 継続中の run（`session_diff` あり = 他の全量節は落ちる）でも
    // `context.clusters` が渡っていれば「クラスタ」節は描かれる（`render` は単一の経路で、差分専用
    // の描画経路は無い。node/organization は継続中は `None`/空になる想定を模して確認する）。
    let continuing = RunContext {
        conversation_addressee: Some(ConversationAddressee::Secretary),
        session_diff: vec!["新しい人の発言: 続き".into()],
        clusters: vec![crate::protocol::ClusterContext {
            id: "pegasus".into(),
            connected: true,
            work_dir: Some("/work/NBB/rmaeda".into()),
        }],
        ..RunContext::default()
    };
    let out = render(&continuing, "artifacts");
    assert!(out.contains("## クラスタ"), "{out}");
    assert!(
        out.contains("- `pegasus`（接続中） 作業ディレクトリ: `/work/NBB/rmaeda`"),
        "{out}"
    );
    assert!(
        !out.contains("## 組織"),
        "継続中は組織の一覧を流し直さない: {out}"
    );
}

/// ADR-0048 D3（Phase 60b）: CoS の対話にだけ「進行中の案件」の節と `actions` の説明が付く。
/// CoS 以外の対話・通常の run には出ない。
#[test]
fn cos_conversations_show_active_projects_and_the_actions_instructions() {
    let secretary = RunContext {
        conversation_addressee: Some(ConversationAddressee::Secretary),
        active_projects: vec![crate::protocol::ActiveProjectContext {
            repos: vec!["agent-platform".into()],
            id: "01PROJECT".into(),
            title: "Pluvio".into(),
            status: "active".into(),
            milestones: vec![crate::protocol::ActiveMilestoneContext {
                id: "01MILESTONE".into(),
                title: "隣接領域の調査".into(),
                status: "in_progress".into(),
            }],
        }],
        ..RunContext::default()
    };
    let out = render(&secretary, "artifacts");
    assert!(out.contains("## 進行中の案件"), "{out}");
    assert!(out.contains("01PROJECT"), "{out}");
    assert!(out.contains("Pluvio"), "{out}");
    assert!(
        out.contains("登録済み repos: `agent-platform`（使用時の project: `01PROJECT`）"),
        "{out}"
    );
    // ADR-0079 D12 / D13（Phase R5a）: 凍結した途中目標は CoS に見せない。
    assert!(!out.contains("隣接領域の調査"), "{out}");
    assert!(!out.contains("01MILESTONE"), "{out}");
    assert!(out.contains("actions"), "{out}");
    assert!(out.contains("create_task"), "{out}");
    assert!(out.contains("propose_project"), "{out}");
    assert!(!out.contains("add_milestone"), "{out}");
    assert!(out.contains("ask_human"), "{out}");
    assert!(out.contains("mode: \"production\""), "{out}");
    // ADR-0069 D1（Phase 114）: CoS は担当とモデルを選ばない。
    assert!(
        out.contains("担当（`assignee`）とモデル（`tier`）は"),
        "{out}"
    );
    assert!(!out.contains("\"assignee\": null"), "{out}");
    // ADR-0063 D3（Phase 109）: 調査系の受け入れ条件は「未確認」の一文（または `partial_ok`）で
    // 1 件の欠落による全体不合格を避ける、という案内が付く。
    assert!(out.contains("未確認"), "{out}");
    assert!(out.contains("literature"), "{out}");
    assert!(out.contains("web-research"), "{out}");

    // CoS 以外の対話には「進行中の案件」も `actions` の説明も出ない。
    let other = RunContext {
        conversation_addressee: Some(ConversationAddressee::Other),
        active_projects: secretary.active_projects.clone(),
        ..RunContext::default()
    };
    let out = render(&other, "artifacts");
    assert!(!out.contains("## 進行中の案件"), "{out}");
    assert!(!out.contains("create_task"), "{out}");

    // 案件が無ければ節ごと出ない（既存の出力を変えない）。
    let empty = RunContext {
        conversation_addressee: Some(ConversationAddressee::Secretary),
        ..RunContext::default()
    };
    assert!(!render(&empty, "artifacts").contains("## 進行中の案件"));
}

/// ADR-0079 D12（Phase R5a）: CoS の前置きの指針。1 依頼 = 1 `create_task`、範囲を狭めない（Phase 1〜4 は全部
/// 書く）、人が名指しした段階だけ `stages_hint` に写す、独立な依頼は 2 つ・依存するなら 1 つ、`add_milestone` と
/// `execution: compound` のヒントは無い、`pause_after` は人が頼んだときだけ。
#[test]
fn cos_preamble_carries_the_adr_0079_guidance() {
    let secretary = RunContext {
        conversation_addressee: Some(ConversationAddressee::Secretary),
        ..RunContext::default()
    };
    let out = render(&secretary, "artifacts");
    assert!(out.contains("1 つの依頼は 1 つの `create_task`"), "{out}");
    assert!(out.contains("範囲を狭めないでください"), "{out}");
    assert!(out.contains("Phase 1〜4 のすべてを書きます"), "{out}");
    assert!(
        out.contains("\"stages_hint\": [{\"title\": \"Phase 1\", \"scope\": \"…\"}]"),
        "{out}"
    );
    assert!(out.contains("人が段階を名指ししたときだけ"), "{out}");
    assert!(out.contains("`create_task` を 2 つ"), "{out}");
    assert!(out.contains("1 つの `create_task` にまとめます"), "{out}");
    assert!(out.contains("人が段階ごとの確認を頼んだときだけ"), "{out}");
    assert!(
        out.contains("途中目標は root task の段階で表します"),
        "{out}"
    );
    // 廃止したもの。
    assert!(!out.contains("add_milestone"), "{out}");
    assert!(!out.contains("\"execution\": \"compound\""), "{out}");
    assert!(!out.contains("1 時間以内"), "{out}");
    assert!(!out.contains("\"milestone\": \"<途中目標の id"), "{out}");
    assert!(!out.contains("対象ごとにタスクを分けてください"), "{out}");
    assert!(!out.contains("方針や途中目標を毎回再承認"), "{out}");
}

/// 記憶が空（ファイルが無い）なら記憶の節は出ないが、書き方の指示は出る（次から覚えられるように）。
#[test]
fn empty_memory_shows_no_memory_section_but_still_explains_how_to_write_it() {
    let context = RunContext {
        memory: Some(MemoryContext::default()),
        ..RunContext::default()
    };
    let out = render(&context, "artifacts");
    assert!(!out.contains("## 覚えていること"), "{out}");
    assert!(out.contains("## 覚えておくこと"), "{out}");
    // `[memory]` を設定していない run には何も出ない。
    assert!(!render(&RunContext::default(), "artifacts").contains("覚えておくこと"));
}

/// Phase 33（実機の事故 — 担当が自分の直近の失敗を知らずに聞き返した — の再発防止）:
/// `context.recent_work` は記憶の直後、直近のやり取りより前に 1 行ずつ出す。
#[test]
fn recent_work_is_shown_right_after_memory_and_before_conversation() {
    let context = RunContext {
        recent_work: vec![
            task_worker_recent_work_sample(
                Status::Failed,
                "web-research タスク A",
                Some("Pluvio の関連研究調査"),
                Some(
                    "web search returned nothing (possible search path failure: expired key, CAPTCHA, or network block)",
                ),
                &[],
            ),
            task_worker_recent_work_sample(
                Status::Done,
                "先行研究のまとめ",
                None,
                Some("Pluvio と比較可能な非同期ランタイムを 3 件確認した"),
                &["survey.md".into()],
            ),
        ],
        ..full_context()
    };
    let out = render(&context, "artifacts");
    let at = |needle: &str| {
        out.find(needle)
            .unwrap_or_else(|| panic!("missing {needle:?} in:\n{out}"))
    };
    assert!(
        at("## 覚えていること") < at("## あなたの直近の仕事"),
        "{out}"
    );
    assert!(
        at("## あなたの直近の仕事") < at("## 直近のやり取り"),
        "{out}"
    );
    assert!(
        out.contains(
            "- [failed] web-research タスク A（案件: Pluvio の関連研究調査）: web search returned nothing \
                 (possible search path failure: expired key, CAPTCHA, or network block)"
        ),
        "{out}"
    );
    assert!(
        out.contains(
            "- [done] 先行研究のまとめ: Pluvio と比較可能な非同期ランタイムを 3 件確認した 成果物: survey.md"
        ),
        "{out}"
    );

    // 空なら節そのものが無い。
    let without = RunContext {
        recent_work: Vec::new(),
        ..full_context()
    };
    assert!(!render(&without, "artifacts").contains("あなたの直近の仕事"));
    // 対話でない通常 run の前置きは 1 バイトも変わらない（既定値には `recent_work` が無い）。
    assert_eq!(
        render(&RunContext::default(), "artifacts"),
        deliverables_placement_note()
    );
}

/// Phase 41（ADR-0038 D1）: レビューの対話 run にだけ、途中目標とそこまでの成果の節が出て、
/// 末尾に「結果 → 達成の可否 → 次の提案」の指示が足される（対話の指示は消えない）。
#[test]
fn a_milestone_review_shows_the_results_and_asks_for_the_next_proposal() {
    use crate::protocol::{MilestoneBrief, MilestoneReviewContext, MilestoneTaskResult};
    let context = RunContext {
        conversation_addressee: Some(ConversationAddressee::Secretary),
        milestone_review: Some(MilestoneReviewContext {
            milestone: MilestoneBrief {
                id: "01HM".into(),
                title: "隣接領域の動向調査".into(),
                description: "近い分野の直近 3 年を洗う".into(),
                status: "in_progress".into(),
            },
            tasks: vec![MilestoneTaskResult {
                title: "web 調査".into(),
                status: Status::Done,
                outcome: Some("候補を 3 本に絞った".into()),
                artifacts_excerpt: "# answer.md\n候補 A / 候補 B / 候補 C".into(),
            }],
        }),
        ..full_context()
    };
    let out = render(&context, "artifacts");
    let at = |needle: &str| {
        out.find(needle)
            .unwrap_or_else(|| panic!("missing {needle:?} in:\n{out}"))
    };
    // 節は「あなたの直近の仕事」の後、直近のやり取りの前。
    assert!(
        at("## 覚えていること") < at("## 途中目標『隣接領域の動向調査』のここまで"),
        "{out}"
    );
    assert!(
        at("## 途中目標『隣接領域の動向調査』のここまで") < at("## 直近のやり取り"),
        "{out}"
    );
    assert!(out.contains("(in_progress)"), "{out}");
    assert!(out.contains("### [done] web 調査"), "{out}");
    assert!(out.contains("要約: 候補を 3 本に絞った"), "{out}");
    assert!(out.contains("候補 A / 候補 B / 候補 C"), "{out}");
    // 指示は対話の指示の後ろ。
    assert!(
        at("CoS の対話と仕事の開始") < at("## 途中目標の判定をお願いする返事です"),
        "{out}"
    );
    assert!(out.contains("(c) **次の途中目標の提案**"), "{out}");
    assert!(out.contains("milestone_proposal"), "{out}");

    // レビューでない run には何も出ない（通常の対話 run の前置きは 1 バイトも変わらない）。
    let plain = RunContext {
        milestone_review: None,
        ..context.clone()
    };
    let plain_out = render(&plain, "artifacts");
    assert!(
        !plain_out.contains("途中目標の判定をお願いする返事です"),
        "{plain_out}"
    );
    assert!(!plain_out.contains("のここまで"), "{plain_out}");
}

/// ADR-0044 D7（Phase 57）: 「作業場所」に文書の書き方が 1 行出る（題名は 1 行目、紐付けは
/// front matter の `tasks:`、既定のブランチには直接コミットしない）。
#[test]
fn the_workspace_section_says_how_to_write_documents() {
    let note = RepoNote {
        name: "benchfs".into(),
        dir: "/ws/01J/repos/benchfs".into(),
        git: true,
        branch: Some("celeris/01J".into()),
        base: Some("abc1234".into()),
        base_kind: Some("main".into()),
        description: None,
        check: vec![],
        docs: "docs".into(),
        deliverables: ".".into(),
    };
    let out = repos_note(std::slice::from_ref(&note));
    assert!(
        out.contains("文書は `/ws/01J/repos/benchfs/docs/` に Markdown で書く"),
        "{out}"
    );
    assert!(out.contains("題名は 1 行目の `# `"), "{out}");
    assert!(
        out.contains("front matter の `tasks: [<このタスクの id>]`"),
        "{out}"
    );
    assert!(out.contains("既定のブランチに直接コミットせず"), "{out}");
    // `[outputs] docs` を変えるとその場所になる。
    let moved = RepoNote {
        docs: "doc/pages".into(),
        ..note
    };
    assert!(
        repos_note(&[moved]).contains("文書は `/ws/01J/repos/benchfs/doc/pages/` に"),
        "{out}"
    );
    // リポジトリが無いタスクの前置きは 1 バイトも変わらない（空）。
    assert_eq!(repos_note(&[]), "");
}

/// ADR-0047 D2（Phase 61）: 知識の節は**索引だけ**（本文は入れない）。マウントごとに並び、
/// D3 の使い方（`search` / `get` / `record`）が出る。マウントが無い run の前置きは
/// Phase 60 までと 1 バイトも変わらない。
#[test]
fn the_knowledge_section_lists_the_index_of_every_mount_kind() {
    use task_core::{KnowledgeItem, KnowledgeMount};
    let mounts = vec![
        KnowledgeMount::kb("environment/clusters"),
        KnowledgeMount::repo("pluvio", None),
        KnowledgeMount::memory(Some("cluster-hpc".into())),
    ];
    let index = vec![
        KnowledgeItem {
            path: "environment/clusters/pegasus.md".into(),
            title: "pegasus の使い方".into(),
            tags: vec!["hpc".into(), "cluster".into()],
            scope: Some("environment".into()),
            ..KnowledgeItem::default()
        },
        // 別の scope のページはこのマウントには出ない。
        KnowledgeItem {
            path: "user/profile.md".into(),
            title: "人のプロフィール".into(),
            scope: Some("user".into()),
            ..KnowledgeItem::default()
        },
        KnowledgeItem {
            path: "docs/design.md".into(),
            title: "design.md".into(),
            scope: Some("repo:pluvio".into()),
            ..KnowledgeItem::default()
        },
        KnowledgeItem {
            path: "/home/u/.local/celeris/memory/cluster-hpc/notes.md".into(),
            title: "あなたの手帳（案件をまたぐ記憶）".into(),
            scope: Some("memory:cluster-hpc".into()),
            ..KnowledgeItem::default()
        },
    ];
    let out = knowledge_section(&mounts, &index);
    let at = |needle: &str| {
        out.find(needle)
            .unwrap_or_else(|| panic!("missing {needle:?} in:\n{out}"))
    };
    assert!(
        out.starts_with("## 知識 (knowledge base — 索引だけ。本文は道具で読む)\n"),
        "{out}"
    );
    assert!(
        out.contains(
            "あなたが読める知識: `kb:environment/clusters`、`repo:pluvio`、`memory:cluster-hpc`。"
        ),
        "{out}"
    );
    // ADR-0047 D3 の案内文。
    assert!(
        out.contains("`celerisctl knowledge search <語> [--scope …]` で探し"),
        "{out}"
    );
    assert!(
        out.contains("`celerisctl knowledge get <path>` で読む"),
        "{out}"
    );
    assert!(out.contains("`celerisctl knowledge record"), "{out}");
    assert!(out.contains("一時的な情報・雑談・推測は入れない"), "{out}");
    // マウントごとに並ぶ（マウントの順）。
    assert!(
        at("### kb:environment/clusters") < at("### repo:pluvio"),
        "{out}"
    );
    assert!(
        at("### repo:pluvio") < at("### memory:cluster-hpc"),
        "{out}"
    );
    assert!(
        out.contains("- `environment/clusters/pegasus.md` — pegasus の使い方（hpc、cluster）"),
        "{out}"
    );
    assert!(out.contains("- `docs/design.md` — design.md\n"), "{out}");
    assert!(
        out.contains("/memory/cluster-hpc/notes.md` — あなたの手帳"),
        "{out}"
    );
    // マウントしていない scope のページは出ない（本文も出ない）。
    assert!(!out.contains("user/profile.md"), "{out}");

    // マウントが無い・索引が空なら節ごと出ない。
    assert_eq!(knowledge_section(&[], &index), "");
    assert_eq!(knowledge_section(&mounts, &[]), "");

    // `render` に入れても、他の節の後ろ（作業場所の後、役割の前）に 1 回だけ出る。
    let context = RunContext {
        knowledge: Some(crate::protocol::KnowledgeContext {
            mounts: mounts.clone(),
            index: index.clone(),
        }),
        ..full_context()
    };
    let rendered = render(&context, "artifacts");
    let at = |needle: &str| {
        rendered
            .find(needle)
            .unwrap_or_else(|| panic!("missing {needle:?}"))
    };
    assert_eq!(
        rendered.matches("## 知識 (knowledge base").count(),
        1,
        "{rendered}"
    );
    assert!(
        at("## 覚えていること") < at("## 知識 (knowledge base"),
        "{rendered}"
    );
    assert!(
        at("## 知識 (knowledge base") < at("## Role: literature-reader"),
        "{rendered}"
    );
    // 知識を渡さない run には `knowledge_section` の見出しが出ない（ADR-0067 D1 の「成果物の置き場所」
    // の節は知識ベースに触れるので、素朴な「知識」という文字列の有無ではなく見出しそのものを見る）。
    assert!(!render(&full_context(), "artifacts").contains("## 知識 (knowledge base"));
    assert_eq!(
        render(&RunContext::default(), "artifacts"),
        deliverables_placement_note()
    );
}

/// ADR-0047 D2: 索引は最大 200 件（溢れた分は件数だけ）。
#[test]
fn the_knowledge_index_is_capped_at_two_hundred_items() {
    use task_core::{KnowledgeItem, KnowledgeMount};
    let mounts = vec![KnowledgeMount::kb("user")];
    let index: Vec<KnowledgeItem> = (0..250)
        .map(|n| KnowledgeItem {
            path: format!("user/p{n}.md"),
            title: format!("page {n}"),
            scope: Some("user".into()),
            ..KnowledgeItem::default()
        })
        .collect();
    let out = knowledge_section(&mounts, &index);
    assert_eq!(
        out.matches("- `user/p").count(),
        task_core::knowledge::MAX_PREAMBLE_ITEMS
    );
    assert!(out.contains("- （ほか 50 件。`search` で探す）"), "{out}");
}

fn task_worker_recent_work_sample(
    status: Status,
    title: &str,
    project_title: Option<&str>,
    outcome: Option<&str>,
    artifacts: &[String],
) -> crate::protocol::RecentWork {
    crate::protocol::RecentWork {
        task_id: task_core::TaskId::new(),
        title: title.to_string(),
        project_title: project_title.map(str::to_string),
        status,
        finished_at: Some("2026-09-18T00:00:00Z".to_string()),
        outcome: outcome.map(str::to_string),
        artifacts: artifacts.to_vec(),
    }
}

/// ADR-0072 D9（Phase E1）: `context.continuation` が無ければ、この節は 1 バイトも出ない。
#[test]
fn continuation_section_is_empty_without_continuation_context() {
    assert_eq!(continuation_section(&RunContext::default()), "");
}

/// ADR-0072 D9: 続きの実行の節は「Run #N」「前の run の終わり方」「checkpoint の要点」
/// 「これまでの run の 1 行ずつ」を含み、会話・出力の全文は載せない。
#[test]
fn continuation_section_summarizes_the_checkpoint_without_the_full_transcript() {
    use crate::protocol::ContinuationContext;
    let context = RunContext {
        continuation: Some(ContinuationContext {
            run_seq: 3,
            previous_end: "budget_exhausted(turns)".into(),
            checkpoint: serde_json::json!({
                "completed": ["store に execution.rs を追加した", "単体テスト 12 本を通した"],
                "remaining": ["dispatcher の配線"],
                "decisions": [{"what": "WU は直列実行", "why": "worktree 共有のため"}],
                "files_changed": [{"path": "crates/task-core/src/execution.rs"}],
                "tests_run": [{"command": "cargo test -p task-core execution"}],
                "known_failures": [{"what": "clippy の needless_borrow 1 件"}],
                "next_action": "dispatcher.rs の dispatch_ready で next_work_unit を呼ぶ",
            }),
            prior_runs: vec![
                "Run #1 budget_exhausted(turns)".into(),
                "Run #2 budget_exhausted(turns)".into(),
            ],
        }),
        ..RunContext::default()
    };
    let out = continuation_section(&context);
    assert!(out.contains("## 続きの実行（Run #3）"), "{out}");
    assert!(out.contains("budget_exhausted(turns)"), "{out}");
    assert!(out.contains("### checkpoint（Run #2 の終わり）"), "{out}");
    assert!(
        out.contains("store に execution.rs を追加した; 単体テスト 12 本を通した"),
        "{out}"
    );
    assert!(out.contains("dispatcher の配線"), "{out}");
    assert!(out.contains("WU は直列実行"), "{out}");
    assert!(out.contains("crates/task-core/src/execution.rs"), "{out}");
    assert!(out.contains("cargo test -p task-core execution"), "{out}");
    assert!(out.contains("clippy の needless_borrow 1 件"), "{out}");
    assert!(
        out.contains("dispatcher.rs の dispatch_ready で next_work_unit を呼ぶ"),
        "{out}"
    );
    assert!(out.contains("### これまでの Run（1 行ずつ）"), "{out}");
    assert!(out.contains("- Run #1 budget_exhausted(turns)"), "{out}");
    assert!(out.contains("- Run #2 budget_exhausted(turns)"), "{out}");
    // 前の run の生の会話・出力は載らない（checkpoint 由来の要約だけ）。
    assert!(!out.contains("assistant"), "{out}");
}

/// 型が合わない・未知の欄は黙って飛ばす（寛容に読む）。
#[test]
fn continuation_section_tolerates_a_sparse_or_malformed_checkpoint() {
    use crate::protocol::ContinuationContext;
    let context = RunContext {
        continuation: Some(ContinuationContext {
            run_seq: 1,
            previous_end: "yielded".into(),
            checkpoint: serde_json::json!({"completed": "not-an-array"}),
            prior_runs: vec![],
        }),
        ..RunContext::default()
    };
    let out = continuation_section(&context);
    assert!(out.contains("完了: （なし）"), "{out}");
    assert!(
        !out.contains("### これまでの Run"),
        "prior_runs が空なら節ごと出さない: {out}"
    );
}

/// F5-fix5: claude-code の全 run に足す system prompt は、headless であること・turn を終えると
/// run が終わること・長い command も foreground で走らせること・通知を待って turn を終えないことを言う。
#[test]
fn f5_fix5_headless_run_note_forbids_background_tasks_and_waiting_for_notifications() {
    let note = HEADLESS_RUN_NOTE;
    for needle in [
        "`claude -p`",
        "headless",
        "turn を終えた",
        "run は終わり",
        "`run_in_background`",
        "background task を使わない",
        "foreground で実行",
        "`cargo test --workspace`",
        "`timeout`",
        "「完了の通知を待つ」",
        "`result.json`",
    ] {
        assert!(note.contains(needle), "missing {needle:?} in:\n{note}");
    }
    // 決定的（run ごとの値を持たない）で、プロンプト本文（`render`）には入らない。
    assert!(!render(&RunContext::default(), "artifacts").contains("headless 実行"));
}
