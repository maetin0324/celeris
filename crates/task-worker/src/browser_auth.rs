//! Authentication orchestration, credential approval and operation resume policy.
use super::*;

pub(super) struct BrokerLiveSession {
    client: crate::browser_cdp_sink::UnixInjectionClient,
    session_id: String,
}

impl Drop for BrokerLiveSession {
    fn drop(&mut self) {
        let _ = self.client.unregister_live_session(&self.session_id);
    }
}

pub(super) fn broker_client(
    sup: &crate::browser_credential::CredentialSupervisor,
) -> Result<crate::browser_cdp_sink::UnixInjectionClient, &'static str> {
    let runtime = sup
        .runtime_dir
        .clone()
        .or_else(|| std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from))
        .ok_or("broker_unavailable")?;
    Ok(crate::browser_cdp_sink::UnixInjectionClient::new(
        celeris_credentiald::injection_ipc::injection_socket(&runtime),
    ))
}

pub(super) fn register_broker_session(
    sup: &crate::browser_credential::CredentialSupervisor,
    supervisor: &crate::browser_supervisor::Supervisor,
    session_id: &str,
) -> Result<BrokerLiveSession, &'static str> {
    let client = broker_client(sup)?;
    let controller_pid = std::process::id();
    let controller_start = crate::browser_runtime::process_starttime(controller_pid as i32)
        .ok_or("isolated_runtime_unavailable")?;
    let runtime_pid =
        u32::try_from(supervisor.runtime_pid()).map_err(|_| "isolated_runtime_unavailable")?;
    let runtime_start = crate::browser_runtime::process_starttime(supervisor.runtime_pid())
        .ok_or("isolated_runtime_unavailable")?;
    client
        .register_live_session(
            celeris_credentiald::injection_ipc::LiveSessionRegistration {
                session_id: session_id.into(),
                controller_pid,
                controller_start,
                runtime_pid,
                runtime_start,
            },
        )
        .map_err(|_| "isolated_runtime_unavailable")?;
    Ok(BrokerLiveSession {
        client,
        session_id: session_id.into(),
    })
}

/// The login tab after injection: the controller's own CDP session on it and the loader id of the
/// injected document (ADR 2026-10-09 credential username / post-login D2-2 checks both).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoginTab {
    pub(crate) own: String,
    pub(crate) loader: String,
}

/// ADR 2026-10-09 credential username / post-login D2-2: how long the close conditions may take.
pub(crate) const POST_LOGIN_TIMEOUT: Duration = crate::browser_cdp_sink::POST_LOGIN_WAIT;

pub(super) async fn inject_h3(
    relay: &crate::browser_shared_cdp::SharedCdp,
    broker: &mut crate::browser_cdp_sink::UnixInjectionClient,
    session_id: &str,
    auth_id: &str,
    lease_id: &str,
    origin: &str,
    trusted: &task_core::browser_wait::TrustedLogin,
) -> Result<LoginTab, &'static str> {
    use serde_json::json;
    let controller = relay.controller();
    let target = {
        let mut c = controller.lock().map_err(|_| "cdp_unavailable")?;
        c.controller_command("Target.createTarget", json!({"url":"about:blank"}), None)
            .map_err(|_| "cdp_unavailable")?["result"]["targetId"]
            .as_str()
            .ok_or("cdp_unavailable")?
            .to_owned()
    };
    broker
        .open_auth_section(
            celeris_credentiald::injection_ipc::AuthSectionRegistration {
                session_id: session_id.into(),
                auth_section_id: auth_id.into(),
                lease_id: lease_id.into(),
                exact_origin: origin.into(),
                cdp_target_id: target.clone(),
            },
        )
        .map_err(|_| "auth_section_open_failed")?;
    async {
        let own = {
            let mut c = controller.lock().map_err(|_| "cdp_unavailable")?;
            c.controller_command(
                "Target.attachToTarget",
                json!({"targetId":target,"flatten":true}),
                None,
            )
            .map_err(|_| "cdp_unavailable")?["result"]["sessionId"]
                .as_str()
                .ok_or("cdp_unavailable")?
                .to_owned()
        };
        {
            let mut c = controller.lock().map_err(|_| "cdp_unavailable")?;
            c.controller_command("Network.enable", json!({}), Some(&own))
                .map_err(|_| "navigation_failed")?;
            c.begin_login_navigation(&own, Duration::from_secs(15))
                .map_err(|_| "navigation_failed")?;
            let nav = c
                .controller_command(
                    "Page.navigate",
                    json!({"url":trusted.login_url}),
                    Some(&own),
                )
                .map_err(|_| "navigation_failed")?;
            if !nav["error"].is_null() || nav["result"]["errorText"].is_string() {
                return Err("navigation_failed");
            }
        }
        let request = crate::browser_cdp_sink::InjectionRequest {
            request_id: format!("inject-{auth_id}"),
            session_id: session_id.into(),
            cdp_target_id: target,
            frame_id: String::new(),
            loader_id: String::new(),
            exact_origin: origin.into(),
            redirect_chain: Vec::new(),
            selector: trusted.password_selector.clone(),
            field: "password".into(),
            auth_section_id: auth_id.into(),
            lease_id: lease_id.into(),
            username_selector: trusted.username_selector.clone(),
        };
        complete_trusted_login(&controller, broker, &own, request, trusted).await
    }
    .await
}

pub(super) async fn complete_trusted_login(
    controller: &Arc<std::sync::Mutex<crate::browser_cdp_sink::CdpController>>,
    broker: &mut (dyn crate::browser_cdp_sink::BrokerClient + Send),
    own: &str,
    mut request: crate::browser_cdp_sink::InjectionRequest,
    trusted: &task_core::browser_wait::TrustedLogin,
) -> Result<LoginTab, &'static str> {
    use serde_json::json;
    let until = controller
        .lock()
        .map_err(|_| "cdp_unavailable")?
        .login_deadline()
        .ok_or("navigation_failed")?;
    let (frame_id, loader_id, redirect_chain) = loop {
        let ready = {
            let mut c = controller.lock().map_err(|_| "cdp_unavailable")?;
            let ready = c
                .login_password_document(
                    own,
                    &request.exact_origin,
                    &trusted.password_selector,
                    trusted.username_selector.as_deref(),
                )
                .map_err(|e| {
                    if e == crate::browser_cdp_sink::InjectionError::Redirected {
                        "redirected"
                    } else {
                        "navigation_failed"
                    }
                })?;
            match ready {
                Some((frame, loader)) => Some((
                    frame,
                    loader,
                    c.login_redirect_chain(&request.exact_origin)
                        .map_err(|e| e.code())?,
                )),
                None => None,
            }
        };
        if let Some(document) = ready {
            break document;
        }
        if std::time::Instant::now() >= until {
            return Err("navigation_failed");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    request.frame_id = frame_id;
    request.loader_id = loader_id.clone();
    request.redirect_chain = redirect_chain;
    {
        let mut c = controller.lock().map_err(|_| "cdp_unavailable")?;
        c.inject(&request, own, broker).map_err(|e| e.code())?;
    }
    if let Some(selector) = &trusted.submit_selector {
        let expr = crate::browser_cdp_sink::login_submit_expression(selector)
            .map_err(|_| "submit_failed")?;
        let submitted = controller
            .lock()
            .map_err(|_| "cdp_unavailable")?
            .controller_command(
                "Runtime.evaluate",
                json!({"expression":expr,"returnByValue":true}),
                Some(own),
            )
            .map_err(|_| "submit_failed")?;
        if submitted["result"]["result"]["value"] != "ok" {
            return Err("submit_failed");
        }
        // requestSubmit starts navigation asynchronously. Keep H3 closed to
        // observation until the old document is gone, then field cleanup
        // can safely skip its now-invalid CDP object ID.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            let tree = controller
                .lock()
                .map_err(|_| "cdp_unavailable")?
                .controller_command("Page.getFrameTree", json!({}), Some(own));
            if tree.is_ok_and(|tree| tree["result"]["frameTree"]["frame"]["loaderId"] != loader_id)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    Ok(LoginTab {
        own: own.to_owned(),
        loader: loader_id,
    })
}

/// ADR 2026-10-09 credential username / post-login D2-2: wait (polling every 50 ms, at most
/// [`POST_LOGIN_TIMEOUT`]) until the login tab left the injected document for a `read_origins`
/// page without a password input. Observation stays stopped throughout; only a fixed code returns.
pub(crate) async fn await_post_login(
    controller: &Arc<std::sync::Mutex<crate::browser_cdp_sink::CdpController>>,
    tab: &LoginTab,
    idp_origin: &str,
    read_origins: &[String],
    consent: Option<&task_core::browser_wait::ConsentPolicy>,
    timeout: Duration,
) -> Result<bool, crate::browser_cdp_sink::PostLoginHeldInfo> {
    let mut wait = crate::browser_cdp_sink::PostLoginWait::new(
        &tab.own,
        &tab.loader,
        idp_origin,
        read_origins,
        consent,
        timeout,
    );
    loop {
        if let Some(outcome) = wait.step(controller) {
            return outcome;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// ADR 2026-10-09 credential username / post-login 付記 2026-10-10 / 2026-10-10b: the post-login
/// outcome as progress — `resumed` (and whether the consent button was pressed), or
/// `post_login_unconfirmed` with the fixed reason, the top origin's category and, on a consent
/// page, the consent form's control names (never a URL, label or page text). Both runtimes report
/// through this.
pub(crate) fn post_login_progress(
    sink: &dyn EventSink,
    outcome: &Result<bool, crate::browser_cdp_sink::PostLoginHeldInfo>,
) {
    let (msg, summary) = match outcome {
        Ok(pressed) => (
            format!(
                "browser.post_login: resumed{}",
                if *pressed { " (consent_pressed)" } else { "" }
            ),
            "resumed",
        ),
        Err(held) => {
            let mut detail = format!(
                "reason={}, top={}",
                held.reason.code(),
                held.reason.top_origin()
            );
            if held.consent_pressed {
                detail.push_str(", consent_pressed");
            }
            if !held.consent_controls.is_empty() {
                detail.push_str(", consent_controls=");
                detail.push_str(&crate::browser_cdp_sink::format_consent_controls(
                    &held.consent_controls,
                ));
            }
            (
                format!("browser.post_login: post_login_unconfirmed ({detail})"),
                held.reason.code(),
            )
        }
    };
    sink.progress_with(
        &msg,
        &ProgressFields {
            kind: Some(ProgressKind::ToolResult),
            tool: Some("browser.post_login".into()),
            summary: Some(summary.into()),
            error: outcome.is_err(),
            ..Default::default()
        },
    );
}

/// ADR 2026-10-09 credential username / post-login D2-1・D2-5: what the agent may read after the
/// login of an opted-in site. `read_origins` is the site policy's list narrowed to the task's
/// allowed domains, `actions` the shim verbs of `post_login.actions` that the task policy (task ∩
/// grant) allows. Either empty means no opt-in (H3 until the session ends).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PostLoginRead {
    pub read_origins: Vec<String>,
    pub actions: Vec<String>,
}

/// The effective post-login read and the harness `policy.json` that goes with it, or `None` when
/// the pinned trusted login has no (effective) opt-in.
pub(crate) fn post_login_read(
    policy: &crate::browser_policy::PreparedBrowserPolicy,
    trusted: &task_core::browser_wait::TrustedLogin,
) -> Result<Option<(PostLoginRead, Vec<u8>)>, AdapterError> {
    let Some(post) = &trusted.post_login else {
        return Ok(None);
    };
    let read_origins: Vec<String> = post
        .read_origins
        .iter()
        .filter(|o| origin_in_domains(o, policy.allowed_domains()))
        .cloned()
        .collect();
    let mut file: task_core::AgentBrowserActionPolicy =
        serde_json::from_slice(&policy.action_policy)
            .map_err(|_| AdapterError::Other("browser policy rejected".into()))?;
    let granted = file.allow.clone();
    let actions: Vec<task_core::browser_wait::PostLoginAction> = post
        .actions
        .iter()
        .copied()
        .filter(|a| granted.iter().any(|g| g == a.upstream_action()))
        .collect();
    if read_origins.is_empty() || actions.is_empty() {
        return Ok(None);
    }
    file.allow.retain(|a| {
        let gated = OBSERVATION_UPSTREAM_ACTIONS.contains(&a.as_str()) || a == "click";
        a != "auth_login"
            && a != task_core::browser::CREDENTIAL_PLUGIN_ACTION
            && (!gated || actions.iter().any(|p| p.upstream_action() == a))
    });
    let bytes = serde_json::to_vec(&file)
        .map_err(|_| AdapterError::Other("browser policy rejected".into()))?;
    Ok(Some((
        PostLoginRead {
            read_origins,
            actions: actions.iter().map(|a| a.as_str().to_owned()).collect(),
        },
        bytes,
    )))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CredentialRequest {
    pub(crate) policy_id: String,
    pub(crate) origin: String,
    pub(crate) purpose: String,
}

/// A concrete request origin must be covered by an effective allowed origin (scheme, host, port).
pub(super) fn origin_in_domains(origin: &str, domains: &[String]) -> bool {
    domains
        .iter()
        .any(|domain| task_core::browser::origin_covers(domain, origin))
}

pub(super) fn read_credential_request(
    path: &Path,
    policy: &crate::browser_policy::PreparedBrowserPolicy,
) -> Result<CredentialRequest, AdapterError> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 2048 {
        return Err(AdapterError::Other(
            "browser credential request invalid".into(),
        ));
    }
    let bytes = std::fs::read(path)?;
    let request: CredentialRequest = serde_json::from_slice(&bytes)
        .map_err(|_| AdapterError::Other("browser credential request invalid".into()))?;
    if !policy
        .effective
        .actions
        .contains(&task_core::BrowserAction::CredentialUse)
        || !policy
            .effective
            .credential_policy_ids
            .contains(&request.policy_id)
        || task_core::browser::normalize_https_origin(&request.origin).as_deref()
            != Some(&request.origin)
        || !origin_in_domains(&request.origin, policy.allowed_domains())
        || !task_core::browser_wait::valid_purpose(&request.purpose)
    {
        return Err(AdapterError::Other(
            "browser credential request denied".into(),
        ));
    }
    Ok(request)
}

/// ADR 2026-10-08 D2: the shim's `request-approval` record (`approval-request.json`). No secret.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ApprovalRequest {
    /// Shim verb (`click` / `download`).
    pub(crate) action: String,
    /// Snapshot ref (`@eN`) of the element the agent wants to act on.
    pub(crate) target: String,
    pub(crate) origin: String,
    pub(crate) purpose: String,
}

pub(super) fn valid_snapshot_ref(target: &str) -> bool {
    target.len() <= 16
        && target
            .strip_prefix("@e")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The operation an agent may ask approval for: a business action of the effective policy that
/// needs a per-run approval, on an exact HTTPS origin inside the allowed domains.
pub(crate) fn read_approval_request(
    path: &Path,
    policy: &crate::browser_policy::PreparedBrowserPolicy,
) -> Result<(task_core::BrowserAction, ApprovalRequest), AdapterError> {
    let invalid = || AdapterError::Other("browser approval request invalid".into());
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 2048 {
        return Err(invalid());
    }
    let bytes = std::fs::read(path)?;
    let request: ApprovalRequest = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    let action = task_core::BrowserAction::parse(&request.action)
        .map_err(|_| AdapterError::Other("browser approval request denied".into()))?;
    if !policy
        .effective
        .operation_approval_actions()
        .contains(&action)
        || !valid_snapshot_ref(&request.target)
        || task_core::browser::normalize_https_origin(&request.origin).as_deref()
            != Some(&request.origin)
        || !origin_in_domains(&request.origin, policy.allowed_domains())
        || !task_core::browser_wait::valid_purpose(&request.purpose)
    {
        return Err(AdapterError::Other(
            "browser approval request denied".into(),
        ));
    }
    Ok((action, request))
}

/// The durable wait for an approval request: the intent (action + digest of its target) is
/// frozen so the human decides on exactly what the agent asked for.
pub(crate) fn operation_wait(
    task_id: task_core::TaskId,
    run_id: &str,
    session_id: &str,
    policy: &crate::browser_policy::PreparedBrowserPolicy,
    action: task_core::BrowserAction,
    request: &ApprovalRequest,
) -> NewBrowserWait {
    let digest = Sha256::digest(format!("{} {}", action.as_str(), request.target));
    NewBrowserWait {
        work_unit_id: None,
        run_id: run_id.into(),
        session_id: session_id.into(),
        reason: BrowserWaitReason::WaitingForApproval,
        origin: request.origin.clone(),
        purpose: request.purpose.clone(),
        credential_policy_id: None,
        credential: None,
        operation: Some(OperationIntent {
            intent_id: format!("intent-{:x}", Sha256::digest(format!("{task_id}/{run_id}")))[..48]
                .to_string(),
            action: action.as_str().into(),
            args_digest: Some(format!("sha256:{digest:x}")),
        }),
        trusted_login: None,
        policy_revision: policy.binding.revision,
        policy_hash: policy.binding.hash.clone(),
        owner_id: None,
        ttl_secs: None,
        resume_key: format!("operation:{task_id}:{run_id}"),
    }
}

/// The question text both paths report for a credential-registration wait (intent only: no
/// credential value, no origin).
pub(crate) const CREDENTIAL_REQUEST_QUESTION: &str = "Browser credential registration requested";

/// The durable wait for the shim's `credential-request.json`: a `WaitingForAuth` wait that carries
/// only the intent (`origin` / `purpose` / `credential_policy_id`) and resumes the same logical
/// session through `auth:<task>:<run>`. Both the daemon path and the launcher runtime build it
/// here so the resume key and the frozen intent stay identical. `credential` is always `None`:
/// the secret is registered by the human, never carried by the wait.
pub(crate) fn credential_wait(
    task_id: task_core::TaskId,
    run_id: &str,
    session_id: &str,
    policy: &crate::browser_policy::PreparedBrowserPolicy,
    request: &CredentialRequest,
) -> NewBrowserWait {
    NewBrowserWait {
        work_unit_id: None,
        run_id: run_id.into(),
        session_id: session_id.into(),
        reason: BrowserWaitReason::WaitingForAuth,
        origin: request.origin.clone(),
        purpose: request.purpose.clone(),
        credential_policy_id: Some(request.policy_id.clone()),
        credential: None,
        operation: None,
        trusted_login: None,
        policy_revision: policy.binding.revision,
        policy_hash: policy.binding.hash.clone(),
        owner_id: None,
        ttl_secs: None,
        resume_key: format!("auth:{task_id}:{run_id}"),
    }
}

/// shim が run 後に runtime dir に残した request を durable wait に変える共有段。daemon 経路
/// （[`run_with_executable_attempt`]）と launcher 経路（`browser_launcher_run.rs::run`）が同じ
/// 呼び出しをする。
///
/// 見る順（両経路で同じ。ADR 2026-10-09 付記「launcher runtime の credential wait」）:
/// 1. `credential-request.json` があれば**それを先に**処理する。policy に合う request だけが
///    保存済み候補があれば使用承認、なければ `WaitingForAuth` wait を開いて `Terminal::Question` で止まる。
/// 2. `credential-request.json` が無いときだけ `approval-request.json` を見る
///    （`WaitingForApproval`、ADR 2026-10-08 D2）。
///
/// どちらの request も policy に合わなければ wait を開かず `Err`、wait が開けなければ `Err`
/// （fail closed）。返り値は (run の outcome, wait が開いたときの browser state)。
#[allow(clippy::too_many_arguments)]
pub(crate) fn shim_request_wait(
    runtime: &Path,
    task_id: task_core::TaskId,
    run_id: &str,
    session_id: &str,
    policy: &crate::browser_policy::PreparedBrowserPolicy,
    sink: &dyn EventSink,
    outcome: Result<RunOutcome, AdapterError>,
) -> (Result<RunOutcome, AdapterError>, Option<BrowserRunState>) {
    let question = |text: String| {
        Ok(RunOutcome {
            terminal: Terminal::Question { text },
            exit_code: None,
        })
    };
    if runtime.join("credential-request.json").exists() {
        return match read_credential_request(&runtime.join("credential-request.json"), policy) {
            Ok(intent) => {
                let mut wait = credential_wait(task_id, run_id, session_id, policy, &intent);
                if let Some(saved) = sink
                    .browser_saved_credential(&intent.policy_id, &intent.origin)
                    .filter(|s| origin_in_domains(&s.origin, policy.allowed_domains()))
                {
                    wait.origin = saved.origin;
                    wait.reason = BrowserWaitReason::WaitingForApproval;
                    wait.credential = Some(saved.reference);
                    wait.trusted_login = Some(saved.trusted_login);
                    wait.owner_id = Some(saved.owner_id);
                    wait.operation = Some(OperationIntent {
                        intent_id: format!("reuse:{task_id}:{run_id}"),
                        action: "credential_use".into(),
                        args_digest: None,
                    });
                    wait.resume_key = format!("reuse:{task_id}:{run_id}");
                }
                let reused = wait.reason == BrowserWaitReason::WaitingForApproval;
                match sink.browser_wait_open(&wait) {
                    Ok(()) => (
                        question(
                            if reused {
                                "Browser credential use approval requested"
                            } else {
                                CREDENTIAL_REQUEST_QUESTION
                            }
                            .into(),
                        ),
                        Some(if reused {
                            BrowserRunState::WaitingForApproval
                        } else {
                            BrowserRunState::WaitingForAuth
                        }),
                    ),
                    Err(e) => (wait_unopenable(&e), None),
                }
            }
            Err(e) => (Err(e), None),
        };
    }
    if runtime.join("approval-request.json").exists() {
        // ADR 2026-10-08 D2: freeze the requested operation in a durable wait; the run resumes
        // in this logical session after the human approves once.
        return match read_approval_request(&runtime.join("approval-request.json"), policy) {
            Ok((action, intent)) => {
                let wait = operation_wait(task_id, run_id, session_id, policy, action, &intent);
                match sink.browser_wait_open(&wait) {
                    Ok(()) => (
                        question(format!("Browser {} approval requested", action.as_str())),
                        Some(BrowserRunState::WaitingForApproval),
                    ),
                    Err(e) => (wait_unopenable(&e), None),
                }
            }
            Err(e) => (Err(e), None),
        };
    }
    (outcome, None)
}

/// Open a fresh manual-registration wait after an unusable saved approval or failed login.
pub(crate) fn credential_reentry(
    task_id: task_core::TaskId,
    run_id: &str,
    previous: &task_core::browser_wait::BrowserWait,
    policy: &crate::browser_policy::PreparedBrowserPolicy,
    sink: &dyn EventSink,
) -> Result<RunOutcome, AdapterError> {
    let intent = CredentialRequest {
        policy_id: previous
            .credential_policy_id
            .clone()
            .ok_or_else(|| policy_changed("policy_id_missing"))?,
        origin: previous.origin.clone(),
        purpose: previous.purpose.clone(),
    };
    let mut wait = credential_wait(
        task_id,
        run_id,
        &session_id(task_id, run_id),
        policy,
        &intent,
    );
    wait.owner_id = previous.owner_id.clone();
    wait.resume_key = format!("reentry:{}:{run_id}", previous.wait_id);
    sink.browser_wait_open(&wait)
        .map_err(|_| AdapterError::Other("browser credential wait could not be opened".into()))?;
    sink.browser_updated(&BrowserRun {
        task_id,
        run_id: run_id.into(),
        session_id: wait.session_id,
        state: BrowserRunState::WaitingForAuth,
        live_view_url: None,
        policy: Some(policy.binding.clone()),
    });
    Ok(RunOutcome {
        terminal: Terminal::Question {
            text: CREDENTIAL_REQUEST_QUESTION.into(),
        },
        exit_code: None,
    })
}

/// Failed Authenticate or held post-login: retire the exact credential before asking for input.
pub(crate) fn credential_failure_reentry(
    task_id: task_core::TaskId,
    run_id: &str,
    previous: &task_core::browser_wait::BrowserWait,
    policy: &crate::browser_policy::PreparedBrowserPolicy,
    sink: &dyn EventSink,
    sup: &crate::browser_credential::CredentialSupervisor,
    stopped: bool,
) -> Result<RunOutcome, AdapterError> {
    let reference = previous
        .credential
        .as_ref()
        .ok_or_else(|| policy_changed("credential_reference_missing"))?;
    crate::browser_credential::invalidate(sup, reference)?;
    if !stopped {
        return Err(AdapterError::Other("browser session cleanup failed".into()));
    }
    credential_reentry(task_id, run_id, previous, policy, sink)
}

/// 承認済みの credential_use wait が今の task policy にまだ合うかの照合（daemon 経路と launcher 経路で
/// 共有）。合わなければ承認を消費せずに拒否する（fail closed）。
pub(crate) fn check_approved_credential(
    wait: &task_core::browser_wait::BrowserWait,
    policy: &crate::browser_policy::PreparedBrowserPolicy,
    credentials: Option<&crate::browser_credential::CredentialSupervisor>,
) -> Result<(), AdapterError> {
    if wait.policy_hash != policy.binding.hash
        || wait.policy_revision != policy.binding.revision
        || !policy
            .effective
            .actions
            .contains(&task_core::BrowserAction::CredentialUse)
        || wait
            .credential_policy_id
            .as_ref()
            .is_none_or(|id| !policy.effective.credential_policy_ids.contains(id))
        || !origin_in_domains(&wait.origin, policy.allowed_domains())
        || credentials.is_none()
    {
        return Err(AdapterError::Other(
            "approved browser credential use denied".into(),
        ));
    }
    Ok(())
}

/// 最後の wait が登録済み（`Registered`）なら、credential 使用の承認 wait（`WaitingForApproval`、
/// operation `credential_use`、resume key `approval:<wait_id>`）を開いて `Terminal::Question` を返す
/// （ADR-0110 D2 / ADR-0116）。daemon 経路と launcher 経路が runtime を起動する**前に**同じ呼び出しを
/// する。policy に合わない登録・credential 参照の欠落・trusted login を引けない場合・wait を開けない
/// 場合は `Err`（fail closed）。describe 失敗・現行 site policy 不一致は再入力 wait に戻す。
/// 最後の wait が `Registered` でなければ `Ok(None)`。
pub(crate) fn registered_credential_approval(
    task_id: task_core::TaskId,
    run_id: &str,
    waits: &[task_core::browser_wait::BrowserWait],
    policy: &crate::browser_policy::PreparedBrowserPolicy,
    credentials: Option<&crate::browser_credential::CredentialSupervisor>,
    sink: &dyn EventSink,
) -> Result<Option<RunOutcome>, AdapterError> {
    let Some(registered) = waits
        .last()
        .filter(|w| w.state == BrowserWaitState::Registered)
    else {
        return Ok(None);
    };
    if registered.policy_hash != policy.binding.hash
        || registered.policy_revision != policy.binding.revision
        || !policy
            .effective
            .actions
            .contains(&task_core::BrowserAction::CredentialUse)
        || registered
            .credential_policy_id
            .as_ref()
            .is_none_or(|id| !policy.effective.credential_policy_ids.contains(id))
        || !origin_in_domains(&registered.origin, policy.allowed_domains())
    {
        return Err(AdapterError::Other(
            "browser credential approval request denied".into(),
        ));
    }
    let Some(credential) = registered.credential.clone() else {
        return Err(AdapterError::Other(
            "registered browser credential reference missing".into(),
        ));
    };
    let sup = credentials.ok_or_else(|| policy_changed("supervisor_missing"))?;
    let trusted_login =
        match crate::browser_credential::describe_policy(sup, &credential, &registered.origin) {
            Ok(trusted) => trusted,
            Err(_) => {
                return credential_reentry(task_id, run_id, registered, policy, sink).map(Some);
            }
        };
    {
        let mut check = registered.clone();
        check.trusted_login = Some(trusted_login.clone());
        if !sink.browser_saved_credential_valid(&check) {
            return credential_reentry(task_id, run_id, registered, policy, sink).map(Some);
        }
    }
    if trusted_login.policy_id != credential.policy_id {
        return Err(policy_changed("policy_id_mismatch"));
    }
    let wait = NewBrowserWait {
        work_unit_id: registered.work_unit_id.clone(),
        run_id: run_id.into(),
        session_id: session_id(task_id, run_id),
        reason: BrowserWaitReason::WaitingForApproval,
        origin: registered.origin.clone(),
        purpose: registered.purpose.clone(),
        credential_policy_id: registered.credential_policy_id.clone(),
        credential: Some(credential),
        operation: Some(OperationIntent {
            intent_id: format!("intent-{}", registered.wait_id),
            action: "credential_use".into(),
            args_digest: None,
        }),
        trusted_login: Some(trusted_login),
        policy_revision: policy.binding.revision,
        policy_hash: policy.binding.hash.clone(),
        owner_id: registered.owner_id.clone(),
        ttl_secs: None,
        resume_key: format!("approval:{}", registered.wait_id),
    };
    sink.browser_wait_open(&wait)
        .map_err(|_| AdapterError::Other("browser approval wait could not be opened".into()))?;
    sink.browser_updated(&BrowserRun {
        task_id,
        run_id: run_id.into(),
        session_id: wait.session_id,
        state: BrowserRunState::WaitingForApproval,
        live_view_url: None,
        policy: Some(policy.binding.clone()),
    });
    Ok(Some(RunOutcome {
        terminal: Terminal::Question {
            text: "Browser credential use approval requested".into(),
        },
        exit_code: None,
    }))
}

/// Which part of the site policy's login changed since it was pinned to the approval (a fixed
/// sub-reason, never a value), or `None` if they are the same.
pub(crate) fn trusted_login_difference(
    current: &task_core::browser_wait::TrustedLogin,
    pinned: &task_core::browser_wait::TrustedLogin,
) -> Option<&'static str> {
    if current == pinned {
        return None;
    }
    let (c, p) = (
        serde_json::to_value(current).unwrap_or_default(),
        serde_json::to_value(pinned).unwrap_or_default(),
    );
    let fields = [
        ("policy_id", "pinned_differs_policy_id"),
        ("revision", "pinned_differs_revision"),
        ("login_url", "pinned_differs_login_url"),
        ("password_selector", "pinned_differs_password_selector"),
        ("submit_selector", "pinned_differs_submit_selector"),
        ("username_selector", "pinned_differs_username_selector"),
        ("post_login", "pinned_differs_post_login"),
        ("consent", "pinned_differs_consent"),
    ];
    Some(
        fields
            .iter()
            .find(|(key, _)| c.get(key) != p.get(key))
            .map_or("pinned_differs_other", |(_, sub)| sub),
    )
}

/// A wait the store refused to open: fail closed, the run does not report a question it cannot
/// resume from.
pub(super) fn wait_unopenable(code: &str) -> Result<RunOutcome, AdapterError> {
    // The store's fixed code (e.g. `browser_wait_invalid:credential_origin`), never a value.
    let code = if code.len() <= 64
        && code
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b':')
    {
        code
    } else {
        "other"
    };
    tracing::warn!(code, "browser wait could not be opened");
    Err(AdapterError::Other(format!(
        "browser wait could not be opened ({code})"
    )))
}

/// The approved business action a resumed run may perform once, or `None` when the approved
/// wait is a credential use (handled by the credential path). An approval that no longer
/// matches the task's effective policy is refused (the approval is not consumed).
pub(crate) fn approved_operation(
    wait: &task_core::browser_wait::BrowserWait,
    policy: &crate::browser_policy::PreparedBrowserPolicy,
) -> Result<Option<task_core::BrowserAction>, AdapterError> {
    let Some(intent) = wait
        .operation
        .as_ref()
        .filter(|o| o.action != "credential_use")
    else {
        return Ok(None);
    };
    let denied = || AdapterError::Other("approved browser operation denied".into());
    let action = task_core::BrowserAction::parse(&intent.action).map_err(|_| denied())?;
    if wait.reason != BrowserWaitReason::WaitingForApproval
        || wait.policy_hash != policy.binding.hash
        || wait.policy_revision != policy.binding.revision
        || wait.credential.is_some()
        || !policy
            .effective
            .operation_approval_actions()
            .contains(&action)
        || !origin_in_domains(&wait.origin, policy.allowed_domains())
    {
        return Err(denied());
    }
    Ok(Some(action))
}

/// Shim verbs that still need an approval in this session (`config.json` `approval_actions`).
pub(crate) fn shim_approval_actions(
    policy: &crate::browser_policy::PreparedBrowserPolicy,
    approved: Option<task_core::BrowserAction>,
) -> Vec<String> {
    policy
        .effective
        .operation_approval_actions()
        .into_iter()
        .filter(|a| Some(*a) != approved)
        .map(|a| a.as_str().to_string())
        .collect()
}

/// `policy.json` bytes for a run that resumes an approved operation.
pub(crate) fn resumed_policy_bytes(
    policy: &crate::browser_policy::PreparedBrowserPolicy,
    approved: task_core::BrowserAction,
) -> Result<Vec<u8>, AdapterError> {
    let file = policy
        .effective
        .harness_action_policy_with_approved(Some(approved))
        .map_err(|e| AdapterError::Other(format!("browser policy rejected: {}", e.code())))?;
    serde_json::to_vec(&file).map_err(|_| AdapterError::Other("browser policy rejected".into()))
}
