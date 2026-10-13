# Wire model name implementation

Implemented daemon-side proxy wire model qualification. `RoutePick` now preserves the canonical candidate model while carrying `wire_model_id`; only a matching `via = "llm_proxy"` route qualifies it as `<source>/<model>`, and models already containing `/` are preserved. Adapter launch and `SelectedLlmRoute.model_id` use the wire value; proxy endpoints are omitted. Config validation requires every default candidate for `local_deep_research`, `paperqa`, and `langmem` to have a matching `llm_proxy` route.

The daemon tests cover proxy qualification, direct model preservation, adapter launch, and the launch model name for LDR, PaperQA, and LangMem with qwen and claude sources. Core tests cover explicit proxy route qualification and slash-containing model IDs. Celeris config validation test covers rejecting a non-proxy route for a proxy-bound harness.

Validation passed: all six `proxy_model_name` dispatch tests, the six requested daemon/config tests, `migration_runbook_toml_loads`, and `cargo clippy -p task-core -p task-dispatch -p celeris -- -D warnings`. The runbook TOML and its progress copy matched and parsed after the new validation.
