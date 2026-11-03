//! `celerisctl script` — ADR 2026-11-02-ts-control-layer-boundary 付記 S2-5・S5-1・S5-4: TS 制御層の領域の mode・
//! generation・script-guard を daemon の HTTP API で読む・切り替える（DB は開かない）。
//!
//! - `script domains [--json]` — `GET /script/domains`（既定は表。`--json` で応答そのまま）。
//! - `script mode <domain> rust|compare|canary|ts --expected-version <n> [--generation <bundle_sha256>]
//!   [--canary-bps <1..9999>] [--mode-reason …]` — `PUT /script/domains/{domain}/mode`。
//! - `script generations list [--json]` — `GET /script/generations`。
//! - `script generations register <bundle_sha256>` — `POST /script/generations`。
//! - `script generations retire <id> [--retire-reason …]` — `POST /script/generations/{id}/retire`。
//! - `script guard status [--json]` — `GET /script/domains` の `guard_halted`。
//! - `script guard clear --clear-reason …` — `POST /script/guard/clear`。
//!
//! 認可は API が決める（CLI は権限を足さない）。`→canary`（比率の変更を含む）・`→ts`・`guard clear` は人の API token の
//! 主体だけが通り、外部 operator・MCP・CoS は 403 `script_mode_requires_human`。CoS の run では変更系を
//! `/cos/operations` に包み、`guard clear` は registry の EXCLUDED（human_only）で rejected になる。

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Subcommand, ValueEnum};
use serde_json::{Map, Value, json};

use super::cron::{ApiConfig, request};
use crate::error::CliError;
use crate::outln;

const DEFAULT_CONFIG: &str = "~/.config/celeris/config.toml";

#[derive(Subcommand, Debug, Clone)]
pub enum ScriptCommand {
    /// List each domain's mode, generation, canary ratio and version, and the guard halt.
    Domains(DomainsArgs),
    /// Switch a domain to `rust`, `compare`, `canary` or `ts` (compare-and-set on `--expected-version`).
    ///
    /// The stages are rust -> compare -> canary -> ts with the same generation. `canary` (including a
    /// change of `--canary-bps`) and `ts` are accepted from a human API token only, and only while the
    /// script-guard counts are ready; operator, MCP and CoS tokens get 403
    /// `script_mode_requires_human`. Rolling back (`rust`, or `canary`/`ts` -> `compare`) is open to any
    /// authenticated caller. While the guard is halted only `rust` is accepted (409).
    Mode(ModeArgs),
    /// List, register or retire TS control-layer generations (bundles).
    #[command(subcommand)]
    Generations(GenerationsCommand),
    /// Show or clear the script-guard halt.
    #[command(subcommand)]
    Guard(GuardCommand),
}

#[derive(Subcommand, Debug, Clone)]
pub enum GenerationsCommand {
    /// List the generations (oldest first) with their state.
    List(DomainsArgs),
    /// Register a deployed bundle: the daemon loads it into a new isolate and validates it.
    ///
    /// Deploy first with `script-deploy`. A hash that is not deployed yet is refused with 409
    /// `script_bundle_not_deployed` and nothing is recorded, so the same hash can be registered after
    /// the deploy. A bundle that fails validation is recorded `invalid` (422) and cannot be registered again.
    Register(RegisterArgs),
    /// Retire a `loaded` or `draining` generation (an `active` one is refused with 409).
    Retire(RetireArgs),
}

#[derive(Subcommand, Debug, Clone)]
pub enum GuardCommand {
    /// Print whether the script guard is halted and why.
    Status(DomainsArgs),
    /// Clear the script-guard halt (human API token only; operator, MCP and CoS get 403).
    Clear(GuardClearArgs),
}

#[derive(Args, Debug, Clone)]
pub struct DomainsArgs {
    /// Print the API response as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug, Clone)]
pub struct RegisterArgs {
    /// `bundle_sha256` printed by `script-deploy` (64 lowercase hex digits).
    pub bundle_sha256: String,
}

#[derive(Args, Debug, Clone)]
pub struct RetireArgs {
    /// Generation id (from `script generations list`).
    pub id: String,
    /// Reason recorded in `script_generation_retired`.
    #[arg(long = "retire-reason")]
    pub reason: Option<String>,
}

#[derive(Args, Debug, Clone)]
pub struct GuardClearArgs {
    /// Reason recorded in `script_guard_cleared` (required).
    #[arg(long = "clear-reason")]
    pub reason: String,
}

/// 切り替え先にできる mode（付記 S5-4）。可否（主体・元の段・計数・停止）は API が決める。
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeArg {
    Rust,
    Compare,
    Canary,
    Ts,
}

impl ModeArg {
    fn as_str(self) -> &'static str {
        match self {
            ModeArg::Rust => "rust",
            ModeArg::Compare => "compare",
            ModeArg::Canary => "canary",
            ModeArg::Ts => "ts",
        }
    }
}

#[derive(Args, Debug, Clone)]
pub struct ModeArgs {
    /// lane_rules | route_priority | cos_digest
    pub domain: String,
    pub mode: ModeArg,
    /// The `version` read from `script domains` (a stale value is refused with 409).
    #[arg(long)]
    pub expected_version: u64,
    /// `bundle_sha256` of the generation (required for `compare`, `canary` and `ts`; for `canary` the
    /// one in `compare`, for `ts` the one in `canary`).
    #[arg(long)]
    pub generation: Option<String>,
    /// Canary ratio in basis points, 1..9999 (required for `canary`, not allowed for other modes).
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..=9999))]
    pub canary_bps: Option<u32>,
    /// Reason recorded in `script_domain_mode_changed`.
    #[arg(long = "mode-reason")]
    pub mode_reason: Option<String>,
}

pub fn run(config_path: Option<PathBuf>, command: ScriptCommand) -> Result<ExitCode, CliError> {
    let config_path = config_path.unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG));
    let config = celeris::config::Config::load(&config_path)
        .map_err(|e| CliError::msg(format!("config {}: {e}", config_path.display())))?;
    let listen = config
        .api
        .listen
        .ok_or_else(|| CliError::msg("[api] listen is not configured"))?;
    let api = ApiConfig {
        base_url: format!("http://{listen}/api/v1"),
        token: config
            .api
            .client_token()
            .map_err(|e| CliError::msg(e.to_string()))?,
    };
    if let Some((method, path, body)) = request_of(&command) {
        let result = request(&api, method, &path, Some(body))?;
        print_json(&result)?;
        return Ok(ExitCode::SUCCESS);
    }
    match &command {
        ScriptCommand::Domains(args) => {
            let result = request(&api, "GET", "/script/domains", None)?;
            if args.json {
                print_json(&result)?;
            } else {
                for line in render_table(&result) {
                    outln!("{line}");
                }
            }
        }
        ScriptCommand::Generations(GenerationsCommand::List(args)) => {
            let result = request(&api, "GET", "/script/generations", None)?;
            if args.json {
                print_json(&result)?;
            } else {
                for line in render_generations(&result) {
                    outln!("{line}");
                }
            }
        }
        ScriptCommand::Guard(GuardCommand::Status(args)) => {
            let result = request(&api, "GET", "/script/domains", None)?;
            if args.json {
                print_json(&json!({"guard_halted": result["guard_halted"]}))?;
            } else {
                outln!("{}", render_guard(&result));
            }
        }
        _ => {}
    }
    Ok(ExitCode::SUCCESS)
}

/// The mutating request (method, path under `/api/v1`, body) of `script mode`, `generations
/// register|retire` and `guard clear` (`None` for the read-only subcommands). The direct run and the
/// CoS operation (`cos_mapped`) send the same request.
pub fn request_of(command: &ScriptCommand) -> Option<(&'static str, String, Value)> {
    match command {
        ScriptCommand::Domains(_)
        | ScriptCommand::Generations(GenerationsCommand::List(_))
        | ScriptCommand::Guard(GuardCommand::Status(_)) => None,
        ScriptCommand::Mode(args) => {
            let mut body = Map::new();
            body.insert("mode".into(), json!(args.mode.as_str()));
            body.insert("expected_version".into(), json!(args.expected_version));
            if let Some(generation) = &args.generation {
                body.insert("generation".into(), json!(generation));
            }
            if let Some(bps) = args.canary_bps {
                body.insert("canary_bps".into(), json!(bps));
            }
            if let Some(reason) = &args.mode_reason {
                body.insert("reason".into(), json!(reason));
            }
            Some((
                "PUT",
                format!("/script/domains/{}/mode", args.domain),
                Value::Object(body),
            ))
        }
        ScriptCommand::Generations(GenerationsCommand::Register(args)) => Some((
            "POST",
            "/script/generations".to_string(),
            json!({"bundle_sha256": args.bundle_sha256}),
        )),
        ScriptCommand::Generations(GenerationsCommand::Retire(args)) => {
            let mut body = Map::new();
            if let Some(reason) = &args.reason {
                body.insert("reason".into(), json!(reason));
            }
            Some((
                "POST",
                format!("/script/generations/{}/retire", args.id),
                Value::Object(body),
            ))
        }
        ScriptCommand::Guard(GuardCommand::Clear(args)) => Some((
            "POST",
            "/script/guard/clear".to_string(),
            json!({"reason": args.reason}),
        )),
    }
}

fn print_json(value: &Value) -> Result<(), CliError> {
    outln!(
        "{}",
        serde_json::to_string_pretty(value).map_err(|e| CliError::msg(e.to_string()))?
    );
    Ok(())
}

/// `GET /script/domains` の応答を人が読む表にする（領域・mode・canary の比率・版・bundle の hash の先頭 12 桁・更新者）。
/// 全体の停止があれば最後の行に書く。
pub(crate) fn render_table(view: &Value) -> Vec<String> {
    let mut lines = vec![format!(
        "{:<16} {:<8} {:>6} {:>7} {:<12} {}",
        "DOMAIN", "MODE", "BPS", "VERSION", "BUNDLE", "UPDATED_BY"
    )];
    for item in view["domains"].as_array().into_iter().flatten() {
        let s = |v: &Value| v.as_str().unwrap_or("-").to_string();
        let bundle: String = item["bundle_sha256"]
            .as_str()
            .map_or_else(|| "-".to_string(), |b| b.chars().take(12).collect());
        let bps = item["canary_bps"]
            .as_u64()
            .map_or_else(|| "-".to_string(), |b| b.to_string());
        lines.push(format!(
            "{:<16} {:<8} {:>6} {:>7} {:<12} {}",
            s(&item["domain"]),
            s(&item["mode"]),
            bps,
            item["version"].as_u64().unwrap_or(0),
            bundle,
            s(&item["updated_by"]),
        ));
    }
    if view["guard_halted"].is_string() {
        lines.push(render_guard(view));
    }
    lines
}

/// `GET /script/generations` の応答を人が読む表にする（id・状態・bundle の hash の先頭 12 桁・領域・更新時刻）。
pub(crate) fn render_generations(view: &Value) -> Vec<String> {
    let mut lines = vec![format!(
        "{:<28} {:<9} {:<12} {:<40} {}",
        "ID", "STATE", "BUNDLE", "DOMAINS", "UPDATED_AT"
    )];
    for item in view["generations"].as_array().into_iter().flatten() {
        let s = |v: &Value| v.as_str().unwrap_or("-").to_string();
        let bundle: String = item["bundle_sha256"]
            .as_str()
            .map_or_else(|| "-".to_string(), |b| b.chars().take(12).collect());
        let domains = item["domains"]
            .as_array()
            .map(|ds| {
                ds.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| "-".to_string());
        lines.push(format!(
            "{:<28} {:<9} {:<12} {:<40} {}",
            s(&item["id"]),
            s(&item["state"]),
            bundle,
            domains,
            s(&item["updated_at"]),
        ));
    }
    lines
}

/// script-guard の停止の 1 行。
pub(crate) fn render_guard(view: &Value) -> String {
    match view["guard_halted"].as_str() {
        Some(reason) => {
            format!("GUARD HALTED: {reason} (only `rust` is accepted; a human clears it)")
        }
        None => "guard: running".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(domain: &str, mode: ModeArg, generation: Option<&str>) -> ScriptCommand {
        ScriptCommand::Mode(ModeArgs {
            domain: domain.into(),
            mode,
            expected_version: 4,
            generation: generation.map(str::to_string),
            canary_bps: None,
            mode_reason: None,
        })
    }

    #[test]
    fn script_domain_mode_request_carries_mode_version_and_generation() {
        let (method, path, body) =
            request_of(&mode("lane_rules", ModeArg::Compare, Some("ab12"))).expect("mutating");
        assert_eq!(
            (method, path.as_str()),
            ("PUT", "/script/domains/lane_rules/mode")
        );
        assert_eq!(
            body,
            json!({"mode": "compare", "expected_version": 4, "generation": "ab12"})
        );
        let (_, _, body) =
            request_of(&mode("route_priority", ModeArg::Rust, None)).expect("mutating");
        assert_eq!(body, json!({"mode": "rust", "expected_version": 4}));
        let (_, path, body) =
            request_of(&mode("cos_digest", ModeArg::Ts, Some("cd34"))).expect("mutating");
        assert_eq!(path, "/script/domains/cos_digest/mode");
        assert_eq!(
            body,
            json!({"mode": "ts", "expected_version": 4, "generation": "cd34"})
        );
        assert!(request_of(&ScriptCommand::Domains(DomainsArgs { json: false })).is_none());
    }

    #[test]
    fn script_domain_table_shows_mode_version_and_short_bundle() {
        let view = json!({"domains": [
            {"domain": "lane_rules", "mode": "compare", "generation_id": "g1",
             "bundle_sha256": "0123456789abcdef0123", "version": 3,
             "updated_at": "t", "updated_by": "human:web"},
            {"domain": "route_priority", "mode": "rust", "generation_id": null,
             "bundle_sha256": null, "version": 0, "updated_at": "t", "updated_by": "migration"}
        ]});
        let lines = render_table(&view);
        assert_eq!(lines.len(), 3);
        assert!(lines[1].starts_with("lane_rules"));
        assert!(lines[1].contains("compare"));
        assert!(lines[1].contains("0123456789ab "));
        assert!(!lines[1].contains("0123456789abc"));
        assert!(lines[2].contains("route_priority"));
        assert!(lines[2].contains(" - "));
    }

    #[test]
    fn script_canary_generations_and_guard_requests() {
        let canary = ScriptCommand::Mode(ModeArgs {
            domain: "route_priority".into(),
            mode: ModeArg::Canary,
            expected_version: 7,
            generation: Some("ab12".into()),
            canary_bps: Some(500),
            mode_reason: Some("canary 5%".into()),
        });
        let (method, path, body) = request_of(&canary).expect("mutating");
        assert_eq!(
            (method, path.as_str()),
            ("PUT", "/script/domains/route_priority/mode")
        );
        assert_eq!(
            body,
            json!({"mode": "canary", "expected_version": 7, "generation": "ab12",
                   "canary_bps": 500, "reason": "canary 5%"})
        );
        let register = ScriptCommand::Generations(GenerationsCommand::Register(RegisterArgs {
            bundle_sha256: "f".repeat(64),
        }));
        assert_eq!(
            request_of(&register),
            Some((
                "POST",
                "/script/generations".to_string(),
                json!({"bundle_sha256": "f".repeat(64)})
            ))
        );
        let retire = ScriptCommand::Generations(GenerationsCommand::Retire(RetireArgs {
            id: "g1".into(),
            reason: None,
        }));
        assert_eq!(
            request_of(&retire),
            Some((
                "POST",
                "/script/generations/g1/retire".to_string(),
                json!({})
            ))
        );
        let clear = ScriptCommand::Guard(GuardCommand::Clear(GuardClearArgs {
            reason: "原因を直した".into(),
        }));
        assert_eq!(
            request_of(&clear),
            Some((
                "POST",
                "/script/guard/clear".to_string(),
                json!({"reason": "原因を直した"})
            ))
        );
        for read in [
            ScriptCommand::Generations(GenerationsCommand::List(DomainsArgs { json: true })),
            ScriptCommand::Guard(GuardCommand::Status(DomainsArgs { json: false })),
        ] {
            assert!(request_of(&read).is_none(), "{read:?}");
        }
    }

    #[test]
    fn script_generation_table_and_guard_line() {
        let view = json!({"generations": [
            {"id": "01GEN", "bundle_sha256": "0123456789abcdef0123", "state": "loaded",
             "domains": ["lane_rules", "cos_digest"], "updated_at": "t1"},
            {"id": "01OLD", "bundle_sha256": "ffff", "state": "invalid",
             "domains": [], "updated_at": "t0"}
        ]});
        let lines = render_generations(&view);
        assert_eq!(lines.len(), 3);
        assert!(lines[1].starts_with("01GEN"));
        assert!(lines[1].contains("loaded"));
        assert!(lines[1].contains("0123456789ab "));
        assert!(lines[1].contains("lane_rules,cos_digest"));
        assert!(lines[2].contains("invalid"));
        let halted = json!({"domains": [], "guard_halted": "mismatch in lane_rules"});
        assert!(render_guard(&halted).starts_with("GUARD HALTED: mismatch in lane_rules"));
        assert_eq!(render_table(&halted).len(), 2);
        assert_eq!(
            render_guard(&json!({"guard_halted": null})),
            "guard: running"
        );
    }

    /// CoS の run では変更系が `/cos/operations` に包まれる（`main.rs` の `cos_mapped` は `request_of` に `/api/v1` を足すだけ）。
    /// 登録・退役は cos-operator skill の操作表の ALLOWED の行で、`guard clear` は行に無い（registry の EXCLUDED・human_only で
    /// rejected になる）。CLI は権限を足さない。
    #[test]
    fn script_cos_wrapped_requests_match_the_operations_table() {
        let text = include_str!("../../../../config/skills/cos-operator/operations.md");
        let allowed = text.split("## 除外する操作").next().expect("table");
        let row = |method: &str, path: &str| {
            allowed.lines().any(|line| {
                line.split('|').nth(2).map(|c| c.trim().trim_matches('`'))
                    == Some(&format!("{method} /api/v1{path}"))
            })
        };
        let register = ScriptCommand::Generations(GenerationsCommand::Register(RegisterArgs {
            bundle_sha256: "a".repeat(64),
        }));
        let retire = ScriptCommand::Generations(GenerationsCommand::Retire(RetireArgs {
            id: "g1".into(),
            reason: None,
        }));
        let (m, p, _) = request_of(&register).expect("register");
        assert!(row(m, &p), "{m} {p}");
        let (m, _, _) = request_of(&retire).expect("retire");
        assert!(row(m, "/script/generations/<id>/retire"), "{m}");
        let clear =
            ScriptCommand::Guard(GuardCommand::Clear(GuardClearArgs { reason: "x".into() }));
        let (m, p, _) = request_of(&clear).expect("clear");
        assert!(!row(m, &p), "guard clear must not be an ALLOWED row");
    }
}
