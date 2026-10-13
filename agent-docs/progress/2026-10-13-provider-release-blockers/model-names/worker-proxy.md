# Worker proxy model names

PaperQA now keeps the route's source qualified model for its query LLM and applies the route model to `settings.llm`, `settings.summary_llm`, and `settings.agent.agent_llm` in `paperqa_ask.py`. LangMem now implements `with_model`, replacing its configured model with the route model sent in `langmem_input.json`. LDR's existing route model override remains in place for `settings.llm.model`.

Offline stub tests cover qwen and `claude_oauth/claude-sonnet-5-5` names for LDR, PaperQA, and LangMem. PaperQA's embedded Python runner test checks all three settings fields.

Checks run: the four required named tests, `cargo test -p task-worker proxy_model_name`, and `cargo clippy -p task-worker -- -D warnings`.
