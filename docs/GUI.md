# Rillwright desktop application

`rillwright-gui` is the Linux desktop frontend for the pinned Dory model and the existing workspace agent. It loads one model at a time. Chat sessions use no workspace tools; Agent sessions are tied to the workspace shown in the window and use the same policy, built-ins, and sandbox as `rillwright agent`.

## Build and launch

Build the llama.cpp fork pinned in `crates/rillwright-engine/src/verify.rs`, then build the desktop app:

```sh
export RILLWRIGHT_LLAMA_DIR=/path/to/llama.cpp-dory  # legacy alias: DORY_LLAMA_DIR
cargo build --release -p rillwright-gui
target/release/rillwright-gui
```

The app opens in Agent mode and automatically verifies and loads the selected model in the background. Model selection precedence is `--model PATH`, nonempty `RILLWRIGHT_MODEL` (legacy alias `DORY_MODEL`), the saved model preference, then the in-app model picker. The startup checks the model SHA-256 and fork commit before loading. Use `target/release/rillwright-gui --demo` to exercise the scripted interface without loading a model; its approvals and responses do not execute tools.

Launch arguments accept explicit overrides:

```sh
target/release/rillwright-gui --model /path/to/Dory.gguf --workspace /path/to/project
```

Workspace selection precedence is `--workspace DIR`, a meaningful inherited working directory, then the last valid saved workspace; otherwise the in-app folder chooser appears. The home directory and filesystem root require an explicit project choice unless passed with `--workspace`. A project does not need to be a Git repository. Desktop launchers may not inherit shell environment variables or the shell's current project directory, so configure the model path or use the model picker and choose a workspace when needed. `--demo`, `--model`, and `--workspace` are validated; unknown or incomplete arguments show an error.

For a local Linux bundle, run `scripts/package_gui.sh`. It stages the executable, launcher, app icon, and the fork's llama/ggml shared libraries under `dist/`. The same icon is embedded in the native window and shipped as `share/icons/hicolor/512x512/apps/rillwright.png`. Keep the model external. The launcher uses the bundled fork libraries; CUDA driver/runtime libraries and the desktop portal are system requirements. This bundle is for the Linux environment used to build it, not a general-purpose cross-distro image.

## First use

1. The configured project opens as a fresh Agent session once both workspace and model are ready. The composer remains editable during initialization; sending is enabled only after the session is acknowledged.
2. Use **Choose model** or **Browse** to recover from a missing model, then retry loading. Workspace changes create a session for the selected project. A new conversation keeps the current mode and workspace.
3. The default permission mode is `ask`, with sandbox `auto`. Read only allows reads; Ask requests writes and commands; Auto edit allows writes and normally asks for commands; Full allows tool execution subject to sandbox rules. The Agent header shows the requested settings and effective sandbox status. A change is acknowledged before it is used by a turn; changes during a turn apply to the next turn.
4. Command network access is controlled by the active sandbox. When commands run unconfined, the network checkbox does not provide isolation. Web search uses a separate permission and provider configuration.
5. Press **Ctrl+Enter** to send and **Escape** to request cancellation. On close during a turn, choose whether to keep it running or stop and close. Native model initialization itself cannot be interrupted, so shutdown waits for that operation to return.
6. Use **History** to inspect and continue compatible sessions. A session owned by another process opens read-only. Interrupted tool calls are marked with an unknown outcome and are never replayed automatically.
7. Use **Prompt Inspector** to see the rendered system/user prompt and token count for the currently loaded model; it does not attach workspace tools.

## Model loops and conversation navigation

Settings exposes **Model loops** with the supported values 1–4 and a default of 2. This is the model's recurrence count. It is separate from the **Agent round limit**, which caps agent tool/generation rounds. The selected loop count is saved with preferences and used for future model loads. Settings shows the selected count and the acknowledged loaded count; a difference means a reload is required. Use **Reload model with selected settings** to apply the selected model-loading options. Saving preferences and applying next-turn settings do not reload the model. The existing conversation and draft are preserved while the model reloads, and sending stays disabled until the load is acknowledged.

You can open Settings or Diagnostics during a turn and return to the same Agent conversation while it is running, waiting for approval, applying settings, or initializing. The transcript, draft, workspace, and approval remain attached to that conversation. Switching to a different Chat or Agent conversation and starting a new conversation can remain unavailable while work is pending.

## Tool calls in the conversation

Agent tool calls appear in the transcript as cards with the tool name, a short argument summary, and a text status. Expand **Arguments** or **Result** to inspect the retained structured arguments and result; the copy buttons copy the complete values. Long results show a UTF-8-safe preview and can be expanded to the full retained text. A separate `[tool output truncated]` label means the tool itself shortened its output.

Statuses distinguish **Pending**, **Processing**, **Waiting for approval**, **Completed**, **Failed**, **Not executed**, and **Outcome unknown**. An approval request is also shown in the existing approval panel. Historical results from version 1 sessions show **Result available** because those transcripts did not record reliable success flags, tool-side truncation, approval decisions, or timing. Repaired interrupted calls show **Outcome unknown**; reopening a card never reruns a tool. Unmatched historical tool results remain visible as notices.

Plain chat uses the legacy `${XDG_DATA_HOME:-~/.local/share}/dory-harness-chat` directory when it exists, otherwise `${XDG_DATA_HOME:-~/.local/share}/rillwright-chat`. If both exist, the app reports the conflict and keeps using the legacy root; close the app before manually consolidating histories. Unreadable storage is reported as an error. Agent transcripts stay in `<workspace>/.dory/sessions/`. Settings prefer `${XDG_CONFIG_HOME:-~/.config}/rillwright/settings.json`, migrate valid legacy settings from `dory-harness/settings.json`, and preserve the original.

## Permissions and web search

Agent policy defaults to `ask` and sandbox `auto`. The interface distinguishes `bwrap active`, `Auto fallback: unconfined`, `None: unconfined`, and `bwrap unavailable: commands blocked`. `full` permission still follows the CLI's sandbox refusal rules; selecting sandbox `none` is an explicit less-restricted choice. Network access inside sandboxed commands is independent of the web search tool.

Web search is disabled by default. Enable it in Settings and choose Tavily or Brave. The key is read from `TAVILY_API_KEY` or `BRAVE_SEARCH_API_KEY`; the app does not save keys in settings. Desktop launchers often do not inherit shell environment variables, so configure the variable in the desktop session before launching. Web quota state is persisted by the existing web provider layer.

## Troubleshooting

- **Fork or checksum mismatch:** verify `RILLWRIGHT_LLAMA_DIR` points at the pinned fork and the selected model is the pinned file. Diagnostics shows the expected and built fork identifiers.
- **Shared library error before a window appears:** build against the matching fork, or use the generated launcher and check that system CUDA libraries are installed. Native loader failures occur before the application can display an error panel.
- **Sandbox unavailable:** install `bubblewrap` (`bwrap`) on Linux, or select the intended sandbox mode in Settings. The effective state is shown for Agent sessions.
- **File picker unavailable:** install a working desktop portal and file chooser service for the current Wayland/X11 session.
- **GPU allocation failure:** lower context or GPU layers, or close other GPU workloads before loading the model.
- **Session locked:** another frontend owns its writer lock. The current UI reports the lock rather than resuming concurrently; close that session in the other process first.

## Platform and implementation limits

Linux is the supported target. Windows and macOS need separate native-library and sandbox work. Accessibility integration is enabled through AccessKit, but screen-reader, IME, clipboard, and high-DPI behavior still need hands-on validation on supported desktop environments. The UI currently keeps complete message text in memory; very long histories should be collapsed or split across sessions. Benchmarking, external-tool configuration, and server controls remain outside this release.

## Verification record

For the loop/navigation changes, `cargo fmt --all --check`, `cargo test -p rillwright-gui -p rillwright-app` (19 tests), `cargo test -p rillwright-engine --test loops` (3 tests), and `cargo check -p rillwright-gui -p rillwright-app` passed with the pinned fork's `build/bin` on `LD_LIBRARY_PATH`. `rillwright-gui --demo` launched without a startup error and was stopped after eight seconds; visual checks at minimum/default sizes and keyboard interaction were not performed. A real-model reload and live streaming/approval navigation check were not performed, so native loop application and those desktop interactions remain unverified.
