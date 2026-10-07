// Generated from docs/api/v1/api-v1.schema.json by web/scripts/gen-types.mjs. Do not edit.
export type AccountCheckResponse = {
  "checked_at": string;
  "detail"?: string | null;
  "result": ProviderCheckResult;
  "usage"?: AccountUsageView | null;
};

export type AccountCooldownLive = {
  "reason": string;
  "until": number;
};

export type AccountCooldownView = {
  "reason": string;
  "until": string;
};

export type AccountList = {
  "items": Array<AccountView>;
  "max_runs_per_account": number;
  "root"?: string | null;
  "roots"?: {
  [key: string]: string | null;
};
};

export type AccountLive = {
  "adapter"?: string;
  "cooldown"?: AccountCooldownLive | null;
  "excluded_reason"?: string | null;
  "id": string;
  "in_use": number;
  "last_check"?: ProviderCheckView | null;
  "logged_in": boolean;
  "login_pending"?: boolean;
  "score"?: number | null;
  "usage"?: AccountUsageLive | null;
};

export type AccountLoginResult = {
  "detail"?: string | null;
  "result": string;
};

export type AccountLoginStart = {
  "expires_at": string;
  "kind"?: string;
  "url": string;
  "user_code"?: string | null;
};

export type AccountNowView = {
  "cooldown_until"?: number | null;
  "id": string;
  "remaining_long"?: number | null;
  "remaining_short"?: number | null;
  "source": string;
};

export type AccountStats = {
  "done": number;
  "error": number;
  "input_tokens": number;
  "output_tokens": number;
  "runs": number;
};

export type AccountUsageLive = {
  "five_hour"?: RateWindow | null;
  "observed_at": number;
  "one_month"?: RateWindow | null;
  "seven_day"?: RateWindow | null;
  "source": string;
  "status"?: string | null;
};

export type AccountUsageView = {
  "five_hour"?: RateWindowView | null;
  "observed_at": string;
  "one_month"?: RateWindowView | null;
  "seven_day"?: RateWindowView | null;
  "source": string;
  "status"?: string | null;
};

export type AccountView = {
  "adapter"?: string;
  "cooldown"?: AccountCooldownView | null;
  "dir": string;
  "excluded_reason"?: string | null;
  "id": string;
  "in_use": number;
  "last_check"?: ProviderCheckView | null;
  "logged_in": boolean;
  "login_pending": boolean;
  "score"?: number | null;
  "stats": AccountStats;
  "usage"?: AccountUsageView | null;
};

export type Action = "approve" | "reject" | "answer" | "cancel" | "retry" | "edit" | "reopen" | "rereview" | "phase_gate" | "plan_gate";

export type ActualSource = {
  "account"?: string | null;
  "from": string;
  "model"?: string | null;
  "source_id"?: string | null;
};

export type ActualWriteSetView = {
  "base_sha"?: string | null;
  "head_sha"?: string | null;
  "owner_id": string;
  "paths": Array<string>;
  "reason"?: string | null;
  "recorded_at": string;
  "repo_id": string;
  "status": string;
};

export type AdoptRequest = {
  "stage": string;
  "task_id": TaskId;
  "unit_key": string;
};

export type AdoptionOutcome = {
  "adopted": boolean;
  "detail": string;
  "plan_id": string;
  "stage": string;
  "task_id": TaskId;
  "task_status": Status;
  "unit_key": string;
  "unit_status": WorkUnitStatus;
};

export type AnswerBody = {
  "answer": string;
  "expected_status"?: Status | null;
};

export type AnswerNote = {
  "answer": string;
  "question": string;
};

export type ApiConfigView = {
  "allowed_hosts": Array<string>;
  "auth_required": boolean;
  "bind": string;
};

export type Approval = {
  "answer"?: string | null;
  "created_at": string;
  "decided_at"?: string | null;
  "decision"?: Decision | null;
  "id": ApprovalId;
  "node_id": string;
  "project_id"?: ProjectId | null;
  "question": string;
  "task_id"?: TaskId | null;
};

export type ApprovalArtifact = {
  "declared"?: boolean;
  "idx": number;
  "kind": string;
  "name": string;
  "path": string;
  "sha256": string;
};

export type ApprovalDecideBody = {
  "answer": string;
  "decision": Decision;
  "scope"?: string | null;
};

export type ApprovalDecideResult = {
  "approval": Approval;
  "note"?: string | null;
  "standing_rule"?: StandingRule | null;
  "transition"?: TransitionResult | null;
};

export type ApprovalDecisionView = {
  "approved": boolean;
  "by": string;
  "note"?: string | null;
  "ts": string;
};

export type ApprovalId = string;

export type ApprovalItem = {
  "approval": TaskRef;
  "artifacts": Array<ApprovalArtifact>;
  "attempt"?: number | null;
  "criterion_idx"?: number | null;
  "criterion_text": string;
  "evidence": Array<EvidenceView>;
  "knowledge_pages": Array<KnowledgePageRef>;
  "last_run"?: RunSummary | null;
  "other_verdicts": Array<VerdictView>;
  "parent"?: TaskRef | null;
  "previous_decisions": Array<ApprovalDecisionView>;
  "requested_at": string;
};

export type ApprovalLink = {
  "approval": TaskRef;
  "attempt"?: number | null;
  "criterion_idx"?: number | null;
  "decided"?: ApprovalDecisionView | null;
};

export type ApprovalList = {
  "items": Array<Approval>;
};

export type ArtifactList = {
  "items": Array<ArtifactView>;
};

export type ArtifactPromoteBody = {
  "name": string;
  "overwrite"?: boolean;
  "path": string;
  "title"?: string | null;
};

export type ArtifactRef = {
  "declared"?: boolean;
  "kind": string;
  "name": string;
  "path": string;
  "sha256": string;
};

export type ArtifactView = {
  "artifact": ArtifactRef;
  "exists": boolean;
  "forbidden": boolean;
  "idx": number;
  "run_id": string;
  "sha256_current"?: string | null;
  "sha256_matches"?: boolean | null;
  "size"?: number | null;
  "ts": string;
};

export type AssignmentList = {
  "effective": Array<RoleSlotView>;
  "items": Array<EffectiveAssignmentView>;
};

export type AssignmentPreviewBody = {
  "model_id"?: string | null;
  "source": string;
  "tier": Tier;
};

export type AssignmentPreviewResponse = {
  "impact": ImpactView;
};

export type AssignmentPutBody = {
  "model_id": string;
  "note"?: string | null;
};

export type AssignmentPutResponse = {
  "impact": ImpactView;
  "item": EffectiveAssignmentView;
};

export type AssignmentStateView = "assigned" | "excluded";

export type AttentionItem = {
  "at": string;
  "class": FailureClass;
  "delivered_release"?: string | null;
  "integration_repair"?: IntegrationRepairView | null;
  "reason": string;
  "task": TaskRef;
  "type": "failed";
} | {
  "at": string;
  "count": number;
  "max": number;
  "task": TaskRef;
  "type": "requeue_limit_near";
} | {
  "at": string;
  "hint": WorkerHint;
  "task": TaskRef;
  "type": "unroutable";
} | {
  "at": string;
  "cluster": string;
  "host": string;
  "tasks": number;
  "type": "cluster_unavailable";
} | {
  "at": string;
  "next_phase"?: string | null;
  "phase": string;
  "phase_title": string;
  "phases_done": number;
  "phases_total": number;
  "report_idx"?: number | null;
  "task": TaskRef;
  "type": "phase_checkpoint";
} | {
  "at": string;
  "decision_ids": Array<string>;
  "plan_id": string;
  "plan_version": number;
  "reasons": Array<string>;
  "stages": Array<PlanApprovalStage>;
  "summary": string;
  "task": TaskRef;
  "type": "plan_approval";
} | {
  "at": string;
  "detail": string;
  "head"?: string | null;
  "reason": DeliverySkipReason;
  "summary": string;
  "task": TaskRef;
  "type": "delivery_skipped";
} | {
  "at": string;
  "request": IntegrationRequest;
  "request_id": string;
  "task": TaskRef;
  "type": "integration_request";
};

export type AttestationClaims = {
  "actor_id": string;
  "decision": string;
  "expires_at": number;
  "nonce": string;
  "owner_session_hash": string;
  "policy_hash": string;
  "task_id": string;
  "version": number;
  "wait_id": string;
};

export type AwaitedChildView = {
  "status"?: Status | null;
  "task_id"?: TaskId | null;
  "title": string;
  "unit_key": string;
};

export type BehindTarget = {
  "behind_target_age_seconds"?: number | null;
  "behind_target_commits"?: number | null;
  "behind_target_observed_at"?: string | null;
  "repos"?: Array<BehindTargetRepo>;
};

export type BehindTargetRepo = {
  "behind_target_age_seconds"?: number | null;
  "behind_target_commits"?: number | null;
  "behind_target_observed_at": string;
  "head_sha"?: string | null;
  "repo_id": string;
  "target_ref": string;
  "target_sha"?: string | null;
};

export type Billing = "subscription" | "metered_api" | "self_hosted";

export type BrowserAction = "navigate" | "click" | "snapshot" | "extract" | "screenshot" | "download" | "scroll" | "credential_use";

export type BrowserCapability = {
  "allowed_actions"?: Array<BrowserAction> | null;
  "allowed_domains": Array<string>;
  "credential_identity_ids"?: {
  [key: string]: string;
};
  "credential_policy_ids"?: Array<string>;
  "live_view_url"?: string | null;
};

export type BrowserCredentialBody = {
  "attestation": HumanAttestation;
  "expected_version": number;
  "password": string;
  "username": string;
};

export type BrowserDecision = "approve_once" | "deny" | "revoke";

export type BrowserDecisionBody = {
  "attestation": HumanAttestation;
  "decision": BrowserDecision;
  "expected_version": number;
  "idempotency_key": string;
};

export type BrowserPendingList = {
  "items": Array<BrowserWaitItem>;
};

export type BrowserPolicyBinding = {
  "hash": string;
  "policy_id": string;
  "revision": number;
};

export type BrowserRegisteredBody = {
  "attestation": HumanAttestation;
  "expected_version": number;
  "receipt": CredentialRecord;
};

export type BrowserRequestResult = {
  "created": boolean;
  "wait": BrowserWait;
};

export type BrowserRequirements = {
  "allowed_domains": Array<string>;
};

export type BrowserRevokeBody = {
  "attestation": HumanAttestation;
  "expected_version": number;
  "idempotency_key": string;
};

export type BrowserRun = {
  "live_view_url"?: string | null;
  "policy"?: BrowserPolicyBinding | null;
  "run_id": string;
  "session_id": string;
  "state": BrowserRunState;
  "task_id": TaskId;
};

export type BrowserRunState = "RUNNING" | "WAITING_FOR_AUTH" | "WAITING_FOR_APPROVAL" | "WAITING_FOR_HUMAN" | "COMPLETED" | "FAILED";

export type BrowserSettingsPatch = {
  "allowed_domains"?: Array<string> | null;
  "budget"?: BudgetPrefs | null;
  "credential_identity_ids"?: {
  [key: string]: string;
} | null;
  "credential_policy_ids"?: Array<string> | null;
  "harnesses"?: HarnessPrefs | null;
};

export type BrowserWait = {
  "approval_id"?: string | null;
  "created_at": string;
  "credential"?: CredentialRef | null;
  "credential_policy_id"?: string | null;
  "deadline": string;
  "operation"?: OperationIntent | null;
  "origin": string;
  "owner_id"?: string | null;
  "policy_hash": string;
  "policy_revision": number;
  "purpose": string;
  "reason": BrowserWaitReason;
  "resolution_code"?: string | null;
  "resolved_at"?: string | null;
  "resume_key": string;
  "run_id": string;
  "session_id": string;
  "state": BrowserWaitState;
  "task_id": TaskId;
  "trusted_login"?: TrustedLogin | null;
  "version": number;
  "wait_id": string;
  "work_unit_id"?: string | null;
};

export type BrowserWaitItem = {
  "run_state": BrowserRunState;
  "task": TaskRef;
  "wait": BrowserWait;
};

export type BrowserWaitList = {
  "items": Array<BrowserWait>;
};

export type BrowserWaitReason = "waiting_for_auth" | "waiting_for_approval";

export type BrowserWaitResult = {
  "replayed": boolean;
  "task_status": Status;
  "wait": BrowserWait;
};

export type BrowserWaitState = "pending" | "denied" | "expired" | "cancelled" | "revoked" | "invalidated" | "registered" | "approved" | "resumed";

export type Budget = {
  "max_retries": number;
  "max_turns": number;
  "max_wall_secs": number;
};

export type BudgetKind = "turns" | "wall_clock" | "context";

export type BudgetPrefs = {
  "max_attempts"?: number | null;
  "max_lane"?: Tier | null;
};

export type CancelBody = {
  "expected_status"?: Status | null;
};

export type CandidateTrace = {
  "cash_usd"?: number | null;
  "config_order"?: number | null;
  "cost_usd"?: number | null;
  "deployment_id": string;
  "effective_usd"?: number | null;
  "eligible_provider_ids": Array<string>;
  "excluded_reason"?: ExcludedReason | null;
  "excluded_reasons": Array<string>;
  "latency_ms"?: number | null;
  "model_profile_id": string;
  "pressure"?: number | null;
  "quality"?: QualityEstimate | null;
  "resource_usd"?: number | null;
  "score"?: number | null;
  "score_breakdown"?: ScoreTrace | null;
  "shadow_usd"?: number | null;
};

export type CatalogCapabilitiesView = {
  "reasoning_efforts"?: Array<string> | null;
  "streaming"?: boolean | null;
  "structured_output"?: boolean | null;
  "tools"?: boolean | null;
  "vision"?: boolean | null;
};

export type CatalogDelta = {
  "added": Array<string>;
  "removed": Array<string>;
  "restored": Array<string>;
  "source": string;
};

export type CatalogDeploymentView = {
  "allowed_lanes": Array<Tier>;
  "billing": Billing;
  "id": string;
  "model_profile_id": string;
  "price_override"?: TokenPricing | null;
  "source_ref": string;
  "upstream_model": string;
};

export type CatalogModelView = {
  "capabilities": CatalogCapabilitiesView;
  "context_limits": ContextLimits;
  "family": string;
  "id": string;
  "pricing"?: TokenPricing | null;
  "quality"?: Array<QualityIndex> | null;
  "revision": string;
};

export type ChangeDiffView = {
  "diff": string;
  "path": string;
  "repo": string;
  "truncated": boolean;
};

export type ChangedFile = {
  "additions": number;
  "binary"?: boolean;
  "deletions": number;
  "path": string;
  "status": string;
};

export type ChangesView = {
  "delivery"?: Delivery | null;
  "gh": boolean;
  "merge_method": string;
  "repos": Array<RepoChangesView>;
  "task_id": string;
};

export type Check = {
  "cmd": string;
  "expect_exit": number;
  "type": "command";
} | {
  "name": string;
  "type": "artifact_exists";
} | {
  "path": string;
  "type": "knowledge_page";
} | {
  "type": "reviewer";
} | {
  "type": "human";
};

export type Checkpoint = {
  "artifact_refs"?: Array<CheckpointArtifactRef>;
  "completed"?: Array<string>;
  "created_at": string;
  "decisions"?: Array<CheckpointDecision>;
  "end": CheckpointEnd;
  "files_changed"?: Array<CheckpointFileChange>;
  "known_failures"?: Array<CheckpointKnownFailure>;
  "next_action": string;
  "open_questions"?: Array<string>;
  "plan_issue"?: string | null;
  "recent_activity"?: Array<string>;
  "remaining"?: Array<string>;
  "repo_state"?: RepoState | null;
  "run_id": string;
  "run_seq": number;
  "schema": string;
  "source": CheckpointSource;
  "task_id": string;
  "tests_run"?: Array<CheckpointTestRun>;
  "work_unit"?: string | null;
};

export type CheckpointArtifactRef = {
  "kind"?: string | null;
  "path": string;
};

export type CheckpointDecision = {
  "what": string;
  "why": string;
};

export type CheckpointEnd = "completed" | "yielded" | "budget_exhausted" | "waiting";

export type CheckpointFileChange = {
  "change": string;
  "note"?: string | null;
  "path": string;
};

export type CheckpointKnownFailure = {
  "detail"?: string | null;
  "what": string;
};

export type CheckpointSource = "worker" | "yield" | "mechanical" | "merged";

export type CheckpointTestRun = {
  "command": string;
  "exit"?: number | null;
  "summary"?: string | null;
};

export type ClusterConfigView = {
  "auth"?: string;
  "concurrency": number;
  "delete_on_push": boolean;
  "env_keys": Array<string>;
  "forwards"?: Array<ClusterForwardView>;
  "has_setup": boolean;
  "host": string;
  "id": string;
  "rsync_excludes": Array<string>;
  "sync": string;
  "work_dir"?: string | null;
};

export type ClusterConnectResult = {
  "detail"?: string | null;
  "ok": boolean;
};

export type ClusterConnectStart = {
  "expires_at"?: string | null;
  "kind": string;
  "prompt"?: string | null;
};

export type ClusterConnectionStats = {
  "connects_borrowed"?: number;
  "connects_publickey"?: number;
  "connects_totp"?: number;
  "key_auth_attempts"?: number;
  "last_lost_at"?: string | null;
  "last_lost_cause"?: string | null;
  "losses"?: number;
  "losses_by_cause"?: {
  [key: string]: number;
};
};

export type ClusterForwardView = {
  "last_error"?: string | null;
  "listen": string;
  "listener"?: boolean | null;
  "target": string;
  "target_healthy"?: boolean | null;
  "up"?: boolean | null;
};

export type ClusterJobState = "queued" | "held" | "running" | "exiting" | "finished" | "gone" | "unknown";

export type ClusterJobStatus = {
  "exit_status"?: number | null;
  "job_id": string;
  "raw_state"?: string | null;
  "state": ClusterJobState;
};

export type ClusterJobWait = {
  "checkpoint"?: unknown;
  "cluster": string;
  "created_at": string;
  "deadline": string;
  "finished_at"?: string | null;
  "jobs": Array<string>;
  "last_polled_at"?: string | null;
  "last_status"?: Array<ClusterJobStatus>;
  "poll_secs": number;
  "run_id": string;
  "scheduler": ClusterScheduler;
  "state": ClusterJobWaitState;
  "summary"?: string;
  "task_id": TaskId;
  "timeout_secs": number;
  "wait_id": string;
  "work_unit_id"?: string | null;
};

export type ClusterJobWaitState = "waiting" | "satisfied" | "timed_out" | "cancelled";

export type ClusterJobWaitView = {
  "cluster": string;
  "created_at": string;
  "deadline": string;
  "jobs": Array<ClusterJobStatus>;
  "last_polled_at"?: string | null;
  "next_poll_at"?: string | null;
  "poll_secs": number;
  "run_id": string;
  "scheduler": ClusterScheduler;
  "status_line": string;
  "summary"?: string;
  "wait_id": string;
  "work_unit_id"?: string | null;
};

export type ClusterLive = {
  "auth"?: string;
  "concurrency": number;
  "connect_pending"?: boolean;
  "connected": boolean;
  "connection_stats"?: ClusterConnectionStats;
  "cooldown_until"?: string | null;
  "host": string;
  "id": string;
  "in_use": number;
  "tunnel_forwards"?: Array<TunnelForwardLive>;
  "tunnel_login_needed"?: boolean;
};

export type ClusterScheduler = "pbs" | "slurm";

export type ClusterSettingsPutBody = {
  "work_dir"?: string | null;
};

export type ClusterSettingsView = {
  "cluster_id": string;
  "updated_at": string;
  "work_dir"?: string | null;
};

export type ClusterStatsView = {
  "last_24h": ClusterConnectionStats;
  "since_start"?: ClusterConnectionStats | null;
};

export type ClusterView = {
  "auth"?: string;
  "concurrency": number;
  "connect_pending"?: boolean;
  "connected"?: boolean | null;
  "cooldown_remaining_secs"?: number | null;
  "cooldown_until"?: string | null;
  "delete_on_push": boolean;
  "env_keys": Array<string>;
  "has_setup": boolean;
  "host": string;
  "id": string;
  "in_use"?: number | null;
  "rsync_excludes": Array<string>;
  "stats"?: ClusterStatsView;
  "sync": string;
  "tunnel_forwards"?: Array<ClusterForwardView>;
  "tunnel_login_needed"?: boolean;
  "work_dir"?: string | null;
  "work_dir_source"?: string | null;
};

export type Clusters = {
  "items": Array<ClusterView>;
};

export type CommentAuthorKind = "human" | "node" | "system";

export type CommentBody = {
  "body": string;
};

export type CommentEffect = "stored" | "interrupted" | "answered" | "terminal";

export type CommentId = string;

export type CommentList = {
  "items": Array<TaskComment>;
};

export type CommentResult = {
  "can_reopen": boolean;
  "comment": TaskComment;
  "effect": CommentEffect;
  "transition"?: TransitionResult | null;
};

export type CommitIntent = {
  "sha": string;
  "subject": string;
};

export type Confidence = "high" | "medium" | "low";

export type ConfigView = {
  "api": ApiConfigView;
  "clusters"?: Array<ClusterConfigView>;
  "config_path": string;
  "db": string;
  "delegation"?: DelegationLimits;
  "error_cooldown_secs": number;
  "genres"?: Array<GenreConfigView>;
  "idle_timeout_secs": number;
  "kill_grace_secs": number;
  "lease_grace_secs": number;
  "max_concurrency": number;
  "max_requeues": number;
  "plan_auto_accept": boolean;
  "providers": Array<ProviderConfigView>;
  "retry_backoff_base_secs": number;
  "retry_backoff_max_secs": number;
  "review_timeout_secs": number;
  "reviewer": ReviewerConfigView;
  "roles"?: Array<RoleConfigView>;
  "tick_ms": number;
  "workspace_root": string;
};

export type ConflictKind = "Record" | "Migration" | "Adr" | "Generated" | "Code";

export type ConsoleBlock = {
  "at": string;
  "author"?: string | null;
  "cursor": string;
  "kind": "human";
  "message_id": string;
  "node_id": string;
  "project_id"?: ProjectId | null;
  "task_id"?: TaskId | null;
  "text": string;
} | {
  "actions_result"?: MessageMetadata | null;
  "at": string;
  "cursor": string;
  "kind": "reply";
  "message_id": string;
  "node_id": string;
  "project_id"?: ProjectId | null;
  "run_id"?: string | null;
  "state"?: ConsoleReplyState;
  "steps"?: Array<ConsoleReplyStep>;
  "task_id"?: TaskId | null;
  "text": string;
  "thinking"?: string | null;
} | {
  "at": string;
  "cursor": string;
  "kind": "task";
  "task": ConsoleTaskLine;
} | {
  "assignee"?: string | null;
  "at": string;
  "cursor": string;
  "harness"?: string | null;
  "kind": "progress";
  "progress": ConsoleProgress;
  "project_id"?: ProjectId | null;
  "tier": Tier;
  "title": string;
} | {
  "answer"?: string | null;
  "answered": boolean;
  "at": string;
  "cursor": string;
  "kind": "question";
  "node_id"?: string | null;
  "project_id"?: ProjectId | null;
  "run_id": string;
  "task_id": TaskId;
  "text": string;
} | {
  "approval": Approval;
  "at": string;
  "cursor": string;
  "kind": "approval";
} | {
  "at": string;
  "cursor": string;
  "kind": "milestone";
  "milestone": Milestone;
  "review"?: MilestoneReviewView | null;
} | {
  "at": string;
  "cursor": string;
  "kind": "report";
  "report": Report;
} | {
  "at": string;
  "cursor": string;
  "discarded": number;
  "inbox": number;
  "ingested": number;
  "kind": "knowledge";
  "project_id"?: ProjectId | null;
  "run_task_id": TaskId;
  "state": string;
  "task_id": TaskId;
  "task_title": string;
  "via"?: string | null;
};

export type ConsoleHello = {
  "cursor": string;
  "now": string;
  "scope": string;
};

export type ConsoleInstructAccepted = {
  "message_id": string;
  "node_id": string;
  "task_id": TaskId;
};

export type ConsolePage = {
  "items": Array<ConsoleBlock>;
  "next_cursor"?: string | null;
};

export type ConsoleProgress = {
  "count": number;
  "first": Array<ConsoleProgressLine>;
  "last": Array<ConsoleProgressLine>;
  "last_status"?: string | null;
  "run_id": string;
  "started_at": string;
  "task_id": TaskId;
  "tool_count": number;
  "truncated"?: boolean;
  "updated_at": string;
};

export type ConsoleProgressLine = {
  "at": string;
  "error"?: boolean;
  "kind"?: ProgressKind | null;
  "seq": number;
  "text": string;
  "tool"?: string | null;
};

export type ConsoleReplyState = "streaming" | "done";

export type ConsoleReplyStep = {
  "error"?: boolean;
  "kind": ProgressKind;
  "text": string;
  "tool"?: string | null;
};

export type ConsoleTaskLine = {
  "assignee"?: string | null;
  "elapsed_secs"?: number | null;
  "from": Status;
  "harness"?: string | null;
  "mode"?: string | null;
  "project_id"?: ProjectId | null;
  "reason": string;
  "task_id": TaskId;
  "tier": Tier;
  "title": string;
  "to": Status;
};

export type Constraints = {
  "allowed_deployments"?: Array<string> | null;
  "allowed_sources"?: Array<string> | null;
  "data_retention_allowed"?: boolean | null;
  "external_network_allowed"?: boolean | null;
  "max_cost_usd"?: number | null;
  "max_latency_ms"?: number | null;
  "required_region"?: string | null;
};

export type ContainerProbeView = {
  "detail": string;
  "runtime": string;
};

export type ContainersLive = {
  "build_dir": string;
  "image_default": string;
  "preference": string;
  "probes"?: Array<ContainerProbeView>;
  "runtime"?: string | null;
};

export type ContextLimits = {
  "input"?: number | null;
  "output"?: number | null;
  "total"?: number | null;
};

export type ContinuationMetrics = {
  "fresh"?: ContinuationRunTotals;
  "fresh_fallback_by_reason"?: {
  [key: string]: number;
};
  "resumed"?: ContinuationRunTotals;
  "unknown"?: ContinuationRunTotals;
};

export type ContinuationRunTotals = {
  "duplicate_reads": number;
  "input_tokens": number;
  "runs": number;
  "wall_ms": number;
};

export type CooldownView = {
  "provider": string;
  "reason": string;
  "until": string;
};

export type CostOfReversal = "low" | "medium" | "high";

export type CreatedOrigin = "plan_unit" | {
  "worker_run": {
  "run_id": string;
  "task_id": TaskId;
};
};

export type CredentialRecord = {
  "credential_id": string;
  "credential_revision": number;
  "origin": string;
  "policy_id": string;
  "provider": string;
  "receipt_id": string;
};

export type CredentialRef = {
  "credential_id": string;
  "policy_id": string;
  "provider": string;
};

export type Criterion = {
  "check": Check;
  "text": string;
};

export type CriterionSpec = {
  "text": string;
  "type": "human";
} | {
  "cmd": string;
  "expect_exit"?: number;
  "type": "command";
} | {
  "name": string;
  "type": "artifact_exists";
} | {
  "path": string;
  "type": "knowledge_page";
} | {
  "text": string;
  "type": "reviewer";
};

export type CriterionView = {
  "approval"?: ApprovalLink | null;
  "check": Check;
  "idx": number;
  "latest_verdict"?: VerdictView | null;
  "text": string;
};

export type CronCatchUp = "latest" | "skip";

export type CronJobCreateBody = {
  "catch_up"?: CronCatchUp;
  "enabled"?: boolean;
  "name": string;
  "overlap"?: CronOverlap;
  "schedule": string;
  "template": CronTaskTemplate;
  "timezone": string;
};

export type CronJobId = string;

export type CronJobList = {
  "items": Array<CronJobView>;
};

export type CronJobPatchBody = {
  "catch_up"?: CronCatchUp | null;
  "name"?: string | null;
  "overlap"?: CronOverlap | null;
  "schedule"?: string | null;
  "template"?: CronTaskTemplate | null;
  "timezone"?: string | null;
};

export type CronJobRun = {
  "detail"?: string | null;
  "id": CronJobRunId;
  "job_id": CronJobId;
  "outcome": CronRunOutcome;
  "recorded_at": string;
  "scheduled_for": string;
  "task_id"?: TaskId | null;
  "trigger": CronTrigger;
};

export type CronJobRunId = string;

export type CronJobRunList = {
  "items": Array<CronJobRun>;
  "job_id": CronJobId;
};

export type CronJobView = {
  "catch_up": CronCatchUp;
  "created_at": string;
  "enabled": boolean;
  "id": CronJobId;
  "last_run"?: CronJobRun | null;
  "name": string;
  "next_fire_at"?: string | null;
  "overlap": CronOverlap;
  "schedule": string;
  "template": CronTaskTemplate;
  "timezone": string;
  "updated_at": string;
};

export type CronOverlap = "skip" | "queue";

export type CronRunOutcome = "created" | "queued" | "skipped_overlap" | "skipped_missed" | "error";

export type CronRunResult = {
  "job_id": CronJobId;
  "job_name": string;
  "runs": Array<CronJobRun>;
  "task_id"?: TaskId | null;
};

export type CronTaskTemplate = {
  "acceptance"?: Array<unknown>;
  "assignee"?: string | null;
  "harness"?: string | null;
  "lane"?: Tier | null;
  "objective"?: string;
  "priority"?: unknown;
  "project"?: string | null;
  "repos"?: Array<string>;
  "title": string;
};

export type CronTrigger = "schedule" | "catch_up" | "manual";

export type DaemonInstance = {
  "drained_at"?: string | null;
  "handoff_requested_at"?: string | null;
  "heartbeat_at": string;
  "instance_id": string;
  "pid": number;
  "release": string;
  "role": InstanceRole;
  "started_at": string;
};

export type DaemonSnapshot = {
  "accounts"?: Array<AccountLive>;
  "accounts_root"?: string | null;
  "accounts_roots"?: {
  [key: string]: string;
};
  "approvals_pending"?: number;
  "awaiting_children"?: Array<TaskId>;
  "awaiting_human": Array<TaskId>;
  "clusters"?: Array<ClusterLive>;
  "containers"?: ContainersLive | null;
  "cooldowns": Array<CooldownView>;
  "decisions_open"?: number;
  "hostname": string;
  "in_flight": Array<InFlight>;
  "instance_id": string;
  "last_tick_at": string;
  "max_runs_per_account"?: number | null;
  "pid": number;
  "providers": Array<ProviderLive>;
  "reports"?: ReportsLive | null;
  "scratch"?: ScratchStatus | null;
  "started_at": string;
  "tick_ms": number;
  "ticks": number;
  "unroutable": Array<TaskId>;
};

export type DaemonView = {
  "now": string;
  "snapshot"?: DaemonSnapshot | null;
};

export type DailyUsage = {
  "day": string;
  "input_tokens": number;
  "output_tokens": number;
  "runs": number;
};

export type DbInfo = {
  "busy_timeout_ms": number;
  "device"?: string | null;
  "filesystem"?: string | null;
  "journal_mode": string;
};

export type Decision = "once" | "standing" | "denied" | "withdrawn";

export type DecisionAnswer = {
  "by": string;
  "note"?: string | null;
  "option": string;
};

export type DecisionAnswerBody = {
  "note"?: string | null;
  "option"?: string | null;
};

export type DecisionBody = {
  "expected_status"?: Status | null;
  "note"?: string | null;
};

export type DecisionEffect = "resume" | "raise_once" | "replan" | "atomic" | "withdraw";

export type DecisionInboxItem = {
  "age_secs": number;
  "cost_note"?: string | null;
  "cost_of_reversal": CostOfReversal;
  "created_at": string;
  "id": string;
  "key": string;
  "kind": DecisionKind;
  "needed_before": Array<string>;
  "options": Array<DecisionOption>;
  "origin": DecisionOrigin;
  "path": Array<DecisionPathEntry>;
  "question": string;
  "recommended": string;
  "root_id": TaskId;
  "task_id": TaskId;
};

export type DecisionKind = "choice" | "leaf_too_large" | "limit" | "plan_invalid";

export type DecisionList = {
  "items": Array<DecisionView>;
};

export type DecisionOption = {
  "consequence"?: string | null;
  "key": string;
  "label": string;
};

export type DecisionOrigin = "planner" | "worker" | "daemon" | "human";

export type DecisionOutcome = {
  "cancelled": Array<string>;
  "cancelled_task"?: TaskId | null;
  "decision": DecisionView;
  "effect": DecisionEffect;
  "notified_children"?: Array<TaskId>;
  "replan_requested": boolean;
  "resumed": Array<string>;
};

export type DecisionPathEntry = {
  "stage"?: string | null;
  "task_id": TaskId;
  "title": string;
  "unit"?: string | null;
};

export type DecisionRaisedBy = {
  "origin": DecisionOrigin;
  "run_id"?: string | null;
  "task_id": TaskId;
};

export type DecisionRequest = {
  "answer"?: DecisionAnswer | null;
  "cost_note"?: string | null;
  "cost_of_reversal": CostOfReversal;
  "id": string;
  "key": string;
  "kind": DecisionKind;
  "needed_before": Array<string>;
  "options": Array<DecisionOption>;
  "path": Array<DecisionPathEntry>;
  "question": string;
  "raised_by": DecisionRaisedBy;
  "recommended": string;
  "status": DecisionStatus;
  "withdrawn_reason"?: string | null;
};

export type DecisionSpec = {
  "cost_note"?: string | null;
  "cost_of_reversal": CostOfReversal;
  "key": string;
  "needed_before"?: Array<string>;
  "options": Array<DecisionOption>;
  "question": string;
  "recommended": string;
};

export type DecisionStatus = "open" | "answered" | "withdrawn";

export type DecisionView = {
  "answered_at"?: string | null;
  "created_at": string;
  "decision": DecisionRequest;
  "effect"?: DecisionEffect | null;
  "root_id": TaskId;
  "task_id": TaskId;
};

export type DecisionWithdrawBody = {
  "reason"?: string | null;
};

export type DecomposeRequest = {
  "mode": ExecutionMode;
  "note"?: string | null;
};

export type DecomposeResult = {
  "mode": ExecutionMode;
  "previous_decision"?: ExecutionGateDecision | null;
  "replan": boolean;
  "task": Task;
};

export type DelegatedView = {
  "run_id": string;
  "tasks": Array<TaskRef>;
  "ts": string;
};

export type DelegationLimits = {
  "max_delegate_per_run": number;
  "max_tree_depth": number;
  "max_tree_runs": number;
  "on_child_failure": OnChildFailure;
};

export type Delivery = {
  "base": string;
  "branch": string;
  "criterion_idx": number;
  "decision"?: boolean | null;
  "default_branch": string;
  "department": string;
  "detail": string;
  "head": string;
  "merge_candidate_sha"?: string | null;
  "notification"?: MessageId | null;
  "prepare_pid"?: number | null;
  "project_id": ProjectId;
  "push_error"?: string | null;
  "pushed_at"?: string | null;
  "release"?: string | null;
  "repo": string;
  "repo_id": RepoId;
  "review_run": string;
  "reviewed_sha"?: string | null;
  "state": DeliveryState;
  "target_sha"?: string | null;
  "task_id": TaskId;
  "worker_run": string;
};

export type DeliveryHead = {
  "base"?: string | null;
  "branch": string;
  "head": string;
  "merge_candidate_sha"?: string | null;
  "release"?: string | null;
  "repo": string;
  "reviewed_sha"?: string | null;
  "state": string;
  "task_id": string;
};

export type DeliveryList = {
  "items": Array<DeliveryHead>;
};

export type DeliverySkipReason = "multiple_repos" | "no_marker" | "marker_repo_mismatch" | "repo_row_missing" | "repo_not_local" | "repo_path_mismatch" | "not_git" | "no_branch" | "branch_name_mismatch" | "refs_unresolvable" | "department_unresolved";

export type DeliveryState = "reviewing" | "merge_queued" | "merging" | "preparing" | "ready" | "blocked";

export type DiffStat = {
  "additions": number;
  "deletions": number;
  "files": number;
};

export type DiscoverBody = {
  "source"?: string | null;
};

export type DiscoverResponse = {
  "results": Array<DiscoverySummaryView>;
  "unavailable": boolean;
};

export type DiscoveryRecordView = {
  "at": string;
  "count": number;
  "error"?: string | null;
  "ok": boolean;
  "source": string;
};

export type DiscoverySummaryView = {
  "count": number;
  "delta": CatalogDelta;
  "error"?: string | null;
  "ok": boolean;
  "source": string;
};

export type DocCommit = {
  "at": string;
  "author": string;
  "sha": string;
  "subject": string;
};

export type DocItem = {
  "last_commit"?: DocCommit | null;
  "path": string;
  "title": string;
  "updated_at"?: string | null;
};

export type DocPage = {
  "default_branch": string;
  "etag"?: string | null;
  "history": Array<DocCommit>;
  "html": string;
  "path": string;
  "project_id": ProjectId;
  "raw": string;
  "repo": string;
  "root": string;
  "tags"?: Array<string>;
  "tasks"?: Array<string>;
  "title": string;
  "too_large": boolean;
};

export type DocPagePutBody = {
  "body": string;
  "etag"?: string | null;
  "message"?: string | null;
  "path": string;
};

export type DocPageResult = {
  "deleted": boolean;
  "etag"?: string | null;
  "path": string;
  "project_id": ProjectId;
  "repo": string;
  "sha": string;
  "unchanged": boolean;
};

export type DocsInitResult = {
  "created": boolean;
  "default_branch": string;
  "path": string;
  "project_id": ProjectId;
  "repo": string;
  "root": string;
};

export type DocsTree = {
  "default_branch": string;
  "items": Array<DocItem>;
  "project_id": ProjectId;
  "q"?: string | null;
  "repo": string;
  "root": string;
  "truncated": boolean;
};

export type DraftGroup = {
  "drafts": Array<TaskSummary>;
  "parent"?: TaskRef | null;
  "plan_summary"?: string | null;
  "project_plan"?: ProjectPlanRef | null;
};

export type EditResult = {
  "fields": Array<string>;
  "task": Task;
};

export type EffectiveAssignmentView = {
  "excluded_reason"?: string | null;
  "model_id": string;
  "note"?: string | null;
  "source": string;
  "state": AssignmentStateView;
  "tier": Tier;
  "updated_at": string;
  "updated_by": string;
};

export type EffectiveProfile = {
  "allowed_tiers"?: Array<Tier>;
  "approvals"?: Array<string>;
  "browser"?: BrowserCapability | null;
  "chain"?: Array<string>;
  "deny_tools"?: Array<string>;
  "harness_default"?: string | null;
  "harnesses_allowed"?: Array<string>;
  "knowledge"?: Array<KnowledgeMount>;
  "max_attempts"?: number | null;
  "max_lane"?: Tier | null;
  "node_id": string;
  "policy"?: Array<string>;
  "review_escalate_on_fail"?: boolean | null;
  "review_harness"?: string | null;
  "review_tier"?: Tier | null;
  "run"?: ProfileRun | null;
  "skills"?: Array<string>;
  "skills_mounts"?: Array<string>;
  "tier"?: Tier | null;
  "tools"?: Array<string>;
};

export type EscalationAudit = {
  "counted_failures": number;
  "interval_id": string;
  "previous_lane"?: Tier | null;
  "reason": string;
  "requested_lane": Tier;
  "selected_lane": Tier;
};

export type EstimatorComparison = "same" | "differs" | "no_candidate";

export type EstimatorDependencyAudit = {
  "external_embeddings"?: boolean | null;
  "needs_network"?: boolean | null;
  "needs_prompt"?: boolean | null;
  "not_allowed"?: boolean;
};

export type EstimatorShadowAudit = {
  "dependencies": EstimatorDependencyAudit;
  "estimator_id"?: string | null;
  "estimator_version"?: string | null;
  "outcome": EstimatorShadowOutcome;
  "overhead_ms"?: number | null;
  "reason"?: ShadowReason | null;
  "unavailable_reason"?: string | null;
  "vs_heuristic"?: EstimatorComparison | null;
  "vs_primary"?: EstimatorComparison | null;
};

export type EstimatorShadowOutcome = "completed" | "failed" | "timeout" | "dropped" | "prompt_required";

export type EstimatorShadowSummary = {
  "completed": number;
  "coverage": number;
  "differs_from_heuristic": number;
  "differs_from_primary": number;
  "dropped": number;
  "estimators": Array<string>;
  "failed": number;
  "mean_overhead_ms"?: number | null;
  "prompt_required": number;
  "targets": number;
  "timeout": number;
};

export type Event = {
  "attempt": number;
  "before_sha": string;
  "merge_candidate_sha": string;
  "repo_id": RepoId;
  "review_run": string;
  "reviewed_sha": string;
  "target_ref": string;
  "target_sha": string;
  "type": "review_target_synced";
} | {
  "attempt": number;
  "repo_id": RepoId;
  "review_run": string;
  "reviewed_sha": string;
  "target_sha": string;
  "type": "review_target_advanced";
} | {
  "attempt": number;
  "before_sha": string;
  "conflict_files": Array<string>;
  "key": string;
  "repo_id": RepoId;
  "target_ref": string;
  "target_sha": string;
  "type": "integration_repair_scheduled";
  "work_unit_id": string;
} | {
  "attempt": number;
  "repo_id": RepoId;
  "reviewed_sha": string;
  "target_sha": string;
  "type": "integration_repair_resolved";
  "work_unit_id": string;
} | {
  "attempt": number;
  "before_sha": string;
  "fallback": boolean;
  "reason": IntegrationRepairExhaustReason;
  "repo_id": RepoId;
  "rollback_to_sha"?: string | null;
  "target_sha": string;
  "type": "integration_repair_exhausted";
  "work_unit_id"?: string | null;
} | {
  "browser": BrowserRun;
  "type": "browser_updated";
} | {
  "type": "browser_wait_opened";
  "wait": BrowserWait;
} | {
  "actor_id"?: string | null;
  "approval_id"?: string | null;
  "code": string;
  "credential_id"?: string | null;
  "reason": BrowserWaitReason;
  "state": BrowserWaitState;
  "type": "browser_wait_resolved";
  "version": number;
  "wait_id": string;
} | {
  "type": "cluster_job_wait_started";
  "wait": ClusterJobWait;
} | {
  "jobs": Array<ClusterJobStatus>;
  "type": "cluster_job_wait_polled";
  "wait_id": string;
} | {
  "detail"?: string;
  "jobs"?: Array<ClusterJobStatus>;
  "state": ClusterJobWaitState;
  "type": "cluster_job_wait_finished";
  "wait_id": string;
} | {
  "origin"?: CreatedOrigin | null;
  "task": Task;
  "type": "created";
} | {
  "from": Status;
  "reason": string;
  "to": Status;
  "type": "transitioned";
} | {
  "account"?: string | null;
  "adapter": string;
  "model": string;
  "provider"?: string | null;
  "role"?: RunRole | null;
  "run_id": string;
  "task_role"?: string | null;
  "type": "worker_started";
} | {
  "detail"?: string | null;
  "error"?: boolean;
  "kind"?: ProgressKind | null;
  "msg": string;
  "run_id": string;
  "summary"?: string | null;
  "tool"?: string | null;
  "truncated"?: boolean;
  "type": "worker_progress";
} | {
  "artifact": ArtifactRef;
  "run_id": string;
  "type": "artifact_produced";
} | {
  "end"?: RunEnd | null;
  "metrics"?: RunMetrics | null;
  "outcome": string;
  "role"?: RunRole | null;
  "run_id": string;
  "type": "worker_finished";
  "usage"?: Usage | null;
} | {
  "criterion_idx": number;
  "pass": boolean;
  "reason": string;
  "run_id": string;
  "type": "review_verdict";
} | {
  "type": "approval_requested";
} | {
  "approved": boolean;
  "by": string;
  "note"?: string | null;
  "type": "approval_decided";
} | {
  "approval_ids": Array<ApprovalId>;
  "reason": string;
  "task_status": Status;
  "type": "approvals_withdrawn";
} | {
  "answer": string;
  "question": string;
  "type": "answered";
} | {
  "run_id": string;
  "text": string;
  "type": "question_raised";
} | {
  "run_id": string;
  "task_ids": Array<TaskId>;
  "type": "delegated";
} | {
  "cluster": string;
  "host"?: string;
  "reason": string;
  "type": "cluster_unavailable";
} | {
  "cluster": string;
  "exit_code"?: number | null;
  "stderr_tail"?: string;
  "type": "cluster_master_exited";
} | {
  "provider": string;
  "reason"?: string | null;
  "type": "provider_throttled";
  "until": string;
} | {
  "from": TaskId;
  "type": "retried";
} | {
  "by": string;
  "fields": Array<string>;
  "type": "edited";
} | {
  "node": string;
  "reason": string;
  "score": number;
  "type": "assigned";
} | {
  "cluster": string;
  "path": string;
  "reason": string;
  "type": "workspace_mode_downgraded";
} | {
  "removed": Array<string>;
  "type": "workspace_pruned";
} | {
  "record": RoutingRecord;
  "run_id": string;
  "type": "routing_decided";
} | {
  "context_version": string;
  "decision_id": string;
  "features"?: unknown;
  "missing_fields"?: Array<string>;
  "provenance"?: {
  [key: string]: string;
};
  "request_id"?: string | null;
  "run_id"?: string | null;
  "stage"?: FeatureStage | null;
  "type": "routing_features_recorded";
} | {
  "attempts"?: Array<RequestSourceAttempt>;
  "decision_id": string;
  "fallback_reason"?: string | null;
  "parent_decision_id"?: string | null;
  "request_id": string;
  "run_id"?: string | null;
  "trace"?: RoutingTraceV1 | null;
  "type": "routing_request_decided";
} | {
  "acceptance_passed"?: boolean | null;
  "cash_usd"?: number | null;
  "decision_id": string;
  "evaluation_version": string;
  "failed_criterion_ids"?: Array<string>;
  "failure_class"?: string | null;
  "outcome_id": string;
  "request_id"?: string | null;
  "retries"?: number | null;
  "review_passed"?: boolean | null;
  "reward"?: number | null;
  "run_id"?: string | null;
  "supersedes"?: string | null;
  "tokens"?: number | null;
  "type": "routing_outcome_recorded";
  "wall_ms"?: number | null;
} | {
  "candidate_model"?: string | null;
  "candidate_source"?: string | null;
  "cash_usd"?: number | null;
  "detail"?: string | null;
  "effective_usd"?: number | null;
  "input_tokens"?: number | null;
  "kind": ShadowKind;
  "latency_ms"?: number | null;
  "output_sha256"?: string | null;
  "output_tokens"?: number | null;
  "policy_version": string;
  "primary_decision_id": string;
  "reason"?: ShadowReason | null;
  "request_id"?: string | null;
  "reservation_id"?: string | null;
  "run_id"?: string | null;
  "shadow_id": string;
  "status": ShadowStatus;
  "type": "routing_shadow_recorded";
} | {
  "checkpoint": Checkpoint;
  "run_id": string;
  "type": "checkpoint_saved";
  "work_unit_id"?: string | null;
} | {
  "origin": PlanOrigin;
  "plan": ExecutionPlanSpec;
  "plan_id": string;
  "reason"?: string | null;
  "supersedes"?: string | null;
  "type": "execution_planned";
  "version": number;
} | {
  "from": WorkUnitStatus;
  "key": string;
  "reason": string;
  "run_id"?: string | null;
  "to": WorkUnitStatus;
  "type": "work_unit_transitioned";
  "work_unit_id": string;
} | {
  "cwd": string;
  "failed": Array<FailedWorkUnitCheck>;
  "key": string;
  "run_id": string;
  "type": "work_unit_checks_failed";
  "work_unit_id": string;
} | {
  "changed_fields"?: Array<string>;
  "key": string;
  "plan_id": string;
  "plan_version": number;
  "type": "work_unit_spec_overridden";
  "work_unit_id": string;
} | {
  "decision": ExecutionGateDecision;
  "type": "execution_gated";
} | {
  "decision": RouteDecision;
  "type": "execution_routed";
} | {
  "mode": ExecutionMode;
  "note"?: string | null;
  "previous"?: ExecutionHintSpec | null;
  "previous_decision"?: ExecutionGateDecision | null;
  "replan"?: boolean;
  "source": string;
  "type": "execution_hint_set";
} | {
  "class": string;
  "key": string;
  "origin": RepairOrigin;
  "type": "repair_scheduled";
  "work_unit_id": string;
} | {
  "account"?: string | null;
  "calibration"?: QuotaCalibration | null;
  "list_price_usd"?: number | null;
  "method": QuotaMethod;
  "run_id": string;
  "source": string;
  "type": "quota_estimated";
  "weighted_tokens": number;
  "weights_version": string;
  "windows": Array<QuotaWindowUse>;
  "work_unit_id"?: string | null;
} | {
  "base"?: string | null;
  "branch": string;
  "commit": string;
  "key": string;
  "type": "work_unit_committed";
  "work_unit_id": string;
} | {
  "checks"?: Array<PhaseCheckResult>;
  "head": string;
  "merged": Array<PhaseMerged>;
  "phase": string;
  "type": "phase_integrated";
  "work_unit_id": string;
} | {
  "cmd": string;
  "index": number;
  "key": string;
  "log_path": string;
  "started_at": string;
  "total": number;
  "type": "integration_check_started";
  "work_unit_id": string;
} | {
  "cmd": string;
  "duration_ms": number;
  "exit"?: number | null;
  "index": number;
  "key": string;
  "pass": boolean;
  "timed_out"?: boolean;
  "total": number;
  "type": "integration_check_finished";
  "work_unit_id": string;
} | {
  "cmd": string;
  "index": number;
  "key": string;
  "log_path": string;
  "run_id": string;
  "started_at": string;
  "total": number;
  "type": "work_unit_check_started";
  "work_unit_id": string;
} | {
  "cmd": string;
  "duration_ms": number;
  "exit"?: number | null;
  "index": number;
  "key": string;
  "pass": boolean;
  "run_id": string;
  "timed_out"?: boolean;
  "total": number;
  "type": "work_unit_check_finished";
  "work_unit_id": string;
} | {
  "key": string;
  "run_id": string;
  "type": "work_unit_checks_handed_off";
  "work_unit_id": string;
} | {
  "branch": string;
  "child_task": TaskId;
  "head_sha": string;
  "key": string;
  "merge_candidate_sha": string;
  "phase": string;
  "repo_id": RepoId;
  "target_sha": string;
  "type": "merge_candidate_stale";
  "work_unit_id": string;
} | {
  "plan_id": string;
  "reason": string;
  "type": "work_units_serialized";
} | {
  "phases"?: Array<string>;
  "plan_id": string;
  "source": PauseSource;
  "type": "pause_points_resolved";
} | {
  "phase": string;
  "report": PhaseReport;
  "type": "phase_reported";
} | {
  "delta"?: ProjectPlanDelta | null;
  "milestones": Array<ProposedMilestone>;
  "plan": ProjectPlanSpec;
  "project_id": ProjectId;
  "supersedes"?: number | null;
  "type": "project_plan_proposed";
  "version": number;
} | {
  "approved": boolean;
  "note"?: string | null;
  "project_id": ProjectId;
  "type": "project_plan_decided";
  "version": number;
} | {
  "child_task_id": TaskId;
  "depth": number;
  "plan_id": string;
  "type": "child_task_created";
  "unit_key": string;
} | {
  "child_task_id": TaskId;
  "plan_id": string;
  "stage": string;
  "type": "child_adopted";
  "unit_key": string;
} | {
  "action": UnitGateAction;
  "declared": UnitDeclared;
  "depth": number;
  "gate": ExecutionMode;
  "plan_id": string;
  "reason"?: string;
  "score"?: number;
  "threshold": number;
  "type": "unit_gate_overridden";
  "unit_key": string;
} | {
  "decision": DecisionRequest;
  "type": "decision_requested";
} | {
  "by": string;
  "id": string;
  "note"?: string | null;
  "option": string;
  "type": "decision_answered";
} | {
  "id": string;
  "reason": string;
  "type": "decision_withdrawn";
} | {
  "plan_id": string;
  "reasons": Array<string>;
  "type": "plan_approval_requested";
} | {
  "detail": string;
  "head"?: string | null;
  "reason": DeliverySkipReason;
  "type": "delivery_skipped";
} | {
  "commit_error"?: string | null;
  "commit_sha"?: string | null;
  "date": string;
  "deleted": number;
  "fixed": number;
  "merged": number;
  "new": number;
  "push": string;
  "push_detail"?: string | null;
  "type": "knowledge_curation_applied";
} | {
  "origin": string;
  "request": IntegrationRequest;
  "type": "integration_requested";
} | {
  "answer": string;
  "note"?: string | null;
  "request_id": string;
  "type": "integration_answered";
} | {
  "added"?: Array<string>;
  "removed"?: Array<string>;
  "restored"?: Array<string>;
  "source": string;
  "type": "model_catalog_changed";
} | {
  "actor": string;
  "model_id"?: string | null;
  "previous"?: string | null;
  "source": string;
  "tier": Tier;
  "type": "model_role_assignment_changed";
} | {
  "detail": string;
  "path"?: Array<DecisionPathEntry>;
  "reason"?: string;
  "since"?: string;
  "task_id": TaskId;
  "type": "stall_detected";
};

export type EventRow = {
  "event": Event;
  "id": number;
  "seq": number;
  "task_id": TaskId;
  "ts": string;
};

export type EventsPage = {
  "has_more": boolean;
  "items": Array<EventRow>;
};

export type EvidenceView = {
  "command"?: string | null;
  "criterion": number;
  "exit"?: number | null;
  "stdout_tail"?: string | null;
};

export type ExcludedReason = {
  "kind": "constraint";
  "name": string;
} | {
  "kind": "quota_exhausted";
} | {
  "kind": "cooldown";
} | {
  "kind": "concurrency";
} | {
  "kind": "rate_limit";
} | {
  "kind": "health_down";
} | {
  "kind": "circuit_open";
} | {
  "kind": "disabled";
} | {
  "field": string;
  "kind": "unknown_required";
} | {
  "kind": "quality_invalid";
} | {
  "kind": "quality_below_min";
} | {
  "kind": "invalid_estimate";
} | {
  "code": string;
  "kind": "other";
};

export type ExecutionChildSpec = {
  "acceptance": Array<Criterion>;
  "depends_on"?: Array<string>;
  "features"?: TaskFeatureHints | null;
  "genre"?: string | null;
  "key": string;
  "objective": string;
  "requirements"?: TaskRequirements;
  "skills"?: Array<string>;
  "title": string;
};

export type ExecutionGateDecision = {
  "depth"?: number | null;
  "mode": ExecutionMode;
  "policy_version": string;
  "rule_id": string;
  "score": number;
  "shadow": boolean;
  "signals"?: Array<GateSignal>;
  "source": GateSource;
  "threshold": number;
};

export type ExecutionHintSpec = {
  "explicit": boolean;
  "mode": ExecutionMode;
};

export type ExecutionMetrics = {
  "behind_target_age_seconds"?: number | null;
  "behind_target_commits"?: number | null;
  "behind_target_observed_at"?: string | null;
  "budget_exhausted_by_kind"?: {
  [key: string]: number;
};
  "continuation"?: ContinuationMetrics;
  "continuations": number;
  "cost_usd"?: number | null;
  "cost_usd_complete"?: boolean;
  "final_status": Status;
  "gate_mode"?: ExecutionMode | null;
  "gate_shadow"?: boolean;
  "has_plan"?: boolean;
  "max_turn_failures": number;
  "peak_context_tokens"?: number | null;
  "quota"?: Array<QuotaUse>;
  "quota_unknown_runs"?: number;
  "repairs_by_class"?: {
  [key: string]: number;
};
  "repairs_total": number;
  "replans": number;
  "retries": number;
  "runs_by_role"?: {
  [key: string]: number;
};
  "total_cache_read_tokens"?: number | null;
  "total_input_tokens"?: number | null;
  "total_output_tokens"?: number | null;
  "wall_ms"?: number | null;
  "work_units_done": number;
  "work_units_total": number;
};

export type ExecutionMetricsGroup = {
  "completion_rate"?: number | null;
  "continuation"?: ContinuationMetrics;
  "continuation_by_work_unit"?: {
  [key: string]: ContinuationMetrics;
};
  "continuations": number;
  "cost_usd_complete"?: boolean;
  "done": number;
  "failed": number;
  "key": string;
  "max_turn_failures": number;
  "other": number;
  "quota"?: Array<QuotaUse>;
  "repairs": number;
  "replans": number;
  "rollup"?: RollupMetrics | null;
  "tasks": number;
};

export type ExecutionMetricsSummary = {
  "accounts_now"?: Array<AccountNowView>;
  "continuation"?: ContinuationMetrics;
  "continuation_by_work_unit"?: {
  [key: string]: ContinuationMetrics;
};
  "group_by": string;
  "groups": Array<ExecutionMetricsGroup>;
  "since"?: string | null;
  "total_tasks": number;
};

export type ExecutionMode = "atomic" | "compound";

export type ExecutionPhase = "planning" | "executing" | "repairing" | "verifying" | "awaiting_human" | "awaiting_children" | "awaiting_plan_approval";

export type ExecutionPlanOverview = {
  "id": string;
  "origin": PlanOrigin;
  "phases"?: Array<PhaseSpec>;
  "rationale": string;
  "serialized_reason"?: string | null;
  "version": number;
  "versions": Array<ExecutionPlanVersionSummary>;
  "work_units": Array<ExecutionWorkUnitView>;
};

export type ExecutionPlanSpec = {
  "children"?: Array<ExecutionChildSpec>;
  "decisions"?: Array<DecisionSpec>;
  "phases"?: Array<PhaseSpec>;
  "rationale": string;
  "schema": string;
  "stages"?: Array<StageSpec>;
  "units"?: Array<PlanUnitSpec>;
  "work_units"?: Array<WorkUnitSpec>;
};

export type ExecutionPlanVersionSummary = {
  "created_at": string;
  "id": string;
  "origin": PlanOrigin;
  "reason"?: string | null;
  "status": PlanStatus;
  "superseded_at"?: string | null;
  "version": number;
};

export type ExecutionPlanVersionView = {
  "created_at": string;
  "id": string;
  "origin": PlanOrigin;
  "planner_run_id"?: string | null;
  "status": PlanStatus;
  "superseded_at"?: string | null;
  "version": number;
};

export type ExecutionPlanView = {
  "adoptions"?: Array<AdoptionOutcome>;
  "created_at": string;
  "decisions_raised"?: number;
  "id": string;
  "origin": PlanOrigin;
  "plan": ExecutionPlanSpec;
  "planner_run_id"?: string | null;
  "replan"?: ReplanDiff | null;
  "serialized_reason"?: string | null;
  "status": PlanStatus;
  "superseded_at"?: string | null;
  "task_id": string;
  "version": number;
  "versions"?: Array<ExecutionPlanVersionView>;
  "work_units": Array<WorkUnitView>;
};

export type ExecutionView = {
  "awaiting_children"?: Array<AwaitedChildView>;
  "gate"?: ExecutionGateDecision | null;
  "metrics": ExecutionMetrics;
  "phase"?: ExecutionPhase | null;
  "phase_checkpoint"?: PhaseCheckpointView | null;
  "plan"?: ExecutionPlanOverview | null;
  "plan_approval"?: PlanApprovalView | null;
  "route"?: RouteDecision | null;
};

export type ExecutionWorkUnitView = {
  "assignee"?: string | null;
  "blocked_reason"?: WorkUnitBlockedReason | null;
  "branch"?: string | null;
  "check_progress"?: IntegrationCheckProgress | null;
  "child_task_id"?: string | null;
  "continuations": number;
  "created_at": string;
  "depends_on": Array<string>;
  "expected_write_paths"?: Array<string> | null;
  "harness"?: string | null;
  "head_commit"?: string | null;
  "id": string;
  "integrated_commit"?: string | null;
  "key": string;
  "kind": WorkUnitKind;
  "lane"?: Tier | null;
  "last_checkpoint"?: Checkpoint | null;
  "last_reason"?: string | null;
  "model"?: string | null;
  "phase"?: string | null;
  "retries": number;
  "running_run_id"?: string | null;
  "runs": number;
  "seq": number;
  "status": WorkUnitStatus;
  "title": string;
  "updated_at": string;
};

export type FailedWorkUnitCheck = {
  "cmd": string;
  "detail": string;
  "expect_exit": number;
};

export type FailureClass = "infra" | "work";

export type FailureSummary = {
  "class": FailureClass;
  "delivered_release"?: string | null;
  "reason": string;
};

export type FeatureStage = "dispatch" | "proxy";

export type FileDiffStat = {
  "added": number;
  "deleted": number;
};

export type FileIntent = {
  "path": string;
  "source": SideIntent;
  "target": SideIntent;
};

export type GateSignal = {
  "detail": string;
  "name": string;
  "weight": number;
};

export type GateSource = "policy" | "human" | "hint";

export type GenreConfigView = {
  "capabilities"?: Array<string>;
  "default_role"?: string | null;
  "description": string;
  "id": string;
  "input_artifacts"?: Array<string>;
  "output_artifacts"?: Array<string>;
  "roles": Array<string>;
};

export type Graph = {
  "edges": Array<GraphEdge>;
  "nodes": Array<GraphNode>;
};

export type GraphEdge = {
  "from": TaskId;
  "kind": string;
  "to": TaskId;
};

export type GraphNode = {
  "id": TaskId;
  "kind": TaskKind;
  "parent_id"?: TaskId | null;
  "role"?: string | null;
  "status": Status;
  "title": string;
};

export type HarnessErrorClass = "supply" | "infra" | "lease_expired" | "idle_timeout";

export type HarnessPrefs = {
  "allowed"?: Array<string>;
  "default"?: string | null;
};

export type Health = {
  "api_version": string;
  "celeris_version": string;
  "db": DbInfo;
  "instance_id": string;
  "mode": string;
  "now": string;
  "release": string;
  "role": string;
  "schema_version": number;
  "started_at": string;
};

export type HumanAttestation = {
  "payload": string;
  "signature": string;
};

export type HumanInboxCounts = {
  "by_kind": {
  [key: string]: number;
};
  "total": number;
};

export type HumanInboxView = {
  "counts": HumanInboxCounts;
  "items": Array<InboxItem>;
  "suppressed": {
  [key: string]: number;
};
};

export type ImpactChangeView = {
  "after"?: string | null;
  "before"?: string | null;
  "excluded_reason"?: string | null;
  "id": string;
  "kind": ImpactKind;
  "tier": string;
};

export type ImpactKind = "provider" | "proxy";

export type ImpactView = {
  "changes": Array<ImpactChangeView>;
};

export type InFlight = {
  "kind": InFlightKind;
  "provider": string;
  "run_id": string;
  "since": string;
  "task_id": TaskId;
};

export type InFlightKind = "worker" | "reviewer";

export type Inbox = {
  "approvals": Array<ApprovalItem>;
  "attention": Array<AttentionItem>;
  "browser_waits": Array<BrowserWaitItem>;
  "counts": InboxCounts;
  "decisions": Array<DecisionInboxItem>;
  "drafts": Array<DraftGroup>;
  "questions": Array<QuestionItem>;
  "suppressed": {
  [key: string]: number;
};
};

export type InboxAnswer = {
  "body_schema": {
  [key: string]: string;
};
  "method": string;
  "native"?: InboxNativeOp | null;
  "path": string;
};

export type InboxAnswerBody = {
  "note"?: string | null;
  "option": string;
  "payload"?: unknown;
};

export type InboxAnswerResult = {
  "item_id": string;
  "removed": boolean;
  "result": unknown;
};

export type InboxBlocking = {
  "root"?: TaskRef | null;
  "summary": string;
  "tasks": Array<TaskRef>;
  "units": Array<string>;
};

export type InboxCounts = {
  "approvals": number;
  "attention": number;
  "browser_waits": number;
  "by_status": {
  [key: string]: number;
};
  "decisions": number;
  "drafts": number;
  "questions": number;
};

export type InboxItem = {
  "age_secs": number;
  "answer": InboxAnswer;
  "blocked_by": Array<string>;
  "blocking": InboxBlocking;
  "created_at": string;
  "detail"?: string | null;
  "due_at"?: string | null;
  "id": string;
  "kind": InboxKind;
  "links": Array<InboxLink>;
  "options": Array<InboxOption>;
  "project_id"?: string | null;
  "recommended"?: string | null;
  "task"?: TaskRef | null;
  "title": string;
};

export type InboxKind = "decision" | "plan_gate" | "phase_gate" | "authorization" | "browser_wait" | "question" | "acceptance_check" | "draft_accept" | "project_plan" | "failed" | "unroutable" | "cluster_login" | "delivery_skipped" | "integration_request" | "knowledge_review";

export type InboxLink = {
  "href": string;
  "label": string;
};

export type InboxNativeOp = {
  "method": string;
  "path": string;
};

export type InboxOption = {
  "effect": string;
  "key": string;
  "label": string;
  "needs_note": boolean;
};

export type InstanceRole = "active" | "standby" | "draining" | "verify";

export type InstructBody = {
  "scope"?: string | null;
  "text": string;
};

export type IntegrateBody = {
  "confirm"?: boolean;
  "method": IntegrationMethod;
  "note"?: string | null;
};

export type IntegrateResult = {
  "child_task_id"?: string | null;
  "integration": TaskIntegration;
};

export type IntegrationCheckDone = {
  "cmd": string;
  "duration_ms": number;
  "exit"?: number | null;
  "index": number;
  "pass": boolean;
  "timed_out": boolean;
};

export type IntegrationCheckProgress = {
  "current"?: IntegrationCheckRunning | null;
  "finished": Array<IntegrationCheckDone>;
  "total": number;
};

export type IntegrationCheckRunning = {
  "cmd": string;
  "index": number;
  "started_at": string;
};

export type IntegrationId = string;

export type IntegrationMethod = "merge" | "pr" | "discard";

export type IntegrationRepairExhaustReason = "limit_reached" | "plan_issue" | "work_unit_failed" | "budget_exhausted" | "result_untrusted" | "abort_failed" | "worktree_unavailable";

export type IntegrationRepairState = "scheduled" | "resolved" | "exhausted";

export type IntegrationRepairView = {
  "attempt": number;
  "before_sha"?: string | null;
  "conflict_files"?: Array<string>;
  "fallback"?: boolean | null;
  "max_attempts": number;
  "reason"?: IntegrationRepairExhaustReason | null;
  "rollback_to_sha"?: string | null;
  "state": IntegrationRepairState;
  "target_ref"?: string | null;
  "target_sha": string;
  "work_unit_id"?: string | null;
};

export type IntegrationRequest = {
  "actions": Array<ResolutionAction>;
  "candidate_sha"?: string | null;
  "conflict_files": Array<string>;
  "intent": Array<FileIntent>;
  "merge_base"?: string | null;
  "reason": string;
  "recommendation": string;
  "source_branch": string;
  "source_sha": string;
  "target_branch": string;
  "target_sha": string;
};

export type IntegrationState = "done" | "open" | "merged" | "closed" | "conflict" | "failed";

export type KnowledgeAcceptBody = {
  "overwrite"?: boolean;
  "path"?: string | null;
};

export type KnowledgeCandidate = {
  "body": string;
  "confidence"?: Confidence | null;
  "created"?: string | null;
  "html": string;
  "id": string;
  "op"?: string | null;
  "path": string;
  "scope"?: string | null;
  "sources"?: Array<string>;
  "tags"?: Array<string>;
  "target": string;
  "target_exists": boolean;
  "title": string;
};

export type KnowledgeInbox = {
  "initialized": boolean;
  "items": Array<KnowledgeCandidate>;
  "root": string;
};

export type KnowledgeItem = {
  "confidence"?: Confidence | null;
  "path": string;
  "scope"?: string | null;
  "sources"?: Array<string>;
  "tags"?: Array<string>;
  "title": string;
  "updated"?: string | null;
};

export type KnowledgeMount = {
  "docs"?: string | null;
  "kind": MountKind;
  "name"?: string | null;
  "path"?: string | null;
  "scope"?: string | null;
};

export type KnowledgePage = {
  "confidence"?: Confidence | null;
  "etag"?: string | null;
  "history": Array<DocCommit>;
  "html": string;
  "path": string;
  "raw": string;
  "root": string;
  "scope"?: string | null;
  "sources"?: Array<string>;
  "tags"?: Array<string>;
  "title": string;
  "too_large": boolean;
  "updated"?: string | null;
};

export type KnowledgePagePutBody = {
  "body": string;
  "etag"?: string | null;
  "message"?: string | null;
  "path": string;
};

export type KnowledgePageRef = {
  "criterion_idx": number;
  "path": string;
};

export type KnowledgePageResult = {
  "etag"?: string | null;
  "path": string;
  "sha": string;
  "unchanged": boolean;
};

export type KnowledgeRejectResult = {
  "id": string;
  "sha": string;
};

export type KnowledgeTree = {
  "generated_at"?: string | null;
  "inbox_count": number;
  "initialized": boolean;
  "items": Array<KnowledgeItem>;
  "q"?: string | null;
  "root": string;
  "scope"?: string | null;
  "scopes"?: Array<string>;
  "truncated": boolean;
};

export type LaneDecision = {
  "clamped_by"?: string | null;
  "escalation"?: string | null;
  "features": TaskFeatures;
  "hint"?: Tier | null;
  "lane": Tier;
  "policy_version": string;
  "proposed": Tier;
  "reasons"?: Array<string>;
  "rule_id": string;
  "shadow"?: ShadowDecision | null;
  "source": TierSource;
};

export type LaneResolution = {
  "account"?: string | null;
  "adapter"?: string;
  "lane"?: Tier | null;
  "model_id"?: string;
  "provider"?: string | null;
  "reasoning_effort"?: string | null;
  "selection"?: ProviderSelection | null;
};

export type Lease = {
  "expires_at": string;
  "worker_run_id": string;
};

export type Level = "low" | "medium" | "high";

export type LlmCelerisTierView = {
  "resolves_to"?: string | null;
  "tier": string;
};

export type LlmSourceAccountView = {
  "cooldown_reason"?: string | null;
  "cooldown_until"?: number | null;
  "id": string;
  "logged_in": boolean;
  "remaining"?: number | null;
  "remaining_long"?: number | null;
  "remaining_short"?: number | null;
};

export type LlmSourceBilledCostView = {
  "cash_usd"?: number | null;
};

export type LlmSourceCostView = {
  "assumptions"?: Array<string>;
  "billed": LlmSourceBilledCostView;
  "effective_usd"?: number | null;
  "opportunity": LlmSourceOpportunityCostView;
};

export type LlmSourceFreshnessView = {
  "age_secs"?: number | null;
  "expires_at"?: string | null;
  "observed_at"?: string | null;
  "stale": boolean;
};

export type LlmSourceOpportunityCostView = {
  "resource_usd"?: number | null;
  "shadow_usd"?: number | null;
};

export type LlmSourceRef = string;

export type LlmSourceStateView = {
  "cost"?: LlmSourceCostView | null;
  "deployment_id": string;
  "freshness": LlmSourceFreshnessView;
  "latency_ms"?: number | null;
  "pressure"?: number | null;
  "quota_remaining"?: number | null;
  "quota_reset_at"?: string | null;
  "reachability": string;
  "unknown"?: Array<string>;
};

export type LlmSourceView = {
  "accounts": Array<LlmSourceAccountView>;
  "deployments"?: Array<LlmSourceStateView>;
  "enabled": boolean;
  "id": string;
  "kind": string;
  "last_hour_completion_tokens": number;
  "last_hour_prompt_tokens": number;
  "last_hour_requests": number;
  "reachable"?: boolean | null;
  "unreachable_reason"?: string | null;
};

export type LlmSourcesView = {
  "celeris_tiers"?: Array<LlmCelerisTierView>;
  "sources": Array<LlmSourceView>;
};

export type McpCall = {
  "at": string;
  "client_id": string;
  "error_kind"?: string | null;
  "id": string;
  "latency_ms": number;
  "ok": boolean;
  "tool": string;
};

export type McpCallsView = {
  "items": Array<McpCall>;
};

export type McpClient = {
  "created_at": string;
  "id": string;
  "last_used_at"?: string | null;
  "name": string;
  "revoked_at"?: string | null;
  "scopes"?: Array<McpScope>;
  "token_hash"?: string | null;
};

export type McpClientsView = {
  "items": Array<McpClient>;
};

export type McpScope = "knowledge:read" | "knowledge:propose" | "tasks:read" | "tasks:interact" | "tasks:control" | "tasks:decide" | "console:instruct" | "org:read" | "org:write" | "skills:read" | "skills:write";

export type MemoryView = {
  "notes": string;
  "notes_path": string;
  "project"?: string | null;
  "project_path"?: string | null;
};

export type Message = {
  "created_at": string;
  "id": MessageId;
  "metadata"?: MessageMetadata | null;
  "node_id": string;
  "project_id"?: ProjectId | null;
  "role": MessageRole;
  "run_id"?: string | null;
  "task_id"?: TaskId | null;
  "text": string;
};

export type MessageAccepted = {
  "message_id": string;
  "task_id": TaskId;
};

export type MessageActionFailure = {
  "kind": string;
  "reason": string;
};

export type MessageActionResult = {
  "kind": string;
  "milestone_id"?: MilestoneId | null;
  "project_id"?: ProjectId | null;
  "summary": string;
  "task_id"?: TaskId | null;
};

export type MessageId = string;

export type MessageList = {
  "items": Array<Message>;
};

export type MessageMetadata = {
  "actions_executed"?: Array<MessageActionResult>;
  "actions_failed"?: Array<MessageActionFailure>;
  "author"?: string | null;
};

export type MessagePostBody = {
  "project_id"?: ProjectId | null;
  "text": string;
};

export type MessageRole = "user" | "node";

export type Milestone = {
  "created_at": string;
  "description"?: string;
  "id": MilestoneId;
  "paused_from"?: MilestoneStatus | null;
  "plan_key"?: string | null;
  "project_id": ProjectId;
  "seq": number;
  "status": MilestoneStatus;
  "title": string;
  "updated_at": string;
};

export type MilestoneCreateBody = {
  "description"?: string | null;
  "status"?: MilestoneStatus | null;
  "title": string;
};

export type MilestoneDecideBody = {
  "decision": MilestoneDecision;
  "note"?: string | null;
};

export type MilestoneDecided = {
  "conversation_task_id"?: TaskId | null;
  "decision": MilestoneDecision;
  "message_id"?: string | null;
  "milestone": Milestone;
  "next_milestone"?: Milestone | null;
  "plan_task_id"?: TaskId | null;
};

export type MilestoneDecision = "ok" | "discuss" | "ng";

export type MilestoneId = string;

export type MilestoneLifecycle = {
  "cancelled_tasks"?: Array<TaskRef>;
  "milestone": Milestone;
};

export type MilestoneModify = {
  "acceptance"?: Array<Criterion> | null;
  "depends_on"?: Array<string> | null;
  "execution"?: ExecutionMode | null;
  "features"?: TaskFeatureHints | null;
  "genre"?: string | null;
  "key": string;
  "objective"?: string | null;
  "pause_after"?: PausePolicy | null;
  "reach_criteria"?: string | null;
  "repos"?: Array<string> | null;
  "skills"?: Array<string> | null;
  "title"?: string | null;
};

export type MilestonePatchBody = {
  "status": MilestoneStatus;
};

export type MilestoneReviewView = {
  "at": string;
  "message_id": string;
  "text": string;
};

export type MilestoneSpec = {
  "acceptance"?: Array<Criterion>;
  "depends_on"?: Array<string>;
  "execution"?: ExecutionMode | null;
  "features"?: TaskFeatureHints | null;
  "genre"?: string | null;
  "key": string;
  "objective": string;
  "pause_after"?: PausePolicy | null;
  "reach_criteria": string;
  "repos"?: Array<string>;
  "skills"?: Array<string>;
  "title": string;
};

export type MilestoneStatus = "proposed" | "approved" | "in_progress" | "reached" | "redesigned" | "paused" | "cancelled";

export type MilestoneView = {
  "created_at": string;
  "description"?: string;
  "id": MilestoneId;
  "paused_from"?: MilestoneStatus | null;
  "plan_key"?: string | null;
  "project_id": ProjectId;
  "proposal"?: Milestone | null;
  "review"?: MilestoneReviewView | null;
  "seq": number;
  "status": MilestoneStatus;
  "title": string;
  "updated_at": string;
};

export type ModelBinding = {
  "model_id"?: string | null;
  "name": string;
  "reasoning_effort"?: string | null;
  "unavailable_reason"?: string | null;
};

export type ModelCatalogItem = {
  "assigned_tiers": Array<Tier>;
  "available": boolean;
  "capabilities": unknown;
  "display_name"?: string | null;
  "first_seen": string;
  "last_seen": string;
  "model_id": string;
  "override"?: ModelCatalogOverrideView | null;
  "routing": ModelCatalogRoutingView;
  "source": string;
};

export type ModelCatalogOverrideView = {
  "alias"?: string | null;
  "disabled"?: boolean;
  "note"?: string | null;
  "tier"?: Tier | null;
};

export type ModelCatalogRoutingView = {
  "deployments": Array<string>;
  "tiers": Array<string>;
};

export type ModelCatalogView = {
  "items": Array<ModelCatalogItem>;
  "last_discovery": Array<DiscoveryRecordView>;
};

export type ModelPrefs = {
  "allowed_tiers"?: Array<Tier>;
  "tier"?: Tier | null;
};

export type MountKind = "kb" | "repo" | "dir" | "memory";

export type NewBrowserWait = {
  "credential"?: CredentialRef | null;
  "credential_policy_id"?: string | null;
  "operation"?: OperationIntent | null;
  "origin": string;
  "owner_id"?: string | null;
  "policy_hash": string;
  "policy_revision": number;
  "purpose": string;
  "reason": BrowserWaitReason;
  "resume_key": string;
  "run_id": string;
  "session_id": string;
  "trusted_login"?: TrustedLogin | null;
  "ttl_secs"?: number | null;
  "work_unit_id"?: string | null;
};

export type NewPlanSpec = {
  "goal": string;
  "max_retries"?: number;
  "max_turns"?: number;
  "max_wall_secs"?: number;
  "priority"?: number;
  "tier"?: Tier;
  "workspace"?: string | null;
};

export type NewTaskBody = {
  "acceptance": Array<CriterionSpec>;
  "adapter"?: string | null;
  "aggregate"?: boolean;
  "assignee"?: string | null;
  "category"?: TaskCategory | null;
  "cluster"?: string | null;
  "depends_on"?: Array<TaskId>;
  "execution"?: ExecutionMode | null;
  "expected_write_paths"?: Array<string> | null;
  "features"?: TaskFeatureHints | null;
  "genre"?: string | null;
  "kind"?: TaskKind;
  "labels"?: Array<string>;
  "max_retries"?: number;
  "max_turns"?: number | null;
  "max_wall_secs"?: number | null;
  "milestone_id"?: MilestoneId | null;
  "mode"?: TaskMode | null;
  "objective": string;
  "parent"?: TaskId | null;
  "pause_after"?: PausePolicy | null;
  "priority"?: PriorityInput | null;
  "project_id"?: ProjectId | null;
  "repos"?: Array<string>;
  "requirements"?: TaskRequirements;
  "role"?: string | null;
  "skills"?: Array<string>;
  "stages_hint"?: Array<StageHint>;
  "status"?: Status | null;
  "tier"?: Tier | null;
  "title": string;
  "workspace"?: string | null;
  "workspace_mode"?: WorkspaceMode | null;
};

export type NodeSessionSummary = {
  "approx_tokens": number;
  "last_used_at": string;
  "node_id": string;
  "turns": number;
};

export type Normalization = {
  "cost_reference_usd": number;
  "latency_reference_ms": number;
};

export type Notice = {
  "count": number;
  "first_at": string;
  "group_key": string;
  "id": NoticeId;
  "kind": NoticeKind;
  "last_at": string;
  "links"?: Array<NoticeLink>;
  "project_id"?: string | null;
  "read_at"?: string | null;
  "summary": string;
  "target"?: NoticeTarget | null;
  "task_id"?: string | null;
  "title": string;
};

export type NoticeId = string;

export type NoticeKind = "task_done" | "report" | "bad_news" | "secretary_reply" | "delivery" | "release" | "cron_run" | "auto_recovered" | "requeue_limit_near";

export type NoticeLink = {
  "href": string;
  "label": string;
};

export type NoticeReadAllResult = {
  "marked": number;
};

export type NoticeReadResult = {
  "id": string;
  "read_at": string;
};

export type NoticeTarget = {
  "id": string;
  "kind": string;
};

export type NotificationKind = "inbox_new" | "digest" | "milestone_ready" | "approval_pending" | "question_blocked" | "bad_news" | "secretary_reply" | "task_ready" | "cluster_login_needed" | "task_failed" | "phase_checkpoint" | "decision_requested" | "plan_approval";

export type NotificationsView = {
  "items": Array<Notice>;
  "next_before"?: string | null;
  "unread": number;
};

export type NotifyRecent = {
  "attempts": number;
  "created_at": string;
  "error"?: string | null;
  "key": string;
  "kind": NotificationKind;
  "ok"?: boolean | null;
  "project_id"?: ProjectId | null;
  "sent_at"?: string | null;
};

export type NotifyTestResult = {
  "detail"?: string | null;
  "ok": boolean;
};

export type NotifyView = {
  "configured": boolean;
  "digest_interval_secs": number;
  "digest_last_sent_at"?: string | null;
  "digest_max_lines": number;
  "fingerprint"?: string | null;
  "gui_base_url"?: string | null;
  "inbox_batch_secs": number;
  "inbox_new_last_sent_at"?: string | null;
  "inbox_reminder_secs": number;
  "recent": Array<NotifyRecent>;
  "secret_id": string;
};

export type Objective = "quality_first" | "balanced" | "resource_first";

export type OnChildFailure = "retry_then_ask" | "ignore";

export type OperationIntent = {
  "action": string;
  "args_digest"?: string | null;
  "intent_id": string;
};

export type OrgCreateBody = {
  "brief"?: string | null;
  "genre"?: string | null;
  "id": string;
  "kind": OrgKind;
  "name": string;
  "parent_id"?: string | null;
  "position"?: number | null;
  "profile"?: Profile | null;
};

export type OrgKind = "secretary" | "department" | "section";

export type OrgList = {
  "effective_profiles"?: Array<EffectiveProfile>;
  "items": Array<OrgNode>;
  "lead_sessions"?: Array<NodeSessionSummary>;
};

export type OrgNode = {
  "brief"?: string;
  "created_at": string;
  "genre"?: string | null;
  "id": string;
  "kind": OrgKind;
  "name": string;
  "parent_id"?: string | null;
  "position"?: number;
  "profile"?: Profile;
  "updated_at": string;
};

export type OrgPatchBody = {
  "brief"?: string | null;
  "genre"?: string | null;
  "kind"?: OrgKind | null;
  "name"?: string | null;
  "parent_id"?: string | null;
  "position"?: number | null;
  "profile"?: Profile | null;
};

export type OrgSkillMountBody = {
  "skill": string;
};

export type ParentUnit = {
  "attempt"?: number;
  "plan_id": string;
  "stage": string;
  "task_id": TaskId;
  "unit_key": string;
};

export type PausePolicy = {
  "mode": "none";
} | {
  "mode": "each_phase";
} | {
  "mode": "after";
  "phases"?: Array<string>;
};

export type PauseSource = "human" | "agent";

export type Permissions = {
  "approvals"?: Array<string>;
};

export type PhaseCheckResult = {
  "cmd": string;
  "pass": boolean;
  "summary": string;
};

export type PhaseCheckpointView = {
  "report": PhaseReport;
  "report_idx"?: number | null;
};

export type PhaseGateAction = "continue" | "replan" | "withdraw";

export type PhaseGateRequest = {
  "action": PhaseGateAction;
  "note"?: string | null;
};

export type PhaseMerged = {
  "commit": string;
  "key": string;
  "parent_head"?: string | null;
  "skipped"?: boolean;
  "target_sha"?: string | null;
};

export type PhaseReport = {
  "artifact_paths"?: Array<string>;
  "child_units"?: Array<string>;
  "diff_stat"?: Array<string>;
  "integration"?: Array<string>;
  "next_phase"?: string | null;
  "next_phase_work_units"?: Array<string>;
  "phase": string;
  "phase_title": string;
  "phases_done"?: Array<string>;
  "quota_summary": string;
  "work_units"?: Array<string>;
};

export type PhaseSpec = {
  "key": string;
  "kind": WorkUnitKind;
  "title": string;
};

export type PlanApprovalStage = {
  "key": string;
  "review_human": boolean;
  "title": string;
  "units": Array<string>;
};

export type PlanApprovalView = {
  "plan_id": string;
  "reasons": Array<string>;
  "summary": string;
};

export type PlanDagNode = {
  "change"?: PlanNodeChange | null;
  "children_done": number;
  "children_total": number;
  "depends_on": Array<string>;
  "key": string;
  "milestone_id": MilestoneId;
  "milestone_status"?: MilestoneStatus | null;
  "quota"?: Array<QuotaUse>;
  "stop_reason"?: PlanStopReason | null;
  "task_id": TaskId;
  "task_status"?: Status | null;
  "title": string;
  "work_units_done": number;
  "work_units_total": number;
};

export type PlanDagProposal = {
  "nodes": Array<PlanDagNode>;
  "rationale": string;
  "supersedes"?: number | null;
  "version": number;
};

export type PlanGateAction = "approve" | "replan" | "withdraw";

export type PlanGateRequest = {
  "action": PlanGateAction;
  "note"?: string | null;
};

export type PlanNodeChange = "add" | "modify" | "remove" | "cancel";

export type PlanOrigin = "planner" | "human" | "repair" | "fixture";

export type PlanStatus = "active" | "superseded" | "completed" | "abandoned";

export type PlanStopReason = "failed" | "awaiting_human" | "question" | "awaiting_go" | "paused";

export type PlanUnitSpec = {
  "acceptance"?: Array<Criterion>;
  "adopt"?: TaskId | null;
  "budget"?: WorkUnitBudget | null;
  "checks"?: Array<WorkUnitCheck>;
  "context"?: UnitContext;
  "decisions"?: Array<DecisionSpec>;
  "depends_on"?: Array<string>;
  "done_when"?: Array<string>;
  "expected_write_paths"?: Array<string> | null;
  "features"?: unknown;
  "gate"?: ExecutionMode | null;
  "genre"?: string | null;
  "harness"?: string | null;
  "key": string;
  "kind": WorkUnitKind;
  "needs_decisions"?: Array<string>;
  "objective": string;
  "outputs"?: Array<string>;
  "repos"?: Array<string>;
  "requirements"?: TaskRequirements;
  "skills"?: Array<string>;
  "stage": string;
  "title": string;
};

export type PriorityInput = PriorityLabel | number;

export type PriorityLabel = "P0" | "P1" | "P2" | "P3";

export type Problem = {
  "code": string;
  "detail": string;
  "instance": string;
  "status": number;
  "title": string;
  "type": string;
};

export type Profile = {
  "browser"?: BrowserCapability | null;
  "budget"?: BudgetPrefs;
  "deny_tools"?: Array<string>;
  "harnesses"?: HarnessPrefs;
  "knowledge"?: Array<KnowledgeMount>;
  "model"?: ModelPrefs;
  "permissions"?: Permissions;
  "policy"?: Array<string>;
  "review"?: ReviewPrefs;
  "run"?: ProfileRun | null;
  "skills"?: Array<string>;
  "skills_mounts"?: Array<string>;
  "tools"?: Array<string>;
};

export type ProfileRun = "host" | "container";

export type ProgressKind = "tool_use" | "tool_result" | "text" | "thinking" | "status";

export type Project = {
  "archived_at"?: string | null;
  "auto_advance"?: boolean;
  "created_at": string;
  "id": ProjectId;
  "paused_from"?: ProjectStatus | null;
  "request": string;
  "secretary_summary"?: string | null;
  "slug"?: string | null;
  "status": ProjectStatus;
  "title": string;
  "updated_at": string;
  "workspace"?: WorkspaceSpec | null;
};

export type ProjectCreateBody = {
  "request": string;
  "title": string;
  "workspace"?: WorkspaceSpec | null;
};

export type ProjectDetail = {
  "milestones": Array<MilestoneView>;
  "milestones_frozen"?: number;
  "milestones_frozen_open"?: number;
  "project": Project;
  "project_plan"?: ProjectPlanDagView | null;
  "repos"?: Array<ProjectRepo>;
  "root_totals"?: ProjectRootTotals | null;
  "tasks": Array<ProjectTaskView>;
};

export type ProjectId = string;

export type ProjectIntegrationItem = {
  "integration": TaskIntegration;
  "task_status": Status;
  "task_title": string;
};

export type ProjectIntegrations = {
  "items": Array<ProjectIntegrationItem>;
};

export type ProjectLifecycle = {
  "cancelled_milestones"?: Array<MilestoneId>;
  "cancelled_tasks"?: Array<TaskRef>;
  "project": Project;
};

export type ProjectList = {
  "items": Array<Project>;
};

export type ProjectPatchBody = {
  "auto_advance"?: boolean | null;
  "request"?: string | null;
  "slug"?: string | null;
  "status"?: ProjectStatus | null;
  "title"?: string | null;
  "workspace"?: WorkspaceSpec | null;
};

export type ProjectPlanAccepted = {
  "task_id": TaskId;
};

export type ProjectPlanBody = {
  "milestone_id"?: MilestoneId | null;
  "mode"?: ProjectPlanMode;
  "note"?: string | null;
};

export type ProjectPlanDagView = {
  "current_version"?: number | null;
  "nodes": Array<PlanDagNode>;
  "pending"?: PlanDagProposal | null;
};

export type ProjectPlanDecideBody = {
  "decision": ProjectPlanDecisionInput;
  "note"?: string | null;
};

export type ProjectPlanDecided = {
  "decision": ProjectPlanDecisionInput;
  "milestones": Array<MilestoneId>;
  "plan_task_id": TaskId;
  "tasks": Array<TaskId>;
};

export type ProjectPlanDecisionInput = "approve" | "reject";

export type ProjectPlanDelta = {
  "add"?: Array<MilestoneSpec>;
  "base_version": number;
  "cancel"?: Array<string>;
  "modify"?: Array<MilestoneModify>;
  "rationale": string;
  "remove"?: Array<string>;
  "schema": string;
};

export type ProjectPlanMode = "decompose" | "milestones";

export type ProjectPlanRef = {
  "project_id": ProjectId;
  "supersedes"?: number | null;
  "version": number;
};

export type ProjectPlanSpec = {
  "milestones": Array<MilestoneSpec>;
  "rationale": string;
  "schema": string;
};

export type ProjectRepo = {
  "created_at": string;
  "default_branch"?: string | null;
  "id": RepoId;
  "is_primary"?: boolean;
  "kind": RepoKind;
  "location": WorkspaceSpec;
  "name": string;
  "project_id": ProjectId;
  "run"?: RepoRun;
  "sync"?: RepoSync | null;
};

export type ProjectRootTotals = {
  "by_status"?: {
  [key: string]: number;
};
  "root_tasks": number;
  "totals": RollupMetrics;
};

export type ProjectStatus = "active" | "done" | "proposed" | "paused" | "cancelled";

export type ProjectTaskView = {
  "assignee"?: string | null;
  "conversation": boolean;
  "depends_on": Array<TaskId>;
  "id": TaskId;
  "is_root_task"?: boolean;
  "milestone_id"?: MilestoneId | null;
  "parent_id"?: TaskId | null;
  "status": Status;
  "support"?: string | null;
  "title": string;
};

export type ProposedMilestone = {
  "key": string;
  "milestone_id": MilestoneId;
  "task_id": TaskId;
};

export type ProviderCandidate = {
  "detail"?: string | null;
  "kind": ProviderCandidateKind;
  "outcome": ProviderCandidateOutcome;
  "provider": string;
};

export type ProviderCandidateKind = "local" | "pool" | "other";

export type ProviderCandidateOutcome = "selected" | "available" | "full" | "down" | "cooldown" | "unsupported" | "no_account";

export type ProviderCheckResponse = {
  "checked_at": string;
  "detail"?: string | null;
  "result": ProviderCheckResult;
};

export type ProviderCheckResult = "ok" | "auth_failed" | "throttled" | "spawn_failed";

export type ProviderCheckView = {
  "at": string;
  "detail"?: string | null;
  "result": string;
};

export type ProviderConfigView = {
  "account_id"?: string | null;
  "account_pool"?: boolean;
  "adapter": string;
  "concurrency": number;
  "credential_refs"?: {
  [key: string]: string;
};
  "env_keys": Array<string>;
  "id": string;
  "kind"?: ProviderKind;
  "llm_source"?: ResolvedLlmSource | null;
  "model"?: string | null;
  "tier_models"?: {
  "cheap"?: ModelBinding;
  "frontier"?: ModelBinding;
  "standard"?: ModelBinding;
};
  "tiers": Array<Tier>;
};

export type ProviderKind = "adapter";

export type ProviderLive = {
  "account_id"?: string | null;
  "account_pool"?: boolean;
  "adapter": string;
  "concurrency": number;
  "credential_refs"?: {
  [key: string]: string;
};
  "env_keys"?: Array<string>;
  "id": string;
  "in_use": number;
  "in_use_cos"?: number;
  "kind"?: ProviderKind;
  "last_check"?: ProviderCheckView | null;
  "llm_source"?: ResolvedLlmSource | null;
  "model"?: string | null;
  "tier_models"?: {
  "cheap"?: ModelBinding;
  "frontier"?: ModelBinding;
  "standard"?: ModelBinding;
};
  "tiers": Array<Tier>;
};

export type ProviderSelection = {
  "candidates"?: Array<ProviderCandidate>;
  "reason": ProviderSelectionReason;
};

export type ProviderSelectionReason = "local_preferred" | "local_full" | "local_down" | "pool" | "fallback" | "sticky";

export type ProviderStats = {
  "by_day": Array<DailyUsage>;
  "done": number;
  "error": number;
  "input_tokens": number;
  "lease_expired": number;
  "output_tokens": number;
  "question": number;
  "requeue": number;
  "runs": number;
};

export type ProviderView = {
  "account_id"?: string | null;
  "account_pool"?: boolean;
  "adapter": string;
  "concurrency": number;
  "cooldown"?: CooldownView | null;
  "credential_refs"?: {
  [key: string]: string;
};
  "env_keys": Array<string>;
  "id": string;
  "in_use"?: number | null;
  "in_use_cos"?: number | null;
  "kind"?: ProviderKind;
  "last_check"?: ProviderCheckView | null;
  "llm_source"?: ResolvedLlmSource | null;
  "model"?: string | null;
  "stats": ProviderStats;
  "tier_models"?: {
  "cheap"?: ModelBinding;
  "frontier"?: ModelBinding;
  "standard"?: ModelBinding;
};
  "tiers": Array<Tier>;
};

export type Providers = {
  "items": Array<ProviderView>;
};

export type QualityEstimate = {
  "confidence"?: number | null;
  "feature_version": string;
  "index"?: number | null;
  "reasons": Array<string>;
};

export type QualityIndex = {
  "domain": string;
  "evaluation_version": string;
  "index": number;
  "provenance": string;
  "samples"?: number | null;
};

export type QuestionItem = {
  "approval_id"?: ApprovalId | null;
  "asked_at"?: string | null;
  "previous": Array<AnswerNote>;
  "question": string;
  "run_id"?: string | null;
  "task": TaskRef;
};

export type QuotaCalibration = {
  "k": number;
  "samples": number;
};

export type QuotaMethod = "measured" | "apportioned" | "estimated" | "unknown" | "free";

export type QuotaUse = {
  "account"?: string | null;
  "method_counts": {
  [key: string]: number;
};
  "runs": number;
  "runs_by_role"?: {
  [key: string]: number;
};
  "source": string;
  "used_pct"?: number | null;
  "window": QuotaWindow;
};

export type QuotaWindow = "five_hour" | "seven_day" | "one_month";

export type QuotaWindowUse = {
  "after"?: number | null;
  "before"?: number | null;
  "method": QuotaMethod;
  "resets_at"?: number | null;
  "used_pct"?: number | null;
  "window": QuotaWindow;
};

export type RateWindow = {
  "resets_at": number;
  "utilization": number;
};

export type RateWindowView = {
  "resets_at": string;
  "utilization": number;
};

export type ReadAllBody = {
  "before"?: string | null;
  "kind"?: NoticeKind | null;
  "project"?: string | null;
};

export type ReleaseChanges = {
  "base"?: string | null;
  "commit_count": number;
  "commits": Array<ReleaseCommit>;
  "file_count": number;
  "sensitive": Array<string>;
  "stale": boolean;
};

export type ReleaseCommit = {
  "sha": string;
  "subject": string;
};

export type ReleaseGate = {
  "failed_step"?: string | null;
  "ok": boolean;
  "steps"?: Array<ReleaseGateStep>;
};

export type ReleaseGateStep = {
  "exit": number;
  "secs": number;
  "step": string;
};

export type ReleaseItem = {
  "built_at"?: string | null;
  "changes"?: ReleaseChanges | null;
  "gate"?: ReleaseGate | null;
  "gate_ok": boolean;
  "is_current": boolean;
  "is_previous": boolean;
  "notes"?: ReleaseNotes | null;
  "on_main"?: boolean | null;
  "problem"?: string | null;
  "promote_failed"?: ReleasePromoteFailure | null;
  "promote_last_line"?: string | null;
  "promote_stale"?: boolean;
  "promoted_at"?: string | null;
  "promoting": boolean;
  "promotion"?: ReleasePromotionPreview | null;
  "ref"?: string | null;
  "schema_version"?: number | null;
  "sha12": string;
  "verify"?: ReleaseVerify | null;
};

export type ReleaseNoteChild = {
  "task_id": string;
  "title"?: string | null;
};

export type ReleaseNoteCommit = {
  "sha": string;
  "subject": string;
};

export type ReleaseNoteConfig = {
  "added_lines"?: Array<string>;
  "added_sections"?: Array<string>;
  "commit"?: string | null;
  "needs_review": boolean;
  "path": string;
  "status": string;
};

export type ReleaseNoteFile = {
  "commit"?: string | null;
  "path": string;
  "status": string;
  "title"?: string | null;
};

export type ReleaseNoteGateSkip = {
  "reason": string;
  "step": string;
};

export type ReleaseNoteSchema = {
  "changed"?: boolean | null;
  "from"?: number | null;
  "to"?: number | null;
};

export type ReleaseNoteTask = {
  "children"?: Array<ReleaseNoteChild>;
  "commits"?: Array<ReleaseNoteCommit>;
  "source": string;
  "status"?: string | null;
  "summary"?: string | null;
  "task_id": string;
  "title"?: string | null;
};

export type ReleaseNotes = {
  "adrs"?: Array<ReleaseNoteFile>;
  "base"?: string | null;
  "config_example"?: ReleaseNoteConfig | null;
  "deliveries_known"?: boolean;
  "direct_commits"?: Array<ReleaseNoteCommit>;
  "first_parent"?: Array<string>;
  "gate_skips"?: Array<ReleaseNoteGateSkip>;
  "generated_at": string;
  "migrations"?: Array<ReleaseNoteFile>;
  "schema": ReleaseNoteSchema;
  "sha": string;
  "sha12": string;
  "tasks"?: Array<ReleaseNoteTask>;
  "truncated"?: boolean;
  "version": number;
};

export type ReleasePromoteAccepted = {
  "log": string;
  "script_from": string;
  "sha12": string;
  "started_at": string;
};

export type ReleasePromoteFailure = {
  "error": string;
  "failed_at": string;
};

export type ReleasePromotionPreview = {
  "adrs": Array<ReleaseNoteFile>;
  "complete": boolean;
  "config_examples": Array<ReleaseNoteConfig>;
  "direct_commits": Array<ReleaseNoteCommit>;
  "from"?: string | null;
  "gate_skips": Array<ReleaseNoteGateSkip>;
  "migrations": Array<ReleaseNoteFile>;
  "mode"?: string | null;
  "problem"?: string | null;
  "releases": Array<ReleasePromotionRelease>;
  "schema": ReleaseNoteSchema;
  "tasks": Array<ReleaseNoteTask>;
  "to": string;
};

export type ReleasePromotionRelease = {
  "built_at"?: string | null;
  "sha12": string;
  "task_count": number;
};

export type ReleaseRunning = {
  "instance_id": string;
  "release": string;
  "role": string;
};

export type ReleaseVerify = {
  "at"?: string | null;
  "checks"?: Array<ReleaseVerifyCheck>;
  "live_ok": boolean;
  "ok": boolean;
};

export type ReleaseVerifyCheck = {
  "detail": string;
  "elapsed_s"?: number | null;
  "id": string;
  "name": string;
  "ok": boolean;
};

export type Releases = {
  "current"?: string | null;
  "instances": Array<DaemonInstance>;
  "items": Array<ReleaseItem>;
  "previous"?: string | null;
  "running": ReleaseRunning;
};

export type ReloadResult = {
  "reloaded": boolean;
};

export type ReopenBody = {
  "expected_status"?: Status | null;
};

export type RepairOrigin = "review" | "integration" | "delivery" | "planner";

export type ReplanDiff = {
  "added": Array<string>;
  "changed": Array<string>;
  "moved"?: Array<string>;
  "overridden_done"?: Array<string>;
  "removed": Array<string>;
  "reopened_stages"?: Array<string>;
};

export type ReplayMismatch = {
  "field": string;
  "replayed": string;
  "stored": string;
  "task_id": TaskId;
};

export type ReplayReport = {
  "mismatches": Array<ReplayMismatch>;
  "tasks": number;
};

export type RepoChangesView = {
  "ahead": number;
  "base": string;
  "branch": string;
  "default_branch": string;
  "dirty": boolean;
  "files": Array<ChangedFile>;
  "head": string;
  "integration"?: TaskIntegration | null;
  "missing": boolean;
  "origin": boolean;
  "repo": string;
  "stat": DiffStat;
};

export type RepoCreateBody = {
  "default_branch"?: string | null;
  "is_primary"?: boolean;
  "kind"?: RepoKind | null;
  "location": WorkspaceSpec;
  "name"?: string | null;
  "run"?: RepoRun | null;
  "sync"?: RepoSync | null;
};

export type RepoId = string;

export type RepoKind = "git" | "dir";

export type RepoList = {
  "items": Array<ProjectRepo>;
};

export type RepoPatchBody = {
  "default_branch"?: string | null;
  "is_primary"?: boolean | null;
  "kind"?: RepoKind | null;
  "location"?: WorkspaceSpec | null;
  "name"?: string | null;
  "run"?: RepoRun | null;
  "sync"?: RepoSync | null;
};

export type RepoRef = {
  "name": string;
  "repo_id": RepoId;
};

export type RepoRun = "auto" | "host" | "container";

export type RepoSelector = string | Array<string>;

export type RepoState = {
  "base": string;
  "branch": string;
  "diff_stat": string;
  "head": string;
  "uncommitted": boolean;
};

export type RepoSync = "worktree" | "rsync" | "none";

export type Report = {
  "body"?: string;
  "created_at": string;
  "headline": string;
  "id": ReportId;
  "kind": ReportKind;
  "level": number;
  "node_id": string;
  "project_id"?: ProjectId | null;
  "read_at"?: string | null;
  "sources"?: Array<ReportId>;
  "task_id"?: TaskId | null;
};

export type ReportDetail = {
  "report": Report;
  "sources_expanded": Array<Report>;
};

export type ReportId = string;

export type ReportKind = "progress" | "result" | "bad_news" | "proposal" | "question";

export type ReportList = {
  "items": Array<Report>;
};

export type ReportsLive = {
  "last_notified_at"?: string | null;
  "notify_now": boolean;
  "unread_bad_news": number;
  "unread_secretary": number;
};

export type ReportsNotifiedResult = {
  "last_notified_at": string;
};

export type ReportsReadBody = {
  "ids": Array<string>;
};

export type ReportsReadResult = {
  "updated": number;
};

export type RequestLogLink = {
  "account"?: string | null;
  "model"?: string | null;
  "snapshot_id"?: string | null;
  "source_id"?: string | null;
};

export type RequestRoutingAudit = {
  "actual"?: ActualSource | null;
  "attempts"?: Array<RequestSourceAttempt>;
  "decision_id": string;
  "fallback_reason"?: string | null;
  "incomplete_reason"?: string | null;
  "log"?: RequestLogLink | null;
  "parent_decision_id"?: string | null;
  "request_id"?: string | null;
  "trace": RoutingTraceV1;
};

export type RequestSourceAttempt = {
  "account_id"?: string | null;
  "fallback_reason"?: string | null;
  "model"?: string | null;
  "source_id": string;
};

export type ResolutionAction = {
  "detail": string;
  "kind": ConflictKind;
  "path": string;
};

export type ResolvedLlmSource = {
  "origin": SourceOrigin;
  "source": LlmSourceRef;
};

export type RetryBody = {
  "accept"?: boolean;
  "execution"?: ExecutionMode | null;
  "workspace"?: WorkspaceSpec | null;
};

export type RetryResult = {
  "rewired"?: Array<TaskId>;
  "task_id": TaskId;
};

export type ReviewNote = {
  "criterion": number;
  "pass": boolean;
  "reason": string;
};

export type ReviewPrefs = {
  "escalate_on_fail"?: boolean | null;
  "harness"?: string | null;
  "tier"?: Tier | null;
};

export type ReviewResult = {
  "failed_criteria"?: Array<number>;
  "passed": boolean;
};

export type ReviewerConfigView = {
  "adapter"?: string | null;
  "tier"?: Tier | null;
};

export type RoleConfigView = {
  "adapter"?: string | null;
  "has_instructions": boolean;
  "id": string;
  "max_turns"?: number | null;
  "max_wall_secs"?: number | null;
  "tier"?: Tier | null;
};

export type RoleSlotView = {
  "available"?: boolean | null;
  "excluded_reason"?: string | null;
  "last_seen"?: string | null;
  "model_id"?: string | null;
  "origin"?: SlotOrigin | null;
  "providers": Array<string>;
  "proxy": boolean;
  "source": string;
  "tier": Tier;
};

export type RollupMetrics = {
  "busy_ms": number;
  "child_tasks_done": number;
  "child_tasks_total": number;
  "cost_usd": number;
  "cost_usd_complete": boolean;
  "first_run_started_at"?: string | null;
  "input_tokens": number;
  "last_run_finished_at"?: string | null;
  "leaves_done": number;
  "leaves_total": number;
  "open_decisions": number;
  "output_tokens": number;
  "quota"?: Array<QuotaUse>;
  "reviewer_cost_usd": number;
  "reviewer_runs": number;
  "runs": number;
  "runs_by_role"?: {
  [key: string]: number;
};
  "runs_in_flight": number;
  "tasks": number;
  "tokens": number;
  "wall_ms"?: number | null;
};

export type Route = "direct" | "planned";

export type RouteDecision = {
  "gate_rule_id": string;
  "overrode_gate": boolean;
  "policy_version": string;
  "reasons": Array<RouteReason>;
  "route": Route;
  "shadow": boolean;
};

export type RouteReason = {
  "detail": string;
  "ok": boolean;
  "rule_id": string;
};

export type RoutingCatalogView = {
  "catalog_version": string;
  "deployments": Array<CatalogDeploymentView>;
  "mode": RoutingMode;
  "models": Array<CatalogModelView>;
  "policies": Array<RoutingPolicy>;
  "warnings": Array<string>;
};

export type RoutingFeaturesRecord = {
  "context_version": string;
  "decision_id": string;
  "features"?: unknown;
  "missing_fields"?: Array<string>;
  "provenance"?: {
  [key: string]: string;
};
  "request_id"?: string | null;
  "run_id"?: string | null;
  "stage"?: FeatureStage | null;
};

export type RoutingMode = "legacy" | "shadow" | "enforce";

export type RoutingOutcome = {
  "acceptance_passed"?: boolean | null;
  "cash_usd"?: number | null;
  "decision_id": string;
  "evaluation_version": string;
  "failed_criterion_ids"?: Array<string>;
  "failure_class"?: string | null;
  "outcome_id": string;
  "request_id"?: string | null;
  "retries"?: number | null;
  "review_passed"?: boolean | null;
  "reward"?: number | null;
  "run_id"?: string | null;
  "supersedes"?: string | null;
  "tokens"?: number | null;
  "wall_ms"?: number | null;
};

export type RoutingOutcomeState = "not_recorded" | "unreviewed" | "judged";

export type RoutingPolicy = {
  "constraints": Constraints;
  "escalation": boolean;
  "fallback": boolean;
  "lane": Tier;
  "local_preference": boolean;
  "min_quality": number;
  "mode": RoutingMode;
  "normalization": Normalization;
  "objective": Objective;
  "prefer_free": boolean;
  "version": string;
  "weights": Weights;
};

export type RoutingRecord = {
  "decision": LaneDecision;
  "escalation"?: EscalationAudit | null;
  "harness"?: string | null;
  "optimizer"?: RoutingTraceV1 | null;
  "org_node"?: string | null;
  "quota_reason"?: string | null;
  "resolution": LaneResolution;
  "work_unit_id"?: string | null;
};

export type RoutingShadowAudit = {
  "candidate_model"?: string | null;
  "candidate_source"?: string | null;
  "differs_from_primary"?: boolean | null;
  "estimator"?: EstimatorShadowAudit | null;
  "input_tokens"?: number | null;
  "kind": ShadowKind;
  "output_tokens"?: number | null;
  "primary_decision_id": string;
  "reason"?: ShadowReason | null;
  "reservation"?: ShadowReservationAudit | null;
  "shadow_id": string;
  "status": ShadowStatus;
};

export type RoutingTraceV1 = {
  "account_id"?: string | null;
  "candidates": Array<CandidateTrace>;
  "catalog_version": string;
  "decision_id": string;
  "estimator_version": string;
  "fallback_order": Array<string>;
  "feature_version": string;
  "mode": RoutingMode;
  "model"?: string | null;
  "observed_at"?: string | null;
  "parent_decision_id"?: string | null;
  "policy_version": string;
  "reasons": Array<string>;
  "request_id"?: string | null;
  "requested_lane": Tier;
  "run_id"?: string | null;
  "selected"?: string | null;
  "selected_lane"?: Tier | null;
  "snapshot_id": string;
  "source_id"?: string | null;
  "stage": string;
  "task_id"?: string | null;
  "work_unit_id"?: string | null;
};

export type RunEnd = {
  "type": "completed";
} | {
  "type": "yielded";
} | {
  "kind": BudgetKind;
  "type": "budget_exhausted";
} | {
  "type": "question";
} | {
  "retryable": boolean;
  "type": "failed";
} | {
  "class": HarnessErrorClass;
  "type": "harness_error";
} | {
  "type": "cancelled";
} | {
  "type": "waiting";
};

export type RunFiles = {
  "prompt"?: boolean;
  "request"?: boolean;
  "result": boolean;
  "stderr": boolean;
  "stdout": boolean;
};

export type RunList = {
  "runs": Array<RunSummary>;
};

export type RunMetrics = {
  "peak_context_tokens"?: number | null;
  "retries": number;
  "turns"?: number | null;
  "wall_ms": number;
};

export type RunOutcomeKind = "done" | "question" | "error" | "requeue" | "lease_expired" | "interrupted" | "continued";

export type RunRole = "worker" | "reviewer" | "planner";

export type RunRoutingAudit = {
  "account"?: string | null;
  "actual_sources"?: Array<ActualSource>;
  "adapter"?: string | null;
  "audit_incomplete"?: boolean | null;
  "cost_usd"?: number | null;
  "decision_id"?: string | null;
  "escalation"?: string | null;
  "escalation_audit"?: EscalationAudit | null;
  "features"?: TaskFeatures | null;
  "harness"?: string | null;
  "incomplete_reasons"?: Array<string>;
  "input_tokens"?: number | null;
  "lane"?: Tier | null;
  "model"?: string | null;
  "optimizer"?: RoutingTraceV1 | null;
  "org_node"?: string | null;
  "outcome"?: string | null;
  "outcome_state"?: RoutingOutcomeState | null;
  "output_tokens"?: number | null;
  "policy_version"?: string | null;
  "provider"?: string | null;
  "reasoning_effort"?: string | null;
  "reasons"?: Array<string>;
  "requests"?: Array<RequestRoutingAudit> | null;
  "retries"?: number | null;
  "review"?: ReviewResult | null;
  "routing_features"?: RoutingFeaturesRecord | null;
  "routing_outcome"?: RoutingOutcome | null;
  "routing_shadow"?: Array<RoutingShadowAudit> | null;
  "rule_id"?: string | null;
  "run_id": string;
  "task_id": TaskId;
  "wall_ms"?: number | null;
};

export type RunSummary = {
  "account"?: string | null;
  "adapter": string;
  "artifacts": number;
  "end"?: RunEnd | null;
  "files"?: RunFiles | null;
  "finished_at"?: string | null;
  "model": string;
  "outcome"?: RunOutcomeKind | null;
  "outcome_text"?: string | null;
  "progress": number;
  "provider"?: string | null;
  "reviewer_deferrals": number;
  "role": RunRole;
  "run_id": string;
  "started_at": string;
  "usage"?: Usage | null;
  "verdicts": number;
  "work_unit"?: string | null;
};

export type ScoreTrace = {
  "c": number;
  "l": number;
  "p": number;
  "q": number;
  "score": number;
  "unknown"?: Array<string>;
  "wc": number;
  "wl": number;
  "wp": number;
  "wq": number;
};

export type ScratchCacheStats = {
  "flush_dropped": number;
  "flush_last_at"?: string | null;
  "flush_mbps": number;
  "flush_oldest_age_secs"?: number | null;
  "flush_queue_bytes": number;
  "flush_queue_len": number;
  "flush_skipped_existing": number;
  "flush_written": number;
  "flush_written_bytes": number;
  "gets": number;
  "l1_bytes": number;
  "l1_dir": string;
  "l1_entries": number;
  "l1_evicted": number;
  "l1_hits": number;
  "l1_max_bytes": number;
  "l2_bytes"?: number | null;
  "l2_corrupt": number;
  "l2_degraded_since"?: string | null;
  "l2_dir"?: string | null;
  "l2_enabled": boolean;
  "l2_entries"?: number | null;
  "l2_errors": number;
  "l2_gc_last_at"?: string | null;
  "l2_gc_removed": number;
  "l2_gc_removed_bytes": number;
  "l2_hits": number;
  "l2_last_error"?: string | null;
  "l2_max_bytes": number;
  "l2_retry_at"?: string | null;
  "l2_scanned_at"?: string | null;
  "l2_state": string;
  "l2_timeouts": number;
  "misses": number;
  "observed_at": string;
  "promotes": number;
  "puts": number;
  "schema": string;
  "started_at": string;
};

export type ScratchCacheView = {
  "endpoint": string;
  "reason"?: string | null;
  "sccache_mode"?: string | null;
  "state": string;
  "stats"?: ScratchCacheStats | null;
};

export type ScratchGcRemovedView = {
  "class": string;
  "estimated_bytes": number;
  "id": string;
  "why": string;
};

export type ScratchGcView = {
  "at": string;
  "emergency": boolean;
  "pressure": string;
  "reclaimed_bytes": number;
  "removed": Array<ScratchGcRemovedView>;
};

export type ScratchLegacyView = {
  "class": string;
  "last_write"?: string | null;
  "path": string;
  "size_bytes"?: number | null;
};

export type ScratchOwnerView = {
  "adopted_from"?: string | null;
  "base_commit"?: string | null;
  "class": string;
  "estimated_bytes": number;
  "has_target": boolean;
  "kind": string;
  "lease_mtime"?: string | null;
  "measured_at"?: string | null;
  "owner": string;
  "reason": string;
  "repo_key"?: string | null;
  "size_bytes"?: number | null;
  "work_unit_key"?: string | null;
};

export type ScratchSccacheStats = {
  "cache_size_bytes"?: number | null;
  "compile_requests": number;
  "hits": number;
  "misses": number;
  "rust_hits": number;
  "rust_misses": number;
};

export type ScratchSccacheView = {
  "binary": string;
  "dir": string;
  "max_bytes": number;
  "port": number;
  "reason"?: string | null;
  "state": string;
  "stats"?: ScratchSccacheStats | null;
};

export type ScratchStatus = {
  "cache"?: ScratchCacheView | null;
  "dir": string;
  "disabled_reason"?: string | null;
  "effective_max_bytes": number;
  "enabled": boolean;
  "fs_free_bytes"?: number | null;
  "fs_total_bytes"?: number | null;
  "high_watermark": number;
  "last_gc"?: ScratchGcView | null;
  "legacy": Array<ScratchLegacyView>;
  "low_watermark": number;
  "observed_at": string;
  "owners": Array<ScratchOwnerView>;
  "pinned_bytes": number;
  "pressure": string;
  "sccache"?: ScratchSccacheView | null;
  "schema": string;
  "targets_bytes": number;
  "targets_max_bytes": number;
  "total_max_bytes": number;
};

export type SecretList = {
  "dir"?: string | null;
  "items": Array<SecretView>;
};

export type SecretPutResult = {
  "fingerprint": string;
  "id": string;
  "updated_at": string;
};

export type SecretUse = {
  "env": string;
  "name": string;
  "scope": string;
};

export type SecretView = {
  "fingerprint"?: string | null;
  "id": string;
  "updated_at"?: string | null;
  "used_by": Array<SecretUse>;
};

export type ShadowDecision = {
  "classifier": string;
  "confidence": number;
  "lane": Tier;
};

export type ShadowKind = "decision" | "execution" | "estimator";

export type ShadowReason = "not_allowlisted" | "sampled_out" | "queue_full" | "concurrency_limit" | "timeout" | "privacy" | "upstream_error" | "off" | "cap_exceeded" | "unknown_cost" | "resource_group_shared" | "primary_pressure";

export type ShadowReservationAudit = {
  "charged_effective_usd": number;
  "charged_tokens": number;
  "reserved_effective_usd": number;
  "reserved_tokens": number;
  "state": string;
  "utc_day": string;
};

export type ShadowStatus = "completed" | "failed" | "dropped";

export type SideIntent = {
  "branch": string;
  "commits": Array<CommitIntent>;
  "diffstat"?: FileDiffStat | null;
  "path": string;
  "unavailable"?: string | null;
};

export type SkillDetailView = {
  "files"?: Array<string>;
  "mounted_by"?: Array<string>;
  "name": string;
  "skill_md": string;
  "updated"?: string | null;
};

export type SkillFileBody = {
  "content": string;
  "path": string;
};

export type SkillList = {
  "initialized": boolean;
  "items": Array<SkillSummaryView>;
  "root": string;
};

export type SkillPutBody = {
  "files"?: Array<SkillFileBody>;
  "skill_md": string;
};

export type SkillPutResult = {
  "path": string;
};

export type SkillSummaryView = {
  "description": string;
  "mounted_by"?: Array<string>;
  "name": string;
  "updated"?: string | null;
};

export type SlotOrigin = "assignment" | "config";

export type SourceOrigin = "explicit" | "derived";

export type StageHint = {
  "scope"?: string;
  "title": string;
};

export type StageReview = "none" | "human";

export type StageSpec = {
  "key": string;
  "kind": WorkUnitKind;
  "review"?: StageReview;
  "title": string;
};

export type StandingRule = {
  "created_at": string;
  "id": StandingRuleId;
  "node_id"?: string | null;
  "rule": string;
};

export type StandingRuleCreateBody = {
  "node_id"?: string | null;
  "rule": string;
};

export type StandingRuleId = string;

export type StandingRuleList = {
  "items": Array<StandingRule>;
};

export type Status = "draft" | "ready" | "running" | "blocked" | "reviewing" | "done" | "failed" | "cancelled";

export type StreamHeartbeat = {
  "now": string;
};

export type StreamHello = {
  "cursor": number;
  "daemon"?: DaemonSnapshot | null;
  "now": string;
};

export type StreamReset = {
  "cursor": number;
  "reason": string;
};

export type Task = {
  "acceptance": Array<Criterion>;
  "aggregate"?: boolean;
  "assignee"?: string | null;
  "attempts": number;
  "budget": Budget;
  "category"?: TaskCategory;
  "conversation"?: MessageId | null;
  "created_at": string;
  "depends_on": Array<TaskId>;
  "genre"?: string | null;
  "id": TaskId;
  "inputs": Array<ArtifactRef>;
  "kind": TaskKind;
  "labels"?: Array<string>;
  "lease"?: Lease | null;
  "milestone_id"?: MilestoneId | null;
  "mode"?: TaskMode;
  "objective": string;
  "parent_id"?: TaskId | null;
  "paused_at"?: string | null;
  "priority": number;
  "project_id"?: ProjectId | null;
  "repos"?: Array<RepoRef>;
  "requirements"?: TaskRequirements;
  "role"?: string | null;
  "routing"?: TaskRouting | null;
  "skills"?: Array<string>;
  "status": Status;
  "title": string;
  "tree"?: TreeInfo | null;
  "updated_at": string;
  "worker_hint": WorkerHint;
  "workspace": WorkspaceSpec;
};

export type TaskCategory = "feature" | "bug" | "research" | "ops" | "docs" | "other";

export type TaskComment = {
  "author"?: string | null;
  "author_kind": CommentAuthorKind;
  "body": string;
  "created_at": string;
  "id": CommentId;
  "run_id"?: string | null;
  "task_id": TaskId;
};

export type TaskDetail = {
  "actions": Array<Action>;
  "actual_run_write_sets": Array<ActualWriteSetView>;
  "actual_work_unit_write_sets": Array<ActualWriteSetView>;
  "answers": Array<AnswerNote>;
  "approvals": Array<ApprovalLink>;
  "behind_target": BehindTarget;
  "children": Array<TaskRef>;
  "cluster"?: string | null;
  "cluster_job_wait"?: ClusterJobWaitView | null;
  "criteria": Array<CriterionView>;
  "delegated": Array<DelegatedView>;
  "dependencies": Array<TaskRef>;
  "dependents": Array<TaskRef>;
  "execution"?: ExecutionView | null;
  "expected_write_paths"?: Array<string> | null;
  "failure"?: FailureSummary | null;
  "genre"?: string | null;
  "integration_repair"?: IntegrationRepairView | null;
  "is_root_task"?: boolean;
  "latest_question"?: string | null;
  "paused_by"?: TaskId | null;
  "prior_review": Array<ReviewNote>;
  "priority_label": string;
  "role"?: string | null;
  "runs": Array<RunSummary>;
  "task": Task;
  "timers": Timers;
  "worker_run_hint"?: string | null;
  "workspace_dir"?: string | null;
  "worktree"?: WorktreeView | null;
};

export type TaskExecutionView = {
  "awaiting_children"?: Array<AwaitedChildView>;
  "gate"?: ExecutionGateDecision | null;
  "metrics": ExecutionMetrics;
  "phase"?: ExecutionPhase | null;
  "phase_checkpoint"?: PhaseCheckpointView | null;
  "plan"?: ExecutionPlanView | null;
  "plan_approval"?: PlanApprovalView | null;
  "route"?: RouteDecision | null;
  "runs": Array<RunSummary>;
};

export type TaskFeatureHints = {
  "ambiguity"?: Level | null;
  "consequence"?: Level | null;
  "context_size"?: Level | null;
  "cross_cutting"?: Level | null;
  "expected_length"?: Level | null;
  "judgment"?: Level | null;
  "reversibility"?: Level | null;
  "tool_intensity"?: Level | null;
  "verifiability"?: Level | null;
};

export type TaskFeatures = {
  "ambiguity": Level;
  "consequence": Level;
  "context_size": Level;
  "cross_cutting": Level;
  "expected_length": Level;
  "judgment": Level;
  "reversibility": Level;
  "tool_intensity": Level;
  "verifiability": Level;
};

export type TaskId = string;

export type TaskIntegration = {
  "created_at": string;
  "detail"?: string | null;
  "id": IntegrationId;
  "merged_at"?: string | null;
  "method": IntegrationMethod;
  "pr_number"?: number | null;
  "pr_url"?: string | null;
  "repo": string;
  "repo_id"?: RepoId | null;
  "state": IntegrationState;
  "task_id": TaskId;
  "updated_at": string;
};

export type TaskKind = "plan" | "execute" | "review" | "approval";

export type TaskList = {
  "counts_by_status": {
  [key: string]: number;
};
  "items": Array<TaskSummary>;
  "next_cursor"?: string | null;
  "total": number;
};

export type TaskMode = "prototype" | "production" | "research";

export type TaskPatchBody = {
  "acceptance"?: Array<CriterionSpec> | null;
  "adapter"?: string | null;
  "assignee"?: string | null;
  "category"?: TaskCategory | null;
  "depends_on"?: Array<TaskId> | null;
  "expected_status"?: Status | null;
  "expected_write_paths"?: Array<string> | null;
  "harness"?: string | null;
  "labels"?: Array<string> | null;
  "max_retries"?: number | null;
  "max_turns"?: number | null;
  "max_wall_secs"?: number | null;
  "milestone_id"?: MilestoneId | null;
  "mode"?: TaskMode | null;
  "objective"?: string | null;
  "pause_after"?: PausePolicy | null;
  "priority"?: PriorityInput | null;
  "project_id"?: ProjectId | null;
  "repos"?: Array<string> | null;
  "role"?: string | null;
  "skills"?: Array<string> | null;
  "tier"?: Tier | null;
  "title"?: string | null;
  "workspace"?: WorkspaceSpec | null;
};

export type TaskPauseResult = {
  "paused_at"?: string | null;
  "subtree"?: Array<TaskRef>;
  "task": TaskRef;
};

export type TaskRef = {
  "actions": Array<Action>;
  "id": TaskId;
  "kind": TaskKind;
  "status": Status;
  "title": string;
};

export type TaskRequirements = {
  "browser"?: BrowserRequirements | null;
};

export type TaskRouting = {
  "assignee_explicit"?: boolean;
  "dropped_assignee"?: string | null;
  "execution"?: ExecutionGateDecision | null;
  "execution_hint"?: ExecutionHintSpec | null;
  "features"?: TaskFeatureHints | null;
  "pause_after"?: PausePolicy;
  "pause_after_source"?: PauseSource;
  "route"?: RouteDecision | null;
  "stages_hint"?: Array<StageHint>;
  "tier_source"?: TierSource;
};

export type TaskRoutingView = {
  "assignee"?: string | null;
  "estimator_shadow"?: EstimatorShadowSummary | null;
  "routing"?: TaskRouting | null;
  "runs": Array<RunRoutingAudit>;
  "task_id": TaskId;
  "unbound_requests"?: Array<RequestRoutingAudit>;
};

export type TaskStatusCounts = {
  "counts_by_status": {
  [key: string]: number;
};
};

export type TaskSummary = {
  "actions": Array<Action>;
  "adapter"?: string | null;
  "assignee"?: string | null;
  "attempts": number;
  "backoff_until"?: string | null;
  "category": TaskCategory;
  "children": number;
  "conversation": boolean;
  "created_at": string;
  "depends_on": Array<TaskId>;
  "genre"?: string | null;
  "id": TaskId;
  "is_root_task"?: boolean;
  "kind": TaskKind;
  "labels": Array<string>;
  "lease_expires_at"?: string | null;
  "max_retries": number;
  "milestone_id"?: MilestoneId | null;
  "parent_id"?: TaskId | null;
  "paused"?: boolean;
  "pending_children": number;
  "priority": number;
  "priority_label": string;
  "project_id"?: ProjectId | null;
  "role"?: string | null;
  "status": Status;
  "support"?: string | null;
  "tier": Tier;
  "title": string;
  "updated_at": string;
};

export type TaskTreeNode = {
  "children"?: Array<TaskId>;
  "depth": number;
  "id": TaskId;
  "open_decisions": number;
  "own": RollupMetrics;
  "parent_id"?: TaskId | null;
  "parent_stage"?: string | null;
  "parent_unit_key"?: string | null;
  "phase"?: TreeNodePhase | null;
  "plan_version"?: number | null;
  "stall"?: TreeNodeStall | null;
  "status": Status;
  "subtree": RollupMetrics;
  "title": string;
  "units"?: Array<TreeUnitView>;
};

export type TaskTreeView = {
  "limits"?: TreeLimitsUsage | null;
  "nodes": Array<TaskTreeNode>;
  "root_id": TaskId;
  "subtree_root": TaskId;
  "totals": RollupMetrics;
  "tree_enabled": boolean;
};

export type Tier = "frontier" | "standard" | "cheap";

export type TierSource = "human" | "system" | "hint" | "default";

export type Timeline = {
  "items": Array<TimelineItem>;
  "task_id": TaskId;
};

export type TimelineItem = {
  "at": string;
  "event": Event;
  "kind": "event";
  "seq": number;
} | {
  "at": string;
  "comment": TaskComment;
  "kind": "comment";
} | {
  "approval": Approval;
  "at": string;
  "kind": "approval";
} | {
  "at": string;
  "kind": "report";
  "report": Report;
} | {
  "at": string;
  "kind": "delegation";
  "run_id": string;
  "tasks": Array<TaskRef>;
} | {
  "at": string;
  "commits": Array<string>;
  "kind": "release";
  "sha12": string;
} | {
  "action": string;
  "at": string;
  "detail": string;
  "kind": "integration";
} | {
  "at": string;
  "kind": "doc";
  "path": string;
  "project_id": ProjectId;
  "title": string;
} | {
  "at": string;
  "discarded"?: number | null;
  "inbox"?: number | null;
  "ingested"?: number | null;
  "kind": "knowledge";
  "run_task_id": TaskId;
  "state": string;
  "via"?: string | null;
};

export type Timers = {
  "backoff_until"?: string | null;
  "consecutive_requeues": number;
  "consecutive_reviewer_requeues": number;
  "lease_expires_at"?: string | null;
  "max_requeues": number;
  "now": string;
};

export type TokenPricing = {
  "as_of"?: string | null;
  "cached_input_usd_per_million"?: number | null;
  "input_usd_per_million"?: number | null;
  "output_usd_per_million"?: number | null;
  "provenance": string;
};

export type TransitionResult = {
  "cascaded"?: Array<TaskRef>;
  "from": Status;
  "id": TaskId;
  "reason": string;
  "to": Status;
};

export type TreeEntry = {
  "kind": string;
  "name": string;
  "path": string;
  "size"?: number | null;
};

export type TreeFileView = {
  "binary": boolean;
  "path": string;
  "repo": string;
  "size": number;
  "text"?: string | null;
  "too_large": boolean;
};

export type TreeInfo = {
  "base_commit"?: string | null;
  "depth": number;
  "parent_unit"?: ParentUnit | null;
  "root_id": TaskId;
};

export type TreeLimitsUsage = {
  "leaves": number;
  "max_leaves": number;
  "max_open_decisions": number;
  "max_replans": number;
  "max_runs": number;
  "max_tokens"?: number | null;
  "open_decisions": number;
  "replans": number;
  "runs": number;
  "tokens": number;
};

export type TreeNodePhase = "planning" | "executing" | "repairing" | "verifying" | "awaiting_human" | "awaiting_children" | "awaiting_plan_approval" | "held_on_decision" | "blocked_infra";

export type TreeNodeStall = {
  "detail": string;
  "reason": string;
  "since"?: string | null;
};

export type TreeRepoView = {
  "base"?: string | null;
  "branch"?: string | null;
  "dir": string;
  "kind": string;
  "name": string;
};

export type TreeUnitView = {
  "blocked_reason"?: WorkUnitBlockedReason | null;
  "child_task_id"?: TaskId | null;
  "key": string;
  "kind": WorkUnitKind;
  "stage"?: string | null;
  "status": WorkUnitStatus;
  "title": string;
};

export type TreeView = {
  "entries": Array<TreeEntry>;
  "path": string;
  "repo": string;
  "repos": Array<TreeRepoView>;
};

export type TrustedLogin = {
  "login_url": string;
  "password_selector": string;
  "policy_id": string;
  "revision": number;
  "submit_selector"?: string | null;
};

export type TunnelForwardLive = {
  "last_error"?: string | null;
  "listen": string;
  "listener"?: boolean;
  "target": string;
  "target_healthy"?: boolean;
  "up": boolean;
};

export type UnitContext = {
  "from_work_units"?: Array<string>;
  "knowledge"?: Array<string>;
  "paths"?: Array<string>;
  "repo"?: RepoSelector | null;
};

export type UnitDeclared = "leaf" | "task";

export type UnitGateAction = "promoted" | "auto_leaf" | "decision" | "kept_task" | "demoted";

export type UnreadCountView = {
  "by_kind": {
  [key: string]: number;
};
  "events": number;
  "unread": number;
};

export type Usage = {
  "cache_creation_tokens"?: number | null;
  "cache_read_tokens"?: number | null;
  "cost_usd"?: number | null;
  "duplicate_reads"?: number | null;
  "input_tokens"?: number | null;
  "output_tokens"?: number | null;
  "session_resumed"?: boolean | null;
};

export type VerdictView = {
  "criterion_idx": number;
  "pass": boolean;
  "reason": string;
  "run_id": string;
  "ts": string;
};

export type Weights = {
  "cost": number;
  "latency": number;
  "pressure": number;
  "quality": number;
};

export type WorkUnitBlockedReason = "question" | "dependency_failed" | "limit" | "plan_issue" | "decision" | "infra" | "cluster_jobs";

export type WorkUnitBudget = {
  "max_turns"?: number | null;
  "max_wall_secs"?: number | null;
};

export type WorkUnitCheck = {
  "cmd": string;
  "expect_exit"?: number;
  "scope"?: boolean;
};

export type WorkUnitCheckLog = {
  "cmd": string;
  "duration_ms"?: number | null;
  "exit"?: number | null;
  "index": number;
  "key": string;
  "pass"?: boolean | null;
  "running": boolean;
  "size": number;
  "started_at": string;
  "tail": string;
  "total": number;
  "truncated": boolean;
  "work_unit_id": string;
};

export type WorkUnitContext = {
  "from_work_units"?: Array<string>;
  "knowledge"?: Array<string>;
  "paths"?: Array<string>;
};

export type WorkUnitKind = "investigate" | "design" | "implement" | "test" | "release" | "repair" | "other" | "integrate" | "task";

export type WorkUnitSpec = {
  "budget"?: WorkUnitBudget | null;
  "checks"?: Array<WorkUnitCheck>;
  "context"?: WorkUnitContext;
  "depends_on"?: Array<string>;
  "done_when"?: Array<string>;
  "features"?: unknown;
  "harness"?: string | null;
  "key": string;
  "kind": WorkUnitKind;
  "objective": string;
  "outputs"?: Array<string>;
  "phase"?: string | null;
  "title": string;
};

export type WorkUnitStatus = "pending" | "ready" | "needs_continuation" | "running" | "done" | "failed" | "blocked" | "superseded" | "cancelled";

export type WorkUnitView = {
  "base_commit"?: string | null;
  "blocked_reason"?: WorkUnitBlockedReason | null;
  "branch"?: string | null;
  "child_task_id"?: string | null;
  "continuations": number;
  "created_at": string;
  "depends_on": Array<string>;
  "head_commit"?: string | null;
  "id": string;
  "integrated_commit"?: string | null;
  "key": string;
  "kind": WorkUnitKind;
  "last_checkpoint_run_id"?: string | null;
  "last_run_id"?: string | null;
  "phase"?: string | null;
  "quota"?: Array<QuotaUse>;
  "retries": number;
  "running_run_id"?: string | null;
  "runs": number;
  "seq": number;
  "spec": WorkUnitSpec;
  "status": WorkUnitStatus;
  "updated_at": string;
};

export type WorkerHint = {
  "adapter"?: string | null;
  "tier": Tier;
};

export type WorkspaceMode = "worktree" | "shared";

export type WorkspaceSpec = {
  "kind": "local";
  "mode"?: WorkspaceMode | null;
  "path": string;
} | {
  "cluster": string;
  "kind": "remote";
  "mode"?: WorkspaceMode | null;
  "path"?: string;
};

export type WorktreeView = {
  "branch": string;
  "dir": string;
  "project": string;
};
