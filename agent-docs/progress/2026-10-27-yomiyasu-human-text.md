---
title: 人への報告・質問・通知の文面を yomiyasu に沿って分かりやすくする
tasks: [01M5XZW4JSHYYPJBNDMEYF4EKM]
status: done
updated: 2026-10-27
---
# 人への報告・質問・通知の文面を yomiyasu に沿って分かりやすくする

完了日: 2026-10-27

設計判断は [ADR 2026-10-27-yomiyasu-human-text](../adr/2026-10-27-yomiyasu-human-text.md)。

## 変更
- `phase 1: 人に返す文面を yomiyasu の要点で書かせ、固定の通知文を lint する` — 次の 5 つ。
  - `config/skills/yomiyasu/` に upstream（v2.4.0、MIT。commit は SOURCE.md）を写した。SOURCE.md に URL・commit・license と
    `modified / excluded:`（`assets/` と `.codex-plugin/` を写していない）を書いた。SKILL.md・yomiyasu_lint.py・
    yomiyasu_check.py・slop-catalog.md の sha256 が upstream の raw と一致することを確かめた。
  - `crates/task-worker/src/human_text.rs` の規則を、worker の結果の指示（summary・question）、決定の要求（問いと label）、
    CoS の Core に入れた。cos-operator の explaining.md と cos-inbox-triage §6 に yomiyasu の要点の節を足した。
  - 版: `[cos.triage] policy_version` の既定 "1" → "3"、cos-inbox-triage 2 → 3、cos-operator 4 → 5（OP の例も "5"）。
    評価契約 `docs/ops/mlsys-main-4-ops-r1.md` に §8「方針の版の変更記録」を足した。
  - `scripts/dev/check-human-text.py`（`check-health.sh` から毎回）で固定の文面を yomiyasu_lint.py にかける。
    指摘 2 件を直した（通知の要約の「—」→ 括弧、束ねた決定の文末の「:」→「。」）。
  - CoS の Core の大きさの試験の上限を 7500 → 8300 B にした（規則の 798 B の分）。

## token の実測（ADR の表と同じ）
- CoS の Core: 試験の fixture で 7439 → 8237 B、`cargo test -p task-worker cos_chat_bench_ -- --nocapture` の core で
  7422 → 8220 B（+798 B、318 文字）。
- worker の結果の指示: +800 B（規則 798 B と区切りの改行）。決定の要求: +238 B（97 文字）。
- 本体を mount する案: 一覧の description 716 B、使うたびに SKILL.md 121,722 B（43,482 文字）、references 304,996 B。
- token は文字数からの見積もり（日本語 1 文字 ≈ 1 token 前後）。Claude の tokenizer は手元に無く、LLM の API は worker から
  呼べないので、tokenizer で数えた値ではない。

## 固定の文面の lint
- `python3 scripts/dev/check-human-text.py` → exit 0（`11 files, 193 literals, 0 findings (5 allowed)`）
- `python3 scripts/dev/check-human-text.py --self-test` → exit 0
- 許可 5 件は `scripts/dev/human-text-lint.allow` に理由つき（一覧の前のラベルの「:」3 件、規則の文の箇条書き 1 件、
  使わない語の例として「手触り」「解像度」を挙げた行 1 件）。

## 最近の実例の前後比較（本番 DB を読み取り専用で読んだ）
- 取り方: `sqlite3 "file:/local/celeris/data/db/celeris.sqlite3?mode=ro"`。escalation は `cos_inbox_items` の
  `state='escalated'` の `reason`（CoS が書き、人に見える文。型の違う新しい順に 10 件）、質問は `decisions` の
  `question` と選択肢の label（新しい順に 10 件）、完了報告は `reports` の `kind='result'` の本文（5 件）。
  秘密・token は含まれていない（含まれる値は task 名・release の sha12・時刻・rev）。
- 書き直しは規則（`HUMAN_TEXT_RULES`）に従って行い、`yomiyasu_check.py run --source <前> --final <後> --no-words` で比べた。
  意味が変わる恐れの候補が出た 2 件（rep02 の「確認が必要」→「確かめる必要がある」、rep06 の「成立未確認」→
  「確かめられなかった」）は、原文の言い方に戻して 2 回目で 0 件にした。
- 数値・識別子の照合: 前と後の英数字の語（数値・時刻・rev・ID・コマンド）を数え比べ、後で消えた語は 0 件。
  後で増えた語は、主語として足した `task`（esc07・esc09）と、rep02 の `clippy/fmt`→`clippy と fmt`、
  `cursor2`→`cursor が 2` の区切りの違いだけ。
- 全文（前・後・check の出力・集計）は run の成果物 `human-text-examples/`（before/・after/・check/・summary.tsv）。

| id | 直した | lint 前→後 | diff 候補 | 意味が変わる恐れ | 値の検査 | 消えた語 |
|---|---|---|---:|---:|---|---|
| esc00 | あり | 0→0 | 1 | 0 | 候補なし | - |
| esc01 | あり | 0→0 | 1 | 0 | 候補なし | - |
| esc02 | あり | 0→0 | 1 | 0 | 候補なし | - |
| esc03 | あり | 0→0 | 0 | 0 | 候補なし | - |
| esc04 | あり | 0→0 | 0 | 0 | 候補なし | - |
| esc05 | あり | 0→0 | 1 | 0 | 候補なし | - |
| esc06 | あり | 0→0 | 4 | 0 | 候補なし | - |
| esc07 | あり | 0→0 | 2 | 0 | 候補なし | - |
| esc08 | あり | 0→0 | 2 | 0 | 候補なし | - |
| esc09 | あり | 0→0 | 0 | 0 | 候補なし | - |
| q00 | なし（直す所なし） | - | - | - | - | - |
| q01 | なし（直す所なし） | - | - | - | - | - |
| q02 | あり | 1→2 | 1 | 0 | 候補なし | - |
| q03 | あり | 1→1 | 0 | 0 | 候補なし | - |
| q04 | なし（直す所なし） | - | - | - | - | - |
| q05 | なし（直す所なし） | - | - | - | - | - |
| q06 | なし（直す所なし） | - | - | - | - | - |
| q07 | なし（直す所なし） | - | - | - | - | - |
| q08 | あり | 1→1 | 0 | 0 | 候補なし | - |
| q09 | あり | 1→1 | 1 | 0 | 候補なし | - |
| rep02 | あり | 2→2 | 7 | 0 | 候補なし | -（区切りだけ） |
| rep03 | あり | 3→3 | 2 | 0 | 候補なし | - |
| rep05 | あり | 2→2 | 7 | 0 | 候補なし | - |
| rep06 | あり | 2→2 | 7 | 0 | 候補なし | - |
| rep07 | あり | 1→1 | 1 | 0 | 候補なし | - |

質問 10 件のうち 6 件は、問いが具体的で label も選んだ結果が分かる形だったので直さなかった。
完了報告は、短い名詞止めの並び（「…を維持。」「…を修正。」）を主語と述語のある文に分けた所が主な変更。

代表の 6 件（質問は問いだけ、完了報告は要約の段落だけ）:

### esc06

- 前: 計画「ADR → store と run の基盤 → 一覧 API の opt-in 化 → web 追従」の 6 回目の同じ形の採用（rev 97 05:09・98 05:41・99 05:47・100 06:52・101 07:54・102 08:26）。rev 99〜101 で人に回した後も作り直しが止まらない。CoS は計画を直せないので人に回す
- 後: 計画「ADR → store と run の基盤 → 一覧 API の opt-in 化 → web 追従」が同じ形で 6 回採用された（rev 97 05:09・98 05:41・99 05:47・100 06:52・101 07:54・102 08:26）。rev 99〜101 で人に回した後も、計画の作り直しが止まらない。CoS は計画を直せないので人に回す。

### esc08

- 前: 最小 pilot 不具合修正 task の配送結果が Blocked（rev 161、20:39:36）。直前 20:39:32 の max_replans 上限 2 回目の決定（人待ち）と同じ出来事の知らせ。CoS は task を再開・取り下げできないので人に回す
- 後: 最小 pilot 不具合修正 task の配送結果が Blocked（rev 161、20:39:36）。CoS は task を再開・取り下げできないので人に回す。この知らせは、直前 20:39:32 の max_replans 上限 2 回目の決定（人待ち）と同じ出来事を伝えている。

### q03

- 前: d-release=post-merge は本 task の変更を含む release を要しますが、本 task は release を pin するまで done になりません。どの経路で release を用意しますか？
- 後: どの経路で release を用意しますか。d-release=post-merge は本 task の変更を含む release を要しますが、本 task は release を pin するまで done になりません。

### q09

- 前: Gradle 試験を走らせる host に JDK が無い（java も JAVA_HOME も見つからない）。どうするか。
- 後: Gradle 試験の確認をどうしますか。試験を走らせる host に JDK が無く、java も JAVA_HOME も見つかりません。

### rep03

- 前: 前回レビューの指摘に従い進捗報告を artifacts/2026-10-27-operator-usage-previous.md へ移し、追跡差分から除外した。既存の operator-usage 実装・合成試験・運用手順は維持。提出内容の test-affected は2729 passed、9 skipped、exit 0。提出時HEADで単独7試験とclippy/fmtもexit 0。作業ツリーclean。
- 後: 前回のレビューの指摘に従い、進捗報告を artifacts/2026-10-27-operator-usage-previous.md へ移し、追跡する差分から外した。operator-usage の既存の実装・合成試験・運用手順は変えていない。提出した内容で test-affected は 2729 passed、9 skipped、exit 0。提出時の HEAD で、単独の 7 試験と clippy/fmt も exit 0。作業ツリーは clean。

### rep05

- 前: human-ops schema を /2 に上げた（ADR 2026-10-27-human-ops-actor-roles-request-ref）。actor.kind を 7 種にし --actor-kind を必須化（既定 human を削除）、decided_by/executed_by/approved_by・credential・request_ref/request_revision（record_id の材料）・policy_ref を追加。新しい欄の無い行・未知の kind・/1 行は拒否。report は介入を request_ref で 1 件にまとめ history（initial/revision/repeat）に並べ、主体別件数（interventions_by_actor_kind）・unknown_actor 別欄・excluded_ops.non_human を出し、人の時間は human かつ manual の区間だけ。定義書・runbook 更新。commit『phase 1: human-ops /2 に主体の種類・役割・論理要求の参照を足し、report を主体別・論理要求単位で集計する』。
- 後: human-ops schema を /2 に上げた（ADR 2026-10-27-human-ops-actor-roles-request-ref）。actor.kind を 7 種にし、--actor-kind を必須にした（既定の human を削除）。decided_by/executed_by/approved_by・credential・request_ref/request_revision（record_id の材料）・policy_ref を足した。新しい欄の無い行・未知の kind・/1 の行は拒否する。report は介入を request_ref で 1 件にまとめて history（initial/revision/repeat）に並べ、主体別の件数（interventions_by_actor_kind）・unknown_actor の別欄・excluded_ops.non_human を出す。人の時間は human かつ manual の区間だけを数える。定義書と runbook を更新した。commit『phase 1: human-ops /2 に主体の種類・役割・論理要求の参照を足し、report を主体別・論理要求単位で集計する』。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- `bash scripts/dev/test-affected.sh` → exit 0（health に check-human-text を含む。nextest 6510 passed・21 skipped、
  web Vitest 682 passed・Node 98 passed、tsc -b 通過）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `sh scripts/dev/progress-index.sh --check`・`check-doc-links.sh`・`check-adr-numbers.sh`・
  `check-doc-layout.sh scripts/dev/docs-layout.tsv` → いずれも ok

## 本番への移行
- 本番 config（`~/.config/celeris/config.toml`）の `[cos.triage]` は `policy_version = "1"` を明示している。この release を
  昇格したら、人が `policy_version = "3"` に書き換えて daemon を再起動する（または行を消して既定の "3" を使う）。
  確認: `curl -s http://127.0.0.1:7710/api/v1/health` の `cos_policy_version` が "3"。書き換えないと、CoS の文面は新しい規則で
  書かれるのに、`cos_inbox_items.policy_version` と `/health` は "1" のまま記録される。
- KB への skill の取り込みは要らない（yomiyasu は run に mount しない。ADR D2）。

## 未解決事項
- token は文字数からの見積もりで、tokenizer で数えていない。
- 生成される文面の変化は、配備後の実際の CoS の返事・escalation で確かめていない（規則を入れただけ）。

## 提案
- 長い報告（CoS の報告のまとめ等）を yomiyasu の本体で推敲したいなら、CoS の run への opt-in の mount
  （`cos_chat_skills` に足し、本番 KB に `celerisctl skills import config/skills --name yomiyasu` で取り込む）を別 task で決める。
- `crates/celeris/src/notify/triage.rs` の cutover の初期項目は `policy_version: "1"` を固定で書いている。config の値を渡すか、
  cutover が済んだなら消すかを別 task で決める。
