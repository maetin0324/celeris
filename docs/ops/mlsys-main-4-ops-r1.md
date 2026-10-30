# main-4-ops-r1: CoS を含む通常運用の評価契約

---
tasks: [01M5XXF7QDVASQFQBT1GZBXS6V]
---

revision: `main-4-ops-r1`。2026-10-27 採用。設計判断は [ADR](../../agent-docs/adr/2026-10-27-main4-ops-evaluation-scope.md)。
これは評価条件の定義であり、計測コードの実装完了・測定開始・本番操作の許可を意味しない。

## 1. 評価対象と境界

利用者の 2026-10-27 の決定により、**Celeris の構成要素は daemon・案件 CoS・統括 CoS・方針・model** とする。
daemon が管理する task 実行系（planner・worker・reviewer）も対象に含む。
外部 operator（人が外で起動する Claude Code 等）は構成要素に含めず、Celeris を活用する実用上の補助とする。
その判断・操作・消費は **「外部 AI による支援」**として別に記録・報告する。境界外であることは記録からの除外を意味しない。
同じ Claude Code でも Celeris が起動した worker と外部 session は、起動元・役割・session の記録で区別する。

[main-4 A0/B0](mlsys-main-4-ab-contract.md) は task 実行系の凍結済み比較であり、本 revision とは別系列。
`mlsys-main-4-v4`・`final-freeze-2` と既存 pilot の bundle/manifest/lock/台帳は読み取り専用の既存証跡として保全する。
過去の試行を本 revision に改名・再分類して通常運用の標本にしない。

通常運用では固定した依頼を案件 CoS（必要な案件横断調整は統括 CoS）に渡し、理解・起票・実行・受信箱対応・報告を追う。
試行の開始は依頼の受付、終了は固定評価器による成果判定と報告の確認、または事前固定した cutoff とする。
受付・task 作成・成果確定・報告・cutoff の時刻を分けて残す。既存 campaign の task API 直接投入でこの経路を代用しない。
CoS の自己申告や reviewer 合格だけを最終成功にせず、固定した独立評価器で判定する。

## 2. 試行開始前に固定する票

本 revision の `measurement_id` は既存 A0/B0 と異なる新しい ID を採番し、`trial_id`・`condition_id` と併記する。
以下は固定票の項目であり、既存 campaign manifest に未対応の field を追加する指示ではない。
固定票と証拠を別の測定 bundle に保存し、内容 digest と保存先を記録する。空欄を推測で埋めず、開始前の未解決条件とする。

| 固定項目 | 必須の記録 |
|---|---|
| Celeris | 昇格済み release の識別・binary digest、実際の稼働版、daemon config、worker/harness/CLI の版 |
| model | 案件 CoS・統括 CoS・planner・worker・reviewer の実 provider/model ID、routing・fallback・更新方針。operator の model/harness/版も境界外の支援条件として固定 |
| 方針 | CoS の prompt/skill/必須規則・認可・停止/再試行方針の版と digest。operator は `external-operator/1` 等の版、許容作業・起動/停止条件・人への引き継ぎ条件 |
| 入力・初期状態 | 依頼本文、公開材料/KB、org/profile/memory、案件 brief、CoS 会話・要約・session 再開状態の snapshot。評価器の非公開入力は分離 |
| 環境・予算 | toolchain・cluster/資源条件、account の種類/共有の有無、並列数・順序、token/wall 上限・cutoff、試行数と承認の出典 |
| 評価器・集計 | 評価器の版/digest、成功条件、計画した分母、欠測規則、再投入の扱い、単価表版、raw schema、collect/export/report の版 |
| 支援記録 | 記録者、operator session ID、principal、判断/実行/承認者、対象 trial/request の対応表、記録の開始/終了と coverage |

model の alias だけでは内部の版が特定できない場合、その制約と観測した ID を記す。
試行間に学習した記憶や CoS の会話を暗黙に持ち越さない。初期 snapshot に戻すか、持ち越しを別条件として固定する。
外部 operator に既知の解答や非公開評価器を渡さない。情報漏洩があれば条件逸脱として残す。
非公開評価器の入力が試行の入力と可読 path に無いことは、試行の前後に [漏洩検査](ops-r1-leak-check.md) で確かめる。

## 3. 自己改善・配備・条件変更

固定条件の比較では、試行開始後の release・model・方針・入力・評価器・設定・初期状態の変更を同じ条件として扱わない。
自己改善でコードを作っただけなら未配備の成果として記録し、実際の配備/設定反映時刻を条件の境界にする。

変更時は旧/新版、時刻、理由、判断/実行/承認者、task/release/operation の参照、影響する trial を追記する。
新しい `condition_id` と測定 bundle を発行し、新条件の試行を別 ID で開始する。設計そのものを変える場合は
`main-4-ops-r2` 等の新 revision にする。旧 raw・失敗・中断・消費・支援記録は消さない。
途中変更のあった試行は「条件混在」として別掲し、旧条件の計画分母から落とさず固定条件の成功数には入れない。
変更後の成功で変更前の失敗を置換せず、再投入回数と通算の観測消費を別掲する。

自己改善を含む長期運用を観察する場合は版の時系列を持つ観察結果として報告し、固定 release の比較と混ぜない。
条件が同一か確認できない場合も未確認として分離する。変更に起因する operator の支援は §4 の記録を伴う。

## 4. 外部 operator の作業区分と数え方

区分は作業名や実行時刻だけでなく、**特定試行の結果へ影響したか**で決める。
[外部 operator 手順](external-operator.md)の権限・担当分担・二重対応防止に従い、評価のために権限を増やさない。

| 区分 | 例 | 記録・集計 |
|---|---|---|
| 測定前の準備・配備 | 共通環境の構築、測定開始前の release 固定、cap-probe | 試行によらないものは shared に一度記録し、準備の外部 AI 支援として別掲。試行へ均等配賦しない |
| 観測・記録 | 状態/ログの読み取り、採点用の記録整理 | 対象 trial または shared と観測区間を記録。状態も入力も変えない観測を結果影響支援と分けて件数・時間・消費を別掲 |
| 結果に影響する支援 | 質問/decision/plan 承認への回答、原因診断の助言を CoS に渡す、修復、再開・再投入、設定変更、試行を救う配備 | trial 固有の外部 AI 支援として必ず数える。失敗した支援も履歴に残す。準備・開発・protocol への付け替えは禁止 |

失敗した試行のための修復は、次の試行開始前に行っても結果影響支援である。汎用的な修正でも、発端の trial と
影響した後続 trial を関連付け、配備による条件変更も記録する。複数試行へ影響する支援は同じ支援 ID を参照し、
各 trial に「支援あり」を表示する。全体件数・消費は ID で重複除去し、帰属できない消費を勝手に按分しない。

支援件数は trial 内の `request_ref` ごとの論理要求単位。改訂・再通知・差戻しは `request_revision` を持つ history として残す。
参照が無い場合は 1 op = 1 件とし、未対応件数を併記する。生の操作回数・attempt 数も併記して反復の手間を隠さない。
主体別件数は一つの要求に複数主体が関与すると重複するため、単純加算しない。
観測から回答に移った場合は別 op を記録する。人が operator の案を Web に転記した場合も、
`decided_by` に外部 operator、`executed_by` に人を記録し、外部 AI 支援と人の実作業の双方を残す。

出力は、Celeris 内部の消費、外部 AI 支援の判断/操作/消費、人の能動時間/介入、共通準備・観測・評価器の費用を別欄にする。
成功数は「支援なし・支援あり・支援有無不明」を分け、支援ありの成功を自律成功と呼ばない。
支援を受けた試行を全体の分母から除外しない。既存 human-op report の `excluded_ops.non_human` は人の時間からの除外であり、
外部 AI 支援を評価報告から消してよいという意味ではない。

## 5. 主体と証跡の共通語彙

A3 の主体監査と A5 の [human-ops /2](eval-raw-record-definitions.md)に揃える。

| `actor.kind` | 意味・評価上の区分 |
|---|---|
| `human` | 人。手動作業の能動時間を計測 |
| `script` | 評価者が起動した script/cron。人の能動時間にはしない |
| `cos_project` | 案件 CoS。Celeris 内部 |
| `cos_general` | 統括 CoS。Celeris 内部 |
| `external_operator` | 外部 session。境界外の外部 AI 支援 |
| `mcp_client` | MCP client。外部 AI が判断した支援なら境界外の支援として記録 |
| `unknown` | 主体未確認。人・CoS・支援なしのいずれにも推測で寄せない |

`actor` と `decided_by`・`executed_by`・`approved_by`、`credential`（種類のみ）、`policy_ref`、
`request_ref`・`request_revision` を使う。API を呼んだ credential だけから判断者を推定しない。
同一主体/不明を null とする既存 schema の規則を守り、不明の理由は note/証跡で補う。
raw は `celeris.eval-human-op/2`。`mode`・`intent`・`op_kind` は既存定義を使い、新しい列挙を独自に追加しない。
本書の 3 作業区分・結果影響・支援 ID・condition の対応は評価用の付随台帳に記録する（現行 CLI の新 field とは扱わない）。
CoS の内部 event と human-op に同じ操作があれば event/operation 参照で照合し、二重計上しない。

## 6. 測れる指標・まだ測れない指標

以下は 2026-10-27 の本 checkout の計測経路を基準にした表。採用 release で取得・照合した証拠がある場合だけ「測定済み」とする。
A3/A5/A7 は起票元の改善計画の識別子。A7 等が別 task で完了しても、この文書だけを根拠に本番へ配備済みとはしない。

| 指標 | 現在の取得範囲・出所 | 通常運用での扱い・残る不足 |
|---|---|---|
| task の成功・失敗・timeout、run 消費 | campaign の trials、独立 grader、decision log。task 実行系として取得可 | CoS 受付から報告までの終端・trial 対応は別途必要。task の成功だけで通常運用成功としない |
| planner/worker/reviewer token 4 欄・定価換算 | decision log と final-accounting。欠測/gap を検出可能 | 実行器の外側値との照合を保つ。CoS を除いた値を全体費用と呼ばない |
| 案件/統括 CoS の消費・model/session | CoS 消費行はあるが、本 checkout の export は役割 cos/共有消費まで | A7 の thread/project/model/session 帰属と採用 release の export 証拠が必要。未帰属共有行は別掲、試行へ按分しない |
| 人・CoS・外部 operator の操作/論理要求数 | A5 human-ops /2 の主体別集計と request history。正しく raw が記録された範囲で可 | A3 の主体監査との照合、記録 coverage が必要。既存の主体不明な履歴を人と断定しない |
| 人の能動時間 | human/manual の開始・終了区間の和集合 | 開いていた質問/承認の待ち時間では代用不可。区間欠落は欠測 |
| 外部 AI 支援の件数・内容 | operator 開始/終了票、human-ops /2、transcript を trial/request に結合すれば取得可 | 3 区分と役割別判断者まで含む自動集計は本変更では実装しない。付随台帳で照合 |
| 外部 operator の token・時間・定価換算 | 外部 session の usage/時刻が保存されている範囲だけ取得可 | Celeris DB だけでは全量も trial 帰属も保証不能。共有 session、未保存、subagent 分を含むか不明なら欠測/部分観測 |
| 通常運用の所要時間・人待ち・報告遅延 | 受付/終端/報告の時刻と raw wait が揃えば計算可 | campaign task wall や operator session 長で代用不可。全経路の収集は開始前に確認 |
| 配備・自己改善による条件の変化 | release/operation と固定票、変更台帳を照合 | 全主体の判断と実行、試行への影響の自動接続は未確認。版混在を分離 |
| 実支出（cash） | 試行単位の請求は取得不能の場合がある | 定価換算/list や harness 報告額で穴埋めしない。cash は欠測 |
| quota | account 全体の期間差は取得可 | 共有利用が混ざる。trial 別消費や費用への変換は不可、期間全体で別掲 |
| 自律性・通常運用全体の費用削減 | 現状の task-run 集計だけでは測れない | 全経路の coverage と支援/欠測の分離、比較条件の一致が必要。A0/B0 pilot から推定しない |

値には取得状態と根拠を添える。未実装、未取得、取得不能、帰属不能は null と理由・対象数を出す。
「呼ばなかった」は呼出経路と対象期間の記録の完全性を確認できた場合だけ `not_called` とする。
通常運用の CoS に、task 直接投入用の `cos_not_called_basis` を流用しない。
観測できた和は `observed_sum`、必要な全成分が揃った場合だけ total とし、観測 0 件を 0 消費にしない。
費用は list/cash/harness 報告額/quota を分離する。外部支援の総額が不明なら「Celeris 内部の観測費用」とだけ呼ぶ。

## 7. 測定開始時と報告時の確認

開始前に、新しい固定票・測定 ID・独立評価器・分母・承認枠と、CoS 経路の trace、主体監査、usage と支援台帳の
結合方法を確認する。未計測指標を明記した観察は可能だが、欠測指標について比較・削減・自律性を主張しない。

報告には revision/condition、計画・実行・未実行・中断・条件混在の数、成功判定、支援 3 区分、主体別の論理要求数と
生の操作回数、人の能動時間、Celeris/外部 AI/共有/評価器の消費、欠測件数と理由、条件変更履歴を載せる。
raw と集計の参照を添え、結果に影響する外部 AI 支援を共有開発費として除外していないことを確認する。

## 8. 方針の版の変更記録

§3 の「変更時」の記録のうち、CoS の方針の版の変更を並べる。測定 daemon で版を変えたら、新しい `condition_id` で試行を始める。

| 日付 | 変更 | 版 | 根拠 | 既存の試行への影響 |
|---|---|---|---|---|
| 2026-10-27 | 人に返す文面の規則（yomiyasu の要点）を CoS の Core と skill、worker の結果・質問の指示に入れた | `cos_policy_version` "1" → "3"（`cos-inbox-triage` 3、`cos-operator` 5） | [ADR](../../agent-docs/adr/2026-10-27-yomiyasu-human-text.md) | main-4 の pinned 測定（RELEASE a28184889f56、別 daemon）は固定した release と config で動くので変わらない。版 "3" の release を配備した daemon の試行は、版 "1" の試行と別の condition |
| 2026-10-28 | 人の窓口を統括 CoS 1 本にした。案件の CoS の escalate は統括 CoS のスレッドに札で集まり、統括 CoS が `chat.relay_forward` で案件の CoS へ取り次ぐ。統括 CoS は案件の件に答えない。版 "4" は 2026-10-27 の取り次ぎ（relayed-instructions）で上げた版 | `cos_policy_version` "4" → "5"（`cos-inbox-triage` 5） | [ADR](../../agent-docs/adr/2026-10-28-single-human-window-general-cos.md) | 固定した release と config で動く pinned 測定は変わらない。版 "5" の release を配備した daemon の試行は、版 "4" 以前の試行と別の condition |
| 2026-10-28 | 承認済みの枠の内側の定型の判断（枠内の続行・上限の一度だけの引き上げ・文書化済みの既定・作業方針の補足）を CoS が根拠（`basis`）つきで代答する。根拠の欠けた代答は API が 422。`[cos.triage] human_required` の分類を改めた（旧名は読み込みエラー） | `cos_policy_version` "5" → "6"（`cos-inbox-triage` 6） | [ADR](../../agent-docs/adr/2026-10-28-cos-routine-delegated-answers.md) | 固定した release と config で動く pinned 測定は変わらない。版 "6" の release を配備した daemon の試行は、版 "5" 以前の試行と別の condition。CoS が代答する件が増えるので、人・operator の介入数を版の間で比べない |
| 2026-10-29 | reviewer の技術的な差し戻しで failed になった task を、CoS が定型の代答（`review_rework`）で開き直す。同じ task で 2 回まで、同じ差し戻し文の繰り返し・human check・`human_required` の label・reviewer の判定以外の失敗は API が拒み人へ回る | `cos_policy_version` "6" → "7"（`cos-inbox-triage` 7） | [ADR](../../agent-docs/adr/2026-10-29-cos-review-rework-reopen.md) | 固定した release と config で動く pinned 測定は変わらない。版 "7" の release を配備した daemon の試行は、版 "6" 以前の試行と別の condition。CoS が開き直す件が増えるので、人・operator の介入数を版の間で比べない |
| 2026-10-30 | おかしなところの無い計画の承認（plan gate の `approve`）を、CoS が定型の代答（`plan_conforms`）で答える。待っている版の id、未回答の決定が無いこと、課金・node-h・外部公開・破壊的・本番の段が前の人の確認の段か人の承認の内側にあること、24 時間に 3 版以上の再版でないことを API が検査し、外れれば人へ回る。計画の承認の `approve` を `documented_default` で答えることはできなくなった | `cos_policy_version` "7" → "8"（`cos-inbox-triage` 8） | [ADR](../../agent-docs/adr/2026-10-30-cos-plan-conforms-approve.md) | 固定した release と config で動く pinned 測定は変わらない。版 "8" の release を配備した daemon の試行は、版 "7" 以前の試行と別の condition。CoS が承認する計画が増えるので、人・operator の介入数を版の間で比べない |
| 2026-10-30 | クラスタ job の待ち上限の認可に、job が全て待ち（Q）のままなら CoS が定型の代答（`job_wait_extend`）で「延長」と答える。同じ job の上限到達は 3 回まで、取り消し・取り下げ・台数変更・job が Q でないものは API が拒み人へ回る | `cos_policy_version` "8" → "9"（`cos-inbox-triage` 9） | [ADR](../../agent-docs/adr/2026-10-30-cos-job-wait-extend.md) | 固定した release と config で動く pinned 測定は変わらない。版 "9" の release を配備した daemon の試行は、版 "8" 以前の試行と別の condition。CoS が答える件が増えるので、人・operator の介入数を版の間で比べない |
| 2026-10-30 | 計画の承認の定型の代答（`plan_conforms`）に、人へ回す決定的な条件を足した。task の受け入れ条件の human check（403）、承認の理由に `near_limit:` など費用・上限の理由がある、前の版の承認以降の目的・受け入れ条件の編集（422 `cos_basis_plan_needs_human`）を API が拒み人へ回る。`basis.plan.guarded_stages` の各段に危うさの種類 `kind`（`billing`・`node_hours`・`production`・`external_publish`・`destructive`・`secrets_permissions`）が必須になった | `cos_policy_version` "9" → "10"（`cos-inbox-triage` 10） | [ADR](../../agent-docs/adr/2026-10-30-plan-conforms-human-guards.md) | 固定した release と config で動く pinned 測定は変わらない。版 "10" の release を配備した daemon の試行は、版 "9" 以前の試行と別の condition。CoS が承認せず人へ回す計画が増えるので、人・operator の介入数を版の間で比べない |
