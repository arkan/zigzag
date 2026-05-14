# z/crates/z-autopilot/src/

## Responsibility

z-autopilot is the **state machine and workflow engine** for the z system. It
defines, schedules, executes, and monitors automation workflows triggered by
repository events (push, PR merge, review, Dependabot, manual). It owns:

- **Workflow DSL** — KDL-based parsing of workflow definitions into a typed AST
- **State machine** — Deterministic step-to-step transition engine with retry,
  timeout, confirm/accept/reject semantics, and terminal-state detection
- **Push governance** — Config-driven rules for auto-push vs. queue-for-review
  vs. wait-for-approval, at both project and per-workflow granularity
- **Persistence** — Filesystem-backed JSON serialization of workflow runs for
  crash recovery and observability
- **Notification dispatch** — Lifecycle event detection (completed, failed,
  stuck, max retries exhausted) and delivery via the `Notifier` trait
- **Built-in templates** — Six production workflow definitions bundled as KDL
  constants (PR CI fix, PR review fix, PR merge-when-ready, Dependabot auto,
  deploy-watch, deploy-sync)

It does **not** own: shell command execution (delegated to `StepExecutor`),
notification delivery (delegated to `Notifier`), or storage I/O (delegated to
`RunStore`). These are injected as trait objects — the crate is a pure
orchestration engine.

## Design Patterns

### Layered Architecture (vertical slice)

```
  run_loop.rs       — top-level orchestration loop (load → execute → persist → notify)
  lifecycle.rs      — step execution dispatch + advance bundling
  state.rs          — pure state machine transitions (no I/O, no side effects)
  dsl.rs            — workflow definition parsing and validation
  config.rs         — project + per-workflow config resolution
  trigger.rs        — event-to-workflow matching
  notify.rs         — lifecycle event detection + message building + dispatch
  persist.rs        — filesystem read/write of WorkflowRun JSON
  builtin.rs        — 6 bundled workflow templates as const KDL strings
```

Each layer depends only on layers below it; `run_loop.rs` is the sole consumer
of all others.

### Trait-Based Adapter Pattern

Three traits define the boundary between the engine and its runtime environment:

- **`StepExecutor`** (`lifecycle.rs`) — `run_command(&str) -> Result<StepResult>`,
  `notify(&str) -> Result<()>`, `confirm(&str) -> Result<bool>`. Abstracts shell
  execution, notification dispatch, and interactive confirmation behind a
  uniform interface.

- **`RunStore`** (`run_loop.rs`) — `load_run`, `save_run`, `delete_run`.
  Abstracts persistence medium (filesystem in production, memory in tests).

- **`Notifier`** (defined in `z-core::traits`) — `notify(&str, NotifyLevel) ->
  Result<()>`. Abstracts notification channels (terminal, system notifications,
  etc.).

### Pure State Machine with Observable Events

`state.rs`'s `advance()` is a pure function: given `(&AutopilotWorkflow,
&mut WorkflowRun, StepResult)` it mutates the run and returns `Option<String>`
(the next step name). It has no I/O side effects. Event detection is a
**separate, post-hoc** operation via `event_from_advance()` in `notify.rs`,
which inspects the run's final status and history to produce an `AutopilotEvent`.

This separation means:
- The state machine is unit-testable without mocking I/O
- Events are materialized from state, not emitted inline during transitions
- The `lifecycle.rs` `advance_run()` function bundles the two calls together
  for convenience (ordering-sensitive protocol enforced locally)

### Retry Semantics with Priority-Ordered Fallbacks

On step failure, `advance()` consults transition targets in strict priority:

1. If `max_retries` is set and retry count < max → **retry same step** (increment count)
2. If `max_retries` exhausted and `on_max_retries` → **transition to that step**
3. If no `on_max_retries` and `on_failure` → **fallback to on_failure**
4. If no `on_failure` and `on_complete` → **fallback to on_complete**
5. Otherwise → **Stuck** (workflow halts, no transition)

On success (including confirm-accept):
- `on_accept` (confirm steps) → `on_success` → `on_complete` → **Completed**

### Merge Resolution: Project + Per-Workflow Config

`resolve_config()` in `config.rs` layers per-workflow `Option<bool>` overrides
on top of project-level `AutopilotConfig`. Per-workflow `Some(v)` wins over
project default. `None` falls through to project value. The resolved config then
feeds `push_decision()` which returns a tri-state `PushDecision`:

```
auto_push? │ review? │ Result
───────────┼─────────┼───────────────────
false      │ any     │ QueueForReview
true       │ true    │ WaitForApproval
true       │ false   │ Push
```

### Validation with Reachability Analysis

`validate_workflow()` in `dsl.rs` performs a fixed-point reachability analysis
starting from `steps[0]`, following all transition targets
(`on_success`/`on_failure`/`on_complete`/`on_max_retries`/`on_accept`/`on_reject`),
marking reachable steps iteratively until convergence. Unreachable (orphan)
steps are rejected. Cycles are explicitly allowed — they are the core retry
pattern.

## Data & Control Flow

### Workflow Definition Lifecycle

```
KDL source text
  │
  ▼
parse_autopilot_workflows()  [dsl.rs]
  │  KDL lexing + AST construction per autopilot node
  │  Named nodes → AutopilotWorkflow, unnamed → config block
  ▼
Vec<AutopilotWorkflow>
  │
  ▼ (optional)
validate_workflow()  [dsl.rs]
  │  duplicate check → reachability fixpoint → transition validity
  ▼
AutopilotWorkflow (guaranteed well-formed)
```

### Runtime Execution Flow

```
TriggerEvent  (from external system: push, PR, manual, etc.)
  │
  ▼
matching_workflows()  [trigger.rs]
  │  filter workflows where trigger == event
  ▼
Vec<&AutopilotWorkflow>
  │
  ▼
execute_workflow_run()  [run_loop.rs]
  │
  ├── load_or_start_run()  → RunStore.load_run()  / WorkflowRun::new()
  ├── store.save_run()     → RunStore.save_run()
  │
  ├── [loop] while Running && current_step.is_some() && under step limit
  │     │
  │     ├── execute_current_step()  [lifecycle.rs]
  │     │     │  current_step() → delegate to StepExecutor
  │     │     │    run_command() → StepResult
  │     │     │    notify()      → StepResult::Success
  │     │     │    confirm()     → StepResult::Success/Failure
  │     │     ▼
  │     │  StepResult
  │     │
  │     ├── advance_run()  [lifecycle.rs → state.rs::advance()]
  │     │     │  record StepExecution in history
  │     │     │  apply retry/transition logic
  │     │     │  update WorkflowRun.status (Running/Completed/Failed/Stuck)
  │     │     ▼
  │     │  AdvanceOutcome { next_step, event }
  │     │
  │     ├── store.save_run()  → persist updated WorkflowRun
  │     └── if event: notify_autopilot_event()  → Notifier.notify()
  │
  └── RunLoopReport { run, outcomes, stop, last_event }
```

### Data Structures

**WorkflowRun** (the fundamental state unit):
```rust
WorkflowRun {
    workflow_name: String,      // identifier matching AutopilotWorkflow.name
    project: String,            // originating repository
    host: Option<String>,       // remote target (None = local)
    status: WorkflowStatus,     // Running | Completed | Failed | Stuck
    current_step: Option<String>, // None = terminal
    retry_count: u32,           // within-current-step retry counter
    history: Vec<StepExecution>, // append-only execution log
}
```

**WorkflowStatus → AutopilotEvent** mapping:

| Status | Condition | Event |
|--------|-----------|-------|
| `Running` → `Running` | retry same step | `None` |
| `Running` → `Running` | step transition | `None` |
| `Running` → `Completed` | terminal success | `Completed` |
| `Running` → `Failed` | terminal failure | `Failed` |
| `Running` → `Stuck` | max retries with no fallback | `Stuck` |
| `Running` → `Running` | max retries but fallback exists | `MaxRetriesExhausted` |

### Persistence Layout

```
{state_dir}/
  {project}/
    {workflow_name}.json    — single JSON object, serde-roundtrippable
```

`prune_terminal_runs()` walks the tree, removes JSON for non-Running workflows.

## Integration Points

### Upstream Dependencies

| Dependency | Module | Use |
|-----------|--------|-----|
| `z-core::config::AutopilotConfig` | `config.rs` | Project-level auto-push/review settings |
| `z-core::config::parse_autopilot_config_doc` | `config.rs` | Parse config from KDL document |
| `z-core::traits::Notifier` | `notify.rs`, `run_loop.rs` | Notification dispatch trait |
| `z-core::domain::NotifyLevel` | `notify.rs` | Severity levels for notification |
| `z-core::error::{Result, ZError}` | All modules | Error handling (ConfigParse, Io variants) |
| `kdl` | `dsl.rs`, `config.rs` | KDL document parsing |

### Downstream Consumers

Any consumer that wants to run an autopilot workflow needs to:

1. **Parse workflows** via `parse_autopilot_workflows()` or `builtin_workflows()`
2. **Match a trigger** via `matching_workflows()` against a `TriggerEvent`
3. **Implement `StepExecutor`** — the concrete adapter for shell commands,
   notifications, and confirmations
4. **Implement `RunStore`** — typically wrapping `persist.rs` functions
5. **Obtain a `Notifier`** — from `z-core::traits`
6. **Call `execute_workflow_run()`** — handles the full lifecycle

The `config.rs::resolve_config()` + `push_decision()` chain is used by the
caller *after* workflow execution to decide whether to push results, queue
them, or wait for approval.

### Exported Surface (public API)

```
pub mod builtin;       → builtin_workflows()
pub mod config;        → RepoAutopilotConfig, parse_repo_autopilot_config(),
                        resolve_config(), push_decision(), PushDecision
pub mod dsl;           → AutopilotWorkflow, Step, StepAction, Trigger,
                        parse_autopilot_workflow(), parse_autopilot_workflows(),
                        parse_autopilot_workflows_doc(), validate_workflow()
pub mod lifecycle;     → StepExecutor trait, AdvanceOutcome, ExecuteStepOutcome,
                        advance_run(), execute_current_step()
pub mod notify;        → AutopilotEvent, event_from_advance(), build_message(),
                        event_level(), notify_autopilot_event()
pub mod persist;       → save_run(), load_run(), delete_run(), list_runs(),
                        prune_terminal_runs()
pub mod run_loop;      → RunStore trait, RunLoopOptions, RunLoopStop,
                        RunLoopReport, load_or_start_run(), execute_workflow_run()
pub mod state;         → WorkflowRun, WorkflowStatus, StepStatus, StepResult,
                        StepExecution, advance(), current_step()
pub mod trigger;       → TriggerEvent, matches_trigger(), matching_workflows()
```
