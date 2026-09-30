//! ADR-0091 D2: 管理者の site policy（`CredentialPolicy`）のログイン URL・trusted selector の形式検証と後方互換。
use celeris_credentiald::{CredentialPolicy, Error};

const ORIGIN: &str = "https://login.example.test";

fn policy() -> CredentialPolicy {
    CredentialPolicy {
        policy_id: "site-1".into(),
        revision: 1,
        exact_origin: ORIGIN.into(),
        task_id: "task-1".into(),
        max_ttl_seconds: 60,
        require_approval: true,
        allow_persistence: false,
        login_url: Some(format!("{ORIGIN}/login?next=%2F")),
        password_selector: Some("form#login > input[name=\"password\"]".into()),
        submit_selector: Some("button[type=submit]".into()),
    }
}

fn with(f: impl FnOnce(&mut CredentialPolicy)) -> Result<(), Error> {
    let mut p = policy();
    f(&mut p);
    p.validate()
}

#[test]
fn admin_login_url_and_selectors_are_accepted_when_well_formed() {
    assert_eq!(policy().validate(), Ok(()));
    assert_eq!(
        policy().trusted_password_selector(),
        Some("form#login > input[name=\"password\"]")
    );
    assert_eq!(with(|p| p.submit_selector = None), Ok(()));
    // selector を持たない policy も有効（ただし注入には使えない）。
    let legacy = with(|p| {
        p.login_url = None;
        p.password_selector = None;
        p.submit_selector = None;
    });
    assert_eq!(legacy, Ok(()));
}

#[test]
fn malformed_admin_selectors_are_rejected() {
    for bad in [
        "",
        "input, #other",
        "input:not([type=text])",
        "input::after",
        "*",
        "a + input",
        "a ~ input",
        "iframe >>> input",
        "iframe /deep/ input",
        "#pa\\ss",
        "css=input",
        "xpath=//input",
        "text=Password",
        "internal:role=textbox",
        "frame=login >> input",
        "@e12",
        "input[name='p']",
        "input\n#p",
        "ｉnput",
    ] {
        assert_eq!(
            with(|p| p.password_selector = Some(bad.into())),
            Err(Error::Invalid),
            "password {bad:?}"
        );
        assert_eq!(
            with(|p| p.submit_selector = Some(bad.into())),
            Err(Error::Invalid),
            "submit {bad:?}"
        );
    }
    let long = format!("#{}", "a".repeat(256));
    assert_eq!(
        with(|p| p.password_selector = Some(long)),
        Err(Error::Invalid)
    );
    let deep = vec!["div"; 9].join(" > ");
    assert_eq!(
        with(|p| p.password_selector = Some(deep)),
        Err(Error::Invalid)
    );
}

#[test]
fn malformed_or_foreign_login_urls_are_rejected() {
    for bad in [
        ORIGIN.to_string(),
        format!("{ORIGIN}.evil.test/login"),
        format!("{ORIGIN}@evil.test/login"),
        format!("{ORIGIN}:8443/login"),
        format!("{ORIGIN}/login#frag"),
        format!("{ORIGIN}/lo gin"),
        "http://login.example.test/login".into(),
        "https://evil.example.test/login".into(),
        format!("{ORIGIN}/{}", "a".repeat(2048)),
    ] {
        assert_eq!(
            with(|p| p.login_url = Some(bad.clone())),
            Err(Error::Invalid),
            "{bad}"
        );
    }
}

#[test]
fn login_url_and_password_selector_come_as_a_pair() {
    assert_eq!(with(|p| p.login_url = None), Err(Error::Invalid));
    assert_eq!(with(|p| p.password_selector = None), Err(Error::Invalid));
    assert_eq!(
        with(|p| {
            p.login_url = None;
            p.password_selector = None;
        }),
        Err(Error::Invalid),
        "submit selector alone"
    );
}

#[test]
fn legacy_policy_json_reads_without_selectors_and_unknown_fields_are_rejected() {
    let legacy = serde_json::json!({
        "policy_id": "site-1", "revision": 1, "exact_origin": ORIGIN, "task_id": "task-1",
        "max_ttl_seconds": 60, "require_approval": true, "allow_persistence": false,
    });
    let p: CredentialPolicy = serde_json::from_value(legacy.clone()).expect("legacy");
    assert_eq!(p.login_url, None);
    assert_eq!(p.trusted_password_selector(), None);
    assert_eq!(p.validate(), Ok(()));
    // 旧い形で書き出すと欄は出ない（skip_serializing_if）。
    assert_eq!(serde_json::to_value(&p).expect("ser"), legacy);
    // username selector はこの ADR の範囲外で、欄を持つ policy は読まない。
    let mut v = legacy;
    v["username_selector"] = "input[name=user]".into();
    assert!(serde_json::from_value::<CredentialPolicy>(v).is_err());
}
