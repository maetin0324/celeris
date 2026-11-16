//! ADR 2026-11-11 dispatcher-state-ownership D1: 本番の既定を持つ注入 port（壁時計・knowledge の到達性・eval の harness の版）。
//!
//! - `#[cfg(test)]` ではない。本番の既定（実時刻・`task_worker::probe_models` の HTTP GET・実行 file の `--version`）を持ち、
//!   試験と daemon は setter で差し替える。
//! - owner（`RoutingRuntime`・`ProjectOrchestration` など）はこの struct を持たない。要る値（今の Unix 秒・probe の結果）は
//!   `Dispatcher` の入口がここから読んで、owner の method に引数で渡す（D2 の 2）。
//! - **LLM は呼ばない**（knowledge の probe は `GET <base_url>/models` の 1 回だけ）。

use std::sync::Arc;

use task_worker::Reachability;
use time::OffsetDateTime;

/// ADR-0052 D1（Phase 65b で `bearer_token` を追加）: 到達性の検査のフック（差し替えられるようにして
/// ある。既定は本物の HTTP GET）。第 2 引数は `[knowledge.langmem].api_key_secret` から解決した
/// 平文のトークン（`llm-proxy` のように `/v1/models` が認証を要求する上流のため。値はログに出さない）。
pub type KnowledgeProbe = Arc<dyn Fn(&str, Option<&str>) -> Reachability + Send + Sync>;

/// 評価 run の harness の版の測り方（harness id → 生の `--version` の出力）。
pub(crate) type EvalVersionProbe = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// 壁時計の Unix 秒の関数。
pub(crate) type NowUnixFn = Arc<dyn Fn() -> i64 + Send + Sync>;

/// 本番の既定を持つ注入 port。field は private で、読むのも差し替えるのも method を通す。
pub(crate) struct ProbePorts {
    /// 壁時計の Unix 秒（ADR-0024 D3 のスコア計算・cooldown の期限。既定は実時刻）。
    now_unix: NowUnixFn,
    /// ADR-0052 D1（Phase 64）: 知識整理タスクを dispatch する直前に当てる到達性の検査
    /// （既定は `task_worker::probe_models`。試験は `set_knowledge` で差し替える）。
    knowledge: KnowledgeProbe,
    /// 評価 run の harness の版の測り方（試験用の差し替え。`None` なら呼び出し側が `probe_harness_version` を使う）。
    eval_version: Option<EvalVersionProbe>,
}

fn real_now_unix() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}

impl Default for ProbePorts {
    fn default() -> Self {
        Self {
            now_unix: Arc::new(real_now_unix),
            knowledge: Arc::new(|base_url, bearer_token| {
                task_worker::probe_models(base_url, task_worker::PROBE_TIMEOUT, bearer_token)
            }),
            eval_version: None,
        }
    }
}

impl ProbePorts {
    /// 今の Unix 秒（注入した時計）。
    pub(crate) fn now_unix(&self) -> i64 {
        (self.now_unix)()
    }

    /// 壁時計を差し替える。
    #[cfg(test)]
    pub(crate) fn set_now_unix(&mut self, f: NowUnixFn) {
        self.now_unix = f;
    }

    /// `base_url` の到達性を 1 回測る（cache は `RoutingRuntime` が持つ）。
    pub(crate) fn probe_knowledge(&self, base_url: &str, token: Option<&str>) -> Reachability {
        (self.knowledge)(base_url, token)
    }

    /// 到達性の検査を差し替える。
    pub(crate) fn set_knowledge(&mut self, probe: KnowledgeProbe) {
        self.knowledge = probe;
    }

    /// 差し替えた harness の版の測り方で測る。差し替えていなければ `None`（呼び出し側が既定の測り方を使う）。
    pub(crate) fn probe_eval_version(&self, harness_id: &str) -> Option<Option<String>> {
        self.eval_version.as_ref().map(|probe| probe(harness_id))
    }

    /// 評価 run の harness の版の測り方を差し替える。
    #[cfg(test)]
    pub(crate) fn set_eval_version(&mut self, probe: EvalVersionProbe) {
        self.eval_version = Some(probe);
    }
}

#[cfg(test)]
mod structure_guard_tests {
    /// 不変条件（ADR 2026-11-11 dispatcher-state-ownership D1・D2）: `ProbePorts` は表の 3 port だけを private に持ち、
    /// `Dispatcher` を受けず `dispatcher` module を使わない。
    /// 既存の試験で検出できない理由: port の差し替えの試験は振る舞いだけを見るので、field を pub にしても、
    /// method が Dispatcher の可変参照を受けても通る。
    #[test]
    fn structure_guard_probe_ports() {
        let src = include_str!("probe_ports.rs");
        let src = &src[..src
            .find("mod structure_guard_tests")
            .expect("the guard module is in the source")];
        let start = src
            .find("pub(crate) struct ProbePorts {")
            .expect("ProbePorts is defined");
        let body = &src[start..];
        let body = &body[..body.find("\n}\n").expect("the struct body ends")];
        let mut fields = Vec::new();
        for line in body.lines().skip(1) {
            let t = line.trim_start();
            if t.starts_with("///") || t.is_empty() {
                continue;
            }
            assert!(
                !t.starts_with("pub"),
                "a ProbePorts field is not private: {line}"
            );
            if let Some((name, _)) = t.split_once(": ") {
                fields.push(name.to_string());
            }
        }
        assert_eq!(fields, ["now_unix", "knowledge", "eval_version"]);
        let owner = ["Dis", "patcher"].concat();
        for bad in [
            format!("&mut {owner}"),
            format!("&{owner}"),
            format!("impl {owner}"),
            format!("crate::{}", owner.to_lowercase()),
            format!("super::{}", owner.to_lowercase()),
        ] {
            assert!(
                !src.contains(&bad),
                "probe_ports.rs must not contain {bad:?}"
            );
        }
    }
}
