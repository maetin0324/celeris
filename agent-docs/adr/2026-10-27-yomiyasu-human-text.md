# 人に返す文面を yomiyasu の要点で書かせ、固定の通知文を yomiyasu_lint.py で検査する

- 日付: 2026-10-27
- 状態: 採用（実装済み: `config/skills/yomiyasu/`、`crates/task-worker/src/human_text.rs`（`HUMAN_TEXT_RULES`・
  `DECISION_TEXT_RULES`）、`claude_code/prompt.rs` の `result_json_instructions`、`preamble::decision_requests_section`、
  `cos_chat::core`、`config/skills/cos-operator/explaining.md`・`cos-inbox-triage/SKILL.md` §6、
  `scripts/dev/check-human-text.py`・`human-text-lint.allow`・`check-health.sh`、`[cos.triage] policy_version` の既定 "3"）
- tasks: [01M5XZW4JSHYYPJBNDMEYF4EKM]
- 関連: [2026-10-05-cos-chat-home](2026-10-05-cos-chat-home.md)（CoS の skill と policy_version）、
  [2026-10-08-cos-chat-prompt-cache](2026-10-08-cos-chat-prompt-cache.md)（Core の大きさ）、
  [2026-10-27-main4-ops-evaluation-scope](2026-10-27-main4-ops-evaluation-scope.md)（方針の版の固定）、
  [2026-10-27-trial-observed-versions](2026-10-27-trial-observed-versions.md)（`cos_policy_version` の観測）

## 背景

利用者は 2026-10-27、エージェントが人に返す文面（CoS の返信・escalation・受信箱の説明・通知、worker の質問と
decision の選択肢、完了報告）を、日本語の推敲 skill yomiyasu（https://github.com/nanaism/yomiyasu、MIT、v2.4.0、
commit 41e5aec）に沿って分かりやすくするよう依頼した。yomiyasu は不自然な比喩・曖昧な主述関係・不要な装飾を、
意味と条件を保って直す。description は「名前を出して頼まれたとき、または CLAUDE.md などの指示で求められたときだけ使う」。

skill の本体は大きい（実測、`config/skills/yomiyasu`）:

| 部分 | bytes | 文字 |
|---|---:|---:|
| SKILL.md（375 行） | 121,722 | 43,482 |
| references/*.md（25 file。必要な時に開く） | 304,996 | 116,815 |
| description（skill の一覧に出る 1 行） | 716 | 298 |
| Celeris 用の規則 `HUMAN_TEXT_RULES` | 798 | 318 |
| 決定の要求に足す `DECISION_TEXT_RULES` | 239 | 97 |

token は bytes と文字数から見積もった。Claude の tokenizer は手元に無く、LLM の API（count_tokens を含む）は
worker から呼べない（ADR 2026-10-07-worker-no-subagents-no-llm-cli）。日本語は 1 文字がおよそ 1 token 前後なので、
規則は 1 run あたり約 300〜400 token、SKILL.md は読み込むたびに約 4〜5.5 万 token になる。

upstream のこの commit には yomiyasu-mini・yomiyasu-pocket は無い（tree を確認した）。短い規則は SKILL.md §1
（意味の保持・情報の不増補）と slop-catalog の「比喩・曖昧語・装飾」から、Celeris の報告に要る分だけを選んで書いた。

## 比べた案

- (a) skill 本体を人が読む文面を書く run（CoS・質問を出す worker・完了報告）に mount し、名前を出して使わせる。
  一覧に載るのは description（716 B）だけだが、使うたびに SKILL.md（約 4〜5.5 万 token）と references を読み、
  check script を回す手順が付く。CoS の run は `max_wall_secs` 900 で、返事 1 回ごとに本体を読むと時間と token が
  返事の本文より大きくなる。CoS の skill は本番の KB（`skills/<名前>`）から解決され、無ければ CoS の run が
  "CoS skills unavailable" で失敗する（`cos_chat/launch.rs`）。mount すると、人が KB に取り込むまで CoS が止まる。
- (b) 要点を短くした規則を prompt に入れ、本体は run に載せない。増分は CoS の Core と worker の結果の指示に
  約 800 B（約 300〜400 token）。Core は run をまたいで同じ bytes なので、prompt cache に乗る
  （ADR 2026-10-08-cos-chat-prompt-cache）。
- (c) 何もしない（固定文の lint だけ）。生成される文面に効かない。

## 決定

- D1（取り込み）: upstream の `skills/yomiyasu` を `config/skills/yomiyasu/` に写す（SKILL.md・references・scripts・
  LICENSE・UNICODE-LICENSE.txt）。作者の画像 `assets/` と Codex plugin の登録 `.codex-plugin/` は写さず、
  SOURCE.md の `modified / excluded:` に書く。scripts は Python 標準ライブラリだけで動き、試験・build から
  外部ネットワークに出ない。
- D2（適用の方式）: (b) を採る。`task_worker::human_text::HUMAN_TEXT_RULES` を、
  - worker の結果ファイルの指示（`result_json_instructions`。execute・plan・案件計画・replan・execution plan・review の
    prompt。summary と question を書く全 run）の後ろに、
  - CoS の Core（`cos_chat::core`。返事・escalation・受信箱の説明）に、
  入れる。決定の要求（`decision_requests_section`）には、問いと選択肢の label の規則 `DECISION_TEXT_RULES` を足す。
  規則には yomiyasu の名前を書く（description の「指示で求められたとき」に当たる）。本体の mount は採らない（(a) の理由）。
  長い報告を本体で推敲する使い方は、必要になった時に、CoS の run への opt-in の mount（KB への取り込み手順込み）として
  別に決める。
- D3（Core の上限）: CoS の Core の大きさの試験（`cos_chat_core_is_byte_identical_across_runs_and_free_of_run_values`）の
  上限を 7500 B から 8300 B に上げる。実測は試験の fixture で 7439 → 8237 B、bench（`cos_chat_bench_prompt_bytes`）の
  core で 7422 → 8220 B。増分は規則の 798 B だけ。
- D4（固定の文面の検査）: `scripts/dev/check-human-text.py` を `check-health.sh`（`test-affected.sh` が毎回走らせる）に入れる。
  人が読む固定の文面を持つ file（通知・root 完了のまとめ・受信箱・通知の feed・担当が見つからない質問・束ねた決定・
  plan gate・CoS triage・`human_text.rs`）から日本語の Rust 文字列を抜き出し、`yomiyasu_lint.py --doc-type mail` の
  warn/error を落とす。format 文字列の断片で切れたために出る候補（一覧の前のラベルの「:」など）だけを
  `scripts/dev/human-text-lint.allow` に理由つきで書く。使われなくなった許可も落とす。`--self-test` は既知の悪い例
  （「解像度」「手触り」）が見つかることを確かめる。Rust の試験にせず Python の検査にしたのは、lint が Python で、
  cargo の試験から python3 を起動する依存を増やさないため。
  初回の検査で直した文面: 通知の要約の「• {kind}: {count} 件 — {title}」を「（{title}）」に、束ねた決定の
  「進めてください:」を「。」に。
- D5（方針の版）: CoS の方針を変えたので版を上げる。`[cos.triage] policy_version` の既定を "1" → "3"
  （`cos-inbox-triage` の `metadata.version` 2 → 3 とそろえる。`GET /health` の `cos_policy_version`）。
  `cos-operator` の `metadata.version` は 4 → 5 にし、Core・operations.md・attachments.md の OP の `policy_version` の例を
  "5" にそろえる（Core は "3"、operations.md は "4" とずれていた）。本番 config は `policy_version = "1"` を明示しているので、
  書き換えは進捗ファイルの「本番への移行」に書く。

## 既存の文面の規則との関係

- `CLAUDE.md` の「完了報告の書き方」（受け入れ条件ごとに条件・コマンド・出力の要点）と、org の方針「人に返す文は
  数十秒で読める分量」はそのまま。yomiyasu の規則は文の書き方（主述・語・順序・装飾）を決め、報告の項目や分量は変えない。
  食い違ったら `CLAUDE.md` と org の方針が優先する。
- cos-operator の explaining.md の「結論を最初に」「web の画面名とボタン名で書く」は残し、yomiyasu の要点を同じ file の
  節として足した。cos-inbox-triage §6 の escalation packet の形（summary 600 文字以内など）は変えない。
- ADR・進捗ファイル・コード・設定・試験の期待値は対象外（文書の書き方は既存の規則のまま）。表記の統一
  （長音・送り仮名）は yomiyasu も扱わないので、Celeris でも直さない。

## 評価条件との関係

main-4 の pinned 測定（RELEASE a28184889f56、別 daemon）は、固定した release と config で動くので、この変更の影響を受けない。
通常運用の評価（main-4-ops-r1、task 01M5XXF7QDVASQFQBT1GZBXS6V）では、CoS の prompt・skill の版は固定票の「方針」の項目で、
`cos_policy_version` は trial の開始時に観測される（ADR 2026-10-27-trial-observed-versions）。版 "3" の daemon で走る試行は、
版 "1" の試行と別の condition として扱う（`docs/ops/mlsys-main-4-ops-r1.md` §8）。

## 結果

- 生成される文面: worker と CoS の prompt に規則が入る。効果は最近の実例 25 件（escalation 10・質問 10・完了報告 5）を
  規則で書き直し、`yomiyasu_check.py` で前後を比べて確かめた（進捗ファイル）。
- 固定の文面: 11 file・193 文字列で yomiyasu_lint の warn/error が 0 件（許可 5 件）。
