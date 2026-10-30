---
title: ops-r2 preflight の L1 を持ち主の uid で判定する
tasks: [01M66JYQ249SZHNXK0AEVTZNEC]
status: completed
updated: 2026-10-30
---
# ops-r2 preflight の L1 を持ち主の uid で判定する

完了日: 2026-10-30

final review の差し戻し（L1 が `File::open` と `read_dir` の成否だけで判定し、人の uid が持つ EVAL_ROOT の下を chmod 000 にすると deny_readable 0 件で通る）を直した。L1 は open の拒否だけでは通さない。

## 変更

- commit 件名: `l1-owner: preflight の L1 が持ち主の uid と書き換えの可否を lstat で確かめ、同じ uid の chmod 000 で止める`
- 新 module `crates/campaign/src/l1_owner.rs`。判定は純粋関数 `owner_findings(run: &RunIds, eval_root, deny, stat)`（run の uid・group と lstat の関数を引数に取る）。deny path のうち `--eval-root` の下のものについて次を確かめる。
  - EVAL_ROOT から path までの各要素: 持ち主が run の uid でない（`owned_by_run_uid`）、0 でない（`owned_by_root`）、run の uid から書けない（`writable_by_run_uid`。sticky でも拒否）、symlink でない（`symlink`）
  - EVAL_ROOT の祖先（/ まで）: 持ち主が run の uid でない（`ancestor_owned_by_run_uid`）、run の uid から書けない（`ancestor_writable_by_run_uid`。sticky の dir は他人の entry を rename できないので可）、symlink でない
  - 書けるかは持ち主・group・その他のうち該当する 1 組の bit で決める。run の uid が 0 なら常に書けるとみなす（root では L1 は必ず止まる）
  - EVAL_ROOT と祖先を stat できなければ止める（`stat_failed`）。EVAL_ROOT より下で lstat が ENOENT・EACCES になった所で歩みを止めて通す（その親は上の条件を満たし、run の uid はそこに作ることも覗くこともできない）
- missing は check=`deny_readable`、detail `deny_readable:L1:<path>:<理由> <要素> (uid <n>, mode <o>)`。1 件でもあれば preflight は exit 1、`run` は out を作らない。
- `Host::run_ids`（既定は `/proc/self/status` の実効 Uid・Gid・Groups）を追加。stat は常に実 lstat で、試験が差し替えられるのは run の uid だけ。
- `preflight_isolation::owner_findings` が L1 の要求（`Wrap::Host`）にだけ判定を走らせ、`preflight.rs` の `check_isolation` は probe の結果に連ねるだけ（`preflight.rs` は 8 行増）。
- 迂回の env 変数・CLI flag は足していない。`celeris.runtime-probe/1` の JSON の形は変えていない（判定は probe の外で行う）。

## 試験

- 単体（`l1_owner::tests`、6 件）: 他 uid（2002）が持つ 0700 の EVAL_ROOT は人の uid（1001）でも注入した uid（4242）でも通る、祖先の sticky 1777 は通る、path が無い（親は別 uid・書けない）なら通る / 同じ uid の chmod 000（EVAL_ROOT・下の file のどちらでも）は `owned_by_run_uid` で止まる / root 持ちの EVAL_ROOT・下の dir は `owned_by_root`、run が root なら止まる / 祖先が run の uid 持ち・group で書ける・その他で書けるなら止まる（group に入っていなければ通る）/ symlink・stat できない祖先と EVAL_ROOT は止まる / `/proc/self/status` の読み取り。
- `crates/campaign/tests/preflight.rs`:
  - `same_uid_chmod_000_eval_root_stops_l1_and_an_injected_other_uid_passes`: tempdir に自分の dir と file を作って 0o000 にした実 fixture で、library の preflight が `deny_readable:L1:<path>:owned_by_run_uid <EVAL_ROOT>` の 1 件で止まり、`run_gated` は task 作成・有料実行 0 件で out を作らない。同じ fixture で Host に別の run uid を注入すると通る。
  - `cli_preflight_stops_on_a_same_uid_chmod_000_eval_root`: 実 binary（RealHost・実 uid・実 lstat）で同じ fixture の preflight が exit 1、`paid_calls_made` 0、L1 の missing が 1 件。`run` も exit 1 で out を作らない。
- 共通 fixture（`tests/preflight_setup/mod.rs` の `isolation`）の EVAL_ROOT を、tempdir（自分の uid 持ち）から `foreign_eval_root()` に替えた。`/var/lib`・`/var/cache` などから L1 の判定（同じ `owner_findings`・実 uid・実 lstat）を通る dir を探し、deny path はその下の `celeris-preflight-absent/k4/private.json`（中には何も作らない）。既存の CLI e2e（launcher・arm_set_cli_normal・trial_state_cli_e2e・preflight の CLI 試験）と library 試験は、これで実 L1 を通る。

## 実際に他 uid を要する通過の e2e について

試験は別の uid を作れない（root でも chown 権限でもない）。このため「評価器の別 uid が持つ 0700 の EVAL_ROOT の中に実在する private file」を L1 が通す e2e は置けない。代わりに次で確かめる。

- 通過の規則は単体試験で、uid を注入して確かめる。
- 実 binary が実 lstat で通る経路は、host に既にある他 uid 持ちの dir（中の path は存在しない）で確かめる。worker sandbox（userns）では root 持ちの dir が overflow uid 65534 に見えるので、`/var/lib` の下の dir が使われる。sandbox の外の host（release gate）では root 持ちは L1 を通らないので、他 uid が持つ dir（候補の例は `/var/cache/man`・`/var/lib/celeris-browser`。持ち主は未確認）が要る。見つからなければ `foreign_eval_root` が理由付きで panic する（未確認: sandbox の外の host でどの dir が選ばれるか）。

## 証拠コマンドと結果（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `cargo test -p celeris-campaign --lib l1_owner`: exit 0、6 passed
- `cargo test -p celeris-campaign --test preflight --test runtime_probe --test launcher --test runner_checker`: exit 0（launcher 15 passed・1 ignored、preflight 32 passed、runner_checker 16 passed、runtime_probe 6 passed）
- `bash scripts/dev/test-affected.sh`: exit 0、nextest 2874 tests run: 2874 passed、9 skipped
- `cargo clippy --workspace -- -D warnings`: exit 0
- `cargo fmt --all -- --check`: exit 0
- `python3 scripts/dev/check-source-size.py`: exit 0（0 failure、3 debt warning は既存）

## 未解決事項

- 漏洩検査（`check-ops-r1-leak.py`）の same_uid の外し方は並行 unit leak-owner の範囲。
- ADR 付記・`docs/ops/ops-r2-isolation.md`・`isolate.md` の記述（「L1 の probe は同じ uid では必ず hit」など）を持ち主の uid の判定に合わせるのは後続 unit docs-align の範囲。
- `cargo clippy -p celeris-campaign --tests` は既存の `tests/runtime_probe.rs:18`（`cloned_ref_to_slice_refs`）で落ちる。この unit では変えていない（検査の対象は `--workspace` の非試験 target）。

## 提案

- 人が EVAL_ROOT を用意するときは、評価器 uid が持つ 0700 の dir を、root 持ちで書けない祖先の下（symlink を経ない実 path）に置く。preflight はこの条件を満たさなければ理由付きで止まる。運用文書（docs-align）にこの条件を書く。
