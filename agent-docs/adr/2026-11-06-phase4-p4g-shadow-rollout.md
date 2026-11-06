---
tasks: [01M6PAMC44JQ6DB90E9HQ076DW]
---
# ADR 2026-11-06-phase4-p4g-shadow-rollout: 本番 1 案件の限定導入の go/no-go・承認の分離・監視と戻し

## 文脈
Project Controller（ADR 2026-11-04-project-controller）の本番導入の手順は `docs/ops/project-controller.md` の
「導入の checklist（P2-J）」に表で書かれていたが、各段の「進む条件」を人が目で確かめるだけで、条件の判定・
承認の範囲・監視・差分の検出・戻しを同じ形で記録する道具が無かった。P4-B〜F の修正と、案件 CoS の usage の修正
（01M6PYNMP4、外部 operator のコメントで go/no-go の条件に指定）が main に入った。本番の操作（promote・config・
mode の変更・shadow の開始）は人の明示の許可が要り、この task では許可されていない。

## 決定
- D1 道具 `scripts/dev/pc-rollout/pc_rollout.py` を置く。`preflight`（go/no-go）・`snapshot`・`diff`・`monitor`・
  `apply`・`undo`・`e2e`。preflight・snapshot・monitor は GET と file の読み取りだけで、何も変えない。
- D2 go/no-go の条件（G1〜G7）を段ごとに pass / fail / pending / waived で出す。1 つでも fail なら `no-go`、
  pending があれば `no-go (pending)`、全部 pass か waived のときだけ `go`。
  - G1 必要な修正（cos-usage・P4-B・P4-C・P4-D の GC）が main にある（件名で照合。hash は rebase で変わるので書かない）。
    G1r 稼働中の release（`GET /config` の `identity.release`）がそれらを含む。
  - G2 `[project_controller] enabled = true`・`allowed_project_ids` が対象の 1 案件だけ。G2s 読んだ config の sha256 が
    daemon の起動時の `identity.config_sha256` と同じ（書き換えて再起動していない config を根拠にしない）。G2b 予算の
    上限が summary に出る。
  - G3 対象以外の案件の controller が全部 `off`。G4 script guard が止めていない。G4m 領域の mode が段に合う。
  - G5 前の段の結果: shadow の前は dry_run の判断が 1 件以上で効果を伴うもの（`pending`・`applied`・`failed`）が 0、
    待機の規則に `pc/invalid`・`pc/script-unavailable` が無い、controller の root が 0。canary・enabled の前は
    本番（scope=production）で 24 時間以上・違反 0 の shadow の監視の報告。
  - G6 段の人の承認。G7 実 LLM の縮小 pilot の再実行の facts で案件 CoS の run が数えられ、`cos_usage_null` が 0
    （外部 operator の指定。facts が無ければ pending）。非本番の予行では G1r・G7 は waived。
- D3 承認を 2 種類に分ける。`kind = "observe"`（`stages` に `dry_run`・`shadow`）は観察の開始だけを許し、
  `kind = "effects"`（`canary` か `enabled`）は起票などの効果を許す。effects の承認は shadow の監視の報告の sha256
  （`shadow_report_sha256`）を引き、canary は `canary_bps` を書く。どちらも `approved_by = "human:<名前>"`・
  `expires_at`・`project_id`・`api`・`generation`（人が認めた bundle の sha256）・`scope` が要る。observe の承認で
  canary・enabled に上げる `apply` は拒む。Rust 側の権限（mode を上げるのは人の token だけ、`canary`・`ts` は人だけ）は
  変えず、この承認はその上に重ねる運用の gate とする。
- D4 Celeris の worker の run の中（`CELERIS_RUN_ID` がある）では scope=production の承認を拒む。e2e の home が本番の
  path の下なら exit 2。
- D5 監視の差分（`diff`）は、観察の段で次を違反にする: 効果を伴う判断の増加、controller の root の増加、controller の
  CoS 入力の増加、案件 mode・領域 mode・generation・他の案件の mode・daemon の config の sha256 の変化、待機の規則の
  `pc/invalid`・`pc/script-unavailable`、API の失敗。daemon の再起動（instance id の変化）は違反でなく記録。
  `monitor --undo-on-violation` は違反で案件を `off` に戻す（`off` への変更は誰でも行える）。
- D6 `undo` は下げるだけ: 案件を `off`、領域は指定の mode（`compare` か `rust`）より上のときだけ下げる。前後の標本を残す。
- D7 非本番の e2e（`scripts/dev/pc-rollout.sh`）は、一時 home・fake harness・127.0.0.1 の daemon で本番と同じ API の順に
  通す（allowed の 1 案件で再起動 → generation の登録 → dry_run → shadow の監視の途中で SIGKILL → 差分の検出 →
  observe の承認での canary の拒否 → operator による undo → 領域を rust）。この結果は道具と手順の検証であり、本番の
  go/no-go の根拠（G5 の shadow の報告）にはしない（`scope = "non-production"` の報告は G5 で fail になる）。

## 結果
- 本番の判定は 2026-11-06 時点で `no-go (pending)`: G6（人の承認）が無く、G7（pilot の再実行）が未実施。G2〜G5 は
  本番の API・config を人が preflight で読むまで未確認。
- 手順の制約が 1 つ分かった: daemon の再起動は、どの領域も指していない `loaded` の generation を retire する
  （`startup_no_isolate`）。generation の登録は allowed の再起動の後に行う。
- 案件の判断は wake の後に約 30 秒まとめてから行う（coalesce）。shadow の監視は 1 分以上の間隔で十分。
