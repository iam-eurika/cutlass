//! AI agent foundation for Cutlass (v1-roadmap M3, `ai-agent-roadmap.md`).
//!
//! Phase 1 of the agent: the machine-callable surface over the command
//! layer, with no network anywhere.
//!
//! - [`wire`]: the LLM-facing JSON vocabulary — every timeline edit the
//!   agent may request, as flat tagged objects with times in seconds and
//!   ids as plain integers, plus the generated tool schemas.
//! - [`validate`]: lowering wire commands to real `cutlass_commands`
//!   commands against a project snapshot, with model-readable rejections.
//!   The whitelist lives here (and in the wire types themselves): no
//!   project commands, no phantom generators.
//! - [`describe`]: the compact project summary + editor context pushed
//!   into every prompt.
//! - [`provider`] / [`providers`]: the `ChatProvider` seam (blocking, tool
//!   calling, streamed text) with the generic OpenAI-compatible HTTP
//!   implementation (Ollama / llama.cpp / LM Studio / cloud gateways) and
//!   the deterministic `ScriptedProvider` test double.
//! - [`config`]: `~/.cutlass/config.toml` `[ai]` parsing; keys never live
//!   in project files.
//!
//! Invariant: **AI proposes, the engine disposes.** Nothing in this crate
//! mutates a project; output is validated commands for the caller to apply
//! through normal dispatch (one history group per prompt, rollback on
//! abort).

pub mod agent;
pub mod config;
pub mod describe;
pub mod provider;
pub mod providers;
pub mod validate;
pub mod wire;

pub use agent::{
    run_prompt, ActionLogEntry, AgentConfig, AgentEvent, EngineBridge, PromptOutcome,
    PromptStatus,
};
pub use describe::{summarize, EditorContext, ProjectSummary};
pub use validate::{validate, Rejection};
pub use wire::{tool_specs, ToolSpec, WireCommand, TOOL_SCHEMA_VERSION};
