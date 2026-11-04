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
    let (_, _, body) = request_of(&mode("route_priority", ModeArg::Rust, None)).expect("mutating");
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
    let clear = ScriptCommand::Guard(GuardCommand::Clear(GuardClearArgs { reason: "x".into() }));
    let (m, p, _) = request_of(&clear).expect("clear");
    assert!(!row(m, &p), "guard clear must not be an ALLOWED row");
}

fn every_command() -> Vec<ScriptCommand> {
    vec![
        ScriptCommand::Domains(DomainsArgs { json: false }),
        mode("lane_rules", ModeArg::Rust, None),
        mode("route_priority", ModeArg::Canary, Some("ab12")),
        ScriptCommand::Generations(GenerationsCommand::List(DomainsArgs { json: true })),
        ScriptCommand::Generations(GenerationsCommand::Register(RegisterArgs {
            bundle_sha256: "a".repeat(64),
        })),
        ScriptCommand::Generations(GenerationsCommand::Retire(RetireArgs {
            id: "g1".into(),
            reason: None,
        })),
        ScriptCommand::Guard(GuardCommand::Status(DomainsArgs { json: false })),
        ScriptCommand::Guard(GuardCommand::Clear(GuardClearArgs { reason: "x".into() })),
        exhaustive_run(),
        ScriptCommand::Exhaustive(ExhaustiveCommand::Status(ExhaustiveStatusArgs {
            domain: "lane_rules".into(),
            json: false,
        })),
    ]
}

fn exhaustive_run() -> ScriptCommand {
    ScriptCommand::Exhaustive(ExhaustiveCommand::Run(ExhaustiveRunArgs {
        domain: "lane_rules".into(),
        generation: "ab12".into(),
        reason: "本番で網羅".into(),
    }))
}

/// 網羅の run は POST で本文は generation と reason。status は GET だけで CoS の run でも直接読める。
/// CoS の run で run を包むと registry の EXCLUDED で 422 になる（`cos_operation_not_allowed`）ので、
/// operations.md の ALLOWED の行には無い。
#[test]
fn script_exhaustive_cos_read_only_and_wrapped() {
    assert_eq!(
        request_of(&exhaustive_run()),
        Some((
            "POST",
            "/script/domains/lane_rules/exhaustive-runs".to_string(),
            json!({"generation": "ab12", "reason": "本番で網羅"})
        ))
    );
    assert!(!cos_read_only(&exhaustive_run()));
    let status = ScriptCommand::Exhaustive(ExhaustiveCommand::Status(ExhaustiveStatusArgs {
        domain: "lane_rules".into(),
        json: true,
    }));
    assert!(cos_read_only(&status));
    assert!(request_of(&status).is_none());
    let text = include_str!("../../../../config/skills/cos-operator/operations.md");
    let allowed = text.split("## 除外する操作").next().expect("table");
    assert!(!allowed.contains("exhaustive-runs"), "must stay EXCLUDED");
}

#[test]
fn script_exhaustive_table_shows_standing_and_counts() {
    let view = json!({
        "domain": "lane_rules", "generation": "0123456789abcdef", "standing": "passed",
        "reference_sha256": "fedcba9876543210",
        "runs": [{"id": "01RUN", "state": "passed", "stop_code": null,
                  "bundle_sha256": "0123456789abcdef", "started_at": "t",
                  "counts": {"calls": 39366, "matches": 39366, "mismatches": 0,
                             "timeouts": 0, "failures": 0, "uncovered_inputs": 0}}]
    });
    let lines = render_exhaustive(&view);
    assert_eq!(lines.len(), 3);
    assert!(lines[0].contains("standing: passed"), "{}", lines[0]);
    assert!(
        lines[0].contains("generation: 0123456789ab "),
        "{}",
        lines[0]
    );
    assert!(lines[2].contains("39366"), "{}", lines[2]);
    assert!(lines[2].starts_with("01RUN"));
}

/// CoS の run で包まずに通るのは読み取り 4 つだけで、変更系（mode・register・retire・guard clear）は入らない。
#[test]
fn script_cos_read_only_is_domains_generations_list_and_guard_status() {
    let read: Vec<String> = every_command()
        .into_iter()
        .filter(cos_read_only)
        .map(|c| format!("{c:?}"))
        .collect();
    assert_eq!(read.len(), 4, "{read:?}");
    assert!(read[0].starts_with("Domains"));
    assert!(read[1].starts_with("Generations(List"));
    assert!(read[2].starts_with("Guard(Status"));
    assert!(read[3].starts_with("Exhaustive(Status"));
    for command in every_command() {
        if matches!(
            command,
            ScriptCommand::Mode(_)
                | ScriptCommand::Generations(
                    GenerationsCommand::Register(_) | GenerationsCommand::Retire(_)
                )
                | ScriptCommand::Guard(GuardCommand::Clear(_))
                | ScriptCommand::Exhaustive(ExhaustiveCommand::Run(_))
        ) {
            assert!(!cos_read_only(&command), "{command:?}");
        }
    }
}

/// 全 subcommand は「読み取り」か「`request_of` の要求（CoS の run では `/cos/operations` に包む）」のどちらか一方に入る。
/// 新しい subcommand をどちらにも入れ忘れると、CoS の run の gate で落ちる。両方に入ると権限を広げうる。
#[test]
fn script_cos_read_only_and_request_of_partition_every_subcommand() {
    for command in every_command() {
        assert_ne!(
            cos_read_only(&command),
            request_of(&command).is_some(),
            "{command:?}"
        );
        if cos_read_only(&command) {
            // 読み取りの CLI は GET だけを送る（`run` の match）。
            assert!(request_of(&command).is_none());
        }
    }
}

/// clap の help（`Command::render_long_help`）が CoS の run の 422 と operator・MCP の 403 を書き分ける。
#[test]
fn script_cos_read_only_guard_clear_help_names_403_and_422() {
    use clap::CommandFactory;
    #[derive(clap::Parser)]
    struct Wrap {
        #[command(subcommand)]
        command: GuardCommand,
    }
    let mut cmd = Wrap::command();
    // `script guard --help` の一覧に出るのは先頭段落だけなので、一覧にも 403 と 422 を書く。
    let listing = cmd.render_long_help().to_string();
    assert!(listing.contains("operator/MCP 403"), "{listing}");
    assert!(
        listing.contains("422 cos_operation_not_allowed"),
        "{listing}"
    );
    let clear = cmd.find_subcommand_mut("clear").expect("clear");
    let help = clear.render_long_help().to_string();
    assert!(help.contains("Operator and MCP tokens get 403"), "{help}");
    assert!(help.contains("422 `cos_operation_not_allowed`"), "{help}");
    assert!(!help.contains("CoS get 403"), "{help}");
}
