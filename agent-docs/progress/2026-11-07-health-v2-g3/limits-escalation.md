---
title: 健全性v2 G3 limits escalation — 同じ health の指摘の往復を event HealthFindingsRepeated と木の root の受信箱 1 件に集約する
tasks: [01M70C8NHEANARRKZD9RGKZ87D]
status: done
updated: 2026-11-09
---
# 健全性v2 G3 limits escalation — 同じ health の指摘の往復を event HealthFindingsRepeated と木の root の受信箱 1 件に集約する

完了日: 2026-11-09

WorkUnit `escalation`（task 01M70C8NHEANARRKZD9RGKZ87D）。[ADR 2026-11-08](../../adr/2026-11-08-health-enforcement-loop.md) の D4.3 の受信箱の項目を作った（経路への配線は段 wire）。基準は `$CELERIS_WU_BASE`。

結論: 完了。event `HealthFindingsRepeated`、木の root に追記する `task_ops::inbox::health_repeat::record_findings_repeated`、受信箱の `health_repeat`（(root, set_sha256) ごとに 1 件）、API の欄と生成型、web の表示と realtime の 2 file を入れ、提出時の HEAD で test-affected・clippy・fmt・`health_limits_inbox_` の 6 件が exit 0。経路への配線は段 wire が行う。

## 実装前 brief
1. 根本原因: D4.3 の「同じ指摘の往復」を上げる先が無い。repair の後も同じ fingerprint の集合が残ったとき、木の root に 1 件だけ出す記録と受信箱の項目が無い。
2. 責務 owner: event の型は `task_core::model::events`、追記と受信箱の導出は `task_ops::inbox`（受信箱の正本の組み立て）、共通形は `task_ops::human_inbox`、answer の委ね先は `task_api::inbox_notifications`。
3. 既存の再利用先: 木の root は `task_ops::delegate::tree_root`（辿り方を `tree_root_by` に切り出して受信箱の読み込み済みの表でも使う）。Blocked の task の events は `build_questions` が 1 回だけ読む（N4 の契約）ので、その読み出しを使う。受信箱では答えない種類の 409 は `Stalled` と同じ扱い。
4. 選択肢: (a) dispatcher の snapshot（`review_resync_halted` と同じ）から作る案は dispatcher の in-memory の状態と再起動の扱いが要るので採らない。正本は追記専用の events なので、root の events から導く。(b) 全 task の events を type で絞る store の問い合わせは trait・migration（範囲の外、index が要る）が要るので採らない。Blocked の task の root だけを読む。(c) `AttentionItem::HealthRepeat` を newtype（struct を包む）にする案は schema が `allOf` になり web の生成型で判別子が消えたので戻した。
5. 新しい状態・分岐・fallback・port: 無し。新しい表・config の欄は無い。正本は event `health_findings_repeated` だけ。
6. 撤去対象: 無し（新しい仕組み。置き換える旧い経路は無い）。
7. 不変条件: (root, set_sha256) ごとに 1 件、源がすべて Blocked を出るか root が終端なら出ない、Blocked の task の events の読み出しは 1 回（既存の `inbox_blocked_events_are_read_once_per_build`）、範囲つきの受信箱は全件と同じ項目（`health_limits_inbox_touching_matches_full_inbox`）。

## 変更（commit の件名）
- 「G3 limits escalation: HealthFindingsRepeated と root の受信箱 health_repeat」
  - `crates/task-core/src/model/health_repeat.rs`（新規、`lib.rs` が `#[path]` で `task_core::health_repeat` として読む）: 位置 `HealthRepeatSite`（`work_unit{key}`・`phase{key}`・`review`）、`normalize_fingerprints`、`fingerprint_set_sha256`（重複を除き辞書順に並べて改行区切りの sha256）、`Event::health_findings_repeated`（集合を正規化し `set_sha256` をここで求める。作る口はこれだけ）。`model.rs` は範囲の外で `mod events` が private なので、型の module は `lib.rs` に置いた。
  - `crates/task-core/src/model/events.rs`: 追記専用の variant `HealthFindingsRepeated { source_task, site, fingerprints, set_sha256 }`。古い events の形は変わらない。
  - `crates/task-ops/src/inbox/health_repeat.rs`（新規）: `record_findings_repeated(store, source, site, fingerprints)` が `delegate::tree_root` で root を求め、root の events に追記して root の id を返す（段 wire の 2 経路が呼ぶ唯一の口）。`attention_items` が Blocked の task の root の events から `(root, set_sha256)` ごとに 1 件の `AttentionItem::HealthRepeat` を作る。源は「今 Blocked で、往復の event の後に `Transitioned { from: Blocked }` が無い」task だけ。項目の時刻は最も古い源の event の時刻。
  - `crates/task-ops/src/inbox/read.rs`: `BlockedAttention` に Blocked の task の読み出しの写し（最後に Blocked を出た行 id と、自身の往復の行）を持たせ、`into_items` が工程・前提・計画の承認と往復の項目を返す。読み足すのは Blocked でない root 1 件につき 1 回だけ。
  - `crates/task-ops/src/inbox.rs`: variant `AttentionItem::HealthRepeat`、`build_questions` の読み出しを `observe` に渡す、`build_attention` は `into_items` の 1 行（関数は伸ばしていない）。
  - `crates/task-ops/src/human_inbox.rs`: `InboxKind::HealthRepeat`（`health_repeat`、`ALL` は 25）、`Builder::health_repeat`（id `health_repeat-<root>-<set_sha256 の先頭 16 字>`、選択肢 comment・retry は源の task の操作を示すだけ、native なし、`blocking.tasks` は源、`blocking.root` は root）。`Builder::attention` は arm 1 行。同じ関数の統合依頼の arm の closure の不要な波括弧を外した（動作は同じ）。`human_inbox/item_task.rs` の `TASK_KEYED` に `health_repeat`。
  - `crates/task-ops/src/delegate.rs`: `tree_root` の辿り方を `tree_root_by`（親の引き方を引数で受ける）に切り出した。`tree_root` の動作は同じ。
  - `crates/task-api/src/inbox_notifications.rs`: `health_repeat` の answer は 409 `native_action_required`。同じ 409 の組み立て 11 か所を `native_required` にまとめた（文言は同じ）。`crates/task-api/src/query.rs` の `event_type_name` に 1 行。
  - `docs/api/v1/api-v1.schema.json`・`event.schema.json`・`web/api/generated/{schema.json,types.ts}` の再生成、`docs/api/v1/api.md` の改訂の行と §3.2 の説明、`web/features/inbox/inbox-model.ts` の種類と表示名「修復の後も同じ health の指摘が残る」。
- 「G3 limits escalation: 進捗に範囲の外の 2 file の申告のやり直しを書く」: 進捗の「未解決」に範囲の拡張の申告を書いた（下の「経緯」）。
- 「G3 limits escalation: web の realtime の種類と invalidation に health_findings_repeated を足す」（replan で範囲に入った 2 file）: `web/api/realtime/event-kinds.ts` の `EVENT_KINDS` に `"health_findings_repeated"`、`web/api/realtime/invalidation-map.ts` の `EVENT_INVALIDATION` に `health_findings_repeated: { sets: ["T", "L"] }`（root の timeline と受信箱が変わる）と ADR を指す注釈 1 行。
- 最初の test-affected で check-health の `size.responsibility-growth` が 3 件（`build_attention`・`Builder::attention`・`delegated_request` は閾値 200 行を既に超えており、arm と呼び出しで伸びた）で落ちた。上の `into_items`・`Builder::health_repeat`・`native_required` で 3 関数とも基準より伸びない形にし、worsened 0 で通した。`fn record` の同名の警告（`dup.public-surface`）は `record_findings_repeated` に改名して消した。

## 試験（守る不変条件）
既存の受信箱の試験は `HealthFindingsRepeated` を知らない。時刻は使わず、遷移の event の順だけで決まる。
- `health_limits_inbox_two_sources_same_set_one_item`: 深さの違う 2 つの源（WU と段）が並びと重複の違う同じ集合を上げても root に 1 件。event は 2 件とも root に載り、項目の源は 2 つ、native なし。
- `health_limits_inbox_different_sets_two_items`: 集合が違えば同じ root で 2 件。root 自身が源なら root に載る。
- `health_limits_inbox_source_leaving_blocked_clears`: 源が Blocked を出ると外れ、全部出ると消える。出た後に別の理由（質問）でまた Blocked になっても戻らない。同じ往復でまた記録すれば 1 件出る。
- `health_limits_inbox_terminal_root_hides_item`: root が終端なら出ない。
- `health_limits_inbox_touching_matches_full_inbox`: 範囲つきの受信箱（CoS の一次対応）は root・源のどれに触れても全件と同じ項目を返す。
- `health_limits_inbox_event_serde_roundtrip`（task-core）: 3 種の位置で event の serde が往復し、`fingerprints` は正規化され、集合の鍵は並びと重複に依らない。
- 既存の `inbox_blocked_events_are_read_once_per_build`（Blocked の root 1 件につき events の読み出し 1 回）が最初の実装（root と源を読み足す）で落ちたので、上の `BlockedReads` で読み出しを共有して合格させた。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `corepack pnpm@12.6.0 -C web exec tsc -b` → exit 0。
- `bash scripts/dev/test-affected.sh` → exit 0。nextest 8094 tests run: 8094 passed（23 skipped）、web の型検査（tsc -b）合格、web の unit 試験 104 files・711 tests passed、`test-ts-verify` 20 passed、repo-health verdict=pass（fail_grade 0、new 1、worsened 0、`source-size` pass）。new の 1 件は optional の `dup.contract` の warning（`required_checks::health::UnavailableReason` と `route_select::availability::UnavailableReason` の同名）で、この unit の変更ではない。
- `cargo nextest run -p task-core -p task-ops -p task-api --no-tests=fail -E 'test(/health_limits_inbox_/)'` → exit 0: 6 tests run: 6 passed。`grep -rhoE 'fn health_limits_inbox_[a-z0-9_]+' crates/ | sort -u | wc -l` → 6。
- 前の run の `cargo nextest run -p task-core -p task-ops -p task-api --no-tests=fail -E 'test(/health_limits_inbox_/) | test(/inbox/) | test(/item_task/) | test(/delegate/) | test(/schema/)'` → 168 passed（この run の test-affected の 8094 件に含まれる）。
- `cargo fmt --all -- --check` → exit 0。`cargo clippy --workspace -- -D warnings` → exit 0。
- 範囲 check（`sh "$CELERIS_WU_SCOPE_PATHS"` と、replan 後の計画の正規表現に `web/api/realtime/(event-kinds|invalidation-map)\.ts` を足したもの）→ 範囲外の path なし。

## 本番への移行
- config・表・migration の変更は無い。新しい event の種類と受信箱の種類が増えるだけで、古い events はそのまま読める。この unit だけでは event を書く経路が無い（段 wire が `record_findings_repeated` を呼ぶ）ので、release に入っても受信箱に項目は出ない。

## 未解決
- 無し。

## 経緯
- 最初の範囲には `web/api/realtime/event-kinds.ts`・`invalidation-map.ts` が無かった。`EventKind` は生成型 `Event["type"]` から導かれ、2 file はそれを網羅で検査するので、新しい event の種類を足すと web の `tsc -b` が落ちる。前の run は申告を checkpoint にだけ書いて result を done で返し、WU の check `bash scripts/dev/test-affected.sh` が同じ 2 件の tsc のエラーで exit 1 になって WU が retry（1/2）になった。retry の run が `yield.plan_issue` で申告し直し、replan で範囲に 2 file が入った。この run が 2 行を足して提出時の検査を通した。

## 提案
- `HealthRepeatSite` の module は `model.rs` が範囲の外のため `lib.rs` の `#[path = "model/health_repeat.rs"]` で読んでいる。`model.rs` を変えてよい後続の unit（record など）で `model.rs` の `mod health_repeat;` と `pub use` に移すとよい。
- 並行の unit `repeat`（`health_review::limits`）が fingerprint の集合を比べるときは、同じ鍵になるように `task_core::health_repeat::{normalize_fingerprints, fingerprint_set_sha256}` を使うとよい（集合の鍵を 2 か所で定義しない）。
