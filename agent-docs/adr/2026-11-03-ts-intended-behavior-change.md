---
tasks: [01M6HGFR1TD2G3H4JF6J67CTBM]
---
# ADR 2026-11-03: TS の意図した方針改善（behavior change）を言語移行（parity）と分けて数える

- 状態: 採用（実装済み。記録は [p1d-intended-change](../progress/2026-11-03-ts-control-ops/p1d-intended-change.md)）。
- 関係: [境界 ADR](2026-11-02-ts-control-layer-boundary.md) の (f)・付記 S2-2・S2-8・S5-4・S5-7・S6-3 の 5、
  [cos_digest の apply と記録の原子化](2026-11-03-cos-digest-persist.md)、[TS の高速検証](2026-11-02-ts-fast-verify.md)、
  [破壊的変更を既定にする](2026-10-11-breaking-changes-by-default.md)。

## 結論
- Rust の参照実装と TS の出力が違う比較のうち、**人が承認した改善仕様（版付き・hash 付き）が予言する差分と byte 一致するものだけ**を、
  新しい outcome `explained`（説明済み）として数える。それ以外の差分は今までどおり `mismatch`（説明のつかない不一致）で、
  script-guard が `canary`・`ts` を `rust` へ戻す。
- 説明は「正規化」ではない。TS の出力に手を加えない。改善仕様の編集を **Rust の出力** に当てて期待出力を作り、TS の出力と
  同じ serde_json の直列化の byte 列で比べる。1 byte でも違えば `mismatch`。
- 承認は人の API token の主体（`human:*`）だけ。取り消しは認証された誰でも行える（説明を減らす向き＝fail-closed の向き）。
- 対象の領域は今は `cos_digest` だけ（p0b の原子化で apply・記録・計数が 1 transaction になったため使える）。
  `lane_rules`・`route_priority` の改善仕様は承認で 422 `behavior_change_domain_unsupported` にする（比較の結線が無い領域で説明を数えない）。
- これで S6 pilot の「TS だけの方針改善は当たる入力で自動の戻しになる」（S6-3 の 5、pilot の分かったこと）が、Rust 参照を直さずに解ける。

## 背景
境界 ADR (f) と付記 S2-2・S5-7 は正規化を持たず、`mismatch` を全部「説明のつかない不一致」に数える。S6 pilot
（`scripts/dev/ts-pilot.sh`）は `cos_digest` の見出しの run の失敗の理由を 200 文字で切る改善を TS だけで作り、当たる入力 15 件で
差分 9 件を出した。この差分は意図した改善だが、canary の間に来ると script-guard が `guard_mismatch` で戻す。
Rust 参照に同じ変更を入れれば通るが、それでは TS の自己改善のたびに Rust の compile・link と試験が要り、TS 化の目的（(f) の
edit→activate の短縮）に反する。

## 決定

### D1 改善仕様（`celeris.script-behavior-change/1`）
置き場は `control-ts/behavior-changes/<domain>/<id>.v<version>.json`。型の正本は `task_core::script_behavior_change::BehaviorChangeSpec`
（`deny_unknown_fields`）。欄:

| 欄 | 中身 |
|---|---|
| `schema` | `celeris.script-behavior-change/1` |
| `id` | `[a-z0-9-]{1,64}`。同じ改善の版をまとめる名前 |
| `version` | 1 以上。同じ `id` の版は 1 つの hash にだけ結び付く（再利用しない） |
| `domain` | 領域（今は `cos_digest` だけを承認できる） |
| `summary` | 改善の説明（1〜512 byte） |
| `supersedes` | 前の版の `spec_sha256`（最初の版は `null`）。変更履歴の鎖 |
| `when` | 入力条件（1〜4 個、すべて成り立つとき）。`{path, op, value}`。`path` は入力 DTO の JSON pointer、`op` は `equals`（JSON 値の一致）・`min_chars`・`max_chars`（文字列の code point 数） |
| `edits` | 期待差分の述語（1〜4 個）。`{path, find, replace}`。`path` は出力の JSON pointer（`*` は配列の全要素）、`find`・`replace` は部品の列で、部品は `{"lit": 文字列}`・`{"input": pointer}`（入力の文字列）・`{"truncate_chars": {input, max, ellipsis}}`（入力の文字列を `max` code point までにし、超えたら `ellipsis` を足す） |
| `cases_sha256` | 期待差分 fixture（D3）の file の byte 列の sha256（64 桁 hex）。仕様の hash がこの fixture も束ねる |

**説明の判定**（`explain`）。次の全部が成り立つときだけ `explained`:
1. 比較が `mismatch`（TS の出力は DTO として正しく、Rust の安全検証を通り、`safety_violations` が空）。`ts_error`・`invalid_output`・
   `unavailable`・安全違反は判定に来ない（説明で救わない）。
2. 承認済み（D4）の仕様で、`domain` が同じで、`when` が入力ですべて成り立つ。
3. `edits` を Rust の出力に当てた期待出力が作れる。`path` の各文字列に `find` がちょうど 1 回現れればそこを `replace` にし、
   2 回以上なら作れない（曖昧）。全 edit で置き換えが 1 回も無ければ作れない。`find` が空・`find` と `replace` が同じ・
   `input` が文字列でない、も作れない。
4. 期待出力と TS の出力が serde_json の直列化で byte 一致する。

仕様の検証（承認時・runner の読み込み時）: 欄の範囲、pointer の形（`/` で始まる。`when` に `*` を書かない）、`lit` は 1024 byte まで、
`max` は 1〜100000、`cases_sha256`・`supersedes` は 64 桁 hex。

**hash**: `spec_sha256` は `BehaviorChangeSpec` の serde_json の直列化（struct の宣言順、空白なし）の sha256（64 桁 hex）。
file の空白や key の順は hash に影響しない。暗号の署名鍵は導入しない。「署名」は、この hash に対する人の token の承認の記録（D4）である。

### D2 outcome `explained` と script-guard
- `CompareOutcome` に `explained` を足す。`cos_digest` の記録（`CosDigestCompareRecord`）に `explained_by`（`{id, version, spec_sha256}`）を足す
  （`explained` の行だけ。他の行では欄を書かない）。`ts_commands_head` は `mismatch` と同じく残す（差分を隠さない）。
- script-guard の窓の文字に `e` を足す（`ScriptGuardCall::Explained`）。`e` は戻す条件に入らず、`clean_streak`・`ts_side_clean_streak` を
  進める（(f) の「説明のつかない不一致」ではないため）。失敗・timeout の連続は切る。`mismatch` は今までどおり 1 件で戻す。
- 昇格の前提（`readiness`）は件数の条件を変えない（純粋判定 1000、CoS 200）。`explained` はその件数に入る。
  つまり「説明済みの差分だけ」の generation は段階昇格の候補になる。昇格は今までどおり人の token の API（付記 S5-4）。
- 安全違反・timeout 率・失敗率の判定は変えない。(f) の数値は 1 つも変えない。

### D3 独立した評価と golden の更新規約
- **旧挙動の fixture**（`control-ts/fixtures/<domain>/`、Rust golden）は Rust の参照実装の出力のまま。TS の改善に合わせて書き換えない。
  再生成は今までどおり task-core の example だけが行う（`script-runner-cache.sh`）。
- **期待差分 fixture** は `control-ts/behavior-changes/<domain>/<id>.v<version>.cases.jsonl`。1 行 = `{name, input, rust, expected, expect}`。
  `rust` は Rust の参照実装の出力、`expected` は改善後の期待出力、`expect` は `explained`（差分があり仕様が説明する）か
  `equal`（改善が当たらない境界の入力で、TS は Rust と同じであるべき）。task-core の試験
  （`behavior_change_cases_match_rust_reference_and_spec`）が、`rust` が参照実装の出力と一致し、`expected` が仕様の期待出力と一致し、
  file の sha256 が仕様の `cases_sha256` と一致することを確かめる。
- **評価**は検証専用の runner（`script-host-runner behavior`。cargo を起動しない事前 build の道具）が行う: 候補の bundle で
  (1) 旧挙動の fixture の全件を走らせ、Rust と同じか、仕様が説明するか（説明できない差分は 0 件でないと失敗）、
  (2) 期待差分 fixture の全件を走らせ、TS の出力が `expected` と byte 一致し、`explained` の行は仕様が説明し、`equal` の行は Rust と同じ、
  を数える。仕様の検証に落ちた file、`cases_sha256` の食い違いも失敗。`scripts/dev/ts-behavior-eval.sh` が候補の patch を使い捨ての
  作業ツリーに当てて bundle を作り、runner を走らせる。
- **版の変更**: 承認した仕様の file は変えない。改善を変えるときは `version` を上げた新しい file（`supersedes` に前の hash）と新しい期待差分
  fixture を足し、改めて人が承認する。古い版の file は消さない（変更履歴）。

### D4 承認・取り消しの記録（migration 0108）
- 表 `script_behavior_changes`（`spec_sha256` PK、`change_id`・`version`（UNIQUE の組）、`domain`、`spec_json`、`state` ∈ `approved`・`revoked`・
  `superseded`、`approved_by`・`approved_at`・`updated_by`・`updated_at`）。追記専用の `script_behavior_change_events`（`id`・`ts`・`spec_sha256`・
  `change_id`・`version`・`action`・`actor`・`reason`）。store に event の表の UPDATE・DELETE の経路を作らない。
- `POST /api/v1/script/behavior-changes`（本文 `{spec, spec_sha256, reason}`）: 人の主体だけ（他は 403 `script_behavior_change_requires_human`、表は不変）。
  仕様の検証に落ちれば 422 `behavior_change_invalid`、`spec_sha256` が計算と違えば 422 `behavior_change_hash_mismatch`、領域が未対応なら
  422 `behavior_change_domain_unsupported`、同じ `id`・`version` に別の hash があれば 409 `behavior_change_version_taken`、
  `version` が承認済み・過去の版以下なら 409 `behavior_change_version_not_newer`。同じ hash がすでに `approved` なら何も変えず 200。
  `revoked` の hash は 409 `behavior_change_revoked`（直すなら版を上げる）。新しい版の承認は同じ transaction で同じ `id` の古い `approved` を
  `superseded` にする。
- `POST /api/v1/script/behavior-changes/{spec_sha256}/revoke`（本文 `{reason}`）: 認証された誰でも。`approved` を `revoked` にする。
- `GET /api/v1/script/behavior-changes`: 全行と記録。
- dispatcher は `cos_digest` の pass ごとに承認済みの仕様を読み、その pass の比較に使う（compare の影も同じ集合を持って走る）。
  承認・取り消しの効果は次の pass から。
- CoS の operations には足さない（承認は人だけ。取り消しは API で行う）。`/cos/operations` の表では `X: human_only`（承認）と
  `X: script_api_only`（取り消し）にする。

### D5 変えないもの
- Rust の再検証・安全条件（付記 S4-3 の規則 1〜9）、`ts_error`・`invalid_output`・timeout の fallback（S5-6）、安全違反の戻しと無効化、
  全体の停止、`clean_streak` の件数、昇格の人の token。
- 正規化の規則（`normalized` は空のまま）。説明済みの行も `rust` と `ts` の要約・`ts_commands_head` を残す。
- 本番の mode・config・DB。本番で改善仕様を承認するのは人の手順（進捗の「本番への移行」）。

## 不採用
- **正規化の規則を足す**（S6 pilot の提案 (2)）: 「この欄の違いは見ない」は TS の任意の誤りも隠す。期待出力との byte 一致の方が狭い。
- **Rust 参照も直して golden を作り直す**（同 (1)）: 改善のたびに cargo の compile・link が要り、TS 化の目的に反する。S6-3 の 2（golden の凍結）までの
  二重実装の間の道として不採用。
- **仕様を bundle の manifest に入れる**: bundle（TS 側）が自分の差分を自分で説明できてしまう。承認は DB の人の記録だけが持つ。
- **gen ごとの承認**: 改善は方針の承認で、bundle の hash には結び付けない。同じ改善を持つ次の bundle で承認し直さずに済む。
  bundle が仕様と違う差分を出せば `mismatch` で戻る。
