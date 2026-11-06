//! 本番 DB の写しで tick の段別時間を測る（手動の計測。既定では走らない）。
//!
//! `TICK_PROD_DB` に DB の写しの path（読み取り専用の backup）を渡すと、その file を一時 dir へ複製し、
//! fake harness だけの非本番 dispatcher で `TICK_PROD_TICKS` 回（既定 5）tick して `step_us` の
//! 段ごとの時間（µs）を JSON 1 行ずつ出す。cluster・browser・通知・remote・LLM・外部接続は config に無い。
//! 元の file には書かない。一時 dir は試験の終わりに消える。

use std::time::Instant;

use celeris::{ClusterMasters, Config, build_dispatcher};

const CONFIG: &str = r#"
db = "celeris.sqlite3"
workspace_root = "ws"
tick_ms = 20
max_concurrency = 1

[api]
listen = "127.0.0.1:17998"

[[api.tokens]]
principal = "operator:test"
token_file = "api.token"

[cos]
enabled = true
max_turns = 2
max_wall_secs = 30
route_candidates = [{ source = "local", model = "model-1" }]

[adapters.fake]
command = ["sh", "-c", "cat >/dev/null"]

[llm_sources.local]
kind = "fake"
accounts = ["a1"]
models = ["model-1"]

[harnesses.claude_code]
kind = "fake"
api_formats = ["fake"]
source_kinds = ["fake"]

[route_defaults.claude_code]
candidates = [{ source = "local", model = "model-1" }]
"#;

#[tokio::test]
#[ignore = "手動の計測: TICK_PROD_DB=<DB の写し> cargo nextest run --run-ignored all -p celeris --test tick_prod_copy_steps"]
async fn prod_copy_tick_steps() {
    let Ok(src) = std::env::var("TICK_PROD_DB") else {
        eprintln!("TICK_PROD_DB is not set; skipped");
        return;
    };
    let ticks: usize = std::env::var("TICK_PROD_TICKS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("ws")).expect("ws");
    std::fs::write(dir.path().join("config.toml"), CONFIG).expect("config");
    std::fs::write(dir.path().join("api.token"), "prod-copy-token\n").expect("token");
    std::fs::copy(&src, dir.path().join("celeris.sqlite3")).expect("copy db");
    let config = Config::load(&dir.path().join("config.toml")).expect("config load");
    let mut d = build_dispatcher(&config, ClusterMasters::default()).expect("dispatcher");
    for i in 0..=ticks {
        let t = Instant::now();
        let report = d.tick().expect("tick");
        let total = t.elapsed().as_micros();
        let steps: Vec<String> = report
            .step_us
            .iter()
            .map(|(n, v)| format!("\"{n}\":{v}"))
            .collect();
        println!(
            "{{\"tick\":{i},\"total_us\":{total},\"steps\":{{{}}}}}",
            steps.join(",")
        );
    }
}
