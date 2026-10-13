# opencode model namespace

Task: `01M4TCTMB0YX58E5WE06EC1W4Q` / WorkUnit `opencode-ns`.

`[harnesses.opencode].model_namespace` is optional and accepted only for the opencode ACP harness. Empty values and values containing `/` fail config validation. `route_adapter` prefixes only that harness's route model with the declared namespace; it never derives the namespace from the route source. Without a namespace, ACP receives the route model unchanged.

The migration runbook and its progress copy declare `model_namespace = "qwen-local"`. Focused tests cover deserialization/validation, adapter propagation, opencode-only qualification, unchanged non-opencode IDs, and the ACP `session/set_config_option` payload.
