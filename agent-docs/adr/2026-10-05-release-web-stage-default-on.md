# release.sh の web 段を既定で走らせる（2026-10-05）

状態: 採用・実装済み

## 背景

`release.sh` の web/ の段（web ADR-W3 D3）は f1904ecd 以来 `SD_GATE_SKIP_WEB` の既定が 1（skip）だった。
理由は NFS 上での web/app の展開（offline の prod install を含む）に 40〜60 分かかることで、既定を戻すかは
人の判断として残されていた（agent-docs/PROGRESS.md「main 取り込みと SD_GATE_SKIP_WEB の扱い」）。

その結果、配送（prepare.sh → release.sh）で作られるリリースには `web/app` が入らず、web-follow.sh は
`web.ok` を満たさないので `celeris-web@` を新 release へ移さなかった。web は 10/02 以前の ea86af6307f8 の
まま止まり、web/ の UI/UX 改善（task 01M3XTCNKM）を promote しても Web に反映されなかった（2026-10-05 の指摘）。

リリースの置き場所はその後 `/local/celeris/state/releases`（ローカル LVM）へ移り（local-hot-data-migration）、
既定 skip の前提だった NFS 上の展開は無くなった。

## 決定

- D1: `SD_GATE_SKIP_WEB` の既定を 0（web の段を走らせる）にする。止めたいときは `SD_GATE_SKIP_WEB=1` を明示する。
- D2: web の段は非 blocking のまま（落ちても gate は倒れず、`web.ok=false` で web-follow が切り替えない）。
- D3: 人の判断（2026-10-05、「既定を 0 に変える」を選択）。

## 影響

- 配送ごとに web の install / typecheck / test / release / bundle が走る。所要は gate.json の steps に残るので、
  遅すぎれば D1 を見直す。prepare.sh の release.sh の上限（7200 秒）は変えない。
- web/ の無い sha・fixture（selfdeploy の試験の多く）は従来どおり `no web/ directory` で skip。
