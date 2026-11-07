#!/usr/bin/env python3
"""dup-candidates.json を組む（判断は agent が file を読んで書いた。走査の raw は dup-candidates-scan.json）。

使い方（repo の根で）:
  python3 agent-docs/progress/2026-11-07-health-v2-b0/dup-candidates-scan.py \
      > agent-docs/progress/2026-11-07-health-v2-b0/dup-candidates-scan.json
  python3 agent-docs/progress/2026-11-07-health-v2-b0/dup-candidates-build.py
各候補の files の path:line は書いた時点で実在を確かめる（無ければ exit 1）。
"""
import json
import os
import subprocess
import sys

HERE = "agent-docs/progress/2026-11-07-health-v2-b0"
REF_SEARCH = (
    "dup-candidates-scan.py の手順 1〜3（tracked の crates/**/*.rs と web/・control-ts/ の *.ts/*.tsx/*.mjs を単語で数え、"
    "定義の 1 回だけ）→ `git grep -n -w <名前> -- crates web control-ts scripts` が定義の行の 1 件だけであることを確認"
)

D, I, K = "削除", "統合", "維持"

C = []


def cand(id, category, files, judgment, reason, canonical, tests, same_invariant=None, same_change_reason=None,
         co_change=None, reference_search=None):
    C.append({
        "id": id,
        "category": category,
        "files": files,
        "judgment": judgment,
        "reason": reason,
        "evidence": {
            "same_invariant": same_invariant,
            "same_change_reason": same_change_reason,
            "co_change": co_change,
        },
        "canonical": canonical,
        "impacted_tests": tests,
        **({"reference_search": reference_search} if reference_search else {}),
    })


# ---- 統合 ----
cand("celerisctl-api-endpoint-resolution", "config-loading",
     ["crates/celerisctl/src/commands/cron.rs:84", "crates/celerisctl/src/commands/cos_ops.rs:78",
      "crates/celerisctl/src/commands/integration.rs:127", "crates/celerisctl/src/commands/models.rs:41",
      "crates/celerisctl/src/commands/script.rs:186", "crates/celerisctl/src/commands/browser_doctor.rs:84",
      "crates/celerisctl/src/commands/cron.rs:14", "crates/celerisctl/src/commands/integration.rs:20",
      "crates/celerisctl/src/commands/knowledge.rs:41", "crates/celerisctl/src/commands/models.rs:16",
      "crates/celerisctl/src/commands/script.rs:31"],
     I,
     "6 つの subcommand が「config を読む → [api] listen から http://<listen>/api/v1 → client_token()」を同じ手順で書き写している。"
     "既定 path の定数 DEFAULT_CONFIG も 5 file（cron・integration・knowledge・models・script）に別々にあり、cos_ops は同じ文字列を直書きする。cos_ops だけが CELERIS_CONFIG を見て、browser_doctor は文言が違う（'API listen missing'）。"
     "接続先の決め方を変えると 6 か所を同時に直す必要があり、既に食い違いが出ている。",
     "crates/celerisctl/src/commands/cron.rs の ApiConfig に `ApiConfig::from_config(path)`（既定 path・CELERIS_CONFIG・listen・token の解決）を 1 つ置き、各 subcommand はそれを呼ぶ",
     ["crates/celerisctl/src/commands/cron_tests.rs", "crates/celerisctl/src/commands/cos_ops_tests.rs",
      "crates/celerisctl/src/commands/query_tests.rs", "crates/celerisctl/src/commands/scratch_tests.rs"],
     same_invariant="同じ: CLI が叩く API の base URL と token は config の [api] から決まる",
     same_change_reason="同じ: [api] の欄（listen・tokens）を変えると全部が変わる",
     co_change="未確認（git log -S の走査は 2 分で打ち切った）。追加は 2026-10-03 cron → 10-06 cos_ops → 10-11 integration → 11-02 script と写しで増えた（各 file の初出 commit の件名）")

cand("account-check-result-vocabulary", "dto",
     ["crates/task-api/src/admin.rs:161", "crates/task-worker/src/claude_account.rs:32",
      "crates/celeris/src/accounts_admin.rs:637", "crates/celeris/src/accounts_admin.rs:646"],
     I,
     "同じ 4 値（ok・auth_failed・throttled・spawn_failed）の enum が task-api と task-worker にあり、celeris が 1 対 1 の写像（map_check_result）と"
     "名前の関数（account_check_result_name、serde の snake_case と同じ文字列）を手で持つ。値を 1 つ足すと 3 file を同時に直す。",
     "task-core に `AccountCheckResult` を 1 つ置き（task-api・task-worker は既に task-core に依存）、両 crate は re-export。写像と名前の関数は消す",
     ["crates/celeris/src/accounts_admin/tests.rs", "crates/task-worker/src/claude_account/tests.rs",
      "crates/task-worker/src/codex_account/tests.rs", "crates/task-api/tests/cos_ops_admin_config.rs",
      "crates/task-api/tests/llm_accounts_admin.rs"],
     same_invariant="同じ: account の疎通確認の結果の語彙（ADR-0024 D6）",
     same_change_reason="同じ: 確認の結果の種類が増えたとき",
     co_change="未確認")

cand("task-features-content-hash", "parser-hash",
     ["crates/task-core/src/route_record.rs:428", "crates/task-core/src/route_record.rs:617",
      "crates/task-dispatch/src/dispatcher/eval_trials.rs:87", "crates/task-dispatch/src/dispatcher/eval_trials.rs:919"],
     I,
     "route_features.hash（内容アドレス）の材料 {schema, template, brief, aux} と 'sha256:' + hex の書式を、route_record の FeatureBody と"
     "eval_trials の EvalFeatureBody が別々に持つ。eval 側の doc comment が「FeatureBody と同じ形」と書いて手で合わせている。"
     "片方の欄順・書式が変わると同じ特徴量が別 hash になる。",
     "task-core::route_record に `pub fn task_features_hash(template, brief, aux) -> String` を置き、eval_trials はそれを呼ぶ（既存行の hash は書き換えない。書式が同じなので値は変わらない）",
     ["crates/task-core/src/route_record/tests.rs", "crates/task-dispatch/src/dispatcher/tests/eval_trial_dispatch.rs"],
     same_invariant="同じ: 同じ特徴量は同じ hash（route_features の内容アドレス）",
     same_change_reason="同じ: TASK_FEATURES_SCHEMA・FeatureBody の欄を変えるとき",
     co_change="未確認")

cand("scratch-status-schema-const", "dto",
     ["crates/task-ops/src/daemon.rs:149", "crates/task-worker/src/scratch.rs:22",
      "crates/task-api/src/handlers/tests.rs:247"],
     I,
     "`celeris.scratch-status/1` の定数が task-ops（SCRATCH_STATUS_SCHEMA）と task-worker（STATUS_SCHEMA）に 2 つある。task-worker 側は workspace で参照されない。",
     "task-ops::daemon::SCRATCH_STATUS_SCHEMA。task-worker::scratch::STATUS_SCHEMA は消す（試験は定数を参照する）",
     ["crates/task-worker/src/scratch/tests.rs", "crates/task-api/src/handlers/tests.rs", "crates/celerisctl/src/commands/scratch_tests.rs"],
     same_invariant="同じ: scratch status の wire schema 名（ADR-0075 D6）",
     same_change_reason="同じ", co_change="未確認", reference_search=REF_SEARCH)

cand("browser-sandbox-inner-uid", "policy-constant",
     ["crates/task-worker/src/browser_runtime.rs:29", "crates/task-worker/src/browser_runtime.rs:359"],
     I,
     "sandbox の内側の UID 1000 を SANDBOX_UID（pub、未参照）と INNER_ID（private、使われている）の 2 つの定数で持つ。同じ file・同じ意味（ADR-0105）。",
     "browser_runtime.rs の 1 定数（SANDBOX_UID を残して INNER_ID を置き換える、または SANDBOX_UID を消す）",
     ["crates/task-worker/tests/browser_runtime_isolated.rs", "crates/task-worker/tests/browser_runtime_supervisor.rs"],
     same_invariant="同じ: sandbox 内の UID/GID", same_change_reason="同じ", co_change="同じ file",
     reference_search=REF_SEARCH)

# ---- 削除（旧い仕組みの残り・未参照の config 欄 / API 欄） ----
cand("config-execution-cheap-local-first", "unused-config-field",
     ["crates/celeris/src/config/execution.rs:57", "crates/celeris/src/config/execution.rs:302",
      "config/celeris.example.toml:629"],
     D,
     "`[execution] cheap_local_first` は読み込むが crates の中で 1 度も読まれない（ADR-0132 L7 の切替。ADR が挙げる試験 dispatcher::tests::cheap_local_first は現存しない）。"
     "書いても挙動が変わらない欄で、CLAUDE.md の「警告だけ出して無視を残さない」に反する。",
     "欄を消し、deny_unknown_fields で読み込みエラーにする。本番 config に書かれていれば「本番への移行」節で人が消す",
     ["crates/celeris/src/config/tests.rs"],
     reference_search="`git grep -n cheap_local_first -- crates` が config/execution.rs の定義・既定値の 4 行だけ。`.cheap_local_first` の読み出しは 0 件")

cand("config-scratch-l1-max-gb", "unused-config-field",
     ["crates/celeris/src/config/scratch.rs:29"],
     D,
     "`[scratch] l1_max_gb` は読まれない。L1 cache server（webdav）は ADR-0129 で廃止され、ADR-0075 付記 12 は「G2 まで使わない」と書いた。"
     "書いても効かない欄である。",
     "欄と default_scratch_l1_max_gb を消し、読み込みエラーにする（本番 config の確認は人）",
     ["crates/celeris/src/config/tests.rs"],
     reference_search="`git grep -n l1_max_gb -- crates` が config/scratch.rs の定義 2 行だけ。`.l1_max_gb` の読み出しは 0 件")

cand("api-scratch-cache-stats-field", "legacy-api-field",
     ["crates/task-ops/src/daemon.rs:209", "crates/task-ops/src/daemon.rs:213", "crates/task-ops/src/daemon.rs:217"],
     D,
     "ScratchCacheView.stats は「廃止（ADR-0129）。型だけ残す（値は常に None）」と doc comment に書かれた API 欄。"
     "SCRATCH_CACHE_STATS_SCHEMA は未参照、web/api/generated/schema.json に型が残る。旧い仕組みの残りで、常に null の API 欄である。",
     "ScratchCacheView から stats 欄・ScratchCacheStats・SCRATCH_CACHE_STATS_SCHEMA を消し、web/ の生成型を再生成する",
     ["crates/task-api/src/handlers/tests.rs", "crates/celerisctl/src/commands/scratch_tests.rs", "web/ の tsc -b と unit 試験"],
     reference_search="`git grep -n -w ScratchCacheStats -- crates web/src` が daemon.rs の 3 行だけ（定義・欄・doc）")

cand("route-record-route-decision", "dto",
     ["crates/task-core/src/route_record.rs:261", "crates/task-core/src/route_record.rs:276",
      "crates/task-core/src/route_select/trace.rs:157"],
     D,
     "route_record::RouteDecision と DecisionSubject は互いの欄以外から参照されない。route_decisions 行の正本は route_select::trace::RouteDecision（store の書き込みも dispatcher もこちらを使う）。"
     "同じ記録の型が 2 つあり、片方は使われていない。",
     "crates/task-core/src/route_select/trace.rs の RouteDecision。route_record 側の 2 型を消す",
     ["crates/task-core/src/route_record/tests.rs"],
     same_invariant="同じ: route_decisions の 1 行（schema celeris.route-decision/1）",
     reference_search="`git grep -n -w DecisionSubject -- crates` が route_record.rs の 2 行だけ。`route_record::RouteDecision` の import は 0 件（route_extract・trace・store は RouteOptions・RouteOutcome・TaskFeatures だけを import）")

cand("script-scratch-u5-stats", "unreferenced-script",
     ["scripts/scratch/u5/stats.py:1"],
     D,
     "sccache の stats JSON を印字する 6 行の使い捨て script。どの file からも呼ばれず、試験も無い。",
     "なし（消す）", [],
     reference_search="dup-candidates-scan.py の unreferenced_scripts（basename・stem を tracked の全 file で検索）。`git grep -n 'u5/stats'` が 0 件")

# ---- 維持（名前が似ているだけ・境界・wire・migration） ----
cand("script-host-cos-dto-mirror", "dto",
     ["crates/script-runtime/src/dto.rs:281", "crates/script-runtime/src/dto.rs:311", "crates/script-runtime/src/dto.rs:368",
      "crates/task-core/src/chat/digest.rs:28", "crates/task-core/src/chat/digest.rs:357", "crates/task-core/src/chat/triage.rs:501"],
     K,
     "CosTriage*・CosDigest* の 8 型が同じ欄で 2 か所にある。ただし script-runtime（TS の host API）は本体で task-core に依存しない境界（付記 S4-12）で、"
     "host API の DTO は Option 欄の必須化（Option::deserialize）など別の wire 規則を持つ。tests/dto.rs が fixture の全行で両方を往復させて一致を確かめる。"
     "security 境界の独立検証として維持する。",
     "形の正本は task-core（chat/digest.rs・chat/triage.rs）。一致は crates/script-runtime/tests/dto.rs が守る",
     ["crates/script-runtime/tests/dto.rs", "crates/task-core/tests/script_fixtures.rs"],
     same_invariant="同じ形だが wire 規則が違う", same_change_reason="欄の追加では同じ")

cand("ts-control-dual-policy", "policy",
     ["crates/task-core/src/model_policy.rs:606", "control-ts/src/domains/lane_rules.ts:114",
      "crates/task-core/src/llm_route.rs:558", "control-ts/src/domains/route_priority.ts:143",
      "crates/task-core/src/execution_gate/scorer.rs:192", "control-ts/src/domains/execution_gate.ts:174",
      "crates/task-core/src/chat/digest.rs:268", "control-ts/src/domains/cos_digest.ts:370"],
     K,
     "lane 規則・route 優先・execution gate・cos digest の判断が Rust と TS の両方にある。ADR 2026-11-02-ts-control-layer-boundary の移行（mode rust→compare→canary→ts）で、"
     "Rust は参照実装として compare の照合に要る。移行の途中の二重化で、旧い仕組みの残りではない。"
     "停止条件: 領域の mode が本番で ts に固定され、fallback の要件を ADR で外したら、その領域の Rust の判定を同じ差分で消す（S4-17 と同じ規則）。"
     "project_controller は TS だけで Rust の参照実装が無い（project_controller.ts:4）ので対象外。",
     "移行後は control-ts/src/domains/<領域>.ts。それまでは Rust（fallback）",
     ["crates/celeris/tests/script_compare_lane_rules.rs", "crates/celeris/tests/script_compare_route_priority.rs",
      "crates/celeris/tests/script_compare_execution_gate.rs", "crates/celeris/tests/script_compare_cos_digest.rs",
      "control-ts/src/domains/*.test.ts"],
     same_invariant="同じ（compare が一致を検査する）", same_change_reason="同じ（規則を変えると両方を変える）")

cand("routing-decision-namesakes", "name-only",
     ["crates/task-core/src/direct_route.rs:26", "crates/task-core/src/route_select/trace.rs:157",
      "crates/task-core/src/routing.rs:88", "crates/task-core/src/browser_backend.rs:335",
      "crates/task-core/src/model_policy.rs:483", "crates/task-core/src/route_select/lane.rs:128",
      "crates/task-core/src/route_select/task_override.rs:142", "crates/llm-proxy/src/routing_context.rs:75"],
     K,
     "名前が似ているだけ。direct_route は planner を通すかの判定、trace は route_decisions の記録、routing.rs は harness の候補列、browser_backend は browser backend の選択、"
     "model_policy::LaneDecision は規則表の結果と監査、lane::LaneDecision は decision-assisted の lane 選択、task_override::ResolvedRouting は routing mode、"
     "llm-proxy の ResolvedRouting は要求 1 件の context と相関 ID。不変条件も変更理由も違う。",
     "それぞれの module", [])

cand("task-features-namesakes", "name-only",
     ["crates/task-core/src/model_policy.rs:42", "crates/task-core/src/route_record.rs:18"],
     K,
     "model_policy::TaskFeatures は lane を決める 9 軸の順序尺度、route_record::TaskFeatures は route 記録の brief と内容 hash。route_record は前者を LaneFeatures の別名で取り込んで使っており、別物として扱われている。",
     "それぞれの module", ["crates/task-core/src/route_record/tests.rs"])

cand("view-dto-projections", "name-only",
     ["crates/celeris-mcp/src/tools/tasks.rs:76", "crates/task-ops/src/view.rs:78",
      "crates/celeris-mcp/src/tools/skills.rs:31", "crates/task-api/src/skills.rs:56",
      "crates/task-api/src/memory.rs:23", "crates/task-ops/src/memory.rs:31"],
     K,
     "MCP の TaskSummary・SkillSummaryView は MCP tool の出力（欄を絞った公開契約）で、task-ops/task-api の view とは公開先が違う。"
     "MemoryView は task-ops（PathBuf を持つ内部値）→ task-api（JSON の文字列）の層の写しで、API の型を内部に漏らさないため。名前が同じだけで統合しない。",
     "それぞれの公開面", ["crates/celeris-mcp/tests", "crates/task-api/tests"])

cand("config-workspace-namesakes", "name-only",
     ["crates/celeris/src/config/workspace.rs:15", "crates/task-core/src/workspace_config.rs:35"],
     K,
     "daemon config の [workspace]（worktree の branch 接頭辞・build cache）と、repo の中の .config/celeris/workspace.toml（検査コマンド・成果物の置き場）。"
     "読む file も規則（daemon は deny_unknown_fields で拒否、repo の file は warn して既定）も違う。",
     "それぞれ", [])

cand("clock-injection-traits", "name-only",
     ["crates/campaign/src/watch.rs:18", "crates/campaign/src/dagbench/pbs/intent.rs:404",
      "crates/jev-classifier/src/time.rs:6", "crates/llm-proxy/src/reservation.rs:20", "crates/script-runtime/src/generation.rs:62"],
     K,
     "SystemClock の名前が 4 つあるが、trait の操作が違う（sleep だけ、now_unix+sleep_until、Send+Sync の now など）。crate ごとの試験の注入点で、共通の crate に寄せると依存が増えるだけ。",
     "それぞれの crate", [])

cand("failure-class-namesakes", "name-only",
     ["crates/campaign/src/grader.rs:1409", "crates/llm-proxy/src/fallback.rs:21",
      "crates/task-core/src/route_select/escalation.rs:8", "crates/task-ops/src/derive.rs:326"],
     K,
     "FailureClass が 4 つあるが、採点の失敗分類・provider fallback の HTTP 失敗分類・escalation の run 失敗分類・表示用の導出で、値の集合も使い道も違う。llm-proxy の分類は provider 固有の wire 仕様に依る。",
     "それぞれ", [])

cand("credential-injection-receipts", "security-boundary",
     ["crates/celeris-credentiald/src/injection_ipc.rs:151", "crates/celeris-credentiald/src/injection.rs:106",
      "crates/celeris-credentiald/src/injection_ipc.rs:123", "crates/task-worker/src/browser_cdp_sink.rs:141"],
     K,
     "InjectionReceipt は IPC の応答（deny_unknown_fields、秘密を含まない欄だけ）と、注入器の内部の commit 結果で欄が違う。InjectionRequest は credentiald の IPC と worker の CDP sink の要求。"
     "security 境界の両側で独立に検証する型なので共通化しない。",
     "それぞれ", ["crates/celeris-credentiald/tests", "crates/task-worker/tests/browser_live_cross.rs"])

cand("review-repair-hint-layers", "dto",
     ["crates/task-worker/src/protocol.rs:997", "crates/task-core/src/execution.rs:572"],
     K,
     "protocol 側は review.json の wire（hint の自由記述込み、serde）、task-core 側は分類に使う scope・class だけの値。分類が自由記述を見ないことを型で保つ層の分離。",
     "wire は task-worker/src/protocol.rs、分類は task-core/src/execution.rs", [])

cand("routing-stage-cli", "name-only",
     ["crates/celerisctl/src/commands/routing.rs:123", "crates/task-core/src/route_select/stage.rs:6"],
     K,
     "CLI の値（shadow・enforce・eval。clap の ValueEnum）と記録の stage（shadow・enforce）。CLI は eval を足した上位集合で、記録の enum に eval を入れると route_decisions.stage の語彙が変わる。",
     "それぞれ", [])

cand("migration-replay-legacy-readers", "migration-replay",
     ["crates/task-core/src/route_select/task_override.rs:139"],
     K,
     "evidence_mode の無い旧い routing_resolved event を pinned/latest と読む規則。events は追記専用で書き換えないので、旧い event を読む経路は残す（CLAUDE.md の禁止事項）。",
     "task_override.rs", [])

# ---- 未参照の pub item（走査の結果を 1 件ずつ） ----
UNREF_KEEP = {
    "PROJECT_SNAPSHOT_SCHEMA": "snapshot の wire schema 名。定数は未参照だが、値 'celeris.project-snapshot/1' もどこにも書かれていない。schema 欄を持たせるか消すかは project controller の owner が決める（未確認のまま削除にしない）",
    "PROJECT_PROPOSAL_SCHEMA": "同上（proposal）",
    "SHADOW_RECORD_VERSION": "shadow 記録の版。記録の形を変えたときに上げる約束の定数で、読み出しは無い。記録側が版を書いていないので、書くか消すかを routing の owner が決める",
}
UNREF_SKIP = {"STATUS_SCHEMA", "SCRATCH_CACHE_STATS_SCHEMA", "SANDBOX_UID"}  # 上の統合/削除の候補に入れた
scan = json.load(open(os.path.join(HERE, "dup-candidates-scan.json")))
for u in scan["unreferenced_pub"]:
    if u["name"] in UNREF_SKIP:
        continue
    keep = u["name"] in UNREF_KEEP
    cand("unref-" + u["name"].lower().replace("_", "-"), "unreferenced-pub",
         [u["at"]], K if keep else D,
         UNREF_KEEP.get(u["name"],
                        f"pub {u['kind']} `{u['name']}` は定義の行の外で workspace（crates・web・control-ts・scripts）に 1 度も出ない。試験からも呼ばれない。"),
         "なし（消す）" if not keep else u["at"], [], reference_search=REF_SEARCH)

# path:line の実在を確かめる
bad = []
for c in C:
    for f in c["files"]:
        path, _, line = f.partition(":")
        if not os.path.exists(path):
            bad.append(f)
            continue
        if line and int(line) > sum(1 for _ in open(path, encoding="utf-8")):
            bad.append(f)
if bad:
    print("missing:", bad, file=sys.stderr)
    sys.exit(1)

base = subprocess.run(["git", "merge-base", "HEAD", "main"], capture_output=True, text=True, check=True).stdout.strip()
counts = {}
for c in C:
    counts[c["judgment"]] = counts.get(c["judgment"], 0) + 1
out = {
    "schema": "celeris.health-v2-b0.dup-candidates/1",
    "base_sha": base,
    "base_note": "着手時の git merge-base HEAD main。capability-map.json の base_sha と同じ",
    "generator": f"{HERE}/dup-candidates-build.py",
    "inputs": [f"{HERE}/capability-map.json", f"{HERE}/dup-candidates-scan.json"],
    "method": {
        "same_name_types": "dup-candidates-scan.py の same_name_types（pub struct/enum の同名 132 組）から、欄・使い道を読んで候補を選んだ",
        "unreferenced": REF_SEARCH,
        "config_fields": "crates/celeris/src/config/*.rs の `pub <欄>:` ごとに `\\.<欄>\\b` を試験以外の crates/**/*.rs で検索し、0 件の欄を候補にした",
        "limits": "co_change（同時に変わったか）は git log -S が 2 分を超えたため多くを未確認とした。名前の違う重複（同じ規則を別名で持つもの）は走査で拾えず、読んだ範囲だけ",
    },
    "summary": {"candidates": len(C), "by_judgment": counts},
    "candidates": C,
}
with open(os.path.join(HERE, "dup-candidates.json"), "w", encoding="utf-8") as f:
    json.dump(out, f, ensure_ascii=False, indent=1)
    f.write("\n")
print(json.dumps(out["summary"], ensure_ascii=False))
