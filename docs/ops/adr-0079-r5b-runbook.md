# ADR-0079 R5b 手順書: 木を有効にし、browser と BenchFS の root task を作る（人が実行する）

- 対象: [ADR-0079](../adr/0079-recursive-task-decomposition.md) §7 R5b、付記「R5b-prep 実装時の逸脱・明確化」
- 前提のコード: R5b-prep（`PUT /tasks/{id}/execution-plan` の /3 が planner と同じ経路を通る・`POST /tasks/{id}/tree/adopt`・
  `/plans/new` の撤去）が main に入り、そのリリースが本番に昇格していること。**migration は無い（schema 33 のまま）**。
- 実行者: 人。各手順は素のコマンド（パイプを使うものは手元の確認だけ）。本番の DB は書かない（書き込みはすべて API）。
- 書いた日: 2026-09-29。値（task id・コミット・件数）は本番の DB を読み取り専用で読んで埋めた（`sqlite3 "file:…?mode=ro"`）。
- 用意されたもの: 本書の JSON をそのまま使う。`<…>` の置き場所は手順の中で変数に取る（貼り付けてそのまま動く）。

## 0. 変数と前提の確認

```bash
export API=http://127.0.0.1:7710/api/v1
export TOKEN="$(cat ~/.config/celeris/api.token)"
export RODB="file:/var/lib/celeris/celeris.sqlite3?mode=ro"
export AP_PROJECT=01M2WTS3DKNZBSZ2JMVB4CZMBW      # 案件「agent-platform の自己改善」（slug agent-platform）
export BF_PROJECT=01M35WRV77A2JPYGERQGXF6V7K      # 案件「BenchFS 国際会議フルペーパー化」（slug benchfs）
mkdir -p ~/r5b && cd ~/r5b

# 動いているリリースと schema（R5b-prep を含むリリースであること。schema_version は 33）
curl -s "$API/health"; echo
export SHA="$(curl -s "$API/health" | python3 -c 'import json,sys; print(json.load(sys.stdin)["release"])')"
echo "release=$SHA"
systemctl --user list-units 'celeris*' --no-legend
# R5b-prep が入っているか（入っていれば 401 ではなく 422 tree_disabled か 404 が返る。401 はトークン無し）
curl -s -o /dev/null -w '%{http_code}\n' -X POST -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' \
  -d '{"task_id":"01M3MFS5T52FXA63W4V10XGC4S","stage":"x","unit_key":"x"}' "$API/tasks/01M3MFS5T52FXA63W4V10XGC4S/tree/adopt"
```

期待: `schema_version` 33、`role` active、最後の行は **422**（`tree_disabled`。R5b-prep より前のリリースなら 404）。

採用する task の今の状態（読み取り専用。どちらも成果は main に入っている）:

```bash
sqlite3 "$RODB" "select id, status, parent_id, root_id, title from tasks where id in ('01M3MFS5T52FXA63W4V10XGC4S','01M3MZKB3DFYJNBH015MJGQ0BT');"
git -C ~/workspace/agent-platform merge-base --is-ancestor celeris/01M3MFS5T52FXA63W4V10XGC4S main && echo "phase1 in main"
git -C ~/workspace/agent-platform merge-base --is-ancestor celeris/01M3MZKB3DFYJNBH015MJGQ0BT main && echo "phase2 in main"
```

2026-09-29 の値: 01M3MFS5…（Phase 1 MVP）= `done`、01M3MZKB3…（Phase 2）= **`failed`**（`review_fail`: main の祖先の検査だけが
不合格。人が main に取り込んだ = ce5d768）。R5b-prep は `done` と `failed` を採用できる（付記 5.）。どちらも `parent_id` / `root_id` は無い。

## 1. 設定を変えて再起動する（`[execution.tree] enabled = true`）

`[execution.tree]` は R1a から全リリースが知っている欄なので、どのリリースでも起動できる。API の `tree_limits` は起動時に読むので
**再起動が要る**（`POST /reload` では効かない）。

```bash
cp ~/.config/celeris/config.toml ~/.config/celeris/config.toml.bak-20260929-r5b
grep -n '^\[execution\.tree\]' ~/.config/celeris/config.toml || printf '\n# ADR-0079 R5b（2026-09-29 人の決定）: 再帰的な task の木を有効にする\n[execution.tree]\nenabled = true\n' >> ~/.config/celeris/config.toml
tail -5 ~/.config/celeris/config.toml
# 設定が読めること（DB は開かない）
~/.local/celeris/releases/$SHA/bin/celerisctl routing show --config ~/.config/celeris/config.toml > /dev/null && echo "config ok"
```

再起動は in-flight が 0 のときだけ（ADR-0074 P-F-1 と同じ）:

```bash
cut -d" " -f1 /proc/loadavg
sqlite3 "$RODB" "select status, count(*) from tasks where status in ('running','reviewing') group by status;"
sqlite3 "$RODB" "select count(*) from runs where status = 'running';"
# 上の 2 つが空 / 0 なら
systemctl --user restart celeris@$SHA
sleep 5
curl -s "$API/health"; echo
# 木が有効になったこと（どの task でもよい。tree_enabled が true）
curl -s "$API/tasks/01M3MFS5T52FXA63W4V10XGC4S/task-tree" | python3 -c 'import json,sys; print("tree_enabled =", json.load(sys.stdin)["tree_enabled"])'
```

## 2. browser の root task を作り、/3 の計画を `PUT` する

### 2.1 root task（draft で作る: 計画より先に dispatch されないように）

```bash
cat > browser-root.json <<'EOF'
{
  "title": "browser capability（Phase 1〜4）",
  "objective": "ADR-0078 / ADR-0080 の browser capability を Phase 1〜4 まで仕上げる。Phase 1（MVP、task 01M3MFS5T52FXA63W4V10XGC4S、main 09a6b0c）と Phase 2（task policy・手動登録の credential broker・人間承認、task 01M3MZKB3DFYJNBH015MJGQ0BT、main ce5d768）は完了して main に取り込み済みなので採用する。Phase 3（Browser Identity・live proxy・takeover）と Phase 4（isolated runtime・stronger injection・backend routing）を、それぞれ独立に受け入れる子 task として実装する。小タスクの依存・検査と人の決定点は /var/lib/celeris/workspaces/01M3MFS5T52FXA63W4V10XGC4S/wu/design-gap/artifacts/phases-and-decisions.md の表に従う。Phase 2 の決定（H1 手動登録から開始・H2 操作ごとの approve_once と短い lease・H3 認証 session の間は LLM の観測を止める・H6 Phase 2 は trusted local で隔離は Phase 4）は ADR-0080 に記録済み。",
  "acceptance": [
    {"type": "reviewer", "text": "Phase 3 と Phase 4 の子 task が done で、この root のブランチに取り込まれている（phase_integrated.merged に p3 / p4）"},
    {"type": "reviewer", "text": "phases-and-decisions.md の P3-A〜C・P4-A〜C の各行の「単独の成果・検査」が満たされているか、満たせない行は理由と後続が明記されている"},
    {"type": "reviewer", "text": "H4・H5・H7 の人の回答と ADR-0080 の決定に反する実装が無い"}
  ],
  "project_id": "01M2WTS3DKNZBSZ2JMVB4CZMBW",
  "status": "draft",
  "stages_hint": [
    {"title": "Phase 1", "scope": "MVP（完了済み。01M3MFS5T52FXA63W4V10XGC4S を採用）"},
    {"title": "Phase 2", "scope": "task policy・手動登録の credential broker・人間承認・durable wait（完了済み。01M3MZKB3DFYJNBH015MJGQ0BT を採用）"},
    {"title": "Phase 3", "scope": "Browser Identity・live proxy・takeover（P3-A〜C）"},
    {"title": "Phase 4", "scope": "isolated runtime・stronger injection・backend routing（P4-A〜C）"}
  ]
}
EOF
curl -s -X POST -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' --data-binary @browser-root.json "$API/tasks" > browser-root.out.json
export BROWSER_ROOT="$(python3 -c 'import json; print(json.load(open("browser-root.out.json"))["id"])')"
echo "BROWSER_ROOT=$BROWSER_ROOT"
```

### 2.2 /3 の計画（段階 phase-1〜4、p1 / p2 は採用、p3 / p4 は子 task、決定 h4 / h5 / h7、phase-3 の後に人の確認）

h1 / h2 / h3 / h6 は Phase 2 の入力で、ADR-0080 に答えが記録済み（H1 = 「credential は手動登録から開始する」、H2 = 操作ごとの
`approve_once`・lease は既定 60 秒 / 上限 300 秒・`max_uses = 1`、H3 = credential を注入した session の終わりまで snapshot・
console・Live View 等を止める、H6 = Phase 2 は trusted local・container / 別 UID / egress は Phase 4）。決定にはせず、p3 / p4 の目的に
写した。h4（P3-B 前）と h5（P3-A 前）は p3 の前、h7（P4-C 前）は p4 の前の決定にする。

```bash
cat > browser-plan.json <<'EOF'
{
  "schema": "celeris.execution-plan/3",
  "rationale": "製品の Phase 1〜4 を root の段階にする（ADR-0079 §7 R5b）。Phase 1・2 は main に取り込み済みの既存の task を採用し（統合は既に基点に入っているので skipped）、Phase 3・4 はそれぞれ独立に受け入れ・レビューされる子 task にする。子の中の P3-A〜C / P4-A〜C は子の planner が leaf に分ける。Phase 3 の後で人が確認する。",
  "stages": [
    {"key": "phase-1", "kind": "implement", "title": "Phase 1 MVP"},
    {"key": "phase-2", "kind": "implement", "title": "Phase 2 policy・credential・人間承認"},
    {"key": "phase-3", "kind": "implement", "title": "Phase 3 identity・live proxy・takeover", "review": "human"},
    {"key": "phase-4", "kind": "implement", "title": "Phase 4 isolated runtime・注入・backend routing"}
  ],
  "units": [
    {"key": "p1", "stage": "phase-1", "kind": "task", "title": "Phase 1 MVP（採用: 01M3MFS5T52FXA63W4V10XGC4S）",
     "objective": "browser capability の Phase 1 MVP。既存の task 01M3MFS5T52FXA63W4V10XGC4S（done、成果は main 09a6b0c に取り込み済み）を採用する。",
     "acceptance": [{"text": "採用した task が done で、成果が main に入っている", "check": {"type": "reviewer"}}],
     "adopt": "01M3MFS5T52FXA63W4V10XGC4S"},
    {"key": "p2", "stage": "phase-2", "kind": "task", "title": "Phase 2 policy・credential・人間承認（採用: 01M3MZKB3DFYJNBH015MJGQ0BT）",
     "objective": "browser capability の Phase 2（P2-A policy 契約・P2-B broker / provider 1 種・P2-C durable wait。ADR-0080）。既存の task 01M3MZKB3DFYJNBH015MJGQ0BT（review_fail の後に人が main ce5d768 に取り込んだ）を採用する。",
     "acceptance": [{"text": "採用した task の成果が main に入っている", "check": {"type": "reviewer"}}],
     "depends_on": ["p1"],
     "adopt": "01M3MZKB3DFYJNBH015MJGQ0BT"},
    {"key": "p3", "stage": "phase-3", "kind": "task", "title": "Phase 3: Browser Identity・live proxy・takeover",
     "objective": "ADR-0078 / ADR-0080 の browser capability の Phase 3 を実装する。phases-and-decisions.md（01M3MFS5T52FXA63W4V10XGC4S の WU design-gap）の表の行をそのまま写す（ID | 先行依存 | 単独の成果・検査 | 現在の扱い）:\n- P3-A Browser Identity | P2-C、H5、必要なP4-A | project/origin単位の暗号化・期限/削除/失効、他identity混入拒否 | 未実装。0.38.1のallowlistとrestore非互換を解決する境界が先。解除だけで対応しない\n- P3-B live proxy | P1-C、H4 | task/run ACL、WS接続/再接続、frame/status/tabs/url/consoleと永続event、他task拒否、cookie/token非記録 | 未実装。認証task表示にはP2-Cのobservation制御も必要\n- P3-C takeover | P2-C、P3-B | pause収束・controller lease排他・takeover/resume/stop、切断・競合・二重action試験 | 未実装\nPhase の数字だけで直列化しない。P3-A が P4-A の隔離を要するなら契約を先に決め、利用は P4 の後にする（循環を作らない）。Phase 2 の決定（ADR-0080）: H1 credential は手動登録から開始、H2 操作ごとの approve_once・lease 既定 60 秒 / 上限 300 秒・max_uses 1、H3 credential を注入した session の終わりまで LLM の観測（snapshot・console・Live View・artifact）を止める、H6 Phase 2 は trusted local（container / 別 UID / egress は Phase 4）。Live View は本人の session だけ（ADR-0080 D6）。H4・H5 は人の決定（下に注入される）に従う。",
     "acceptance": [
       {"text": "P3-A〜C の各行の「単独の成果・検査」が満たされている。満たせない行は理由と後続（Phase 4 など）が明記されている", "check": {"type": "reviewer"}},
       {"text": "H4・H5 の回答と ADR-0080 の決定（観測の停止・本人限定の Live View・短い lease）に反する実装が無い", "check": {"type": "reviewer"}}
     ],
     "depends_on": ["p2"]},
    {"key": "p4", "stage": "phase-4", "kind": "task", "title": "Phase 4: isolated runtime・stronger injection・backend routing",
     "objective": "ADR-0078 / ADR-0080 の browser capability の Phase 4 を実装する。phases-and-decisions.md（01M3MFS5T52FXA63W4V10XGC4S の WU design-gap）の表の行をそのまま写す（ID | 先行依存 | 単独の成果・検査 | 現在の扱い）:\n- P4-A isolated runtime | P2-Aの契約確定、H6 | 別UID/namespace/read-only、broker/CDP/IPC分離、proxy/DNS/IPv6/private IP等egress負例、orphan回収 | 未実装。機密auth前へ前倒し。P2-Aの分離enforcementがP4-A依存でも契約を先に決め循環を避ける\n- P4-B stronger injection | P2-Bのinterface確定、P4-A、H3 | trusted injection/extension接点、TOCTOU/redirect/iframe/DOM再表示攻撃、workerからの取得拒否 | 未実装。provider契約はP2-B、利用開始は検証完了後。P2-Bとの循環依存を作らない\n- P4-C backend routing | P1-D、H7、候補のP0再調査 | backend適合fixture、browser-specialist、既存loop再利用、能力を失わないfallback、同一task評価 | 未実装。機密機能を宣言するbackendはP4-A/B相当の適合も必要\nPhase 2 の決定（ADR-0080）: H3 認証 session の間は LLM の観測を止める、H6 container / 別 UID / egress の隔離はこの Phase の責務（peer UID と file mode だけで同一 UID の worker を隔離したと主張しない。ADR-0080 D7）。separate_origins（ADR-0080 D1）は P4 の enforcement が成立してから有効化する。H7 は人の決定（下に注入される）に従う。",
     "acceptance": [
       {"text": "P4-A〜C の各行の「単独の成果・検査」が満たされている。満たせない行は理由と後続が明記されている", "check": {"type": "reviewer"}},
       {"text": "egress / 隔離の負例と、H7 の回答どおりの backend の選び方がある", "check": {"type": "reviewer"}}
     ],
     "depends_on": ["p3"]}
  ],
  "decisions": [
    {"key": "h4", "question": "H4 dashboard / Live View の公開範囲（P3-B live proxy の前）",
     "options": [
       {"key": "operator-only", "label": "operator 専用の既存画面のまま", "consequence": "複数利用者には開けない"},
       {"key": "task-acl-proxy", "label": "task 別 ACL の proxy を先行する", "consequence": "proxy の導入・認証導線の変更が要る"},
       {"key": "shared-namespace", "label": "共有 namespace を一般利用者へ開放する", "consequence": "全 session の閲覧履歴は取り消せない（推奨しない）"}
     ],
     "recommended": "task-acl-proxy", "cost_of_reversal": "medium",
     "cost_note": "proxy 導入・認証導線変更・cookie/token 再発行。既存全 session の閲覧履歴は取り消せない。Phase 2 の既定（ADR-0080 D6: Live View は本人の session だけ）は維持する",
     "needed_before": ["p3"]},
    {"key": "h5", "question": "H5 persistent auth（P3-A Browser Identity の前）",
     "options": [
       {"key": "per-run-isolation", "label": "毎 run 隔離のまま（identity を持たない）", "consequence": "P3-A は境界の設計だけになる"},
       {"key": "project-origin-identity", "label": "需要が確認された project+origin に限り期限付き identity", "consequence": "暗号鍵・保存 state・失効・削除の運用が要る"},
       {"key": "personal-profile", "label": "個人 Chrome profile を共有する", "consequence": "推奨しない（混入と漏洩の範囲が広い）"}
     ],
     "recommended": "project-origin-identity", "cost_of_reversal": "high",
     "cost_note": "暗号鍵・保存 state・サイト cookie の失効・削除の移行が要る。agent-browser 0.38.1 の allowlist / restore の非互換を先に解決する",
     "needed_before": ["p3"]},
    {"key": "h7", "question": "H7 長時間 profile / backend（P4-C backend routing の前）",
     "options": [
       {"key": "keep-acp-claude", "label": "ACP 継続と Claude 明示を維持し、専用 backend は同一 fixture で比較してから", "consequence": "専用 backend は比較の結果が出るまで入らない"},
       {"key": "specialist", "label": "browser-specialist + Browser Use 等を導入する", "consequence": "adapter・監視・実行予算・conformance suite が要る。Browser Use の仕様は未確認"}
     ],
     "recommended": "keep-acp-claude", "cost_of_reversal": "medium",
     "cost_note": "adapter・監視・実行予算・conformance suite が要る。task/run/event の共通化で UI の移行を抑える",
     "needed_before": ["p4"]}
  ]
}
EOF
curl -s -X PUT -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' --data-binary @browser-plan.json "$API/tasks/$BROWSER_ROOT/execution-plan" > browser-plan.out.json
python3 -c 'import json; d=json.load(open("browser-plan.out.json")); print(d.get("status"), d.get("detail","")); print("adoptions:", [(a["unit_key"], a["adopted"], a["unit_status"]) for a in d.get("adoptions", [])]); print("decisions_raised:", d.get("decisions_raised")); print([(u["key"], u["status"], u.get("blocked_reason")) for u in d.get("work_units", [])])'
```

期待: `adoptions: [('p1', True, 'done'), ('p2', True, 'done')]`、`decisions_raised: 3`、work_units は `p1 done`・`p2 done`・`p3 pending`・
`p4 pending` と統合 WU 4 つ。422 / 409 が返ったら何も書かれていない（`detail` を読んで直す。`adopt_*` の code は付記 4.）。

### 2.3 受け入れて走らせる

```bash
curl -s -X POST -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' -d '{}' "$API/tasks/$BROWSER_ROOT/accept"; echo
```

root は `ready` になり、次の tick で phase-1 / phase-2 の統合（採用した task のブランチは main の祖先なので `skipped`）を行う。
phase-3 の p3 は h4 / h5 の回答を待つ（計画の承認は挟まない = 人の計画。付記 3.）。

### 2.4 決定に答える（受信箱の「決定」か API）

```bash
curl -s -H "Authorization: Bearer $TOKEN" "$API/tasks/$BROWSER_ROOT/decisions" > browser-decisions.json
python3 -c 'import json; d=json.load(open("browser-decisions.json")); [print(x["decision"]["id"], x["decision"]["key"], x["decision"]["status"], "recommended=" + x["decision"]["recommended"]) for x in d["items"]]'
# 例: h4 に推奨どおり答える（<h4 の id> を上の出力の id に置き換える）
# curl -s -X POST -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' -d '{"option":"task-acl-proxy","note":"R5b: 推奨どおり"}' "$API/decisions/<h4 の id>/answer"
```

## 3. BenchFS の root task を作り、/3 の計画を `PUT` する

採用するのは `kind = plan` の分解 task 01M35X04345ZNDM09VE6FT168Z（done）の子のうち **done の Phase0 / Phase1 の 6 件**（`parent_id` は書き換えない）:

```bash
sqlite3 "$RODB" "select id, status, title from tasks where project_id='01M35WRV77A2JPYGERQGXF6V7K' and parent_id='01M35X04345ZNDM09VE6FT168Z' and status='done' order by created_at, id;"
```

| unit | task | 題名 |
|---|---|---|
| phase0-inventory | 01M35X86XT4T3CSNMB8RNQ9WG9 | Phase0: 既存BenchFS成果の棚卸し |
| phase0-topology | 01M35X86XTHHN9C6XDAYD2FZ7T | Phase0: Sirius実機トポロジ監査とhardware upper bound候補の確定 |
| phase0-validity | 01M35X86XTEPXVZMBY7HSSEP7X | Phase0: Siriusノード離脱症状の調査とvalidity policy策定 |
| phase1-related-work | 01M37JKZ4Q97KYPY70WQ30DRMD | Phase1: Related work調査 |
| phase1-oss | 01M3768090QQQMQEFK2JK6SCNB | Phase1: 比較対象OSSの実装・推奨設定・deployment model調査(Web) |
| phase1-framing | 01M35X86XTK84F97QW0CN5PGMR | Phase1: framing/contribution候補案の作成(HUMAN GATE 1向け決定材料) |

決定 `framing` の選択肢は framing 候補の成果物（知識ベース `projects/benchfs/framing-candidates.md` = `~/.local/share/celeris/knowledge/projects/benchfs/framing-candidates.md`
の「まず判断すること」の表、案 A / B / C）から写した。**成果物は順位付けをしていない**（「本書は順位付けも実験投入の承認代行もしない」）。
計画の検証は `recommended` を要するので、表の「後戻りコスト」が最も小さい案 A（中。否定されても分解の結果を B / C で再利用できる）を
推奨として置き、そのことを `cost_note` に書いた。人は推奨と違う案・保留を選べる。

```bash
cat > benchfs-root.json <<'EOF'
{
  "title": "BenchFS 国際会議フルペーパー化",
  "objective": "BenchFS（Rust の ad hoc file system）を国際会議のフルペーパーにする。既存の Phase0（成果の棚卸し・Sirius の実機トポロジと hardware upper bound・ノード離脱の validity policy）と Phase1（related work・比較対象 OSS の調査・framing / contribution 候補）の done の task を採用し、HUMAN GATE 1（framing の選択。知識ベース projects/benchfs/framing-candidates.md の案 A / B / C）を決定の要求にする。選んだ framing に沿って、実験計画 → 実装 → 実験 → 執筆を子 task で進める。",
  "acceptance": [
    {"type": "reviewer", "text": "決定 framing の回答どおりの RQ と contribution（最大 3 件）で、実験計画・実装・実験・執筆の子 task が done になっている"},
    {"type": "reviewer", "text": "論文の原稿が、実験の原票と条件（意味論・資源予算・比較対象の設定）を出典付きで引いている"}
  ],
  "project_id": "01M35WRV77A2JPYGERQGXF6V7K",
  "status": "draft",
  "stages_hint": [
    {"title": "棚卸しと関連研究", "scope": "Phase0 / Phase1 の done の task を採用"},
    {"title": "framing の選択", "scope": "HUMAN GATE 1（案 A / B / C / 保留）"},
    {"title": "実験計画", "scope": "選んだ framing の RQ に対する実験計画"},
    {"title": "実装", "scope": "計測点・配置制御など実験に要る実装"},
    {"title": "実験", "scope": "Sirius での実験と原票"},
    {"title": "執筆", "scope": "フルペーパーの原稿"}
  ]
}
EOF
curl -s -X POST -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' --data-binary @benchfs-root.json "$API/tasks" > benchfs-root.out.json
export BENCHFS_ROOT="$(python3 -c 'import json; print(json.load(open("benchfs-root.out.json"))["id"])')"
echo "BENCHFS_ROOT=$BENCHFS_ROOT"
```

段階は 5（`max_stages` 5）。「framing の選択」は段階にせず、決定 `framing` を後の 4 つの子 task の前に置く（`needed_before` に 4 つ。
回答はそれぞれの子の目的に固定の書式で入る）。採用の 6 件は子 task の上限（6）に数えない（付記 8.）。

```bash
cat > benchfs-plan.json <<'EOF'
{
  "schema": "celeris.execution-plan/3",
  "rationale": "既存の Phase0 / Phase1 の done の task を最初の段階に採用し、HUMAN GATE 1（framing）を決定の要求にする。以後の段階（実験計画 → 実装 → 実験 → 執筆）は framing の回答に依存する子 task（ADR-0079 §7 R5b 3.）。",
  "stages": [
    {"key": "inventory", "kind": "investigate", "title": "棚卸しと関連研究（Phase0 / Phase1 の採用）"},
    {"key": "experiment-plan", "kind": "design", "title": "実験計画"},
    {"key": "implementation", "kind": "implement", "title": "実装"},
    {"key": "experiments", "kind": "implement", "title": "実験"},
    {"key": "writing", "kind": "design", "title": "執筆"}
  ],
  "units": [
    {"key": "phase0-inventory", "stage": "inventory", "kind": "task", "title": "Phase0: 既存BenchFS成果の棚卸し（採用）", "objective": "既存の done の task 01M35X86XT4T3CSNMB8RNQ9WG9 を採用する。", "acceptance": [{"text": "採用した task が done", "check": {"type": "reviewer"}}], "adopt": "01M35X86XT4T3CSNMB8RNQ9WG9"},
    {"key": "phase0-topology", "stage": "inventory", "kind": "task", "title": "Phase0: Sirius 実機トポロジ監査と hardware upper bound（採用）", "objective": "既存の done の task 01M35X86XTHHN9C6XDAYD2FZ7T を採用する。", "acceptance": [{"text": "採用した task が done", "check": {"type": "reviewer"}}], "adopt": "01M35X86XTHHN9C6XDAYD2FZ7T"},
    {"key": "phase0-validity", "stage": "inventory", "kind": "task", "title": "Phase0: Sirius ノード離脱の調査と validity policy（採用）", "objective": "既存の done の task 01M35X86XTEPXVZMBY7HSSEP7X を採用する。", "acceptance": [{"text": "採用した task が done", "check": {"type": "reviewer"}}], "adopt": "01M35X86XTEPXVZMBY7HSSEP7X"},
    {"key": "phase1-related-work", "stage": "inventory", "kind": "task", "title": "Phase1: Related work 調査（採用）", "objective": "既存の done の task 01M37JKZ4Q97KYPY70WQ30DRMD を採用する。", "acceptance": [{"text": "採用した task が done", "check": {"type": "reviewer"}}], "adopt": "01M37JKZ4Q97KYPY70WQ30DRMD"},
    {"key": "phase1-oss", "stage": "inventory", "kind": "task", "title": "Phase1: 比較対象 OSS の調査（採用）", "objective": "既存の done の task 01M3768090QQQMQEFK2JK6SCNB を採用する。", "acceptance": [{"text": "採用した task が done", "check": {"type": "reviewer"}}], "adopt": "01M3768090QQQMQEFK2JK6SCNB"},
    {"key": "phase1-framing", "stage": "inventory", "kind": "task", "title": "Phase1: framing / contribution 候補（採用）", "objective": "既存の done の task 01M35X86XTK84F97QW0CN5PGMR（HUMAN GATE 1 の決定材料。知識ベース projects/benchfs/framing-candidates.md）を採用する。", "acceptance": [{"text": "採用した task が done", "check": {"type": "reviewer"}}], "adopt": "01M35X86XTK84F97QW0CN5PGMR"},
    {"key": "bf-plan", "stage": "experiment-plan", "kind": "task", "title": "実験計画", "objective": "決定 framing で選ばれた案の RQ と contribution（最大 3 件）について、実験計画（workload・意味論・比較対象と推奨構成・CPU / 資源予算・計測点・反復回数・撤回 / 縮小の条件）を書く。framing-candidates.md の「全案に共通する比較・測定条件」と「数値の再現・監査入口」を前提にする。", "acceptance": [{"text": "実験計画が選んだ RQ の各 contribution に対応し、比較の公平条件と撤回条件がある", "check": {"type": "reviewer"}}]},
    {"key": "bf-impl", "stage": "implementation", "kind": "task", "title": "実験に要る実装", "objective": "実験計画に要る BenchFS 側の実装（計測点・切替・配置制御など、計画が名指ししたもの）を行う。", "acceptance": [{"text": "実験計画が名指しした実装が入り、既存のテストが通る", "check": {"type": "reviewer"}}], "depends_on": ["bf-plan"]},
    {"key": "bf-exp", "stage": "experiments", "kind": "task", "title": "実験", "objective": "Sirius で実験計画どおりに実験し、原票（生データ・条件・commit）を残す。", "acceptance": [{"text": "実験計画の各実験に原票と条件の記録がある。未実施の実験は理由が明記されている", "check": {"type": "reviewer"}}], "depends_on": ["bf-impl"]},
    {"key": "bf-write", "stage": "writing", "kind": "task", "title": "執筆", "objective": "選んだ framing でフルペーパーの原稿を書く（原票と条件を出典付きで引く）。", "acceptance": [{"text": "原稿が RQ・contribution・評価・関連研究を持ち、数値が原票に辿れる", "check": {"type": "reviewer"}}], "depends_on": ["bf-exp"]}
  ],
  "decisions": [
    {"key": "framing", "question": "HUMAN GATE 1: 論文の主案（framing）をどれにするか（projects/benchfs/framing-candidates.md）",
     "options": [
       {"key": "a", "label": "案 A: RPC とストレージをまたぐ非同期実行の因果分解", "consequence": "既存の切替実装と 4-way ablation を使える。後戻り: 中"},
       {"key": "b", "label": "案 B: NUMA 局所性と計算資源予算から配置原則を示す", "consequence": "配置制御・コア会計・干渉測定が要る。後戻り: 高"},
       {"key": "c", "label": "案 C: 意味論と資源境界を揃えた性能説明", "consequence": "OSS 導入・設定検証の比重が大きい。後戻り: 中〜高"},
       {"key": "hold", "label": "保留（追加の証拠を先に集める）", "consequence": "後の段階は回答まで始まらない"}
     ],
     "recommended": "a", "cost_of_reversal": "medium",
     "cost_note": "成果物は順位付けをしていない。推奨 a は表の後戻りコストが最小（中、否定されても分解結果を B / C で再利用できる）ことだけに基づく手順書の置き値。人は RQ・contribution・許容する追加実験を note に書く",
     "needed_before": ["bf-plan", "bf-impl", "bf-exp", "bf-write"]}
  ]
}
EOF
curl -s -X PUT -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' --data-binary @benchfs-plan.json "$API/tasks/$BENCHFS_ROOT/execution-plan" > benchfs-plan.out.json
python3 -c 'import json; d=json.load(open("benchfs-plan.out.json")); print(d.get("status"), d.get("detail","")); print("adoptions:", [(a["unit_key"], a["adopted"]) for a in d.get("adoptions", [])]); print("decisions_raised:", d.get("decisions_raised"))'
curl -s -X POST -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' -d '{}' "$API/tasks/$BENCHFS_ROOT/accept"; echo
```

期待: 6 件とも `adopted: True`、`decisions_raised: 1`。BenchFS の案件のリポジトリは sirius の remote（`/work/NBB/rmaeda/workspace/rust/benchfs`）
なので、root は並列 1 に倒れ、統合は merge せずに進む（R1c 付記 3.）。子の担当・作業場所は matching と ADR-0062 D5 の規則のまま
（`cluster:sirius` を持つ部署に当たらなければ local に落ちる。子が作られたら `GET /tasks/<子>` の `workspace` を確かめる）。

## 4. dogfood: browser の phase-3 の子を 1 本 done まで通す

h4 / h5 に答えると p3 の子 task（`labels: child-p3`、深さ 2）が作られ、子自身の gate（深さ 2 の閾値 7。木の子は shadow でも採用）→
compound なら子の planner が P3-A〜C を leaf に分ける → leaf → 子の最終レビュー（親のブランチと比べる）→ 親の `integrate-phase-3` が
`celeris/<子>` を `celeris/$BROWSER_ROOT` に merge → phase-3 は `review: human` なので途中確認で止まる（`POST /tasks/$BROWSER_ROOT/execution/phase-gate`
`{"action":"continue"}` で phase-4 へ）。Phase 3 は大きい（3 つの小タスク）ので、leaf に収まらない単位は `leaf_too_large` の決定に
なる（max_depth 3 なので孫 task も作れる）。途中で木の上限の決定（`kind: limit`）が来たら受信箱で答える（採用した Phase 1 / 2 の run
30 件・leaf 14 件・replan 4 件は最初から木の数に入っている。付記 13.）。

```bash
sqlite3 "$RODB" "select key, status, blocked_reason, child_task_id from work_units where task_id='$BROWSER_ROOT' order by seq;"
export P3_CHILD="$(sqlite3 "$RODB" "select child_task_id from work_units where task_id='$BROWSER_ROOT' and key='p3';")"
echo "P3_CHILD=$P3_CHILD"
sqlite3 "$RODB" "select id, status, root_id, json_extract(json,'$.tree.depth'), title from tasks where root_id='$BROWSER_ROOT' order by created_at;"
```

## 5. 受け入れ条件の確かめ方（ADR-0079 §7 R5b (a)〜(e)）

### (a) 4 段階、phase-1 は採用で done・統合は skip、未回答の決定が受信箱と Discord に path 付き

```bash
curl -s "$API/tasks/$BROWSER_ROOT/task-tree?root=true" > browser-tree.json
python3 -c 'import json; t=json.load(open("browser-tree.json")); r=t["nodes"][0]; print("tree_enabled", t["tree_enabled"]); print("stages", sorted({u["stage"] for u in r["units"] if u.get("stage")})); print([(u["key"], u["status"], u.get("child_task_id")) for u in r["units"]]); print("open_decisions", r["open_decisions"])'
sqlite3 "$RODB" "select json_extract(json,'$.phase'), json_extract(json,'$.merged') from events where task_id='$BROWSER_ROOT' and json_extract(json,'$.type')='phase_integrated' order by seq;"
sqlite3 "$RODB" "select seq, json from events where task_id='$BROWSER_ROOT' and json_extract(json,'$.type')='child_adopted' order by seq;"
sqlite3 "$RODB" "select key, kind, status, json_extract(json,'$.raised_by.origin'), json_extract(json,'$.path') from decisions where root_id='$BROWSER_ROOT';"
curl -s -H "Authorization: Bearer $TOKEN" "$API/inbox" | python3 -c 'import json,sys; d=json.load(sys.stdin); print([(x["key"], x["recommended"], " › ".join(p["title"] for p in x["path"])) for x in d.get("decisions", [])])'
sqlite3 "$RODB" "select kind, key, sent_at, ok, substr(body,1,200) from notifications where kind='decision_requested' order by created_at desc limit 5;"
```

期待: 段階 `phase-1`〜`phase-4`、`p1` / `p2` が `done` で `child_task_id` = 採用した id、phase-1 / phase-2 の `merged` に
`{"key":"p1",…,"skipped":true}` / `{"key":"p2",…,"skipped":true}`、`child_adopted` が 2 件、決定 h4 / h5 / h7 が `open`・origin `human`・
path の先頭が root、通知 `plan:<plan_id>:decisions` が 1 通（本文に『browser capability（Phase 1〜4）』とパンくず・推奨）。

### (b) phase-3 の子が親ブランチに取り込まれ、子の deliveries が無く、main は動いていない

```bash
sqlite3 "$RODB" "select json_extract(json,'$.merged') from events where task_id='$BROWSER_ROOT' and json_extract(json,'$.type')='phase_integrated' and json_extract(json,'$.phase')='phase-3';"
sqlite3 "$RODB" "select count(*) from deliveries where task_id in (select id from tasks where root_id='$BROWSER_ROOT');"
git -C ~/workspace/agent-platform merge-base --is-ancestor "celeris/$P3_CHILD" "celeris/$BROWSER_ROOT" && echo "p3 child in the root branch"
git -C ~/workspace/agent-platform merge-base --is-ancestor "celeris/$P3_CHILD" main || echo "p3 child NOT in main (expected until the root is delivered)"
```

期待: phase-3 の `merged` に `p3`（`skipped: false`、commit = 子のブランチの HEAD）、deliveries 0、子は root のブランチにあり main には無い。

### (c) roll-up の run・quota・壁時計が子の値の和と一致

```bash
python3 -c 'import json; t=json.load(open("browser-tree.json")); tot=t["totals"]; own=[n["own"] for n in t["nodes"]]; print("totals.runs", tot["runs"], "sum(own.runs)", sum(o["runs"] for o in own)); print("totals.reviewer_runs", tot["reviewer_runs"], "sum", sum(o["reviewer_runs"] for o in own)); print("totals.tokens", tot["tokens"], "sum", sum(o["tokens"] for o in own)); print("wall", tot.get("first_run_started_at"), tot.get("last_run_finished_at"))'
sqlite3 "$RODB" "select role, count(*), min(started_at), max(finished_at) from runs where task_id in (select id from tasks where id='$BROWSER_ROOT' or root_id='$BROWSER_ROOT') group by role;"
```

期待: `totals.runs` = SQL の reviewer 以外の件数の和、`reviewer_runs` = reviewer の件数、壁時計 = SQL の最小の開始・最大の終わり
（採用した Phase 1 / 2 の run も木に入るので同じ集合で比べる。付記 13.）。quota は `totals.quota` を `GET /metrics/execution?group_by=depth`
の深さごとの `rollup.quota` の和と比べる（`curl -s "$API/metrics/execution?group_by=depth"`）。

### (d) 人に届いたものの内訳と、基盤の失敗が質問として届かなかったこと

```bash
export T0="$(python3 -c 'import json; print(json.load(open("browser-root.out.json"))["created_at"])')"
sqlite3 "$RODB" "select kind, count(*) from notifications where created_at >= '$T0' group by kind;"
sqlite3 "$RODB" "select t.id, json_extract(e.json,'$.type'), substr(e.json,1,200) from events e join tasks t on t.id = e.task_id where (t.id='$BROWSER_ROOT' or t.root_id='$BROWSER_ROOT') and json_extract(e.json,'$.type') in ('question_raised','stall_detected','plan_approval_requested');"
sqlite3 "$RODB" "select t.id, json_extract(e.json,'$.to'), json_extract(e.json,'$.reason') from events e join tasks t on t.id = e.task_id where (t.id='$BROWSER_ROOT' or t.root_id='$BROWSER_ROOT') and json_extract(e.json,'$.type')='transitioned' and json_extract(e.json,'$.reason') in ('infra_requeue','requeue','lease_expired','worker_question');"
```

期待: 通知は `decision_requested`（決定）と、あれば `phase_checkpoint`（phase-3 の後の確認）・`task_failed`（`tree-infra:` / `tree-stall:` の障害）
だけ。`plan_approval_requested` は 0（人の計画）。基盤の失敗（`infra_requeue` / `requeue` / `lease_expired`）はあっても `question_raised` /
`worker_question` として人に来ていない。

### (e) BenchFS の root の決定 `framing` が受信箱に出ている

```bash
sqlite3 "$RODB" "select key, kind, status, json_extract(json,'$.recommended') from decisions where root_id='$BENCHFS_ROOT';"
curl -s -H "Authorization: Bearer $TOKEN" "$API/inbox" | python3 -c 'import json,sys; d=json.load(sys.stdin); print([(x["key"], [p["title"] for p in x.get("path", [])]) for x in d.get("decisions", []) if x["key"] == "framing"])'
sqlite3 "$RODB" "select key, status, child_task_id from work_units where task_id='$BENCHFS_ROOT' order by seq;"
```

期待: `framing` が `open`、受信箱の「決定」に『BenchFS 国際会議フルペーパー化』のパンくずで出ている、inventory の 6 unit が `done`。

### ADR の手順 5: 途中目標と案件計画が見えないこと

```bash
curl -s -o /dev/null -w '%{http_code}\n' -X POST -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' -d '{"mode":"milestones"}' "$API/projects/$AP_PROJECT/plan"
curl -s "$API/projects/$AP_PROJECT" | python3 -c 'import json,sys; d=json.load(sys.stdin); print("milestones", len(d["milestones"]), "frozen", d.get("milestones_frozen"))'
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:7700/plans/new
```

期待: 410、`milestones 0 frozen 25`（既定で隠れる）、GUI の `/plans/new` は 404（R5b-prep で撤去。GUI の port は本番の設定どおり）。

## 6. 巻き戻し

1. 走っている木を止める（subtree に連鎖。採用した task は終端なので状態は変わらない）:

   ```bash
   curl -s -X POST -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' -d '{}' "$API/tasks/$BROWSER_ROOT/cancel"; echo
   curl -s -X POST -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' -d '{}' "$API/tasks/$BENCHFS_ROOT/cancel"; echo
   ```

2. 設定を戻して再起動する（in-flight 0 を 1. の手順で確かめてから）:

   ```bash
   cp ~/.config/celeris/config.toml.bak-20260929-r5b ~/.config/celeris/config.toml
   systemctl --user restart celeris@$SHA
   curl -s "$API/tasks/01M3MFS5T52FXA63W4V10XGC4S/task-tree" | python3 -c 'import json,sys; print("tree_enabled =", json.load(sys.stdin)["tree_enabled"])'
   ```

3. 残るもの（戻さない。events が正本）: 採用した task の `tree` と、`parent_id` が無かった browser の 2 件の `parent_id = $BROWSER_ROOT`
   （`Event::Edited{fields: ["tree", "parent_id"]}`）。これらは案件ページの root task の一覧から外れたまま、木が無効でも読み取りは
   できる（`GET /tasks/{id}/task-tree`）。書き戻しが要るなら R6（回収）で入口を作る（DB を手で書かない）。R5b-prep のリリース自体を
   戻すときは ADR-0040 の `promote.sh <前の sha12>`（schema 33 のままなので migration の巻き戻しは無い）。
