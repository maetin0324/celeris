---
tasks: [01M6HGFR1TDAP02XY98ES1ZDA1]
---
# ADR 2026-11-03: cos_digest の apply・記録・script-guard の計数を 1 つの transaction に入れる

- 状態: 採用（実装は task 01M6HGFR1TDAP02XY98ES1ZDA1。親 task 01M6HG7T6ZKTX239KGH0QY4HFQ の子 p0b-cos-digest-persist）。
- 関係: [TS 制御層の境界 ADR](2026-11-02-ts-control-layer-boundary.md) 付記 S4-12・S4-14・S4-15・S5-6・S5-7・S5-13 の (12)、
  [破壊的変更を既定にする](2026-10-11-breaking-changes-by-default.md)。
- 根拠: [S6 の進捗](../progress/2026-11-02-ts-control-layer-s6.md) の既知の制約 11・未解決 5。

## 結論
1. digest pass の apply は、pass 1 回につき SQLite の transaction 1 つ（`BEGIN IMMEDIATE`）で行う。
   その中で run ごとに SAVEPOINT を切り、次の 4 つを同じ SAVEPOINT に入れる。
   - `post_digest`: 冪等 key `cos-triage-digest:<run_id>` の assistant message（`chat_messages`）
   - `requeue_run`: `cos_inbox_items` の `running`→`pending`（`WHERE state='running'` のまま）
   - mode `canary`・`ts`（TS の側）と、TS を呼べなかった pass の記録: `script_compare_log` の 1 行と script-guard の計数
     （`script_guard_state`。戻し・無効化が当たればその mode の CAS も同じ transaction）
   - mode `compare` と canary の Rust の側: 影の outbox の 1 行（下の 3）
   run の書き込みのどれかが失敗したら、その run の SAVEPOINT だけを戻す。message も requeue も記録も残らないので、
   その run は次の pass の走査（`cos_triage_runs_without_digest`）に再び載る。他の run の結果は残る。
2. TS の出力を apply する pass（`applied_by = ts`）は、transaction の中で `script_domain_modes` の `cos_digest` の行を読み直し、
   pass の始めの (mode, generation, version) と違えば 1 行も書かずに返す（`mode_changed`）。dispatcher はその pass を
   Rust の command と fallback `mode_changed` で apply し直す。`BEGIN IMMEDIATE` が書き込みを直列にするので、
   照合と apply の間に人や script-guard の mode の CAS が入ることはない（S4-14 の「apply の直前の読み直し」を transaction の中へ移す）。
3. mode `compare`（と canary の Rust の側）の TS は、Rust の apply の後に別の tokio task で走る（S4-14 のまま。TS の結果を待って
   apply を遅らせない）。記録はその後に書くので、apply と同じ transaction には入れられない。そこで apply の transaction で
   表 `cos_digest_shadow_outbox`（migration 0107）に (pass_id, run_id) の行を足す。行には「TS が終わらなかったときの記録」
   （outcome `unavailable`、error `unavailable`・`compare shadow lost before the TS pass finished`、`applied` は apply の結果）を持たせる。
   - TS が終わったら、影の task が 1 つの transaction で outbox の行を消し、TS の結果の記録を追記し計数する。
   - 行が既に無ければ（別の daemon や tick が先に閉じた）何も書かない。記録が 2 件になることはない。
   - 起動の後と、影の task が記録を書けずに終わったとき（打ち切り・in-flight の上限で捨てた・追記の失敗）は、dispatcher が
     次の tick で「この process で走っていない pass」の行を、行に持たせた `unavailable` の記録で閉じる。
     `unavailable` は script-guard が数えない（S2-8）ので、計数は記録と同じ規則で揃う。
4. 冪等: 同じ pass を 2 回 apply しても、message は冪等 key で 1 件、requeue は `WHERE state='running'` で 1 回、記録は
   `script_compare_log` の重複の抑え（S4-15。同じ run・generation・input_digest・outcome・applied_by）で 1 件、計数は
   追記した記録だけを数えるので 1 回である。outbox は (pass_id, run_id) が主 key で、2 度目は足さない。
5. 試験用の失敗の口: store の apply は段（transaction の始め・message の後・requeue の後・記録の後・commit の前）ごとに
   呼ぶ hook を引数で受ける。本番は何もしない hook を渡す。試験は hook で「その段で process が落ちた」を作る。
   hook が `Err` を返すと transaction を commit せずに捨てる（SQLite では未 commit の書き込みは残らない。process の abort と同じ）。
   別の試験は子 process を実際に `abort()` させ、親が同じ DB file を開いて確かめる。時計・待ち時間には頼らない。

## 背景
S5 の実装（`crates/task-dispatch/src/dispatcher/cos_chat/digest.rs`）は、`apply_digest_pass` が command ごとに別の
transaction で書き（`chat_assistant_message_add_once`・`cos_triage_requeue_run`）、その後で `append_pass_record` が記録と計数を
別の transaction で書いていた。追記の失敗は warn だけだった。このため次の欠けがあった。
- apply の後・記録の前に process が落ちると、TS の apply の記録が残らず計数もされない（S5-13 の (12)・S6 の既知の制約 11）。
  記録の自己矛盾で副作用を見つける script-guard の検出（`guard_unvalidated_effect`）にも掛からない。
- message の後・requeue の前に落ちる（または requeue だけ失敗する）と、run は digest 済みとして走査から外れ、requeue が
  行われない。item は退避（fallback）でしか動かない。mode `rust` の本番にもある窓である。
- apply の直前の mode の読み直しと apply の間に mode の CAS が入ると、`rust` に戻った後に TS の command を apply しうる。
- mode `compare` の影の task が in-flight の上限で捨てられた・process が落ちた場合、その run の記録は残らない。

## 選ばなかった案
- **記録を apply の前に書く**: 記録の `applied`（message id・requeue の数）は apply の結果で、前には決まらない。
  前に書くと、apply に失敗した run の記録が残り、計数と副作用が食い違う。
- **mode `compare` の TS を apply の前に待つ**: compare は記録だけの影で、Rust の apply を TS の時間で遅らせない（S4-14）。
  待つと mode `ts` と同じ tick の遅れが本番の `compare` にも入る。
- **原子化をあきらめ、`cos_digest` の canary/ts への切替を拒否する**: 上の 1〜3 で原子化できたので採らない。
  mode CAS の拒否は追加しない（既存の昇格の規則・人だけの昇格・script-guard の停止は変えない）。

## 変えないもの
- 信頼境界: DB を書くのは Rust の apply だけ。TS の command は規則 1〜9 と S5-6 の照合に通ったときだけ apply する。
  照合できない副作用（`script_effect_unverified`）は、その run の SAVEPOINT を戻してから pass を止め、`digest_due` を下ろさず、
  `system:script-guard` で `rust` に戻す（S5-6 のまま。戻しの書き込みは従来どおり別の CAS で、失敗は process の中で `rust`）。
- `digest_due`・`dirty`・20 件の上限・空の走査で `digest_due` を下ろす規則・冪等 key・`script_compare_log` の欄と重複の抑え。
- `→canary`・`→ts` は人の API token だけ、自動の昇格は無い、script-guard の (f) の数値。
- TS だけの変更の高速検証（`scripts/dev/ts-verify.sh`、cargo 0 回）。この変更は Rust 側だけで、bundle・Host API は変えない。

## 消すもの（同じ変更で）
- `CosChatLaunch::apply_digest_commands`・`apply_digest_commands_verified`・`append_pass_record`（command ごとの transaction と、
  apply と別の記録の追記）。
- `ScriptDomainStore::script_compare_log_append_observed` と `SqliteStore::script_compare_log_append_observed`
  （`cos_digest` の記録を apply と別に書く口）。記録と計数は `SqliteStore::cos_digest_apply_pass` と
  `SqliteStore::cos_digest_shadow_finish` だけが書く。
- 旧い挙動の試験（requeue の失敗の後も message が残る、を確かめていた 3 件）は新しい挙動（run ごと書かれず、次の pass で書く）に改める。

## 本番への移行
- migration 0107 は表を 1 つ足すだけで、既存の行を変えない。本番は全領域 `rust` で、`rust` の pass は outbox も記録も書かない。
- 0107 より前の窓で「message はあるが item が `running` のまま」の run が本番に残っているかは未確認。
  確かめ方（読み取り専用）: `sqlite3 -readonly <db> "SELECT i.run_id, count(*) FROM cos_inbox_items i JOIN chat_messages m ON m.client_message_id = 'cos-triage-digest:' || i.run_id WHERE i.state='running' GROUP BY 1"`。
  行があれば人が判断する（この変更は既存の行を直さない）。
