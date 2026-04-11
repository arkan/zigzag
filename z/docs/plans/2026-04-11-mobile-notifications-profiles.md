# PRD: Mobile Notifications via Moshi + Generic Profile System

## Problem Statement

z runs on a VPS, accessed via SSH/Mosh from both macOS and iOS (using the Moshi app). Currently, z supports desktop notifications (macOS native via `osascript`) and file-based TUI badge notifications. When working from iOS, there is no way to receive push notifications for autopilot events (workflow completed, failed, stuck).

The user runs two separate instances of z on the same VPS — one from Mac, one from iOS — sharing the same projects and Zellij sessions but using different SSH sessions. They need a way to route notifications to the right channel depending on which device they are connecting from.

## Solution

Two additions to z:

1. **MoshiNotifier** — A new notification channel that sends push notifications to iOS via the Moshi webhook API (`POST https://api.getmoshi.app/api/webhook`).

2. **Generic Profile System** — A configuration profile mechanism that allows overriding any config section at launch via a `--profile=<name>` CLI flag. Profiles use inheritance: they merge on top of the default config, only overriding specified fields.

### Usage

```sh
# From Mac (default profile — macOS native notifications)
z autopilot run my-session

# From iOS (ios profile — Moshi push notifications, no macOS native)
z --profile=ios autopilot run my-session
```

### Configuration

```kdl
config {
    notifications {
        macos-native true
        moshi true
        moshi-token "env:MOSHI_TOKEN"
        tui true
    }
}

profile "ios" {
    notifications {
        macos-native false
    }
}
```

## User Stories

1. As a mobile user, I want to receive push notifications on my iPhone when an autopilot workflow completes, so that I don't have to keep checking the terminal.
2. As a mobile user, I want to receive push notifications on my iPhone when an autopilot workflow fails or gets stuck, so that I can intervene quickly.
3. As a desktop user, I want macOS native notifications to continue working as before when I connect from my Mac, so that my existing workflow is unchanged.
4. As a user with multiple devices, I want to select a configuration profile at launch via `--profile=<name>`, so that notifications are routed to the right device.
5. As a user, I want the iOS profile to disable macOS native notifications automatically, so that I don't get noise on a device I'm not using.
6. As a user, I want profiles to inherit from the default config and only override what I specify, so that I don't have to duplicate my entire configuration.
7. As a user, I want to configure the Moshi token via an environment variable (`env:MOSHI_TOKEN`), so that secrets are not stored in plain text in my config file.
8. As a user, I want TUI badge notifications to remain active regardless of profile, so that I always see notification badges in the TUI.
9. As a user, I want notification failures (Moshi API unreachable) to be best-effort and not block my workflow, so that a network issue doesn't break autopilot.
10. As a user, I want to use profiles to override any config section (not just notifications) in the future, so that the system is extensible.
11. As a user, I want a clear error when I specify a profile name that doesn't exist, so that I catch typos immediately.
12. As a user, I want the default behavior (no `--profile` flag) to work exactly as it does today, so that this change is backwards-compatible.

## Implementation Decisions

### Module 1: Config Profile System (`z-core` — config module)

- Add `moshi: bool` and `moshi_token: Option<String>` fields to `NotificationsConfig`.
- Parse `profile "<name>" { ... }` blocks in `parse_global_config_kdl()`.
- Store profiles in `GlobalConfig` as a `HashMap<String, ProfileOverride>` where `ProfileOverride` contains optional overrides for each config section.
- Implement `GlobalConfig::with_profile(name: &str) -> Result<GlobalConfig>` that merges the profile's overrides on top of the default config. Returns an error if the profile name is unknown.
- Profile merge is shallow per-section: if a profile specifies a `notifications` block, each field present in the profile overrides the corresponding field in the default; fields not mentioned in the profile are inherited from the default.
- The `moshi-token` field uses the existing `env:VAR` resolution mechanism.

### Module 2: MoshiNotifier (`z-cli` — notify module)

- New `MoshiNotifier` struct with a `token: String` field, implementing the `Notifier` trait.
- Sends an HTTP POST to `https://api.getmoshi.app/api/webhook` with a JSON body: `{"token": "<token>", "title": "<title>", "message": "<message>"}`.
- Uses `curl` for the HTTP request (same pattern as `TelegramNotifier`).
- Title is derived from `NotifyLevel`: `"z"` for Info, `"z ⚠️"` for Warning, `"z ❌"` for Error.
- Best-effort error handling: if `curl` fails, return `ZError::Io` but `DispatchNotifier` continues with other channels.
- Added to `DispatchNotifier::from_config()` when `config.moshi == true` and `config.moshi_token` is `Some`.

### Module 3: CLI Flag (`z-cli` — main module)

- Add a global `--profile <name>` flag to the clap CLI definition.
- After loading `GlobalConfig`, apply the profile via `with_profile()` before passing the config to any command handler.
- If `--profile` is not specified, the default config is used unchanged.

### Architectural Decisions

- Profile is fixed at launch time. No runtime switching between profiles.
- `FileNotifier` (TUI badges) is always active regardless of profile — it is not controlled by the profile system.
- The profile system is implemented generically (any section can be overridden) but only `notifications` is tested and validated in V1.
- No compile-time platform guards (`#[cfg(target_os)]`) — the config flags control which notifiers are active.

## Testing Decisions

Good tests for this feature test external behavior through the public API, not internal implementation details. They should verify that config parsing produces the right `GlobalConfig`, that profile merging produces correct overrides, and that notifiers produce the right shell commands / HTTP payloads.

### Module 1: Config Profile System

- Parse a config with one or more `profile` blocks and verify the resulting `GlobalConfig` contains the right overrides.
- Apply a profile and verify that overridden fields change while non-overridden fields are inherited.
- Verify that applying an unknown profile name returns an error.
- Verify that `moshi` and `moshi-token` fields parse correctly (including `env:VAR` resolution).
- Verify that a config with no profiles works as before (backwards compatibility).
- **Prior art**: existing `parse_global_config_*` tests in `z-core/src/config.rs`.

### Module 2: MoshiNotifier

- Verify the JSON body construction (token, title based on level, message content).
- Verify the curl command arguments are correct.
- Verify that `DispatchNotifier::from_config()` includes `MoshiNotifier` when `moshi == true` and token is set, and excludes it otherwise.
- **Prior art**: existing `TelegramNotifier` and `DispatchNotifier` tests in `z-cli/src/notify.rs`.

### Module 3: CLI Flag

- Verify that `--profile=ios` applies the profile to the loaded config.
- Verify that omitting `--profile` uses the default config.
- Verify that `--profile=nonexistent` produces a user-facing error.
- **Prior art**: existing CLI integration tests.

## Out of Scope

- **Automatic platform detection** — no runtime detection of whether the user is on Mac or iOS. The user selects the profile explicitly.
- **Active session / dynamic switching** — no mechanism to change the notification channel while z is running. Profile is fixed at launch.
- **Retry / notification queue** — notifications are fire-and-forget, best-effort. No retry logic, no persistent queue.
- **Notification deduplication** — both instances may show TUI badges for the same event. This is acceptable.
- **Other notification providers** — only Moshi is added in this iteration.

## Further Notes

- The Moshi API endpoint is `POST https://api.getmoshi.app/api/webhook` with JSON body `{"token": "...", "title": "...", "message": "..."}`.
- The branch name `feat/mobile-norifications` (typo in "notifications") is pre-existing and should be kept as-is to avoid confusion.
- The profile system lays the groundwork for future per-device customization beyond notifications (e.g., different TUI themes, keybindings, or layouts per device).
