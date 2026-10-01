use super::*;

const AP: &str = "01M2WTS3DKNZBSZ2JMVB4CZMBW";

fn item(path: &str, title: &str, scope: Option<&str>) -> IndexItem {
    IndexItem {
        path: path.into(),
        title: title.into(),
        scope: scope.map(str::to_string),
        ..IndexItem::default()
    }
}

fn layout() -> Layout {
    Layout::new(
        Some(vec![
            ProjectRef {
                id: AP.into(),
                slug: "agent-platform".into(),
                title: "agent-platform の自己改善".into(),
            },
            ProjectRef {
                id: "01M35WRV77A2JPYGERQGXF6V7K".into(),
                slug: "benchfs".into(),
                title: String::new(),
            },
        ]),
        ["clusters".to_string(), "hosts".to_string()],
        vec![
            item("user/profile.md", "人のプロフィール", Some("user")),
            item(
                "environment/clusters/pegasus.md",
                "pegasus の使い方",
                Some("environment"),
            ),
            item(
                "environment/clusters/pegasus-lm-stack.md",
                "Pegasus Qwen vLLM tunnel reachability",
                Some("environment"),
            ),
            item(
                "projects/agent-platform/design.md",
                "Celeris: 上位方針",
                Some(&format!("project:{AP}")),
            ),
        ],
        "2026-09-28",
    )
}

fn req<'a>(
    path: Option<&'a str>,
    scope: Option<&'a str>,
    title: &'a str,
    tags: &'a [String],
) -> PlacementRequest<'a> {
    PlacementRequest {
        op: None,
        path,
        scope,
        title,
        tags,
    }
}

#[test]
fn ulids_and_slugs_are_told_apart() {
    assert!(looks_like_ulid(AP));
    assert!(looks_like_ulid(&AP.to_ascii_lowercase()));
    assert!(!looks_like_ulid("agent-platform"));
    assert!(!looks_like_ulid("20260922t155154z-note"));
    assert!(is_valid_project_slug("agent-platform"));
    assert!(!is_valid_project_slug(&AP.to_ascii_lowercase()));
    assert!(!is_valid_project_slug("Agent"));
    assert!(!is_valid_project_slug("-a"));
}

#[test]
fn project_ids_resolve_to_slugs_and_unknown_projects_are_rejected() {
    let l = layout();
    assert_eq!(resolve_project(AP, &l).expect("id"), "agent-platform");
    assert_eq!(
        resolve_project(&AP.to_ascii_lowercase(), &l).expect("lower id"),
        "agent-platform"
    );
    assert_eq!(resolve_project("benchfs", &l).expect("slug"), "benchfs");
    let err = resolve_project("pluvio", &l).expect_err("unknown");
    assert!(err.to_string().contains("agent-platform, benchfs"), "{err}");
    // 案件を知らない（celerisctl）: 正しい綴りの slug は通し、ULID は拒否する。
    let offline = Layout {
        projects: None,
        ..l
    };
    assert_eq!(resolve_project("pluvio", &offline).expect("slug"), "pluvio");
    assert!(resolve_project(AP, &offline).is_err());
    assert_eq!(
        resolve_scope_label(&format!("project:{AP}"), &layout()).expect("label"),
        "project:agent-platform"
    );
    assert_eq!(
        resolve_scope_label("projects/agent-platform", &layout()).expect("path form"),
        "project:agent-platform"
    );
}

#[test]
fn project_id_scope_lands_in_the_slug_directory() {
    let l = layout();
    let tags = vec!["celeris".to_string()];
    let scope = format!("project:{AP}");
    let p = place(&req(None, Some(&scope), "Research framing", &tags), &l).expect("placed");
    assert_eq!(p.path, "projects/agent-platform/research-framing.md");
    assert_eq!(p.scope.as_deref(), Some("project:agent-platform"));
    // パスに案件 ID が入っていても slug に直す。
    let path = format!("projects/{AP}/celeris.md");
    let p = place(&req(Some(&path), None, "x", &tags), &l).expect("placed");
    assert_eq!(p.path, "projects/agent-platform/celeris.md");
    // 知らない案件は拒否。
    assert!(matches!(
        place(&req(None, Some("project:nope"), "x", &tags), &l),
        Err(PlacementError::UnknownProject { .. })
    ));
}

#[test]
fn environment_pages_need_a_category() {
    let l = layout();
    let none: Vec<String> = vec![];
    let err = place(
        &req(
            Some("environment/pegasus-qwen.md"),
            Some("environment"),
            "Qwen",
            &none,
        ),
        &l,
    )
    .expect_err("root");
    assert!(matches!(err, PlacementError::ScopeRoot { .. }), "{err}");
    assert!(err.to_string().contains("environment/{"), "{err}");
    assert!(matches!(
        place(&req(Some("environment/gpus/a.md"), None, "a", &none), &l),
        Err(PlacementError::UnknownEnvironmentCategory { .. })
    ));
    assert!(matches!(
        place(&req(None, Some("environment"), "Qwen tunnel", &none), &l),
        Err(PlacementError::NeedEnvironmentCategory { .. })
    ));
    // タグが既存ページの stem（pegasus）に当たれば、その分類に置く。
    let tags = vec!["pegasus".to_string(), "qwen".to_string()];
    let p = place(
        &req(None, Some("environment"), "Qwen forward target", &tags),
        &l,
    )
    .expect("category from tags");
    assert_eq!(p.path, "environment/clusters/qwen-forward-target.md");
    let tags = vec!["tool".to_string()];
    let p = place(&req(None, Some("environment"), "rg", &tags), &l).expect("tool");
    assert_eq!(p.path, "environment/tools/rg.md");
    let p = place(&req(Some("environment/README.md"), None, "環境", &none), &l).expect("readme");
    assert_eq!(p.scope.as_deref(), Some("environment"));
}

#[test]
fn scope_labels_must_match_the_directory() {
    let l = layout();
    let none: Vec<String> = vec![];
    assert!(matches!(
        place(
            &req(Some("user/a.md"), Some("project:benchfs"), "a", &none),
            &l
        ),
        Err(PlacementError::ScopeMismatch { .. })
    ));
    let p = place(&req(Some("projects/benchfs/a.md"), None, "a", &none), &l).expect("derive");
    assert_eq!(p.scope.as_deref(), Some("project:benchfs"));
    let p = place(&req(Some("projects/README.md"), None, "案件", &none), &l).expect("readme");
    assert_eq!(p.scope, None);
    assert!(matches!(
        place(&req(Some("projects/a.md"), None, "a", &none), &l),
        Err(PlacementError::ScopeRoot { .. })
    ));
    assert!(matches!(
        place(&req(Some("experience/a.md"), None, "a", &none), &l),
        Err(PlacementError::ExperienceLayout(_))
    ));
    assert!(matches!(
        place(&req(Some("notes/a.md"), None, "a", &none), &l),
        Err(PlacementError::UnknownTop(_))
    ));
    assert!(matches!(
        place(&req(Some("skills/x/SKILL.md"), None, "a", &none), &l),
        Err(PlacementError::Reserved(_))
    ));
    let p = place(&req(None, Some("experience"), "Flaky tests", &none), &l).expect("exp");
    assert_eq!(p.path, "experience/2026/09/flaky-tests.md");
}

#[test]
fn ulid_segments_are_rejected() {
    let l = layout();
    let none: Vec<String> = vec![];
    let path = format!("experience/2026/09/{}.md", AP.to_ascii_lowercase());
    assert!(matches!(
        place(&req(Some(&path), None, "a", &none), &l),
        Err(PlacementError::UlidSegment(_))
    ));
    let path = format!("user/note-{AP}.md");
    assert!(matches!(
        place(&req(Some(&path), None, "a", &none), &l),
        Err(PlacementError::UlidSegment(_))
    ));
}

#[test]
fn same_titles_and_user_canonical_pages_are_merged_into() {
    let l = layout();
    let none: Vec<String> = vec![];
    // user の正準ページ: 題名で当たる。
    let p = place(&req(None, Some("user"), "人のプロフィール", &none), &l).expect("canon");
    assert_eq!(p.path, "user/profile.md");
    // タグで当たる（日本語の題名で stem が作れない）。
    let tags = vec!["user".to_string(), "goals".to_string()];
    let p = place(&req(None, Some("user"), "研究の目標", &tags), &l).expect("tags");
    assert_eq!(p.path, "user/goals.md");
    // 明示のパスでも正準ページに向け直す。
    let tags = vec!["preferences".to_string()];
    let p = place(
        &req(Some("user/writing-style.md"), None, "書き方の好み", &tags),
        &l,
    )
    .expect("redirect");
    assert_eq!(p.path, "user/preferences.md");
    assert!(matches!(p.redirect, Some(Redirect::UserCanonical { .. })));
    // 同じ scope・同じ題名のページ（ラベルが案件 ID の古いページでも置き場から scope を取る）。
    let scope = format!("project:{AP}");
    let p = place(&req(None, Some(&scope), "Celeris: 上位方針", &none), &l).expect("same");
    assert_eq!(p.path, "projects/agent-platform/design.md");
    assert!(matches!(p.redirect, Some(Redirect::SameTitle { .. })));
    let p = place(
        &req(
            Some("environment/clusters/qwen.md"),
            None,
            "pegasus Qwen vLLM tunnel reachability",
            &none,
        ),
        &l,
    )
    .expect("same env");
    assert_eq!(p.path, "environment/clusters/pegasus-lm-stack.md");
    // 違う scope の同じ題名は別物。
    let p = place(
        &req(None, Some("project:benchfs"), "Celeris: 上位方針", &none),
        &l,
    )
    .expect("other scope");
    assert_eq!(p.path, "projects/benchfs/celeris.md");
    // update / merge は指定のページのまま。
    let p = place(
        &PlacementRequest {
            op: Some(CandidateOp::Update),
            ..req(
                Some("projects/agent-platform/other.md"),
                None,
                "Celeris: 上位方針",
                &none,
            )
        },
        &l,
    )
    .expect("update");
    assert_eq!(p.path, "projects/agent-platform/other.md");
    // 題名も タグも ASCII にならなければ path を求める。
    assert_eq!(
        place(&req(None, Some("project:benchfs"), "調査", &none), &l),
        Err(PlacementError::NoName)
    );
}

#[test]
fn slugs_are_derived_from_title_then_repo_then_id() {
    let none = |_: &str| false;
    assert_eq!(
        derive_project_slug(
            "agent-platform の自己改善",
            AP,
            Some("agent-platform"),
            &none
        ),
        "agent-platform"
    );
    assert_eq!(
        derive_project_slug("BenchFS 国際会議フルペーパー化", AP, Some("benchfs"), &none),
        "benchfs"
    );
    assert_eq!(
        derive_project_slug("調査", AP, Some("Pluvio"), &none),
        "pluvio"
    );
    assert_eq!(
        derive_project_slug("調査", AP, None, &none),
        "project-vb4czmbw"
    );
    let taken = |s: &str| s == "pluvio";
    assert_eq!(
        derive_project_slug(
            "Pluvio を基盤に",
            "01M2RCYVZH6RGX8RX0JP572BAT",
            None,
            &taken
        ),
        "pluvio-jp572bat"
    );
}

#[test]
fn the_hint_lists_categories_and_projects() {
    let hint = layout_hint(&layout());
    assert!(hint.contains("`project:agent-platform` = agent-platform の自己改善"));
    assert!(hint.contains("celeris, clusters, hosts, servers, tools"));
}
