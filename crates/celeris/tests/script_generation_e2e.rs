//! 付記 S5（境界 ADR 2026-11-02-ts-control-layer-boundary）の e2e: 本物の bundle と試験用の一時 DB の daemon で、
//! 新旧併存 → 切替 → 途中の error → 自動の戻し → 再起動を通す。userns を使わない（ADR-0126）。外部ネットワークに出ない
//! （API は loopback）。LLM を呼ばない（triage run の結果は store に直接書く。task の harness は `fake`）。
//!
//! 流れ（領域 `cos_digest`）:
//! 1. generation A（本物の bundle）を人の token で `compare` → `canary` → `ts`。人以外の token・主体は 403 で表が変わらない。
//! 2. generation B（本物の bundle + error を注入する包み）を登録して切り替える。切替の前に始めた A の pass は A で終わり、
//!    A は in-flight 0 で `retired` になる。
//! 3. B の `ts` で pass に error を注入すると script-guard が `rust` へ戻す。
//! 4. daemon を再起動すると、A の `ts` と generation を復元する。bundle を消して再起動すると `rust` に戻り受信箱に項目が出る。
//!
//! 合格条件（file の最後の `assert_ledger`）: `script_domain_events`・`script_generation_events`・`script_compare_log`・
//! `events` に欠落が無い、run ごとの digest message が 1 件（二重副作用 0）、人以外の昇格で表が変わらない（未承認の変更 0）。
//!
//! bundle は `support/script_compare.rs` の方式（source から `$TMPDIR` に作る）で用意する。作れなければ SKIP、
//! `CELERIS_SCRIPT_REQUIRE_BUNDLE=1` なら失敗にする。
//!
//! digest の起こし方: daemon の digest pass は「この process の CoS の run が終わった」か「起動した」ときに走る。試験は
//! 完了した triage run を store に書き、偽の CoS（`[adapters.fake]`、LLM なし）の短い run を project thread に起こす。
//! その run が終わると本物の tick の中で pass が走る。store に書く間の run を tick が持ち主の居ない run として閉じないよう、
//! 試験の入口で孤児の回収を切る。lane_rules の task の段だけ `[cos] enabled = false` で動かす
//! （task の問いが受信箱 → triage の run になり、数えた digest に混ざらないように）。
//!
//! 待ちの方針（ADR-0125）: 出来事待ち + 長い保険。切替をまたぐ in-flight は、dispatcher の pass が deadline（200 ms）の内に
//! 終わるので dispatcher では作れない。`run_with_test_hooks` で daemon の登録簿を共有し、daemon の isolate A に
//! 試験が pass を送り、op の答えを止めて作る。drain の時計は `ManualClock` で止めるので、30 秒の打ち切りは起きない。
//! TS の待ちの予算（1 tick 200 ms）は実時間なので、host が遅いと `timeout` で (f) の 10 分 3 件に当たりうる。それは環境の
//! 遅さであって試験の対象でないので、そのときだけ新しい一時 DB で場面をやり直す（`support/script_compare.rs` と同じ方針）。

mod daemon_port;
mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use celeris::{Config, DaemonTestHooks, Exit, RunOptions, ScriptPolicy};
use celeris_script::manifest::{deploy_bundle, deployed_dir, sha256_hex};
use celeris_script::{GenerationRegistry, ManualClock, RuntimeLimits};
use serde_json::{Value, json};
use support::script_compare::{Env, dist_bundle, dist_dir, load_config, repo_root};
use task_core::chat::digest::CosDigestInput;
use task_core::chat::triage::CosTriageSource;
use task_core::chat::{
    ChatMessageQuery, ChatMessageRole, ChatOpenThreadRequest, ChatPostMessageRequest, ChatRunState,
    ChatSendMode, ChatThreadQuery,
};
use task_core::script_compare::{
    CompareOutcome, CosDigestAppliedBy, CosDigestCompareRecord, ScriptDomain, ScriptDomainError,
    ScriptDomainModeRow, ScriptGenerationState, ScriptMode, ScriptModeChange,
};
use task_core::script_guard::{ScriptGuardCall, ScriptGuardObservation, readiness};
use task_core::script_inbox::{ScriptInboxItem, ScriptInboxStore};
use task_core::{DaemonMode, Event, Project, ProjectId, ProjectStatus, Task, TaskStore};
use task_dispatch::script_policy::{
    CosDigestOp, CosDigestOpCall, CosDigestOpReply, CosDigestPassCall, PolicyScript,
    PolicyScriptError,
};
use time::OffsetDateTime;
use tokio::sync::mpsc;

const TEST: &str = "script_generation_e2e_switch_error_rollback_restart";
const DOMAIN: &str = "cos_digest";
const LANE: &str = "lane_rules";
const HUMAN: &str = "human:test";
const HUMAN_TOKEN: &str = "e2e-human-token";
const OPERATOR_TOKEN: &str = "e2e-operator-token";
/// CoS の run を起動しない（lane_rules の task の段）。
const COS_OFF: &str = "[cos]\nenabled = false\n";
/// 偽の CoS の 1 回の run（stdin を捨てて完了の行を出す。`root_done_summary.rs` と同じ形）。
const COS_OK: &str =
    "cat >/dev/null; echo '{\"type\":\"done\",\"summary\":\"ok\",\"evidence\":[]}'";
const DIGEST_KEY_PREFIX: &str = "cos-triage-digest:";
/// 新しい一時 DB でやり直す上限（ファイル先頭の待ちの方針）。
const SCENARIO_ATTEMPTS: usize = 5;
/// 出来事待ちの保険。
const LONG: Duration = Duration::from_secs(300);
const POISON_PREFIX: &str = "run-poison-";

/// generation B: 本物の bundle を包む。`run-poison-` の run が pass に入ると例外を投げる（error の注入）。
/// 他の run では本物の答えをそのまま返すので、B も切替の後は Rust と一致する。
const POISON_WRAPPER: &str = r#"
const __celerisE2eRealDispatch = globalThis.__celerisDispatch;
globalThis.__celerisDispatch = async (domain, input, ctx) => {
  const out = await __celerisE2eRealDispatch(domain, input, ctx);
  if (domain === "cos_digest" && out.runs.some((run) => run.run_id.startsWith("run-poison-"))) {
    throw new Error("e2e: injected error in generation B");
  }
  return out;
};
"#;

/// host が遅くて (f) の timeout の条件に当たった（新しい一時 DB でやり直す）。
#[derive(Debug)]
struct Slow(String);

type Stage = Result<(), Slow>;

/// 1 回の場面の状態。
struct World {
    env: Env,
    client: reqwest::Client,
    reservation: daemon_port::PortReservation,
    base: String,
    state: PathBuf,
    /// A（本物の bundle）と B（包んだ bundle）の hash。
    a: String,
    b: String,
    /// 動いている daemon とその登録簿。drain の時計は止めたまま（30 秒の打ち切りを起こさない）。
    daemon: Option<tokio::task::JoinHandle<Result<Exit, celeris::DaemonError>>>,
    registry: GenerationRegistry,
    /// 偽の CoS の run を起こす project thread と、起こした回数。
    poke_thread: Option<String>,
    pokes: usize,
    /// 種を入れた全 run と、記録が 1 件残るはずか（mode `rust` の run は残らない）。
    runs: Vec<(String, bool)>,
    /// `script_compare_log` を出した lane_rules の task。
    task: Option<Task>,
    /// `ts` の pass が TS の command を apply した run が generation ごとに 1 件以上あったか。
    ts_applied: Vec<String>,
}

impl World {
    /// 配備先に A・B を置き、token を書く。daemon はまだ起こさない。
    fn new() -> Self {
        let env = Env::with_config(DOMAIN, "", COS_OFF);
        let home = env.dir.path();
        std::fs::write(home.join("human.token"), format!("{HUMAN_TOKEN}\n")).expect("human token");
        std::fs::write(home.join("operator.token"), format!("{OPERATOR_TOKEN}\n"))
            .expect("operator token");
        let state = home.join("state");
        let deployed = deploy_bundle(&dist_dir(), &state).expect("deploy A");
        let a = deployed.manifest.bundle_sha256.clone();
        let b = deploy_wrapped(&state, &a, POISON_WRAPPER);
        assert_ne!(a, b);
        let reservation = daemon_port::PortReservation::new();
        let base = format!("http://{}/api/v1", reservation.addr());
        Self {
            env,
            client: reqwest::Client::new(),
            reservation,
            base,
            state,
            a,
            b,
            daemon: None,
            registry: new_registry(),
            poke_thread: None,
            pokes: 0,
            runs: Vec::new(),
            task: None,
            ts_applied: Vec::new(),
        }
    }

    fn config(&self, cos: bool) -> Config {
        let home = self.env.dir.path();
        let knowledge = home.join("knowledge");
        let cos = if cos {
            format!(
                "[cos]\nenabled = true\nmax_turns = 2\nmax_wall_secs = 600\n\
                 route_candidates = [{{ source = \"local\", model = \"fake\" }}]\n\n\
                 [knowledge]\nroot = {knowledge:?}\n\n\
                 [adapters.fake]\ncommand = [\"sh\", \"-c\", {COS_OK:?}]\n\n\
                 [harnesses.claude_code]\nkind = \"fake\"\napi_formats = [\"fake\"]\n\
                 source_kinds = [\"fake\"]\n\n\
                 [route_defaults.claude_code]\n\
                 candidates = [{{ source = \"local\", model = \"fake\" }}]\n\n"
            )
        } else {
            format!("{COS_OFF}\n")
        };
        let config = load_config(
            home,
            &format!(
                "{cos}[api]\nlisten = \"{}\"\n\n\
                 [[api.tokens]]\nprincipal = \"{HUMAN}\"\ntoken_file = \"human.token\"\n\n\
                 [[api.tokens]]\nprincipal = \"operator:e2e\"\ntoken_file = \"operator.token\"\n\n\
                 [script]\nstate_dir = \"state\"\n",
                self.reservation.addr()
            ),
        );
        // ADR 2026-11-11-cos-skill-sync D3: CoS skills that fit `[cos.triage]` (its default version).
        let triage = &config.cos.triage;
        task_ops::cos_skill_gate::write_fixture(
            &knowledge.join("skills"),
            &triage.policy_skill,
            &triage.policy_version,
        )
        .expect("CoS skills");
        config
    }

    /// daemon を起こし、API が答えて起動時の復元が終わるのを待つ。
    async fn start_daemon(&mut self, cos: bool) {
        assert!(self.daemon.is_none());
        self.close_leftover_runs();
        self.registry = new_registry();
        let mut daemon = tokio::spawn(celeris::run_with_test_hooks(
            self.config(cos),
            RunOptions {
                until_idle: false,
                max_ticks: 0,
                mode: DaemonMode::Normal,
                release: None,
            },
            DaemonTestHooks {
                generation_registry: Some(self.registry.clone()),
                no_orphan_takeover: true,
            },
        ));
        let url = format!("{}/script/domains", self.base);
        daemon_port::wait_api_ready(
            &self.client,
            &url,
            HUMAN_TOKEN,
            &mut daemon,
            Duration::from_secs(5),
            Duration::from_secs(180),
        )
        .await
        .unwrap_or_else(|e| panic!("daemon API did not become ready: {e}"));
        self.daemon = Some(daemon);
        if cos {
            // 最初の tick（起動時の取り込み。triage の claim が走りうる）を過ぎてから store に run を書く。
            self.poke().await;
        }
    }

    /// 前の daemon を止めたときに動いていた CoS の run（受信箱の項目から daemon 自身が起こした triage run）を閉じる。
    /// 孤児の回収を切っているので（file 先頭）、次の daemon は閉じない。試験が回収の代わりをする。
    fn close_leftover_runs(&self) {
        let store = &self.env.store;
        let threads = store
            .chat_thread_list(&ChatThreadQuery {
                limit: Some(100),
                ..ChatThreadQuery::default()
            })
            .expect("threads")
            .items;
        for run in threads.iter().filter_map(|t| t.active_run_id.as_deref()) {
            // 前の daemon の worker がこの間に終えていれば `Conflict`（閉じる必要が無い）。
            let closed = store.chat_run_finish(
                run,
                ChatRunState::Interrupted,
                None,
                Some("the test daemon stopped"),
                OffsetDateTime::now_utc(),
            );
            match closed {
                Ok(_) | Err(task_core::chat::ChatError::Conflict(_)) => {}
                Err(error) => panic!("close the leftover run {run}: {error:?}"),
            }
        }
    }

    /// 偽の CoS の run を 1 つ起こし、終わるのを待つ。終わると daemon の tick が digest pass を走らせる。
    async fn poke(&mut self) {
        let store = &self.env.store;
        let now = OffsetDateTime::now_utc();
        let thread = match &self.poke_thread {
            Some(thread) => thread.clone(),
            None => {
                let thread = project_thread(store, "poke");
                self.poke_thread = Some(thread.clone());
                thread
            }
        };
        self.pokes += 1;
        store
            .chat_message_post(
                &thread,
                &ChatPostMessageRequest {
                    client_message_id: format!("poke-{}", self.pokes),
                    text: "digest を起こす".into(),
                    attachment_ids: vec![],
                    reply_to_id: None,
                    mode: ChatSendMode::Queue,
                    resume_queue: false,
                },
                now,
            )
            .expect("message");
        let want = self.pokes;
        wait_for("the fake CoS run finished", || async {
            let replies = store
                .chat_message_list(
                    &thread,
                    &ChatMessageQuery {
                        limit: Some(200),
                        ..Default::default()
                    },
                )
                .expect("messages")
                .items
                .into_iter()
                .filter(|m| m.role == ChatMessageRole::Assistant)
                .count();
            (replies >= want).then_some(())
        })
        .await;
    }

    /// SIGTERM で止める（`systemctl restart` と同じ終わり方）。tick_loop は signal を受けて `Exit::Signal` を返す。
    async fn stop_daemon(&mut self) {
        let Some(daemon) = self.daemon.take() else {
            return;
        };
        let status = std::process::Command::new("kill")
            .args(["-TERM", &std::process::id().to_string()])
            .status()
            .expect("kill");
        assert!(status.success());
        let exit = tokio::time::timeout(LONG, daemon)
            .await
            .expect("the daemon stopped")
            .expect("daemon task")
            .expect("daemon run");
        assert_eq!(exit, Exit::Signal);
    }

    fn row(&self) -> ScriptDomainModeRow {
        self.env.store.script_domain_mode(DOMAIN).expect("mode")
    }

    fn generation_id(&self, sha256: &str) -> String {
        self.env
            .store
            .script_generation_by_sha256(sha256)
            .expect("read")
            .expect("generation")
            .id
    }

    fn state_of(&self, sha256: &str) -> ScriptGenerationState {
        self.env
            .store
            .script_generation_by_sha256(sha256)
            .expect("read")
            .expect("generation")
            .state
    }

    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        bearer: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(bearer);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.expect("request");
        let status = response.status().as_u16();
        (
            status,
            response.json::<Value>().await.unwrap_or(Value::Null),
        )
    }

    /// 登録（人の token）。起動の直後の 503 と、切替の直後の登録簿の上限（409。見回りが旧を捨てるまで）は待つ。
    async fn register(&self, sha256: &str) -> Value {
        let result = wait_for("generation registered", || async {
            let (status, body) = self
                .request(
                    reqwest::Method::POST,
                    "/script/generations",
                    HUMAN_TOKEN,
                    Some(json!({"bundle_sha256": sha256})),
                )
                .await;
            match status {
                200 => Some(body),
                503 | 409 => None,
                other => panic!("register: {other} {body}"),
            }
        })
        .await;
        assert_eq!(result["state"], "loaded", "{result}");
        result
    }

    fn mode_body(&self, domain: &str, mode: &str, sha256: Option<&str>, bps: Option<u32>) -> Value {
        let version = self
            .env
            .store
            .script_domain_mode(domain)
            .expect("mode")
            .version;
        let mut body = json!({
            "mode": mode,
            "expected_version": version,
            "reason": "script_generation_e2e",
        });
        if let Some(sha256) = sha256 {
            body["generation"] = json!(sha256);
        }
        if let Some(bps) = bps {
            body["canary_bps"] = json!(bps);
        }
        body
    }

    async fn put_mode_as(
        &self,
        bearer: &str,
        domain: &str,
        mode: &str,
        sha256: Option<&str>,
        bps: Option<u32>,
    ) -> (u16, Value) {
        self.request(
            reqwest::Method::PUT,
            &format!("/script/domains/{domain}/mode"),
            bearer,
            Some(self.mode_body(domain, mode, sha256, bps)),
        )
        .await
    }

    /// 人の token で mode を変える（200 を要求する）。
    async fn human_mode(&self, domain: &str, mode: &str, sha256: Option<&str>, bps: Option<u32>) {
        let (status, body) = self
            .put_mode_as(HUMAN_TOKEN, domain, mode, sha256, bps)
            .await;
        assert_eq!(status, 200, "{domain} → {mode}: {body}");
    }

    /// 昇格の前提（(f) の「進む条件」）を満たすまで成功の呼び出しを数える。実の pass を数百回回す代わりに計数だけ積む
    /// （`timeout` が窓に入っていれば、率が下がるまで積む）。
    fn observe_until_ready(&self, sha256: &str, from: ScriptMode) {
        let id = self.generation_id(sha256);
        let ts_side = from == ScriptMode::Canary;
        loop {
            let counters = self
                .env
                .store
                .script_guard_counters(ScriptDomain::CosDigest, &id)
                .expect("counters")
                .map(|(_, counters)| counters)
                .unwrap_or_default();
            if readiness(
                ScriptDomain::CosDigest,
                from,
                &counters,
                task_core::script_exhaustive::ScriptExhaustiveStanding::NotApplicable,
            )
            .ready
            {
                return;
            }
            self.env
                .store
                .script_guard_observe(
                    ScriptDomain::CosDigest,
                    &id,
                    ScriptGuardObservation {
                        call: ScriptGuardCall::Ok,
                        safety_violation: false,
                        ts_side,
                    },
                    OffsetDateTime::now_utc(),
                )
                .expect("observe");
        }
    }

    /// 完了した triage run を 1 件入れ、daemon の digest を待つ。記録が残る mode ならその記録を返す。
    async fn digest_run(
        &mut self,
        run_id: &str,
        expect_record: bool,
    ) -> Option<CosDigestCompareRecord> {
        seed_run(&self.env.store, run_id);
        self.settle_digest(run_id, expect_record).await
    }

    /// 終わった triage run の digest を起こして待つ。
    async fn settle_digest(
        &mut self,
        run_id: &str,
        expect_record: bool,
    ) -> Option<CosDigestCompareRecord> {
        self.runs.push((run_id.to_owned(), expect_record));
        self.poke().await;
        wait_for("digest message", || async {
            (self.digest_count(run_id) > 0).then_some(())
        })
        .await;
        if !expect_record {
            return None;
        }
        Some(
            wait_for("compare record", || async {
                self.records(run_id).into_iter().next()
            })
            .await,
        )
    }

    fn digest_count(&self, run_id: &str) -> usize {
        let key = format!("{DIGEST_KEY_PREFIX}{run_id}");
        self.digests().iter().filter(|k| **k == key).count()
    }

    /// general thread の digest message の冪等 key（1 message 1 行）。全 page を古い順に読む: daemon 自身の triage run
    /// （偽の CoS）は待ちのある thread を毎 tick 拾うので（ADR 2026-11-11-cos-fallback-timing D3）、message は新しい 200 件
    /// （1 page の上限）に収まらないことがある。
    fn digests(&self) -> Vec<String> {
        let thread = general_thread(&self.env.store);
        let mut keys = Vec::new();
        let mut after = Some(0);
        while let Some(after_seq) = after {
            let page = self
                .env
                .store
                .chat_message_list(
                    &thread,
                    &ChatMessageQuery {
                        after_seq: Some(after_seq),
                        limit: Some(200),
                        ..Default::default()
                    },
                )
                .expect("messages");
            after = page.next_after_seq;
            keys.extend(
                page.items
                    .into_iter()
                    .filter(|m| m.role == ChatMessageRole::Assistant)
                    .filter_map(|m| m.client_message_id)
                    .filter(|k| k.starts_with(DIGEST_KEY_PREFIX)),
            );
        }
        keys
    }

    fn log(&self) -> Vec<CosDigestCompareRecord> {
        self.env
            .store
            .script_compare_log(None)
            .expect("log")
            .into_iter()
            .map(|row| row.record)
            .collect()
    }

    fn records(&self, run_id: &str) -> Vec<CosDigestCompareRecord> {
        self.log()
            .into_iter()
            .filter(|r| r.run_id == run_id)
            .collect()
    }

    /// 予定していない戻しを判定する。`timeout` の条件なら [`Slow`]、他は試験の失敗。
    fn expect_mode(&self, mode: ScriptMode) -> Stage {
        let row = self.row();
        if row.mode == mode {
            return Ok(());
        }
        let last = last_domain_event(&self.env);
        let reason = match &last {
            Event::ScriptDomainModeChanged { reason, .. } => reason.clone(),
            other => panic!("{other:?}"),
        };
        if row.updated_by == "system:script-guard" && reason.starts_with("guard_timeout") {
            return Err(Slow(reason));
        }
        panic!("mode {:?} (want {mode:?}): {last:?}", row.mode);
    }

    /// `ts` の mode で、TS の command が apply された run が 1 件出るまで run を入れる（`timeout` なら Rust が apply する。
    /// それも正しい。timeout が 3 件続けば [`Slow`]）。
    async fn digest_until_ts_applied(&mut self, prefix: &str, sha256: &str) -> Stage {
        let id = self.generation_id(sha256);
        for n in 0..4 {
            let run_id = format!("{prefix}-{n}");
            let record = self.digest_run(&run_id, true).await.expect("record");
            assert_eq!(record.mode, ScriptMode::Ts, "{record:?}");
            assert_eq!(record.generation, id, "{record:?}");
            assert!(record.canary_selected, "{record:?}");
            self.expect_mode(ScriptMode::Ts)?;
            if record.applied_by == CosDigestAppliedBy::Ts {
                assert!(record.fallback.is_none(), "{record:?}");
                assert_eq!(record.outcome, CompareOutcome::Match, "{record:?}");
                self.ts_applied.push(id);
                return Ok(());
            }
            assert!(is_timeout(&record), "{record:?}");
        }
        Err(Slow(format!("{prefix}: no pass applied TS")))
    }

    /// 人以外の主体による `mode`（`canary`・`ts`）が 403 で、表も記録も変わらないこと（未承認の変更 0）。
    async fn assert_only_humans_may_promote(&mut self, mode: &str, sha256: &str, bps: Option<u32>) {
        let row = self.row();
        let events = domain_events(&self.env).len();

        // 外部 operator の token。
        let (status, body) = self
            .put_mode_as(OPERATOR_TOKEN, DOMAIN, mode, Some(sha256), bps)
            .await;
        assert_eq!(status, 403, "{body}");
        assert_eq!(body["code"], "script_mode_requires_human", "{body}");

        // CoS の run credential: `/cos/operations` は 403（rejected の記録が残る）、直接の PUT は 422 で通らない。
        let run = format!("run-cos-{mode}");
        let (items, bearer) = cos_bearer(&self.env.store, &run);
        let request = self.mode_body(DOMAIN, mode, Some(sha256), bps);
        let (status, body) = self
            .request(
                reqwest::Method::POST,
                "/cos/operations",
                &bearer,
                Some(json!({
                    "idempotency_key": format!("e2e-{mode}"),
                    "expected_revision": null,
                    "reason": "人が依頼した",
                    "policy_version": "1",
                    "request": {
                        "method": "PUT",
                        "path": format!("/api/v1/script/domains/{DOMAIN}/mode"),
                        "body": request,
                    },
                })),
            )
            .await;
        assert_eq!(status, 403, "{body}");
        assert_eq!(body["code"], "script_mode_requires_human", "{body}");
        let (status, body) = self
            .put_mode_as(&bearer, DOMAIN, mode, Some(sha256), bps)
            .await;
        assert_eq!(status, 422, "{body}");
        assert_eq!(body["code"], "cos_audit_context_required", "{body}");
        // 知らない token は 401。
        let (status, body) = self
            .put_mode_as("not-a-token", DOMAIN, mode, Some(sha256), bps)
            .await;
        assert_eq!(status, 401, "{body}");

        // system の主体には token が無い（config が拒む）。store も `system:*` の昇格を拒む。
        let bad_token = self.env.dir.path().join("system.toml");
        let text = std::fs::read_to_string(self.env.dir.path().join("config.toml"))
            .expect("config")
            .replace("operator:e2e", "system:e2e");
        std::fs::write(&bad_token, text).expect("bad config");
        let err = Config::load(&bad_token).expect_err("a system: token is refused");
        assert!(err.to_string().contains("system:e2e"), "{err}");
        let change = ScriptModeChange {
            domain: DOMAIN.into(),
            to_mode: if mode == "canary" {
                ScriptMode::Canary
            } else {
                ScriptMode::Ts
            },
            to_generation: Some(self.generation_id(sha256)),
            expected_version: row.version,
            actor: "system:script-guard".into(),
            reason: "script_generation_e2e".into(),
        };
        let now = OffsetDateTime::now_utc();
        let err = if mode == "canary" {
            self.env.store.script_domain_set_canary(&change, 5000, now)
        } else {
            self.env.store.script_domain_set_mode(&change, now)
        }
        .expect_err("system: may not promote");
        assert!(
            matches!(
                err,
                ScriptDomainError::CanaryRequiresHuman(_) | ScriptDomainError::TsRequiresHuman(_)
            ),
            "{err:?}"
        );

        assert_eq!(self.row(), row, "the table did not change");
        assert_eq!(domain_events(&self.env).len(), events, "no event was added");

        // CoS の run を終える（他の triage run と同じく digest される）。
        finish_seeded_run(&self.env.store, &run, &items);
        self.settle_digest(&run, true).await;
    }

    /// CoS を切った daemon で A を登録して `compare` にし、lane_rules の task の event に記録が残ることを確かめる。
    async fn stage_a_lane_rules_events(&mut self) {
        let a = self.a.clone();
        self.start_daemon(false).await;
        let registered = self.register(&a).await;
        let id = registered["id"].as_str().expect("id").to_owned();
        assert_eq!(self.state_of(&a), ScriptGenerationState::Loaded);
        assert_eq!(self.row().mode, ScriptMode::Rust);
        // rust → compare は認証された主体なら誰でも。この試験は人の token で行う。
        self.human_mode(DOMAIN, "compare", Some(&a), None).await;
        assert_eq!(self.state_of(&a), ScriptGenerationState::Active);
        self.human_mode(LANE, "compare", Some(&a), None).await;
        let task = self.env.insert_task();
        let records = wait_for("lane_rules compare record", || async {
            let records = self.env.records(&task);
            (!records.is_empty()).then_some(records)
        })
        .await;
        assert_eq!(records[0].generation, id, "{records:?}");
        let (status, body) = self
            .request(
                reqwest::Method::POST,
                &format!("/tasks/{}/cancel", task.id),
                HUMAN_TOKEN,
                Some(json!({})),
            )
            .await;
        assert_eq!(status, 200, "{body}");
        self.task = Some(task);
        self.human_mode(LANE, "rust", None, None).await;
        assert_eq!(
            self.state_of(&a),
            ScriptGenerationState::Active,
            "cos_digest still points at A"
        );
        self.stop_daemon().await;
    }

    async fn stage_a_compare_canary_ts(&mut self) -> Stage {
        let a = self.a.clone();
        let id = self.generation_id(&a);
        self.start_daemon(true).await;
        assert_eq!(self.row().mode, ScriptMode::Compare, "restored");
        warm(&self.pinned(&a)).await;

        let record = self
            .digest_run("run-a-compare", true)
            .await
            .expect("record");
        assert_eq!(record.mode, ScriptMode::Compare, "{record:?}");
        assert_eq!(record.generation, id, "{record:?}");
        assert_eq!(record.applied_by, CosDigestAppliedBy::Rust, "{record:?}");
        assert!(
            record.outcome == CompareOutcome::Match || is_timeout(&record),
            "{record:?}"
        );

        // 計数が足りないうちは人の token でも 422（表は変えない）。
        let (status, body) = self
            .put_mode_as(HUMAN_TOKEN, DOMAIN, "canary", Some(&a), Some(5000))
            .await;
        assert_eq!(status, 422, "{body}");
        assert_eq!(body["code"], "script_canary_not_ready", "{body}");
        assert_eq!(self.row().mode, ScriptMode::Compare);

        self.observe_until_ready(&a, ScriptMode::Compare);
        self.assert_only_humans_may_promote("canary", &a, Some(5000))
            .await;
        self.human_mode(DOMAIN, "canary", Some(&a), Some(5000))
            .await;
        assert_eq!(self.row().canary_bps, Some(5000));

        for n in 0..2 {
            let record = self
                .digest_run(&format!("run-a-canary-{n}"), true)
                .await
                .expect("record");
            assert_eq!(record.mode, ScriptMode::Canary, "{record:?}");
            assert_eq!(record.generation, id, "{record:?}");
            assert_eq!(record.canary_bps, Some(5000), "{record:?}");
            if record.applied_by == CosDigestAppliedBy::Ts {
                assert!(
                    record.canary_selected,
                    "only the TS side applies: {record:?}"
                );
            }
            self.expect_mode(ScriptMode::Canary)?;
        }

        self.observe_until_ready(&a, ScriptMode::Canary);
        self.assert_only_humans_may_promote("ts", &a, None).await;
        self.human_mode(DOMAIN, "ts", Some(&a), None).await;
        self.digest_until_ts_applied("run-a-ts", &a).await
    }

    fn pinned(&self, sha256: &str) -> Arc<dyn PolicyScript> {
        Arc::new(ScriptPolicy::pinned(
            self.registry.clone(),
            sha256.to_owned(),
        ))
    }

    /// B を登録して切り替える。切替の前に始めた A の pass は A で終わる。
    async fn stage_switch_to_b(&mut self) -> Stage {
        let (a, b) = (self.a.clone(), self.b.clone());
        let id_b = self.register(&b).await["id"]
            .as_str()
            .expect("id")
            .to_owned();
        assert_eq!(self.state_of(&b), ScriptGenerationState::Loaded);
        warm(&self.pinned(&b)).await;

        // 前の段の pass の後始末は、0 に戻るのを待てば済む。だが dispatcher は、runs を適用した pass の後に
        // 「空の走査」の pass をもう 1 回、次の tick で A へ送る（`cos_digest_pass` は `digest_due` を空の走査まで保つ。
        // `ts` の canary 選択では空の走査でも TS を呼ぶ）。その pass が待ちと start_pass の間に入ると in_flight が 2 になる。
        // この pass は dispatcher の tick の中で自分で終わるので、止めずに終わるのを待つ。
        wait_for("A has no pass in flight from earlier stages", || async {
            (self.registry.get(&a).expect("A").in_flight == 0).then_some(())
        })
        .await;

        // A の pass を daemon の isolate で始める。op の答えを止めるので in-flight のまま。
        let pinned_a = self.pinned(&a);
        let (pass, op) = start_pass(Arc::clone(&pinned_a)).await;
        // dispatcher の空の走査の pass が重なっていても、それが終われば残るのはこの 1 件だけ。
        wait_for("only the held pass remains in A", || async {
            (self.registry.get(&a).expect("A").in_flight == 1).then_some(())
        })
        .await;
        assert_eq!(self.registry.get(&b).expect("B").in_flight, 0);

        // `ts` の A → `compare` の B（`ts` の元は同じ generation の `canary` だけなので段を踏み直す。付記 S5-4）。
        self.human_mode(DOMAIN, "compare", Some(&b), None).await;
        assert_eq!(self.state_of(&a), ScriptGenerationState::Draining);
        assert_eq!(self.state_of(&b), ScriptGenerationState::Active);
        wait_for("the sweep drains A", || async {
            self.registry
                .get(&a)
                .expect("A keeps its isolate while a pass is in flight")
                .draining
                .then_some(())
        })
        .await;

        // 切替の後の要求は B へ行く。A の drain 中も digest は止まらない。
        // ただし dispatcher の tick が上の待ち（in_flight == 1）の後・切替の前に `ts` の A で pass を始めていると、
        // その call は held pass の後ろで 200 ms 待って打ち切られ、Rust が切替の後に入れた run も拾う。その記録は
        // A・`mode_changed`・Rust の apply になる（仕様。task-dispatch の
        // `cos_digest_pass_ts_switch_mid_pass_records_late_run_under_old_generation` が決定的に再現する）。
        // tick は 1 回に pass を 1 つしか走らせず、次の tick は mode を読み直すので、切替をまたぐ pass は高々 1 つ。
        // その記録が出たら、もう 1 件の run で切替の後に始まった pass の記録を取る。
        let id_a = self.generation_id(&a);
        let mut record = self
            .digest_run("run-b-compare", true)
            .await
            .expect("record");
        if record.generation == id_a {
            assert_eq!(record.mode, ScriptMode::Ts, "{record:?}");
            assert_eq!(record.applied_by, CosDigestAppliedBy::Rust, "{record:?}");
            assert_eq!(
                record.fallback.as_ref().map(|f| f.code.as_str()),
                Some("mode_changed"),
                "{record:?}"
            );
            record = self
                .digest_run("run-b-compare-after", true)
                .await
                .expect("record");
        }
        assert_eq!(record.generation, id_b, "{record:?}");
        assert_eq!(record.mode, ScriptMode::Compare, "{record:?}");
        wait_for("A holds only the held pass", || async {
            let held = self.registry.get(&a).expect("A still holds the pass");
            (held.in_flight == 1).then_some(held)
        })
        .await;
        assert_eq!(self.state_of(&a), ScriptGenerationState::Draining);
        // 新しい要求は drain 中の A に入らない。
        let refused = probe_pass(&pinned_a).await.expect_err("draining");
        assert_eq!(refused.kind, "generation_gone", "{refused:?}");

        // 旧の in-flight は旧で終わる（打ち切られていれば `generation_gone`）。
        let _ = op.reply.send(CosDigestOpReply::Ok(json!({"runs": []})));
        let output = pass.await.expect("pass task").expect("A finished the pass");
        assert_eq!(output, json!({"runs": []}));
        wait_for("A retired", || async {
            (self.state_of(&a) == ScriptGenerationState::Retired).then_some(())
        })
        .await;
        assert!(self.registry.get(&a).is_none(), "the isolate was dropped");
        let retired = generation_events(&self.env, &a)
            .into_iter()
            .next_back()
            .expect("retired event");
        assert_eq!(retired.to_state, Some(ScriptGenerationState::Retired));
        assert_eq!(retired.actor, "system:script-runtime");
        assert_eq!(retired.cancelled_in_flight, Some(0), "{retired:?}");

        // B も人の token で `canary` → `ts`。
        self.observe_until_ready(&b, ScriptMode::Compare);
        self.human_mode(DOMAIN, "canary", Some(&b), Some(5000))
            .await;
        self.observe_until_ready(&b, ScriptMode::Canary);
        self.human_mode(DOMAIN, "ts", Some(&b), None).await;
        self.digest_until_ts_applied("run-b-ts", &b).await
    }

    /// B の pass に error を注入する。script-guard が (f) の連続 3 件で `rust` へ戻す。
    async fn stage_error_rolls_back(&mut self) -> Stage {
        let b = self.b.clone();
        let id_b = self.generation_id(&b);
        let mut poisoned = 0;
        while self.row().mode == ScriptMode::Ts {
            assert!(poisoned < 30, "the guard did not stop generation B");
            let run_id = format!("{POISON_PREFIX}{poisoned}");
            poisoned += 1;
            let record = self.digest_run(&run_id, true).await.expect("record");
            assert_eq!(record.generation, id_b, "{record:?}");
            // TS の出力は使わず Rust が apply する（副作用の前の失敗）。
            assert_eq!(record.applied_by, CosDigestAppliedBy::Rust, "{record:?}");
            assert!(record.fallback.is_some(), "{record:?}");
            assert_eq!(record.outcome, CompareOutcome::TsError, "{record:?}");
        }
        let row = self.row();
        assert_eq!(row.mode, ScriptMode::Rust);
        assert_eq!(row.updated_by, "system:script-guard");
        let Event::ScriptDomainModeChanged { reason, .. } = last_domain_event(&self.env) else {
            panic!("not a mode change");
        };
        if reason.starts_with("guard_timeout") {
            return Err(Slow(reason));
        }
        assert!(reason.starts_with("guard_failure_streak"), "{reason}");
        wait_for("B retired after the guard", || async {
            (self.state_of(&b) == ScriptGenerationState::Retired).then_some(())
        })
        .await;

        // `rust` の間は Rust だけが digest し、記録は増えない。
        self.digest_run("run-rust-after-guard", false).await;
        Ok(())
    }

    /// `script_control` の開いた項目（読むだけ）。
    fn open_items(&self) -> Vec<ScriptInboxItem> {
        self.env.store.script_inbox_open().expect("inbox")
    }

    /// 再起動: guard の戻しは `rust` のまま（B を復元しない）、受信箱に項目が出る。人が A を戻して `ts` にし、再起動すると
    /// 同じ mode・generation を新しい isolate で復元する。
    async fn stage_restart_restores(&mut self) -> Stage {
        let (a, b) = (self.a.clone(), self.b.clone());
        self.stop_daemon().await;
        self.start_daemon(true).await;
        // 見回りは起動の直後に 1 回走る。
        let item = wait_for("guard inbox item", || async {
            self.open_items().into_iter().next()
        })
        .await;
        assert_eq!(item.actor, "system:script-guard", "{item:?}");
        assert_eq!(item.code, "guard_failure_streak", "{item:?}");
        assert_eq!(self.open_items().len(), 1, "one item for the rollback");
        let (status, inbox) = self
            .request(reqwest::Method::GET, "/inbox/items", HUMAN_TOKEN, None)
            .await;
        assert_eq!(status, 200, "{inbox}");
        assert!(
            inbox
                .to_string()
                .contains(&format!("script_control-{}", item.id)),
            "{inbox}"
        );
        assert_eq!(self.row().mode, ScriptMode::Rust, "the rollback sticks");
        assert_eq!(self.state_of(&b), ScriptGenerationState::Retired);
        self.digest_run("run-rust-after-restart", false).await;

        // known-good の A へ戻す（`retired` は登録し直して `loaded` に戻す。付記 S5-1・S5-3）。
        let id_a = self.generation_id(&a);
        assert_eq!(self.register(&a).await["id"], json!(id_a));
        self.human_mode(DOMAIN, "compare", Some(&a), None).await;
        warm(&self.pinned(&a)).await;
        self.observe_until_ready(&a, ScriptMode::Compare);
        self.human_mode(DOMAIN, "canary", Some(&a), Some(5000))
            .await;
        self.observe_until_ready(&a, ScriptMode::Canary);
        self.human_mode(DOMAIN, "ts", Some(&a), None).await;
        self.digest_until_ts_applied("run-a2-ts", &a).await?;

        let before_row = self.row();
        let before_events = domain_events(&self.env).len();
        let before_digests = self.digests();
        self.stop_daemon().await;

        self.start_daemon(true).await;
        wait_for("A restored", || async { self.registry.get(&a).map(|_| ()) }).await;
        assert_eq!(self.row(), before_row, "mode and generation are restored");
        assert_eq!(self.row().generation_id.as_deref(), Some(id_a.as_str()));
        assert_eq!(self.state_of(&a), ScriptGenerationState::Active);
        assert_eq!(
            domain_events(&self.env).len(),
            before_events,
            "the restart writes no mode change"
        );
        assert_eq!(
            self.digests(),
            before_digests,
            "the restart writes no second digest"
        );
        // 人の操作で guard の項目は閉じた。
        wait_for("guard item resolved", || async {
            self.open_items().is_empty().then_some(())
        })
        .await;
        warm(&self.pinned(&a)).await;
        self.digest_until_ts_applied("run-a3-ts", &a).await?;
        Ok(())
    }

    /// bundle を消して再起動: `rust` に戻り、受信箱に項目が出る。Rust が digest を続ける。
    async fn stage_bundle_removed(&mut self) -> Stage {
        let a = self.a.clone();
        self.stop_daemon().await;
        std::fs::remove_dir_all(deployed_dir(&self.state, &a)).expect("remove the bundle");
        self.start_daemon(true).await;
        let row = wait_for("startup rollback", || async {
            let row = self.row();
            (row.mode == ScriptMode::Rust).then_some(row)
        })
        .await;
        assert_eq!(row.updated_by, "system:script-startup");
        assert_eq!(row.generation_id, None);
        let item = wait_for("startup inbox item", || async {
            self.open_items().into_iter().next()
        })
        .await;
        assert_eq!(item.actor, "system:script-startup", "{item:?}");
        assert_eq!(item.code, "startup_bundle_unusable", "{item:?}");
        assert_eq!(item.domain.as_deref(), Some(DOMAIN), "{item:?}");
        assert_eq!(self.open_items().len(), 1, "{:?}", self.open_items());
        assert!(self.registry.get(&a).is_none());
        wait_for("A retired after the startup rollback", || async {
            (self.state_of(&a) == ScriptGenerationState::Retired).then_some(())
        })
        .await;
        self.digest_run("run-rust-without-bundle", false).await;
        Ok(())
    }

    /// 合格条件: 欠落 0・二重副作用 0・未承認の変更 0。
    fn assert_ledger(&self) {
        let (a, b) = (self.generation_id(&self.a), self.generation_id(&self.b));
        let store = &self.env.store;

        // 二重副作用 0: run ごとの digest message が 1 件。受信箱の項目から daemon 自身が起こした triage run（偽の CoS）も
        // digest されるので全体の数は決まらないが、同じ key の message は 2 件にならない。
        for (run, _) in &self.runs {
            assert_eq!(self.digest_count(run), 1, "{run}: {:?}", self.digests());
        }
        let mut keys = self.digests();
        keys.sort();
        let all = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), all, "a digest was written twice: {keys:?}");

        // 欠落 0: generation の要る mode の run は記録が 1 件、`rust` の run は 0 件。どの run の記録も 2 件にならない。
        for (run, recorded) in &self.runs {
            assert_eq!(
                self.records(run).len(),
                usize::from(*recorded),
                "{run}: {:?}",
                self.records(run)
            );
        }
        let mut logged: Vec<String> = self.log().into_iter().map(|r| r.run_id).collect();
        logged.sort();
        let all = logged.len();
        logged.dedup();
        assert_eq!(logged.len(), all, "a run was recorded twice: {logged:?}");
        // A・B とも TS の command が apply された。TS の apply は `canary`・`ts` の mode で、検証を通ったものだけ。
        for id in [&a, &b] {
            assert!(self.ts_applied.contains(id), "{id}: {:?}", self.ts_applied);
        }
        for record in self.log() {
            if record.applied_by == CosDigestAppliedBy::Ts {
                assert!(
                    matches!(record.mode, ScriptMode::Canary | ScriptMode::Ts),
                    "{record:?}"
                );
                assert!(record.safety_violations.is_empty(), "{record:?}");
                assert!(record.fallback.is_none(), "{record:?}");
            }
        }

        // 欠落 0・未承認の変更 0: mode の記録は予定の変更だけで、version は 1 ずつ増える。昇格の actor はすべて人。
        let events = domain_events(&self.env);
        let got: Vec<(ScriptMode, ScriptMode, Option<String>, String)> = events
            .iter()
            .map(|e| match e {
                Event::ScriptDomainModeChanged {
                    from_mode,
                    to_mode,
                    to_generation,
                    actor,
                    ..
                } => (*from_mode, *to_mode, to_generation.clone(), actor.clone()),
                other => panic!("{other:?}"),
            })
            .collect();
        use ScriptMode::{Canary, Compare, Rust, Ts};
        let step =
            |from, to, id: Option<&String>, actor: &str| (from, to, id.cloned(), actor.to_owned());
        let want = vec![
            step(Rust, Compare, Some(&a), HUMAN),
            step(Compare, Canary, Some(&a), HUMAN),
            step(Canary, Ts, Some(&a), HUMAN),
            step(Ts, Compare, Some(&b), HUMAN),
            step(Compare, Canary, Some(&b), HUMAN),
            step(Canary, Ts, Some(&b), HUMAN),
            step(Ts, Rust, None, "system:script-guard"),
            step(Rust, Compare, Some(&a), HUMAN),
            step(Compare, Canary, Some(&a), HUMAN),
            step(Canary, Ts, Some(&a), HUMAN),
            step(Ts, Rust, None, "system:script-startup"),
        ];
        assert_eq!(got, want);
        let versions: Vec<u64> = events
            .iter()
            .map(|e| match e {
                Event::ScriptDomainModeChanged { version, .. } => *version,
                other => panic!("{other:?}"),
            })
            .collect();
        for pair in versions.windows(2) {
            assert_eq!(pair[1], pair[0] + 1, "{versions:?}");
        }
        assert_eq!(versions.last().copied(), Some(self.row().version));

        // 欠落 0: generation の状態の遷移。
        use ScriptGenerationState::{Active, Draining, Loaded, Retired};
        let states =
            |sha: &str| -> Vec<(Option<ScriptGenerationState>, Option<ScriptGenerationState>)> {
                generation_events(&self.env, sha)
                    .into_iter()
                    .map(|e| (e.from_state, e.to_state))
                    .collect()
            };
        let lifecycle = vec![
            (None, Some(Loaded)),
            (Some(Loaded), Some(Active)),
            (Some(Active), Some(Draining)),
            (Some(Draining), Some(Retired)),
        ];
        assert_eq!(states(&self.b), lifecycle);
        let mut twice = lifecycle.clone();
        twice.push((Some(Retired), Some(Loaded)));
        twice.extend_from_slice(&lifecycle[1..]);
        assert_eq!(states(&self.a), twice);

        // `events`: lane_rules の compare の記録が task の event に残る。
        let task = self.task.as_ref().expect("task");
        let recorded = self
            .env
            .events(task)
            .into_iter()
            .filter(|e| matches!(e, Event::ScriptCompareRecorded { .. }))
            .count();
        assert!(recorded >= 1, "{:?}", self.env.events(task));
        assert_eq!(recorded, self.env.records(task).len());

        // 全体の停止は起きていない（検証の外の副作用 0）。
        assert_eq!(store.script_guard_halted().expect("halted"), None);
    }
}

/// drain の時計を止めた登録簿（daemon ごとに作る）。
fn new_registry() -> GenerationRegistry {
    GenerationRegistry::with_clock(RuntimeLimits::default(), Arc::new(ManualClock::new()))
}

/// 配備済みの `base` の本文に `wrapper` を足した bundle を配備し、hash を返す。manifest は hash だけが違う。
fn deploy_wrapped(state: &Path, base: &str, wrapper: &str) -> String {
    let dir = deployed_dir(state, base);
    let mut code = std::fs::read_to_string(dir.join("bundle.mjs")).expect("bundle");
    code.push_str(wrapper);
    let sha256 = sha256_hex(code.as_bytes());
    let mut manifest: Value =
        serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).expect("manifest"))
            .expect("manifest json");
    manifest["bundle_sha256"] = json!(sha256);
    let out = deployed_dir(state, &sha256);
    std::fs::create_dir_all(&out).expect("deployed dir");
    std::fs::write(out.join("bundle.mjs"), code).expect("bundle");
    std::fs::write(
        out.join("manifest.json"),
        serde_json::to_vec(&manifest).expect("manifest"),
    )
    .expect("manifest");
    sha256
}

/// 出来事が起きるまで待つ（[`LONG`] で失敗）。
async fn wait_for<T, F, Fut>(what: &str, mut f: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    let started = std::time::Instant::now();
    loop {
        if let Some(value) = f().await {
            return value;
        }
        assert!(
            started.elapsed() < LONG,
            "{what}: not seen after {:?}",
            started.elapsed()
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn is_timeout(record: &CosDigestCompareRecord) -> bool {
    record.outcome == CompareOutcome::TsError
        && record.error.as_ref().is_some_and(|e| e.kind == "timeout")
}

fn general_thread(store: &task_core::SqliteStore) -> String {
    store
        .cos_thread_open(
            &ChatOpenThreadRequest { project_id: None },
            OffsetDateTime::now_utc(),
        )
        .expect("general")
        .thread
        .id
}

/// 動いている triage run を 1 件、general thread に作り、渡した item の id を返す。
///
/// 受信箱の項目から daemon 自身が triage run（偽の CoS）を起こすことがある。その run が thread を使っている間は待つ。
/// daemon が試験の item を先に claim していたら、新しい item を入れてやり直す（item は daemon の run のものになり、run 名は
/// 試験の `id` が残る）。daemon の item が残っていれば試験の run に一緒に渡される。
fn claim_seeded_run(store: &task_core::SqliteStore, id: &str) -> Vec<String> {
    let started = std::time::Instant::now();
    for attempt in 0u64.. {
        if attempt % 20 == 0 {
            store
                .cos_triage_ingest_batch(
                    "script-generation-e2e",
                    id,
                    &[CosTriageSource {
                        source_kind: "notice".into(),
                        source_key: format!("{id}#{attempt}"),
                        source_revision: "1".into(),
                        source_event_id: None,
                        operation_id: None,
                        summary: format!("知らせ {id}"),
                        detail: String::new(),
                        policy_version: "1".into(),
                        task_id: None,
                    }],
                    OffsetDateTime::now_utc(),
                )
                .expect("ingest");
        }
        let now = OffsetDateTime::now_utc();
        match store.cos_triage_claim(&general_thread(store), id, now) {
            Ok(Some(claim)) => return claim.item_ids,
            Ok(None) | Err(task_core::chat::ChatError::Conflict(_)) => {}
            Err(error) => panic!("claim {id}: {error:?}"),
        }
        assert!(started.elapsed() < LONG, "{id}: could not claim a run");
        std::thread::sleep(Duration::from_millis(25));
    }
    unreachable!("the loop returns or panics");
}

/// CoS が書いたはずの結果（resolve）を書いて run を終える（LLM を呼ばない）。
fn finish_seeded_run(store: &task_core::SqliteStore, id: &str, items: &[String]) {
    let now = OffsetDateTime::now_utc();
    for item in items {
        store
            .cos_triage_resolve(item, "observed", None, Some("確認済み"), now)
            .expect("resolve");
    }
    store
        .chat_run_finish(id, ChatRunState::Completed, None, None, now)
        .expect("finish");
}

/// 完了した triage run を 1 件、general thread に作る。
fn seed_run(store: &task_core::SqliteStore, id: &str) {
    let items = claim_seeded_run(store, id);
    finish_seeded_run(store, id, &items);
}

/// 動いている CoS の triage run `run` と、その credential（`(item の id の列, API の bearer)`）。
fn cos_bearer(store: &task_core::SqliteStore, run: &str) -> (Vec<String>, String) {
    let items = claim_seeded_run(store, run);
    let bearer = task_api::cos::issue_run_bearer(
        store,
        &general_thread(store),
        run,
        time::Duration::hours(1),
    )
    .expect("issue");
    store
        .chat_run_record_policy(run, "1", "test")
        .expect("policy");
    (items, bearer)
}

/// 新しい project の CoS thread の id。
fn project_thread(store: &task_core::SqliteStore, key: &str) -> String {
    let now = OffsetDateTime::now_utc();
    let project = Project {
        id: ProjectId::new(),
        title: key.into(),
        request: key.into(),
        status: ProjectStatus::Active,
        workspace: None,
        archived_at: None,
        paused_from: None,
        auto_advance: false,
        slug: None,
        created_at: now,
        updated_at: now,
    };
    TaskStore::project_create(store, &project).expect("project");
    store
        .cos_thread_open(
            &ChatOpenThreadRequest {
                project_id: Some(project.id.to_string()),
            },
            now,
        )
        .expect("project thread")
        .thread
        .id
}

fn domain_events(env: &Env) -> Vec<Event> {
    let mut rows = env
        .store
        .script_domain_events(Some(ScriptDomain::CosDigest), 1000)
        .expect("events");
    rows.sort_by_key(|row| row.id);
    rows.into_iter().map(|row| row.event).collect()
}

fn last_domain_event(env: &Env) -> Event {
    domain_events(env).pop().expect("a mode change")
}

fn generation_events(
    env: &Env,
    sha256: &str,
) -> Vec<task_core::script_compare::ScriptGenerationEvent> {
    env.store
        .script_generation_events_after(0, 1000)
        .expect("events")
        .into_iter()
        .map(|row| row.event)
        .filter(|e| e.bundle_sha256.as_deref() == Some(sha256))
        .collect()
}

/// 新しい isolate の最初の呼び出しは遅い。fixture の 1 件で 1 回 pass を走らせて温める（dispatcher の deadline の前）。
async fn warm(policy: &Arc<dyn PolicyScript>) {
    let path = repo_root().join("control-ts/fixtures/cos_digest/cases.jsonl");
    let text = std::fs::read_to_string(&path).expect("fixture");
    let row: Value = serde_json::from_str(text.lines().next().expect("a case")).expect("line");
    let input: CosDigestInput = serde_json::from_value(row["input"].clone()).expect("input");
    let list = json!({ "runs": [{
        "thread": input.thread, "run_id": input.run_id, "run_state": input.run_state,
    }] });
    let read = serde_json::to_value(&input).expect("input json");
    let (sink, mut rx) = mpsc::channel::<CosDigestOpCall>(64);
    let server = tokio::spawn(async move {
        while let Some(op) = rx.recv().await {
            let reply = match op.op {
                CosDigestOp::ListRuns => CosDigestOpReply::Ok(list.clone()),
                CosDigestOp::ReadRun => CosDigestOpReply::Ok(read.clone()),
            };
            let _ = op.reply.send(reply);
        }
    });
    let call = CosDigestPassCall {
        pass_id: "01M6E2EWARM000000000000000".into(),
        sink,
    };
    policy.cos_digest_pass(call, LONG).await.expect("warm pass");
    server.abort();
}

/// `cos_digest` の pass を別の task で始め、op（`list_runs`）を受け取ったところで返す。返した op に答えるまで pass は
/// in-flight のまま。
async fn start_pass(
    script: Arc<dyn PolicyScript>,
) -> (
    tokio::task::JoinHandle<Result<Value, PolicyScriptError>>,
    CosDigestOpCall,
) {
    let (sink, mut rx) = mpsc::channel(4);
    let pass = tokio::spawn(async move {
        script
            .cos_digest_pass(
                CosDigestPassCall {
                    pass_id: "01M6E2EINFLIGHT00000000000".into(),
                    sink,
                },
                LONG,
            )
            .await
            .map(|reply| reply.output)
    });
    let op = rx.recv().await.expect("the pass asked for an op");
    (pass, op)
}

/// drain 中の generation に新しい要求が入らないことを確かめる要求。
async fn probe_pass(script: &Arc<dyn PolicyScript>) -> Result<Value, PolicyScriptError> {
    let (sink, _rx) = mpsc::channel(1);
    script
        .cos_digest_pass(
            CosDigestPassCall {
                pass_id: "01M6E2EPROBE00000000000000".into(),
                sink,
            },
            LONG,
        )
        .await
        .map(|reply| reply.output)
}

async fn scenario(world: &mut World) -> Stage {
    world.stage_a_lane_rules_events().await;
    world.stage_a_compare_canary_ts().await?;
    world.stage_switch_to_b().await?;
    world.stage_error_rolls_back().await?;
    world.stage_restart_restores().await?;
    world.stage_bundle_removed().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn script_generation_e2e_switch_error_rollback_restart() {
    if dist_bundle(TEST, DOMAIN).is_none() {
        return;
    }
    let mut last = None;
    for attempt in 1..=SCENARIO_ATTEMPTS {
        let mut world = World::new();
        let result = scenario(&mut world).await;
        world.stop_daemon().await;
        match result {
            Ok(()) => {
                world.assert_ledger();
                return;
            }
            Err(Slow(why)) => {
                eprintln!("attempt {attempt}: the host was too slow for the TS budget: {why}");
                last = Some(why);
            }
        }
    }
    panic!("every attempt hit the timeout criterion ({SCENARIO_ATTEMPTS}): {last:?}");
}
