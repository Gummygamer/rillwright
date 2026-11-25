# Plan: make tool calls visible in the GUI

**Written:** 2026-11-08.  
**Status:** Implementation delivered; live desktop/model verification remains incomplete.  
**Scope:** Tool activity in the existing Rust desktop conversation, including live turns and reopened Agent sessions.

## 1. Intended result

Every tool call should have a visible card in the Agent conversation. Its header should show the tool name, a useful argument summary, and its current state. Expanding it should reveal the arguments and available result. Calls should remain visible after the assistant answers, after navigating away and back, and after reopening a saved session.

Examples of compact headers:

- `read_file · src/main.rs · Result available`
- `run_command · cargo test · Waiting for approval`
- `write_file · src/config.rs · Completed`
- `web_search · rust parsing examples · Failed`

The conversation should explain what the agent is doing without requiring the user to expand the activity log or open Diagnostics. Existing approval controls, permissions, sandbox behavior, and model prompts retain their behavior.

## 2. Findings from the current code

| Layer | Current behavior | Consequence |
|---|---|---|
| Agent parsing | `AgentEvent::ToolCall` contains a name and structured arguments. It is emitted when the parser recognizes a complete call in `generate_round`. | A detected call is available for display before a result exists. Detection does not prove execution. |
| Agent execution | `run_call` emits `ToolResult` with sanitized text, `is_error`, and `truncated`. Calls execute sequentially. | Live result state is available, but results carry no call identifier. |
| Application bridge | The controller converts calls and results into `AppEvent::Tool { name, status, detail, ... }`. Arguments become a JSON string. Parsed calls are immediately labeled `running`. | Structure is lost and the running label can be premature. |
| GUI event handling | `drain_events` appends `AppEvent::Tool` to `activity`. | Tool calls appear in the collapsed activity/details section and Diagnostics rather than as conversation items. |
| Snapshot projection | `SessionSnapshot` rebuilds `Vec<ChatLine>` using message role and content. It ignores `Message.tool_calls`. | Names and arguments disappear; a call-only assistant message becomes an empty Dory row. |
| Result projection | Tool messages become independent, generic `Tool result` rows. | The result is detached from the call that produced it. |
| Persistence | Version 1 transcripts retain assistant messages with `tool_calls` and subsequent tool result messages. `Session::record` does not persist standalone call/result events. | History contains enough information to reconstruct calls and ordered results, but lacks structured error/truncation flags and timing. |
| Recovery | `session::repair` adds `UNKNOWN_OUTCOME` results for missing results. | Reopened interrupted calls must show uncertainty and must never be executed by rendering. |
| Navigation | `ModeSession` saves only the current message rows and selected session metadata. | New tool presentation state must be saved with the corresponding session. |

Important edge cases already present in `agent.rs`: calls parsed during cancelled generation are discarded from the committed assistant message; final answer rounds ignore parsed calls; length retries can discard a round; cancellation after one call prevents later calls from starting. The GUI must not turn any of these detections into a claim that a tool ran.

## 3. Design choices

1. Use typed conversation items instead of embedding tool JSON in assistant text or introducing new role strings for each tool state.
2. Treat persisted messages as the authoritative source for committed conversation order. Keep live call metadata and uncommitted generation output as a separate overlay, then reconcile them into committed items.
3. Give each live call a presentation identifier. Never correlate results by tool name alone: two calls may have the same name and identical arguments.
4. Preserve `Message` and `ToolCall` as model-facing types. Presentation identifiers and UI state belong in the application/GUI layer and must not enter rendered prompts.
5. Keep transcript version 1 for the initial implementation. Reconstruct history from its existing messages and label unavailable historical status honestly. Persisting exact lifecycle metadata is a separate optional enhancement.
6. Show a detected call as `Pending`, not `Running`. Use `Processing` for a committed unresolved call if exact execution-start telemetry is unavailable. Use `Waiting for approval` only when an approval request is observed.
7. Keep compaction and malformed-call notifications distinct from executable tool calls. The current application event overloads the same `Tool` variant for all three.
8. Preserve complete retained arguments and sanitized results in memory; bound their initial visual preview. A GUI preview limit and tool-side truncation must have different labels.

## 4. Step-by-step implementation

### Step 1 — Define a typed conversation model

**Files:** new `crates/rillwright-gui/src/conversation.rs`; `app.rs`; register the module in `main.rs`.

1. Introduce `ConversationItem` variants for user/assistant text, reasoning, tool calls, and notices. Replace `ChatLine` where it currently represents conversation data.
2. Define a `ToolCallView` with an identifier, tool name, `Map<String, Value>` arguments, optional result text, presentation state, optional `is_error`/`truncated` metadata, and an optional approval request ID.
3. Represent unavailable metadata explicitly. A historical result without an error flag is `Result available`, rather than a confirmed successful completion.
4. Define states for pending detection, processing, waiting for approval, completed, failed, not executed, and outcome unknown. A missing historical result is not evidence of failure.
5. Keep expansion state keyed by stable item ID, separately from the projected content, so snapshots do not collapse cards the user has opened.
6. Make snapshot projection and live-event reconciliation ordinary functions that can be tested without an eframe window or a loaded model.

**Acceptance:** The model can represent two identical calls independently, a call with no assistant prose, a result with unknown success status, and an interrupted call.

### Step 2 — Reconstruct tool cards from message snapshots

**Files:** `conversation.rs`; the `AppEvent::SessionSnapshot` branch in `app.rs`.

1. Walk messages in their existing order, preserving original message positions before filtering system messages or hidden reasoning. Use `(session, assistant message position, call ordinal)` as the committed presentation key.
2. For each assistant message, emit its visible reasoning and nonempty prose, followed by one tool item for every entry in `tool_calls`. Do not emit an empty assistant bubble solely because it contains calls.
3. Pair following tool messages with the preceding assistant's calls in call order, using the agent's sequential execution contract. Pair repeated names by ordinal, never by a name lookup.
4. Recognize `session::UNKNOWN_OUTCOME` explicitly and present it as `Outcome unknown`. Preserve its explanation in the result details.
5. Retain unmatched tool messages as standalone result/notice items. Malformed-call recovery can produce tool messages without corresponding structured calls; older or damaged histories may do the same. Do not silently drop or misattribute those messages.
6. Pair only within the relevant assistant call group. Stop at the next non-tool message; never borrow a result from a later user turn to fill a missing earlier result.
7. Reuse this projector for writable sessions and read-only history snapshots. Keep rendering free of tool execution and transcript writes.

**Acceptance:** Reopening a version 1 transcript shows each call's name, arguments, and matching result. A call-only assistant message produces a useful card. An orphan result remains inspectable.

### Step 3 — Preserve structured live tool data in the application protocol

**Files:** `crates/rillwright-app/src/protocol.rs`; `controller.rs`.

1. Replace the generic string-based call/result payload with typed presentation events, for example `ToolDetected`, `ToolResolved`, and `AgentNotice`. Keep session, turn, and sequence correlation.
2. Carry structured arguments on detection and separate `text`, `is_error`, and `truncated` fields on resolution. Format JSON only at the presentation boundary.
3. Allocate a distinct live call ID for every detection. Keep a per-generation ordered list of these IDs in the controller bridge, and a queue for committed calls awaiting results.
4. Use the existing `RoundStats` boundary to seal each generation's detected-call batch. When its `MessageAdded(assistant)` arrives, associate the committed calls with their live IDs by ordered position and send the committed message-position mapping with the snapshot or a dedicated binding event.
5. Do not put uncommitted detections into the execution-result queue. A retry, cancelled generation, or final answer round may discard them. Retire such provisional cards explicitly when the batch is superseded or the turn ends; show `Not executed` only when the control flow establishes that fact.
6. Match each `ToolResult` to the next committed unresolved call in execution order, validating the name as an invariant. A mismatch should produce a diagnostic/unmatched result, not update an arbitrary card.
7. Give snapshots a sequence watermark or equivalent binding metadata. Ensure a snapshot cannot roll back a newer result overlay, and ensure IDs survive the transition from provisional detection to committed call.
8. Send compaction as a notice with its stage/token counts. Send malformed-call raw text as a bounded error notice; do not represent it as a valid executable call.
9. Keep the existing bounded worker channel and UI draining behavior. Do not introduce output streaming, an event per output character, or additional model work for this feature.

**Acceptance:** Two same-name calls resolve independently. Arguments stay structured. Compaction is not a tool card. A detected call discarded by the agent never appears as successfully executed.

### Step 4 — Reconcile snapshots and live updates without duplicates

**Files:** `conversation.rs`; `RillwrightApp::drain_events`; `ModeSession`.

1. Store authoritative snapshot messages, pending streamed text/reasoning, and live tool overlays separately. Derive the visible conversation from those sources.
2. Apply only events for the intended session and active turn; preserve the existing stale-turn guard. Track sequence progress where needed to make repeated or obsolete events harmless.
3. Reconcile provisional IDs with committed snapshot keys using the bridge's mapping. Preserve expansion and approval associations during this transition.
4. On `ToolResolved`, update the matching card in place. When the following tool-message snapshot arrives, attach its result to the same card instead of adding a second result row.
5. On assistant snapshots, replace the corresponding streamed assistant draft rather than duplicating it. Later assistant output must append after the tool group instead of being joined to a tool result.
6. Keep live error/truncation flags even when a later snapshot carries only message text. A snapshot is not evidence that those flags should be cleared.
7. Save and restore the conversation sources, identity bindings, and view state in `ModeSession`. Navigating to Settings or Diagnostics must leave the active conversation intact while updates continue.
8. Reset session-specific overlays when a new session is acknowledged. Preserve the current conversation while a replacement session is pending or fails.

**Acceptance:** A call appears once through detection, assistant snapshot, result event, result snapshot, final answer, and final snapshot. Returning from Settings retains the same cards and expansion state.

### Step 5 — Connect approval and terminal states accurately

**Files:** `controller.rs` (`GuiApprover`); application protocol; conversation reducer; existing approval handling in `app.rs`.

1. Associate an approval request with the currently executing committed call. The existing sequential executor allows the application bridge and approver to share the current presentation call ID; include it in `AppEvent::Approval`.
2. Keep the approval request ID separate from the call ID. Continue sending answers through `Controller`'s existing approval channel.
3. Set the matching card to `Waiting for approval` when the request arrives. After an answer is submitted, show an interim processing state until the authoritative result or cancellation arrives. A clicked Allow button is not proof of successful execution.
4. Continue showing the current full approval interface, including workspace, working directory, command, arguments, sandbox, risk, and decline reason. An inline card may point to it, but must not add a second execution path.
5. Resolve completion/failure from `ToolResult.is_error`. Display refusal or denial text as returned; do not infer a distinct denial cause by matching arbitrary result strings.
6. At turn finish, cancellation, worker failure, or disconnection, clear stale approval UI and terminalize unresolved live cards. Distinguish calls known never to have started from calls whose effects cannot be determined.
7. Keep `Outcome unknown` for an uncertain started operation; cancellation alone does not prove that a write or command had no effects. Preserve the existing no-replay recovery behavior.
8. Do not leave pending cards animated after a terminal event. Include cancelled generation and cancellation between two calls in the state-transition tests.

**Acceptance:** Waiting, approval, completion, refusal, and interruption are understandable from the conversation. Closing or cancelling an approval cannot leave a stale spinner or permit a later call to start through the UI.

### Step 6 — Render readable tool cards in the conversation

**Files:** conversation rendering in `app.rs`; optional new `tool_card.rs` module.

1. Add a dedicated renderer for each typed item. Render tool cards in the existing transcript scroll area, immediately after the assistant message that requested them.
2. Keep the tool name and textual state visible when collapsed. Use color or an icon as a secondary cue, so status remains understandable without color.
3. Generate short tool-specific summaries from the actual argument schema: file path for file tools, command and optional working directory for commands, query/URL for web tools. Inspect the built-in definitions before implementing these mappings; unknown tools get a generic summary.
4. Put full pretty-printed arguments and available result text in separate expandable sections. Use selectable monospace text with wrapping and existing font-scale settings.
5. Add clearly labeled Copy arguments and Copy result actions. Copy the complete retained value, even if the visible preview is bounded.
6. Reuse the UTF-8-safe `bounded` behavior. Start with the current output-preview scale (approximately 4,000 bytes), and keep `Show full retained output` separate from `[tool output truncated]`.
7. Keep an unexpanded card short when an argument contains an entire file, a patch, or a large command output. Large content must not become the header summary.
8. Use explicit egui IDs based on item identity for cards and nested collapsing sections. Duplicate labels must not share expansion state.
9. Preserve the anchored composer and useful transcript height. New output should follow the bottom when appropriate without forcibly moving a user who is inspecting an earlier card.
10. Keep reasoning visibility independent of tool visibility. Hiding reasoning must never hide tool calls. Plain Chat should retain its current appearance when it has no tools.

**Acceptance:** A user can identify the tool and state without expanding anything, inspect/copy arguments and results, and read a large Unicode result without broken text or an unusable composer.

### Step 7 — Keep history behavior explicit and compatible

**Files:** snapshot projector; history opening paths in `controller.rs`; `docs/GUI.md`.

1. Reconstruct call cards from existing assistant/tool messages for all version 1 sessions. No migration is required for names, arguments, result text, or ordering.
2. Use `Result available` for restored results without reliable structured execution metadata. Never classify arbitrary output beginning with `error` as a confirmed failure.
3. Use `Outcome unknown` for repaired interrupted calls. Read-only history must also show unmatched/missing results as unknown, even where opening that history does not write repairs.
4. Preserve read-only history, writer locking, torn-tail handling, and continuation review. Expanding or copying a card must not acquire a writer lock or alter a transcript.
5. Document that exact historical success/failure, tool-side truncation flags, approval decisions, and duration are not recoverable from the current message-only data.
6. If exact historical lifecycle later becomes a requirement, design optional presentation records separately. Review version/read compatibility, append ordering, crash recovery, and both CLI/GUI writers before changing the transcript schema. Do not make that migration a dependency of initial visibility.

**Acceptance:** Old sessions open with useful call cards, no schema upgrade, and no invented historical execution status.

### Step 8 — Exercise the same presentation path in demo mode

**Files:** `start_demo`, `advance_demo`, and demo approval handling in `app.rs`.

1. Replace the prose-only demo action with scripted conversation/tool events passed through the same reducer used for real events.
2. Include a read call with arguments and a result, an approval-required command, and a final assistant answer.
3. Extend the existing failure simulation with a failed tool result and an unresolved call that ends as unknown. Include two calls with the same tool name.
4. Include a large Unicode result to exercise preview and copy behavior.
5. Preserve the demo guarantee: no model load, no command execution, and no real provider calls.

**Acceptance:** `rillwright-gui --demo` demonstrates tool cards, approval, results, and failure using the real rendering/reconciliation code.

### Step 9 — Add focused automated regression coverage

**Files:** tests beside the new conversation module and controller bridge; session tests only if their behavior changes.

Use deterministic messages/events and temporary transcript fixtures. The following cases verify behavior rather than widget implementation details:

| Case | Required assertion |
|---|---|
| Assistant contains only a tool call | One named tool card; no empty assistant bubble. |
| Assistant prose and two calls | Prose precedes two distinct cards; ordered results attach correctly. |
| Repeated name and arguments | Distinct identities and independent result/state updates. |
| Detection → snapshots → result → final snapshot | Exactly one card and one retained result throughout. |
| Hidden reasoning | Tools and visible assistant text remain in place. |
| Approval allowed/refused/cancelled | Correct card association; no stale approval at turn end. |
| Cancelled generation/final answer round | Detected but ignored calls are not reported as executed. |
| Cancellation after first of two calls | First result stays attached; second call has no fabricated result. |
| Live failure and truncated output | Structured flags survive subsequent message snapshots. |
| Reopened version 1 session | Name, arguments, order, and text reconstruct; unknown success remains unknown. |
| Unknown outcome, orphan tool message, malformed call | Content remains visible without incorrect attribution. |
| Stale session/turn event | Current conversation is unchanged. |
| Mode switch/new session | State is preserved or reset for the correct session only. |
| Large Unicode arguments/results | Preview remains valid UTF-8; complete retained text is copyable. |

Run focused GUI/application tests as the implementation develops. If shared agent event types change despite the application-layer approach, update exhaustive CLI matches and run agent/CLI checks as well; keep the documented JSON event contract deliberate.

**Acceptance:** Automated tests cover both history reconstruction and the live event/snapshot lifecycle without requiring the model or GPU.

### Step 10 — Validate the desktop experience and document the change

**Files:** `docs/GUI.md`; this plan's implementation/verification record when executed.

1. Run formatting, targeted tests, and compile checks against the existing pinned native fork:

   ```sh
   cargo fmt --all --check
   cargo test -p rillwright-app -p rillwright-gui
   cargo check -p rillwright-app -p rillwright-gui
   ```

   These tests do not need model inference, but building/linking still requires the repository's native library setup. Set `RILLWRIGHT_LLAMA_DIR` to the existing fork and put its `build/bin` directory on `LD_LIBRARY_PATH` where needed. Record missing native dependencies as blockers rather than calling unrun checks passed.

2. Run `cargo check --workspace` if application protocol or shared event changes affect additional crates. Run agent tests if agent behavior or events changed. A documentation-only planning change does not require these implementation checks.
3. Launch `target/debug/rillwright-gui --demo` after building it. Inspect default and minimum window sizes, light/dark themes, supported font scales, keyboard focus, expand/collapse, copy, and long-output scrolling.
4. Navigate to Settings and Diagnostics during streaming/approval, then return. Verify the same conversation, cards, pending approval, and draft remain usable.
5. With the real model, use a disposable workspace containing a small text file. Ask for a file read; then request a harmless command under Ask mode. Verify visibility before completion, approval correlation, final result, and final assistant text.
6. Test decline and cancellation in the disposable workspace. Inspect the workspace and transcript where effects could be uncertain; confirm no automatic re-execution on reopen.
7. Reopen the completed and interrupted sessions from History. Compare call names, arguments, result pairing, and uncertainty labels with the live view.
8. Update `docs/GUI.md` with the tool-card workflow, status meanings, output preview behavior, and historical metadata limitation. Keep the existing activity log for diagnostics rather than duplicating its full text in every card.
9. Record exactly which automated and desktop checks ran and which remain unverified. Mark implementation complete only after the acceptance criteria have supporting evidence.

## 5. Suggested implementation slices

1. **Snapshot visibility:** typed conversation model, history projection, and basic tool-card renderer. This makes existing calls visible immediately, including after reopening sessions.
2. **Live lifecycle:** structured application events, presentation IDs, generation-to-snapshot binding, and result reconciliation. This removes duplicates and preserves live metadata.
3. **Approval and terminal handling:** call/request association, cancellation, worker errors, and navigation/session state.
4. **Release verification:** scripted demo, focused tests, desktop checks, real-model smoke, and user documentation.

Keep each slice reviewable. Do not expand this work into model-parser changes, command-output streaming, new tool execution behavior, or a general transcript redesign.

## 6. Completion checklist

- [x] Calls appear directly in the Agent conversation with names and argument summaries.
- [x] Arguments and results are independently expandable and copyable.
- [x] Repeated tools have distinct call identities; snapshots retain results and streamed text is reconciled with committed assistant messages.
- [x] Approval and terminal states are connected to the corresponding card; finished turns clear pending states.
- [x] Discarded detections are excluded from the committed result queue, and uncertain effects are labeled unknown.
- [x] Calls survive Settings/Diagnostics navigation and version 1 history is reconstructed from messages.
- [x] Version 1 transcripts remain unchanged; missing historical metadata is stated honestly.
- [x] Reasoning visibility, plain Chat, permissions, and the existing approval execution path are retained.
- [ ] Interactive desktop/model acceptance checks are complete. Demo launch and automated checks are recorded below; visual interaction, real-model, cancellation, and history-reopen smoke checks remain unverified.

## 7. Implementation and verification record

Implemented typed conversation items and version 1 snapshot projection, stable live call IDs with ordered binding to committed assistant call positions, structured argument/result/error/truncation events, in-place result reconciliation, approval association, terminal status handling, expandable/copyable cards, scripted demo cards, and user documentation. The transcript schema and agent/model prompt types were not changed.

Checks run in this environment with `RILLWRIGHT_LLAMA_DIR=/run/media/darabat/24f15d44-9691-4cf4-8b52-bc03f1e3693f/src/llama.cpp-dory` and its `build/bin` on `LD_LIBRARY_PATH`:

- `cargo fmt --all --check` — passed.
- `cargo test -p rillwright-app -p rillwright-gui` — passed (9 application tests and 20 GUI tests, including three conversation projection/Unicode tests).
- `cargo check --workspace` — passed.
- `cargo build -p rillwright-gui` — passed.
- `timeout 8s target/debug/rillwright-gui --demo` — started without a startup error in the available Wayland session; the process was stopped by the timeout. The demo was not driven interactively.

Not verified: minimum/default window layout, themes and font scales, keyboard/clipboard interaction, card expansion and long-output scrolling, Settings/Diagnostics navigation during a live turn, real-model read/approval/decline/cancellation, and reopening completed/interrupted sessions in the GUI. No real-model or native visual acceptance is claimed. The final checklist remains open until these checks are performed.
