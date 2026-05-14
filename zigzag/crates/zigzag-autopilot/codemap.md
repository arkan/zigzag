# z/crates/z-autopilot/

## Responsibility

z-autopilot is the **state machine and workflow engine** for the z system. It
parses, schedules, executes, and monitors deterministic automation workflows
triggered by repository events (push, PR merge, review, Dependabot, manual).

**Owns:**
- Workflow DSL parsing (KDL → typed AST with validation + reachability analysis)
- Deterministic step-to-step state machine with retry/timeout/confirm semantics
- Push governance: config-driven auto-push vs. queue-for-review vs. wait-for-approval
- WorkflowRun persistence (filesystem JSON — crash recovery + observability)
- Lifecycle event detection and notification dispatch
- 6 built-in workflow templates (PR CI fix, PR review fix, merge-when-ready,
  Dependabot auto, deploy-watch, deploy-sync)

**Does not own:** Shell execution, notification delivery, or storage I/O — all
three are injected as trait objects. The crate is a pure orchestration engine.

## Design

**Layered architecture (no circular deps):**
`run_loop` → `lifecycle` → `state` (pure), `dsl`, `config`, `trigger`, `notify`, `persist`, `builtin`

Each layer depends only on those below it; `run_loop` is the sole consumer of all others.

**Trait-based adapter pattern** — Three traits define the engine/environment boundary:
- `StepExecutor` — abstracts shell commands, notifications, confirmations
- `RunStore` — abstracts persistence (filesystem vs. in-memory for tests)
- `Notifier` (from `z-core`) — abstracts notification channel

**Pure state machine with observable events:**
`state::advance()` is a pure function — no I/O. Events are materialized
*post-hoc* by inspecting the run's status/history, not emitted inline during
transitions. This makes the state machine unit-testable without mocking.

**Retry with priority-ordered fallbacks:**
On step failure: same-step retry → `on_max_retries` → `on_failure` → `on_complete` → Stuck.
On success: `on_accept` → `on_success` → `on_complete` → Completed.

**Merge resolution (project + per-workflow config):**
Per-workflow `Option<bool>` overrides layered on top of project-level
`AutopilotConfig`. Resolved config feeds `push_decision()` returning a
tri-state: `Push | QueueForReview | WaitForApproval`.

**Validation with reachability fixpoint:**
`validate_workflow()` follows all transition targets from `steps[0]` until
convergence. Orphan steps are rejected; cycles are allowed (core retry pattern).

## Flow

```
TriggerEvent (push, PR, manual, etc.)
    │
    ▼
matching_workflows() — filter definitions by trigger
    │
    ▼
execute_workflow_run() — main entry point
    │
    ├── load_or_start_run() → RunStore::load_run() / WorkflowRun::new()
    ├── RunStore::save_run()
    │
    ├── [loop] while Running && current_step exists
    │     ├── execute_current_step() → StepExecutor::run_command/notify/confirm
    │     ├── advance_run()          → pure state transition (retry/next/terminal)
    │     ├── RunStore::save_run()   → persist checkpoint
    │     └── notify_autopilot_event() → Notifier::notify() [if terminal/max-retries]
    │
    └── RunLoopReport { run, outcomes, stop_reason, last_event }
```

**Persistence layout:**
```
{state_dir}/{project}/{workflow_name}.json
```
Single JSON object per run, serde-roundtrippable. `prune_terminal_runs()` GC.

## Integration

**From z-core:** `AutopilotConfig`, `Notifier` trait, `NotifyLevel`, `ZError`/`Result`, `kdl`

**To consumers (z-cli, z-tui, z-web, z-plugin):**
Consumers must provide:
1. Parsed `AutopilotWorkflow` definitions (via `dsl` or `builtin`)
2. `StepExecutor` impl (shell/notification/confirm adapter)
3. `RunStore` impl (typically wrapping `persist.rs`)
4. `Notifier` impl (from `z-core::traits`)
5. Call `execute_workflow_run()` + `push_decision()` post-execution

**Public surface (9 modules):**
`builtin`, `config`, `dsl`, `lifecycle`, `notify`, `persist`, `run_loop`, `state`, `trigger`
