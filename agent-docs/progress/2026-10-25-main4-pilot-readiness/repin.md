---
title: main-4 pilot readiness — collect 修正入り campaign binary の再固定（repin、mlsys-main-4-v4）
tasks: [01M5WGP4KE9HT0HHC93T9PJ2FX]
status: done
updated: 2026-10-26
---
# main-4 pilot readiness — collect 修正入り campaign binary の再固定（repin、mlsys-main-4-v4）

完了日: 2026-10-26。task HEAD から build した celeris-campaign を R2 に pin し、check-main-pins・cap-probe・env-lock --verify・
実構成 preflight・pilot manifest（K4・K2）の lock をすべて exit 0 で取り直した。新しい measurement_id は不要だった。

以下のパス表記:
- N = `/local/celeris/data/workspaces/01M5SRJMB1CSGBYT028X0B67N2/artifacts/main4-ready`
- R2 = `N/bundle/final-freeze-2`（root。final-freeze-3 は作っていない）
- RP = `N/repin`（unit ごとの機械向け記録 `RP/{revision,lock-probe,live,pilot}.json`、集約 `RP/repin.json`、作業記録 `RP/notes/<unit>.md`）

## binary
- 新 binary（collect 修正入り）sha256: `2344d2c47b001e66560a778e964d9e5c2f2b9f5108c552016b727f5105b2b402`
  - 置き場所 `R2/pinned/evidence/main-k2/bin/celeris-campaign-fixed`。`R2/pinned/main/pins.json` の CAMPAIGN_BIN_SHA も同じ値（1 行だけ変更、旧版は `RP/notes/pins.json.pre-repin`）。
- 旧 binary（collect 修正なし）sha256: `5132230995128dc745593546132dfb947e537d1913718bea482cbc83f5ff93dd`
  - `R2/pinned/evidence/main-k2/bin/celeris-campaign-fixed.pre-repin` に残した。
- source: task HEAD で `cargo build --release -p celeris-campaign`（CARGO_TARGET_DIR は celeris 指定値）→ exit 0。
  HEAD に件名『collect-stub: collect-observed-out は not_submitted だけなら記録を書いて exit 0』の commit があることを `git log --format=%s` で確認。
  `git rev-parse HEAD:crates/campaign` の値は `RP/repin.json` の crates_campaign_tree（record 葉で再取得し一致）。

## 段ごとの結果
| 段 | 結果 | 記録 |
|---|---|---|
| build-pin | build exit 0、pin 差し替え、check-main-pins exit 0 | `RP/revision.json`、`RP/notes/build-pin.md` |
| lock-probe | 新 binary の offline-lock が R2 の lock.json・env-lock.json と byte 一致 → 新 ID 不要、validate exit 0 | `RP/lock-probe.json` |
| live: prepare / start | exit 0 / exit 0（state/v4・port 7741、既存 project 01M5W9014BJF29SS6VQDFX0Z18） | `RP/live.json` |
| live: cap-probe | A-k2・A-k4・B-k2・B-k4 すべて exit 0、paid_calls 0 | 同上 |
| live: lock | exit 0、測定 daemon から models を取り直しても lock.json・env-lock.json は pin と byte 一致 | 同上 |
| live: env-lock --verify | exit 0（verified） | 同上 |
| live: preflight | exit 0、ok=true、missing なし、checked 17、paid_calls 0、run_id 01M5WHQT0TVY435VY470C2KNYB、config=real、release a28184889f56 | 同上 |
| live: stop | exit 0、port 7741 listen なし・daemon process なし | 同上 |
| pilot k4 | validate・start・lock・stop すべて exit 0、manifest sha256 `472594e3ceca7fc79a44d455347a41a755398f4f7a0f0ee67c668d99c2c505de`、ID mlsys-main-4-v4-pilot-k4 | `RP/pilot.json` |
| pilot k2 | validate・start・lock・stop すべて exit 0、manifest sha256 `0bd0db33718536c32f0e2665e6bbf4bb20953793b88002e550718952221654c7`、ID mlsys-main-4-v4-pilot-k2 | 同上 |
| record | check-main-pins 再実行 exit 0（`check-main-pins: ok`） | `RP/notes/record-check-main-pins.out`、`RP/repin.json` |

## 新しい measurement_id の要否
不要。measurement_id は mlsys-main-4-v4 のまま（root=R2、state/v4）。
理由: lock-probe（offline）と live（測定 daemon から models を取り直した lock）の両方で、新 binary の lock.json・env-lock.json が
R2/pinned/main の同名 file と byte 一致した。lock 内容が変わらないので check_successor は新 ID を求めない。pilot k4・k2 の lock.json も rerun の物と byte 一致。

## 受け入れ条件ごとの結果（この葉）
- repin.json が全欄を持ち値が証跡と一致: `RP/repin.json` を revision/lock-probe/live/pilot.json から生成。
  新旧 binary・pilot manifest の sha256 を `sha256sum` で取り直し一致、pins.json の CAMPAIGN_BIN_SHA も一致。
- check-main-pins が root で exit 0: `python3 scripts/campaign/check-main-pins.py --root R2 --pins R2/pinned/main/pins.json` → exit 0。
- 文書検査: 下の「検査」。

## 検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `sh scripts/dev/check-doc-links.sh` → exit 0
- `sh scripts/dev/check-adr-numbers.sh` → exit 0
- `sh scripts/dev/progress-index.sh --check` → exit 0
- 本 unit はコードを変えていない（変更はこの進捗 file だけ）。

## 変更（この task branch の commit、件名）
- `repin: repin.json と進捗を記録する`（本 unit。build-pin・lock-probe・live・pilot の各 unit は repo を変えず、成果は RP と R2 にある）

## 安全
- 本番の systemd・`~/.config/celeris`・本番 DB・release dir は触っていない。測定は state/v4 の専用 DB・port 7741、daemon は stop 済み。
- LLM の有料呼び出し 0、pilot・主測定は実行していない。秘密値は記録に書いていない（各 unit で測定 token の値を grep -F → 0 件）。

## 未解決
- 新 binary は task HEAD から build した物で、collect 修正がまだ main・release gate を通っていない。main 取り込み後に release の binary と差し替えるなら、同じ手順（build-pin → lock-probe → live → pilot）で再固定が要る。
- R2 の final.json の `experiment_sets.main.fixed.approved_trials` は 2 のまま（refreeze.md の未解決と同じ）。pilot 4 試行の前に直す必要がある。

## 提案
- check-main-pins に「argv.txt の binary の sha256 = CAMPAIGN_BIN_SHA」の照合を足す（今は path だけを見る）。
