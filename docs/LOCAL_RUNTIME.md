# Local runtime measurements (Grounded Agent V1)

Record only what is demonstrated on this host. Do not invent numbers for
models that were not loaded.

## Host capacity (planning context)

- System RAM: ~16 GiB
- NVIDIA VRAM: ~8 GiB (RTX 5050 class)
- Primary product target model: `Qwen/Qwen3.5-9B` (Apache-2.0) — **not**
  verified unquantized on this host.
- Development verification model: official `Qwen/Qwen3.5-4B` (Apache-2.0)
  when a compatible OpenAI-style local runtime with tool-call support is up.

## Measurement template / current status (2026-08-16)

| Field | Value |
|---|---|
| Runtime | Ollama installed; HTTP API reachable at `127.0.0.1:11434` |
| Endpoint | `http://127.0.0.1:11434/v1` (OpenAI-compatible path configured) |
| Model id | **none pulled** (`ollama list` empty) |
| Source / license | N/A for this pass — no weights downloaded |
| Precision / quantization | N/A |
| Startup latency | N/A |
| First-token latency | N/A |
| Full response latency (Arabic slow-system) | Exercised via `RuleBasedProvider` harness (deterministic), not live Qwen |
| Tool calls / iterations | Up to 4 (config default); Arabic slow path used memory+cpu+processes |
| Host RAM used (agent+runtime) | Python pytest suite only; no LLM resident |
| VRAM used | 0 (no model loaded) |
| Provider used for automated tests | `rule` (deterministic) |
| Broker unit tests | 12 passed (`cargo +stable test` outside kernel `build-std` tree) |
| Agent unit/e2e tests | 15 passed (`pytest AI-Module/agent/tests`) |

## Notes

- Live Qwen tool-call verification is deferred until a quantized/compatible
  OpenAI-compatible runtime with `Qwen/Qwen3.5-4B` (or 9B) is installed.
- Broker live `/proc` telemetry is Linux-oriented; Windows hosts can still
  compile and run policy/registry unit tests.
- This agent stack is **not production-ready**.
