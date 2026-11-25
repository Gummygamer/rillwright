# Rillwright Android GUI implementation plan

Written: 2026-11-08. Repository inspected at commit `354ac92`, including the current working tree.

This is an implementation plan, not a record of completed Android work. It assumes the goal is a standalone Android application that runs the existing model locally and offers both chat and a workspace file agent. Keep the Linux application working throughout the port. A phone connected to a desktop server is an optional later mode, not a prerequisite for local operation.

## 1. Product and architecture decisions

Build a native Android interface in **Kotlin with Jetpack Compose and Material 3**, backed by the existing Rust engine, agent, and application controller through a small JNI bridge. Compose is Android's recommended native UI toolkit; it provides a practical route to Android text input, accessibility, navigation, and adaptive layouts. [Android Compose documentation](https://developer.android.com/compose)

The existing `egui` application is a Linux frontend, not a portable Android application as currently configured. Reuse its behavior and the underlying Rust libraries. Implement Android screens and platform integrations separately. This recommendation is a project design choice, not a claim that an egui Android frontend is impossible.

### First Android release

- Installable ARM64 APK, initially distributed for direct installation; produce an AAB if Play distribution is selected.
- Local, offline chat after the user imports the approved GGUF model.
- One loaded model and one active turn, matching the current runtime constraint.
- Streaming content, collapsible reasoning, Stop, conversation history, and recoverable errors.
- Workspace import, file listing/search/read, and approved file edits in an app-owned working copy.
- Visible tool calls, arguments, results, approval decisions, and interrupted-call outcomes.
- Model/settings/diagnostics screens, including selected versus loaded loop settings.
- Phone layouts, rotation, Android back navigation, soft keyboard support, and TalkBack usability.

### Explicit first-release limits

- Local Agent mode is a **file agent**: arbitrary shell commands, project builds, and test execution are unavailable on the phone initially. Say this in the UI and model prompt.
- No dependency on root, Termux, a desktop, CUDA, or a downloaded executable toolchain.
- CPU inference is the correctness baseline. Vulkan acceleration is a separate measured enhancement.
- Do not assume every ARM64 phone can run the pinned model. Publish a tested support envelope after the hardware spike.
- Active inference is foreground-only initially. Moving the app out of the foreground requests cancellation; returning after rotation or navigation within the app preserves the active session.
- Web search is optional and off by default. Add it after local chat and file-agent behavior work.

If full desktop command execution is a requirement for the first release, add the companion-service work described in section 8 to the required scope before implementation starts. Do not label local file edits as full desktop-agent parity.

## 2. What the repository already provides

| Existing area | Reuse | Android work needed |
| --- | --- | --- |
| `crates/rillwright-engine` | Model ownership, generation worker, cancellation, chat rendering/parsing, recurrence loops, prefill logic | Android native build, CPU/device profiles, memory handling, Android logging |
| `crates/rillwright-sys/build.rs` | Bindgen and llama API boundary | Replace fixed host `build/bin` lookup with target-aware library discovery; use NDK headers/sysroot for Android bindings |
| `crates/rillwright-agent` | Agent loop, policies, compaction, built-in file tools, transcripts, scripted backend | Select tools by platform capability; isolate command/process/sandbox code; adapt prompt and secret access |
| `crates/rillwright-app` | Single-owner controller, state machine, settings acknowledgments, approval channel, typed events | Inject storage/platform services; add stable bridge DTOs and snapshots; remove desktop assumptions from mobile paths |
| `crates/rillwright-gui` | Screen behavior, tool-card behavior, settings/history interaction as references | New Compose UI; do not link its X11/Wayland/file-portal dependencies into Android |
| `crates/rillwright-serve` | Optional remote inference endpoint | Not a complete remote agent service: it returns tool calls and does not execute workspace tools |
| Existing acceptance scripts/tests | Renderer, generation, policy, tool and session regression cases | Add Android build/instrumentation/device tests; retain Linux coverage |

Important constraints found in the code:

1. `verify.rs` pins fork commit `1c3cdd23787750814aa21a070dc4d6dc451af871` and a specific model SHA-256. Preserve these checks; a generic newer llama.cpp build is not a drop-in replacement.
2. Desktop `LoadConfig` defaults to 32K context, batch 2048, micro-batch 512, eight threads, and all GPU layers. These are unsuitable as unmeasured mobile defaults.
3. `rillwright-sys/build.rs` expects a prebuilt host `libllama.so`; engine/frontend build scripts add a host library rpath. Android needs its own target artifacts and packaging.
4. `ControllerHandle` already has direct Stop and approval-answer paths. Preserve these rather than putting cancellation behind a busy worker queue.
5. `AppEvent` and configuration types are Rust types, not yet a versioned mobile wire contract. Some contain `PathBuf`, `SystemTime`, and nested engine types.
6. The controller currently registers `RunCommand`, constructs Linux sandbox settings, and resolves chat storage from desktop environment conventions. These require actual runtime changes, not only hidden Android controls.
7. Agent workspaces use real filesystem paths. Android document-provider URIs cannot be passed to `Workspace::new` as ordinary paths.
8. `prompt.rs` currently advertises shell commands and instructs the agent to run tests. The Android capability profile must change that text.
9. Agent sessions use `.dory/sessions/` and a writer-lock file. Preserve compatible transcripts and address lock recovery after Android process death.

## 3. Proposed source layout

Create these paths during implementation; they do not exist merely because this plan lists them:

```text
android/
  settings.gradle.kts
  build.gradle.kts
  gradle/libs.versions.toml
  gradlew, gradlew.bat, gradle/wrapper/...
  app/
    build.gradle.kts
    src/main/AndroidManifest.xml
    src/main/java/<chosen/package>/
      MainActivity.kt
      runtime/NativeBridge.kt
      runtime/RuntimeRepository.kt
      runtime/RuntimeOwner.kt
      storage/ModelRepository.kt
      storage/WorkspaceRepository.kt
      ui/conversation/...
      ui/history/...
      ui/models/...
      ui/settings/...
      ui/diagnostics/...
    src/test/...
    src/androidTest/...
crates/rillwright-android/
  Cargo.toml
  src/lib.rs
  src/bridge.rs
  src/dto.rs
scripts/build_android_native.sh
scripts/check_android.sh
docs/ANDROID.md
docs/ANDROID_DEVICE_RESULTS.md
```

Keep the initial Gradle project small: one Android application module plus a Rust `cdylib` bridge crate. Extract more modules only when a concrete build or ownership need appears. Generated JNI libraries belong in build output, not committed source directories.

Runtime flow: Compose screens → ViewModels → runtime repository → JNI bridge → application controller → engine/agent → pinned llama.cpp fork. Android repositories provide paths, document access, and secrets; Compose renders state and sends actions.

## 4. Ordered implementation steps

### Step 1 — Record a baseline and choose the device target

1. Read `README.md`, `docs/GUI.md`, `docs/GUI_DECISIONS.md`, and the current engine/app/agent interfaces before changing code.
2. Record the Rust toolchain, fork commit, model hash, dependency lockfile state, existing tests, and known failures. Preserve unrelated working-tree files.
3. Pick at least one physical ARM64 reference phone with enough storage for the model, import staging, and a workspace copy. Record Android version, SoC, RAM, free space, and page size.
4. Use Android 10/API 29 as a **provisional** minimum SDK. Raise it if measured runtime needs justify it. Set compile/target SDK to the current supported versions selected at implementation time, checking distribution requirements then.
5. Pin a compatible JDK, Gradle wrapper, Android Gradle Plugin, Kotlin/Compose toolchain, NDK, CMake, Rust toolchain, and bridge dependencies. Store versions in source control; do not rely on each developer's latest installed versions.
6. Define release acceptance thresholds before tuning: startup/load time, first-token latency, cancellation latency, sustained throughput, peak memory, battery drain, and UI responsiveness. Record numerical targets with the reference device results rather than inventing universal phone requirements.

**Deliverable:** a baseline section in `docs/ANDROID_DEVICE_RESULTS.md` and pinned toolchain choices.

**Exit criterion:** desktop baseline is reproducible and the reference device and performance acceptance targets are explicit.

### Step 2 — Prove the pinned model can run on Android

This is the highest-risk gate. Complete it before building the full interface.

1. Build the exact pinned llama.cpp fork for Android ARM64 using the NDK CMake toolchain. Start with CPU support and disable CUDA, desktop integrations, and optional backends that are not needed.
2. Add the smallest temporary Android test harness that can load a packaged native library, import a model file, run one prompt, and report results. It may later become part of the final app; avoid implementing the full bridge here.
3. Import the existing pinned GGUF without changing the checksum requirement. The repository comments describe a roughly 3.6 GB model, so account for source, temporary copy, final copy, and runtime allocations separately.
4. Try a conservative candidate profile: CPU, 2K context, batch 128, micro-batch 32, and a small measured thread count. These are spike inputs, not established compatible settings. Retain the model's loop/cache/prefill semantics, with two loops as the initial comparison point.
5. Record model verification time, load time, first-token latency, prefill/decode speed, peak process memory, and behavior over several minutes. Check for low-memory kills and thermal slowdown.
6. Run known prompts and inspect Dory recurrence behavior and tool syntax. Tokenization/rendered prompt equivalence should remain testable; CPU ARM and desktop GPU floating-point results need not be bit-identical.
7. If the fork needs Android patches, keep them small and reviewable, record a separate source/patch identity, and update verification deliberately. Never simply turn verification off.
8. Stop at a written decision: proceed locally on a measured device class, reduce the supported profile, or investigate another model/remote mode as a separately scoped product change. A smaller quantization needs a new approved hash and validation; it is not interchangeable with the pinned file.

**Deliverable:** an APK that produces a real local response and a hardware feasibility report.

**Exit criterion:** the pinned runtime generates on a physical phone within the chosen acceptance targets, or the blocking condition and required product decision are documented before further investment.

### Step 3 — Make platform dependencies explicit in Rust

1. Add an injected platform/storage configuration to `rillwright-app`: application data root, model root, chat session root, workspace registry, platform capabilities, and secret-provider access.
2. Keep existing environment/XDG behavior as the Linux default. Android passes absolute directories from its application context; it should not simulate a shell environment or use process-wide `chdir`.
3. Introduce a capability profile that controls registration and execution of file writes, commands, external executable tools, and web search. Enforce it below the UI.
4. Isolate desktop command and `bwrap` integrations with appropriate modules/features/platform gates. Audit `process.rs`, `tools.rs`, `builtin/command.rs`, sandbox probes, and controller imports/registration. Android is Unix-like, so a generic `cfg(unix)` is insufficient to identify desktop support.
5. Update sandbox/status reporting so Android reports app isolation and workspace restrictions accurately. The Android app sandbox does not prevent native code in the same app from accessing that app's other private data.
6. Build agent prompts from available capabilities. On Android, explain that the agent can inspect and edit workspace files and cannot execute tests locally. Keep desktop prompt behavior covered by existing snapshots.
7. Preserve permission enforcement, output limits, traversal checks, protected-file rules, and no-replay behavior. Mobile does not justify bypassing them.

**Deliverable:** shared runtime usable with either a Linux platform profile or an Android profile.

**Exit criterion:** Linux behavior remains intact and Android cannot register or execute unavailable command tools, even through a direct bridge request or malformed model call.

### Step 4 — Establish a reproducible cross-compilation and packaging pipeline

1. Add `rillwright-android` to the Cargo workspace as a JNI-facing `cdylib`. Build only the relevant packages for Android, excluding CLI/server/Linux GUI binaries.
2. Add the Rust `aarch64-linux-android` target and configure its linker from the pinned NDK, using an Android API level compatible with the minimum SDK.
3. Refactor `rillwright-sys/build.rs` to distinguish source headers from target library output. A proposed `RILLWRIGHT_LLAMA_LIB_DIR` override should identify the Android build output without changing the source pin.
4. Feed bindgen the Android target triple, NDK sysroot, and correct include paths. The bindings generator runs on the host while the layouts and native code must match the target.
5. Avoid Android rpaths pointing at a developer's Linux filesystem. Audit dependent build scripts as well as `rillwright-sys`.
6. Choose one documented native linking layout. Prefer statically linking pinned llama/ggml archives into the Rust shared library if the fork builds cleanly with PIC; otherwise package the bridge and every required Android shared-library dependency together. Resolve C++ runtime linkage consistently.
7. Make the Gradle build depend on a cacheable native build task with declared source/toolchain inputs and ABI-specific outputs. Fail rather than accidentally copying stale host `.so` files.
8. Inspect ELF architecture and dependency lists. Exclude Linux/glibc, CUDA, host paths, and unsupported CPU assumptions from the Android package.
9. Build all shipped native libraries for 16 KB page compatibility and verify both ELF segment alignment and APK packaging alignment. Exercise a 16 KB environment rather than relying only on a linker flag. [Android page-size guidance](https://developer.android.com/guide/practices/page-sizes)
10. Add an x86_64 native variant only if needed for emulator tests and supported by the fork. Otherwise use a scripted/mock runtime for emulator UI tests and run real JNI/model tests on ARM64.

Use the NDK's documented CMake toolchain and explicit ABI/API inputs. [NDK CMake documentation](https://developer.android.com/ndk/guides/cmake)

**Deliverable:** a clean-checkout build script and Gradle task that package the correct native artifacts.

**Exit criterion:** the APK installs and loads the JNI library on ARM64 without developer-specific environment variables or host library paths.

### Step 5 — Define and implement the JNI contract

1. Create serializable bridge DTOs instead of exposing Rust object layout. Use a versioned command/event envelope with `schema_version`, runtime generation, session ID, turn ID, and event sequence where applicable.
2. Provide create/destroy runtime, load/unload model, configure, create/open/list sessions, submit, cancel, answer approval, inspect prompt, and obtain current snapshot operations. Extend the controller where the intended operation is not yet exposed.
3. Keep the first wire format simple, such as UTF-8 JSON strings in batched JNI messages. Profile before replacing it with a more complex binary protocol.
4. Own the controller and model from a runtime handle registry. Validate handles, prevent use-after-close, make close idempotent, and reject calls from stale runtime generations.
5. Map `PathBuf` and `SystemTime` to explicit DTO fields. Kotlin should use stable workspace/model IDs for user actions; resolve and validate native paths internally rather than accepting arbitrary tool paths from the UI.
6. Run a dedicated event pump off the UI thread. Batch frequent text deltas without losing ordering, terminal events, tool-call boundaries, or approval requests. Keep bounded buffers and define overflow/recovery behavior.
7. Keep consuming controller events while screens change; otherwise its bounded event queue can stall inference. Use durable transcripts plus current snapshots for reattachment rather than an unbounded Kotlin event history.
8. Preserve direct cancellation and approval channels. Neither should wait behind a generation or a blocking event read. Reject duplicate, stale, or wrong-session approval answers.
9. Catch recoverable Rust panics at the FFI boundary and return structured errors; never unwind across JNI. Treat native aborts and low-memory termination as process failures requiring restart recovery.
10. Manage JNI references and thread attachment explicitly, avoid holding an Activity in native code, and avoid UI callbacks from a native worker. Prefer Kotlin-driven batched event reads on a worker dispatcher. [Android JNI guidance](https://developer.android.com/ndk/guides/jni-tips)

**Deliverable:** documented bridge schemas, implementation, and contract fixtures for Kotlin and Rust.

**Exit criterion:** load/submit/stream/stop/approval/shutdown work repeatedly, and invalid handles/events produce controlled errors rather than crashes or cross-session actions.

### Step 6 — Build the Compose application with a scripted runtime first

1. Create the Gradle application, choose a stable application ID, add launcher/adaptive icons using the existing brand assets, and configure debug/release variants.
2. Build conversation, history, workspace, model, settings, and diagnostics destinations. Use a navigation bar on compact screens and a rail or split layout where space permits.
3. Implement immutable UI state in ViewModels and collect it with lifecycle-aware APIs. Keep native runtime ownership outside individual composables and Activity instances. [ViewModel documentation](https://developer.android.com/topic/libraries/architecture/viewmodel)
4. Add a fake repository that emits the real bridge schema: loading, streamed content/reasoning, tool calls, approval, cancellation, settings acknowledgment, and errors. Reuse scripted-agent fixtures where practical.
5. Implement the composer with multiline input, IME/inset handling, Send and Stop states, draft preservation, selection/copy, and code blocks that can be read on a narrow screen.
6. Use a lazy transcript with stable item IDs. Auto-scroll only while the user is following the bottom; show a new-content affordance when they have scrolled upward.
7. Preserve conversation and drafts when opening settings/diagnostics. Treat navigation as navigation, not runtime teardown.
8. Cover loading, no model, missing workspace, unsupported device, failed load, empty history, and interrupted-session states from the beginning.
9. Add semantic labels and readable tool statuses. Check dark/light themes, large font sizes, contrast, hardware keyboard actions, and touch target sizes.

**Deliverable:** an installable UI that exercises complete scripted flows without a multi-gigabyte model.

**Exit criterion:** a user can navigate every main flow and retain session state across rotation without native inference being required for routine UI development.

### Step 7 — Implement model import and workspace storage

1. Use Android's Storage Access Framework for selecting a model and importing/exporting project files. Treat returned `content://` URIs as document handles, not filesystem paths. Persist URI grants where supported and handle revocation. [Android document access](https://developer.android.com/training/data-storage/shared/documents-files)
2. Stream the chosen model into an app-owned temporary file on a background dispatcher. Check available space, bound input size, show progress, support cancellation, and handle unknown provider-reported lengths.
3. Compute SHA-256 while copying, compare with the approved model pin, then atomically promote the verified file. Reuse the engine's verification guarantees; avoid accidentally hashing the entire multi-gigabyte file twice without a deliberate policy.
4. Load from a real app-owned path so the current engine can use native file access and mmap. Direct URI/file-descriptor loading is a later optimization requiring engine API and descriptor-lifetime work.
5. Maintain a model registry with hash, size, source display name, import status, and compatibility metadata. Exclude GGUF files and temporary imports from automatic cloud backup.
6. Create app-private workspace roots such as `filesDir/workspaces/<id>/`. Keep each workspace's `.dory/sessions/` compatibility layout there; keep chat sessions, settings, and secrets outside tool-accessible roots.
7. Import a user-selected directory tree or project archive into a private working copy. Preserve relative paths, show limits, reject traversal and unsafe archive/symlink entries, and handle binary/large files intentionally.
8. Maintain an import manifest and original-content hashes. Clearly label that agent edits affect the working copy until exported.
9. Export changes only through an explicit review flow showing changed files and diffs. Check source hashes again, surface conflicts, and never silently overwrite external edits. Document-provider writes may lack atomic rename; use provider-supported operations or export a separate copy when safe replacement is unavailable.
10. Offer transcript/project export through document creation or a content-URI share flow. Handle permission revocation, cloud-provider failures, storage removal, and running out of space.

Do not request broad all-files access to make path-based desktop tools appear portable. A SAF mirror is an intentional first-release storage design; direct editing across arbitrary document providers would require a separate filesystem abstraction.

**Deliverable:** reliable model import and a clearly identified private workspace with explicit export.

**Exit criterion:** imported code can be read/edited locally, external originals remain untouched until export, and interrupted imports never appear as valid installed models/projects.

### Step 8 — Connect real local inference to the interface

1. Replace the fake runtime repository with the JNI-backed implementation while retaining the fake for tests and demos.
2. Add an Android-specific `LoadConfig` constructor/profile. Begin with the measured CPU configuration from step 2; never inherit the desktop all-GPU/32K defaults accidentally.
3. Show separate verifying, loading, ready, running, awaiting approval, cancelling, unloading, and failed states using the controller's actual acknowledgments.
4. Load only after a valid imported model is selected. On devices without a verified workspace, start in Chat rather than inventing a workspace from the process working directory.
5. Preserve loops 1–4, default two, the pinned cache mode, and prefill behavior. Distinguish model loops from agent rounds in the UI.
6. Show selected and loaded configuration separately. Reload explicitly when load-time settings change, keep conversation/draft state, and disable sending until the load/session acknowledgment arrives.
7. Preserve reasoning/content separation, UTF-8 chunk boundaries, finish reasons, token statistics, and error messages. Never show a successful completion merely because the JNI call returned.
8. Keep model loading/hash verification/inference off the Android main thread. Native model initialization is not currently interruptible; represent that honestly and perform teardown asynchronously after it returns.
9. Add recovery actions for a deleted model, bad checksum, native dependency mismatch, allocation failure, context overflow, and a runtime that must be recreated.

**Deliverable:** usable offline local chat on the reference phone.

**Exit criterion:** import → verify → load → send → stream → stop → new turn → reopen history works with the real model, with a responsive Android UI.

### Step 9 — Make sessions survive interruption and process death

1. Keep the existing JSONL transcript format as the canonical conversation record unless a versioned migration is demonstrably needed. Android indexes may cache metadata but must not become a conflicting second transcript.
2. Store conversation selection, drafts, workspace/model IDs, and settings durably. Use saved UI state only for small reconstruction data; do not put whole transcripts or native handles into saved-state bundles.
3. Extend persistence as needed so tool identity/status and approval outcomes shown on Android remain reconstructable. Preserve honest handling of older transcripts with incomplete status metadata.
4. Rebuild the UI from persisted transcripts and a current runtime snapshot after Activity recreation. A ViewModel survives configuration changes, not operating-system process death.
5. Audit `.writer.lock` ownership. Android may kill the process without Rust destructors running; implement process-death-safe locking or narrowly validated stale-lock recovery. Never break a lock based only on age or replay an unfinished tool call.
6. Record enough turn state to distinguish completed, cancelled, failed, and interrupted generations. Reopen unfinished tool calls as outcome unknown and pending approvals as expired/requiring a new turn as appropriate.
7. Add paging or bounded transcript presentation and result expansion so long sessions do not duplicate all text across Rust, JNI, and Kotlin memory.
8. Provide delete/export actions with clear scope and handle unreadable/full storage without losing an already saved transcript.

**Deliverable:** reliable history and restart recovery.

**Exit criterion:** force-stop/relaunch and actual background process recreation preserve saved messages, avoid permanent lockouts, and never execute an earlier tool call automatically.

### Step 10 — Add the mobile workspace file agent and approvals

1. Register only the existing list/search/read/write/edit tools that the Android capability profile supports. Keep unavailable tool definitions out of the model prompt and enforce capability checks at dispatch.
2. Use the same workspace confinement, secret-file policy, read-before-edit checks, atomic local writes, output truncation, and agent limits as the existing Rust tools.
3. Offer Read only, Ask, and Auto edit modes with accurate descriptions. Omit or disable desktop Full/`bwrap`/shell-network controls until an implementation gives them meaningful Android behavior.
4. Render tool cards with stable call IDs, arguments, result/error text, truncation labels, and Pending/Processing/Waiting/Completed/Failed/Not executed/Outcome unknown states.
5. Show approvals in a persistent in-app panel or sheet with workspace, relative target path, operation, and exact proposed content/diff when available. If the existing approval event lacks the necessary diff, extend it rather than approximating the change in Kotlin.
6. Bind approval to runtime, session, turn, and request IDs. Recomposition, rotation, double taps, or an old notification must not approve a different request.
7. Cancellation while waiting for approval must release the agent and invalidate that request. Do not grant permission because an Android system document picker granted file access; those are different decisions.
8. Keep private model/settings/key files outside the workspace and ensure symlinks/path traversal cannot expose them through file tools.
9. Validate with small fixture projects: explain a file, locate a symbol, propose a replacement, refuse a write, approve an edit, attempt traversal, hit output/context limits, and recover from malformed/repeated calls.
10. When the user asks the local agent to run tests, report that execution is unavailable and provide commands for external use. Do not fabricate build/test success.

**Deliverable:** an end-to-end local file-agent workflow with visible permissions and explicit export.

**Exit criterion:** a real request can inspect and modify a fixture project after approval, while denied writes and all local command-execution attempts remain blocked.

### Step 11 — Define Android lifecycle and resource ownership

1. Keep one runtime owner at application scope, with a repository consumed by ViewModels. Activity/composable creation must not create duplicate model instances.
2. Distinguish configuration changes from leaving the application. Rotation must retain a turn; moving to another app or locking the device follows the foreground-only cancellation policy.
3. Apply lifecycle transitions outside the UI render loop. Ensure cancellation during a pending approval and during inference has a deterministic terminal outcome.
4. Keep the event pump and persistence active long enough to drain/capture the cancellation result. Release the model asynchronously when idle/background resource policy requires it.
5. React to memory pressure by shedding UI caches and unloading idle models. Lowering context or changing inference threads mid-turn must use supported engine operations or wait for a safe reload boundary.
6. Recover from process death from disk; do not rely on `onDestroy`, JNI cleanup, or a Rust destructor being called. [Android process lifecycle](https://developer.android.com/guide/components/activities/process-lifecycle)
7. Do not use WorkManager as an unlimited interactive inference loop. If background generation becomes required, design it as a separate feature after checking then-current foreground-service types, permissions, start restrictions, and time limits; include a visible Stop action.

**Deliverable:** a documented lifecycle state table and implemented cancellation/unload/recreation behavior.

**Exit criterion:** rotation, app switching, screen lock, back navigation, low-memory recovery, and process restart produce consistent visible states without duplicate generation or main-thread stalls.

### Step 12 — Finish settings, diagnostics, and optional web search

1. Persist Android preferences with an appropriate Android settings store. Pass validated values to Rust with revision IDs and preserve its applied/rejected acknowledgments.
2. Separate next-turn settings from reload-required settings. Include context/output limits, threads/profile, thinking, loops, agent round/time limits, and file permission mode; hide unsupported desktop settings.
3. Add diagnostics for app/native build IDs, fork/patch identity, model hash, ABI, active backend, loaded profile, device memory/page size, performance, and recent errors. Redact paths/secrets from exported logs by default.
4. Keep web search disabled in the initial local-only build if it is not yet implemented. When adding it, inject credentials through a secret provider rather than `TAVILY_API_KEY`/`BRAVE_SEARCH_API_KEY` environment variables.
5. Use Android Keystore-backed key protection for persisted provider credentials, keep ciphertext outside workspace/backup/export paths, and support delete/re-entry. Avoid storing keys in Gradle resources, ordinary preferences, transcripts, or diagnostics.
6. Reuse existing provider/budget/approval semantics where possible; test the Rust TLS/certificate path on Android rather than assuming a desktop TLS setup transfers unchanged.
7. Declare Internet access only in variants that need network features. Android Internet permission applies to the app, not a per-agent security boundary; enforce web capability and consent in the runtime too.
8. Cover offline, DNS/TLS failure, provider errors, quota exhaustion, cancellation, missing credentials, and opt-out. Use fake providers for routine tests.

**Deliverable:** usable settings/diagnostics and, if included, explicitly enabled web search.

**Exit criterion:** settings reflect acknowledged runtime state, and secrets remain inaccessible to workspace tools and routine exports.

### Step 13 — Measure performance and evaluate acceleration

1. Benchmark cold/warm verification/load, prefill, first token, decode, cancellation, session reopening, and tool latency on the reference phone.
2. Measure peak memory for model + KV/cache + compute buffers + Rust/Kotlin transcript state. A model fitting in storage or mapping successfully does not establish that generation fits in memory.
3. Test short/long prompts, two or more consecutive turns, tool rounds, compaction, and loops 1–4. Include sustained runs to reveal thermal throttling and battery cost.
4. Tune context, batch, micro-batch, threads, and output caps against the recorded targets. Keep profiles visible and recoverable; do not silently change recurrence behavior or rewrite a user's selected settings.
5. Batch UI updates and avoid full-transcript copies for every token. Retain complete canonical data on disk while bounding previews and in-memory presentation.
6. Investigate Vulkan only after CPU correctness is established and only if the pinned fork supports the required Dory operations on that backend. Compare output behavior, stability, speed, memory, and power on actual target GPUs.
7. Treat NNAPI/other accelerators as separate research requiring backend support, not a configuration checkbox. Keep a tested CPU fallback.
8. Publish device/profile results and a supported-device policy. If the pinned model is impractical on the intended phone class, return to the step-2 product gate rather than masking failures with endless tuning.

**Deliverable:** measured device profiles and an explicit acceleration decision.

**Exit criterion:** the chosen release profile meets the acceptance targets from step 1 across repeated and sustained runs.

### Step 14 — Add verification and continuous integration

1. Run shared Rust formatting/checks/tests on Linux with the required pinned native build; keep existing renderer/agent/session regressions passing.
2. Add focused Rust tests for injected directories, platform capability enforcement, prompt selection, bridge schemas, stale handles, settings revisions, approval IDs, cancellation, and interrupted-session recovery.
3. Add JVM tests for UI reducers/repositories and bridge decoding. Use a scripted runtime so routine tests do not need the pinned GGUF or native GPU setup.
4. Add Compose/instrumentation tests for onboarding/import states, conversation navigation, streaming/Stop, approvals, tool cards, history, settings, and rotation.
5. Add real ARM64 device smoke tests for native loading, model verification/generation, cancellation, workspace edits, and process restart. Do not equate an emulator mock pass with model compatibility.
6. Test 4 KB and 16 KB page environments, the chosen minimum SDK and a current Android release, low-storage/error cases, revoked URI grants, external edit conflicts, very long histories, and malicious workspace paths/archives.
7. Perform manual TalkBack, large-font, soft/hardware keyboard, clipboard, dark-mode, and narrow/wide-screen checks. Record observations and remaining gaps.
8. Add CI jobs for host Rust checks, Kotlin tests/lint, native cross-build, APK assembly, native dependency/alignment inspection, and emulator UI tests. Keep large model/device runs opt-in or on a dedicated runner; do not download the model into every PR job.
9. Define `scripts/check_android.sh` to run the repeatable required checks and report which device-only checks remain. Example eventual commands, once the project exists:

   ```sh
   cargo fmt --all --check
   cargo test -p rillwright-app -p rillwright-agent
   ./scripts/build_android_native.sh --abi arm64-v8a
   ./android/gradlew -p android testDebugUnitTest lintDebug assembleDebug
   ./android/gradlew -p android connectedDebugAndroidTest
   ```

   Host Rust tests still need their native prerequisites. Adapt commands to the final bridge/test layout; none of these Android commands are claimed to work in the current repository.

**Deliverable:** reproducible checks and a completed device acceptance record.

**Exit criterion:** build/functional/device/accessibility criteria pass, and limitations are documented instead of inferred from compilation alone.

### Step 15 — Package, document, and release

1. Review application, Rust dependency, fork, and model licenses before distribution. Check attribution and model redistribution terms explicitly; do not assume the model can be bundled.
2. Keep the multi-gigabyte model external to the APK initially. Document approved import sources, expected hash, disk requirements, and what remains on-device. Add automatic download only as a separately tested resumable/verified flow.
3. Create release signing configuration using external secret storage; exclude signing keys/passwords from source and build logs. Keep native debug symbols for crash analysis.
4. Produce an ARM64 release APK for direct installation and an AAB if selected. Validate final native dependencies, ABI filters, symbol handling, and page-size compatibility on the actual release artifact.
5. Define backup exclusions and deletion behavior for models, workspaces, transcripts, and credentials. State that private working copies can be lost on uninstall and must be exported when the user needs an external copy.
6. If publishing to Play, verify current target SDK, native-code, privacy/data declarations, and distribution requirements at release time. Complete store disclosures based on actual enabled network features and telemetry.
7. Write `docs/ANDROID.md`: prerequisites, build/install, device support, model import, offline chat, workspace copy/export, approvals, shell limitations, storage usage, lifecycle behavior, diagnostics, and troubleshooting.
8. Update the README platform matrix and link the Android guide. Preserve Linux build/package instructions.
9. Test a fresh install, upgrade without loss of settings/history, failed model import, cancelled generation, exported project, and reinstall recovery from an intentional export. Archive release checksums and the exact app/native/model identities.

**Deliverable:** a signed installable artifact, build instructions, and an honest support matrix.

**Exit criterion:** someone other than the implementer can install the app, import the model/project, use Chat and the local file agent, and export edits using only the documentation.

## 5. Milestones and sequencing

1. **M0 — Feasibility:** steps 1–2. Real pinned-model generation on one phone; stop and resolve feasibility before committing to a complete local product.
2. **M1 — Shared runtime and bridge:** steps 3–5. Cross-built libraries, explicit platform capabilities, stable JNI flow, desktop regressions passing.
3. **M2 — Android chat alpha:** steps 6–9. Native interface, safe model/project import, real streaming chat, persistent history and restart recovery.
4. **M3 — Local file-agent beta:** steps 10–12. Tool visibility, approvals, edits/export, lifecycle handling, settings and diagnostics; web search remains optional.
5. **M4 — Release candidate:** steps 13–15. Measured profiles, device coverage, release packaging, and complete documentation.

The scripted UI in step 6 can be developed while the bridge is taking shape. Model import is required before real local inference; durable sessions and capability enforcement are required before calling workspace editing complete. Validate lifecycle behavior during development, not only at the end.

Use milestone acceptance gates rather than fixed calendar promises until M0 establishes build and hardware feasibility. Keep each step in reviewable changes; split runtime portability changes from UI features where possible.

## 6. First-release acceptance checklist

- [ ] Clean-checkout documented build produces an installable ARM64 APK.
- [ ] Linux CLI, server, and GUI regressions remain passing.
- [ ] All native libraries are Android artifacts with verified 16 KB compatibility.
- [ ] Model and fork/patch identity checks remain enforced.
- [ ] Approved model imports safely and generates offline on supported hardware.
- [ ] Chat streams, Stop works, and stale events cannot update a different turn.
- [ ] Model loops, agent rounds, selected settings, and loaded settings are distinct.
- [ ] History/drafts survive rotation and expected process-death recovery.
- [ ] Interrupted tool calls never replay; stale session locks do not permanently block use.
- [ ] Workspace imports are confined; source files change only through reviewed export.
- [ ] Read/search/edit tools and approvals work; unsupported commands are absent and blocked.
- [ ] Tool statuses, truncation, failures, and unknown outcomes are visible and honest.
- [ ] App switching/background cancellation and slow model-load shutdown behave as documented.
- [ ] Long histories, low memory, low storage, revoked grants, and export conflicts are handled.
- [ ] TalkBack, keyboard/IME, large text, dark mode, and phone/tablet layouts are checked.
- [ ] Optional web credentials and logs remain outside agent-accessible project data.
- [ ] Release artifact, device measurements, limitations, and install/use/export instructions are available.

## 7. Main risks and corresponding decisions

- **Pinned Dory fork does not build or run reliably on Android:** resolve in M0; preserve source identity and isolate required patches.
- **Model exceeds practical memory/thermal limits:** support a measured higher-memory device class, evaluate a separately approved model profile, or choose remote mode explicitly. Do not infer a RAM requirement solely from model size.
- **Desktop assumptions leak through shared code:** inject storage/capabilities and enforce them at tool registration and dispatch, not just in settings screens.
- **JNI/event buffering stalls generation:** bounded batched transport, a runtime-owned event pump, direct cancellation, and snapshot recovery.
- **Android kills the process mid-write/tool:** atomic local writes, durable transcripts, safe writer locks, and no automatic replay.
- **SAF export cannot atomically replace a project:** show conflicts and provider limits, preserve the original, and offer a separate exported copy.
- **Local app isolation is overstated:** distinguish Android UID isolation from workspace policy; native code and tools share the app process and do not gain a new `bwrap` boundary.

## 8. Optional later work

### Desktop companion mode for full coding workflows

1. Define separate local and remote execution modes and clearly identify the active machine/workspace in the UI.
2. Use `rillwright-serve` only for remote inference if that is sufficient. Full remote Agent mode needs a new authenticated application/agent service exposing sessions, tool events, approvals, cancellation, and reconnect snapshots.
3. Run file/command tools on the desktop with its existing policy/sandbox; approve desktop actions against the desktop workspace, not the phone's imported copy.
4. Add authenticated encrypted transport, explicit pairing, revocation, capability negotiation, server-side limits, and durable reconnect behavior. Do not expose the existing unauthenticated inference endpoint to the Internet as a shortcut.
5. Persist pending-operation identity and distinguish a disconnected client from a cancelled server turn; never retry an approval or tool action blindly.

### Additional enhancements

- A validated smaller-model/quantization catalog, each with provenance, checksum, runtime compatibility, and device measurements.
- Vulkan profiles for tested devices; CPU fallback remains available.
- Direct SAF-backed tools once a document-provider filesystem abstraction and conflict model exist.
- Background generation only after a compliant lifecycle/service design is tested.
- Native Git support, project synchronization, or constrained embedded interpreters as separate capabilities with explicit limits. Downloading executables into writable app storage is not a portable toolchain strategy; Android restricts that execution path. [Android executable-code restrictions](https://developer.android.com/about/versions/10/behavior-changes-10)

These enhancements should follow the local release gates unless one is deliberately selected as a first-release requirement.
