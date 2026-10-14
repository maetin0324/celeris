---
tasks: [01M4VAAKFJHRSVKZ3996NFKSKS]
title: eval-run manifest-prod — 本番 DB から manifest-v2（refill、recheck 採用 30 問）を凍結
status: done
completed: 2026-10-14
---
# manifest-prod: 本番 DB からの manifest 凍結と recheck

本番 schema は 77（`sqlite3 'file:/local/celeris/data/db/celeris.sqlite3?mode=ro' 'SELECT MAX(version) FROM schema_migrations;'` → `77`）。本番 DB は読み取りだけ（runner は mode=ro で開く）。本番 config・systemd・releases には触れていない。中間物は `/local/celeris/data/workspaces/01M4VAAKFJHRSVKZ3996NFKSKS/artifacts/eval/manifest-v1/`（repo の外）。問題文と check の command はここに書かない。

## manifest-v1

- command: `celeris-routing-eval manifest --db /local/celeris/data/db/celeris.sqlite3 --out …/eval/manifest-v1`（cutoff 2026-10-05・seed 0・version 1 は既定）。exit 0、1.1 秒。
- 候補（coding で done の task の done WU のうち check を持つもの）: 1,401 件。
- **manifest sha256（runner の凍結値）: `1912477ed126607ed51584070c8ecd7985a24f8c40b5e90b7e9a054d7985ac42`**
- file の sha256: 凍結直後の写し `manifest.frozen.json` = `85f0c546…7d85322`、recheck 鍵を書き戻した `manifest.json` = `6664a226…834b162`（問題集合と runner の sha256 は不変）。
- 問題数 30。全問に空でない `prompt` がある。
- genre: coding 30（runner は coding だけを採る。manifest-tmp の問題 3）
- kind / stratum: design 5・implement 5・test 4・repair 4・other 4・investigate 4・release 4
- split: dev 15・held-out 15
- subset_12: 12 問（design 2・implement 2・investigate 2・other 2・release 2・repair 1・test 1、dev 6・held-out 6）

## recheck

元の repo（`/home/rmaeda/workspace/agent-platform`）に書かないように、`git clone --bare --shared` で `…/eval/recheck-repo` を作り（objects は alternates で読むだけ）、それを `--repo` に渡した。worktree の追加・prune はこの clone の中だけで起きる。

- command: `celeris-routing-eval recheck --manifest …/manifest-v1/manifest.json --repo …/eval/recheck-repo --out …/manifest-v1/recheck.json --timeout 600 --worktree-root $TMPDIR/recheck-wt`。exit 0、1,592 秒（逐次実行、並列なし）。
- 30 問すべてに recheck の行がある。**採用 16・除外 14**。
- 除外の理由と件数: `check_fails_at_head_commit` 5・`no_base_commit` 4・`noop_check_passes_at_base` 4・`check_timed_out` 1（600 秒）。
- 層ごとの除外: repair 4/4（全部 `no_base_commit`。repair WU は base を記録しない）、release 4/4（head 失敗 2・noop 1・時間切れ 1）、implement 2/5、other 2/4、investigate 1/4、test 1/4、design 0/5。
- 採用 16 問: design 5・implement 3・investigate 3・test 3・other 2（dev 8・held-out 8）。repair と release の層は 0。
- subset_12 の採用 8（design 2・implement 2・investigate 2・other 1・test 1）。
- 元の repo に書いていない確認: `/home/rmaeda/workspace/agent-platform/.git/worktrees` の一覧と mtime が recheck の前後で同じ（diff 空）。`$TMPDIR/recheck-wt` は終了時に消えた。

## ADR D1 との照合

有効 24 問以上（30×2 の候補）・高価 model は 12 問全部、の最低条件に届かない（16 問・subset 8 問）。層も 7 から 5 に減る。

`--version` を上げて作り直しても**同じ 30 問が選ばれる**（`manifest.rs` で選択は seed だけで決まり、version は名札。recheck の除外を選択に戻す経路も無い）。従って objective の「--version を上げて作り直す」では解けない。採用率は約 53% で、repair 層は構造的に（base 無し）全滅する。

## 未解決

- 有効 24 問をどう作るか（人の決定 `manifest-refill`）。
- `recheck` は 30 問で約 27 分。作り直しの recheck も 1 run の壁時計（60 分）の半分近くを使う。

## 提案

- runner の `manifest` に「層ごとに多めに引き、recheck の採用だけで 30 問を埋める」経路を足す（例: 前版の recheck 記録を入力に取り、除外した問題を同じ層の次の候補で置き換えた新版を凍結する）。repair 層は base_commit を親 commit 等から補うか、層から外すかを決める。
- recheck は `--limit` だけでなく再開（済んだ問題を飛ばす）を持つと、run の時間上限で切れても続けられる。

## manifest-v2（人の決定 manifest-refill を受けて）

runner に refill を足した: `manifest --repo <repo>` を付けると、層ごとに seed 順で候補を引き（1 層 `--max-draws` 件まで）、recheck で採用（base 失敗・head 成功）したものだけを層の round robin で 30 問まで埋める。落とした候補は manifest の `refill_rejected`（id・層・理由）に凍結し、採用 30 問の recheck 記録は manifest の `recheck` 欄に入る。repair WU で base が無いものは head の第 1 親を base にし、`base_commit_from_parent: true` の印を付ける。済んだ recheck は `--recheck-cache`（JSON Lines）で再利用する（v1 の 30 行を種にした）。新しい欄は既定値なら書き出さないので、v1 の sha256 は変わらない。v1 は書き換えていない。

- command: `celeris-routing-eval manifest --db /local/celeris/data/db/celeris.sqlite3 --out …/eval/manifest-v2 --version 2 --repo …/eval/recheck-repo --timeout 600 --max-draws 10 --worktree-root $TMPDIR/refill-wt --recheck-cache …/manifest-v2/recheck-cache.jsonl`（cutoff・seed は既定）。逐次実行。`--max-draws 8` では採用 28 で止まった（exit 1、1,489 秒）。同じ cache で 10 にして exit 0（491 秒）。
- **manifest sha256（runner の凍結値）: `118b413ebba94d51a0ee6d6703c9dd7b726d84ade3bbd4009c137f903d21e7e7`**。file の sha256（recheck 欄込み）: `80dce7dc391e23fed7b34b53cb2e85944cfd51ed5618101128a89f2587d4dd2a`。
- 問題数 30（全問に空でない prompt、全問に採用の recheck 行。recheck の manifest_sha256 は凍結値と一致）。
- genre: coding 30
- kind / stratum: design 8・investigate 7・implement 5・test 4・other 3・repair 3・release 0
- split: dev 16・held-out 14
- subset_12: 12 問（design・implement・investigate・other・repair・test 各 2。dev 5・held-out 7）
- base を親 commit で補った問題: 3（全部 repair。subset_12 に 2）
- recheck で落とした候補（`refill_rejected`）: 36 件。理由: `check_fails_at_head_commit` 15・`noop_check_passes_at_base` 14・`no_base_commit` 6（repair 5〈head も親も無い／補えない〉・release 1）・`check_timed_out` 1。層別: release 10（head 失敗 6・noop 2・base 無し 1・時間切れ 1）、other 7、test 6、repair 7、implement 5、investigate 1、design 0。
- 元の repo に書いていない: `/home/rmaeda/workspace/agent-platform/.git/worktrees` の一覧が前後で同じ（diff 空）。worktree は bare clone `…/eval/recheck-repo` の中だけ。`$TMPDIR/refill-wt` は終了時に消えた。

### ADR D1 との照合

有効 30 問（最低 24 以上）・subset 12 問で条件を満たす。ただし **release 層は 10 候補を引いて採用 0**（release WU の check は head でも落ちるか base でも通るものが多い）で、層は 6。基準は下げていない。release を評価に入れたいなら `--max-draws` を上げて引き直す（候補 1 件約 50 秒）。

## 未解決（v2）

- release 層 0 を受け入れるか（下の提案）。

## 提案（v2）

- release 層は評価集合から外れた。release の check を集合に入れたいなら、release WU の check を head で通るように記録する仕組みを別に考える。

## 検査（v2）

- refill 経路の試験を `crates/routing-eval/src/manifest.rs` に足した（一時 git repo と一時 DB）: `refill_fills_30_from_adopted_records_rejections_and_marks_parent_bases`（採用 30・`refill_rejected` 6 件の理由・repair の `base_commit_from_parent` と親 commit・manifest の `recheck` 欄・cache 再利用で同じ凍結値）と `refill_reports_a_shortfall_instead_of_lowering_the_bar`（30 に届かなければ error、manifest を書かない）。
- `cargo test -p routing-eval`: exit 0、lib 26 passed・pipeline_stub 1 passed。
- `cargo clippy -p routing-eval --all-targets -- -D warnings`・`cargo fmt --all --check`: exit 0。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `bash scripts/dev/test-parallel.sh`: exit 0（passed 5085・failed 0・ignored 13、userns null）。
- 本番 DB の schema（この run で再確認）: `sqlite3 'file:…/celeris.sqlite3?mode=ro' 'SELECT MAX(version) FROM schema_migrations;'` → 77。
