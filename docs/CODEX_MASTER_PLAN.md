# VexTheater master plan

Consolidated and re-verified 2026-11-13 against commit `ab074b25e5b98d63e8e42c0af92ac93083f8873e`.

Sources: [architecture analysis](C:/Projects/VexTheater/CODEX_ANALYSIS_ARCH.md), [bug analysis](C:/Projects/VexTheater/CODEX_ANALYSIS_BUGS.md), and [feature analysis](C:/Projects/VexTheater/CODEX_ANALYSIS_FEATURES.md). All three requested filenames exist. Only this plan was authored; the reports and source code remain unchanged. Checkboxes represent future implementation, not work performed during this merge.

## Consolidation and ordering

There are **18 original IDs and 17 distinct work items**. `ARCH-02` and `BUG-07` are one persistence defect and share one checkbox. All other IDs remain individually accountable. Related work is grouped into small batches, with prerequisites stated explicitly. No item was dropped as unsupported.

Master priority is implementation order across all three reports: **high** for preventing loss of user state or recovery information, **medium** for other verified defects and bounded features, **low** for lower-return improvements. This does not relabel the original reports' medium-severity bugs as security vulnerabilities. No verified security vulnerability was reported or established in this merge.

Read the batches top to bottom: high → medium → low, then S → M → L within each tier. Each batch contains items of the same priority and effort class; effort is per item, not an estimate that two M items take the same time as one. Related work crossing effort classes stays in consecutive stages or has an explicit dependency. S = localized change; M = coordinated change across a few components; L = staged extraction. Bug fixes precede dependent features within a batch.

### Re-check of all originally high-priority items

The original high-priority items were features, not bugs. Their implementation gaps hold up; their urgency is reduced in this combined repair backlog. These are code-based prioritization decisions, not claims of measured user demand or physical-device reproduction.

| Original ID | Source → master priority | Re-verified evidence and decision |
| --- | --- | --- |
| FEAT-01 | high → medium | [window.rs:1212](C:/Projects/VexTheater/src/window.rs:1212), lines 1212–1264 and 1843–1862, already implements options/reset through Menu/F2/held keys; [README.md:132](C:/Projects/VexTheater/README.md:132) documents the CEC limitation and keyboard/pointer alternatives. Keep the alternate remote navigation path, but schedule after state-loss and existing-input fixes. |
| FEAT-02 | high → medium | [window.rs:450](C:/Projects/VexTheater/src/window.rs:450), lines 450–458 and 1755–1793, already serializes closes and replaces pending requests, including Home. Progress text and a visible cancel action improve discoverability; they do not introduce the missing serialization or cancellation semantics. Keep as a small enhancement beside the Home fix. |
| FEAT-03 | high → medium | [window.rs:237](C:/Projects/VexTheater/src/window.rs:237) calls the existing diagnostic report; [diagnose.rs:38](C:/Projects/VexTheater/src/diagnose.rs:38), lines 38–88 and 151–177, implements checks but blocks while polling helpers. Keep the on-screen health feature after the shared async fix, rather than treating diagnostics as wholly absent. |

### Re-check of items assigned high master priority

The following source-medium issues are raised to high **scheduling** priority to put loss prevention first. Each was reread in the current code before promotion.

| Original ID(s) | Verified path | Risk and limit of the evidence |
| --- | --- | --- |
| BUG-07 / ARCH-02 | [order.rs:26](C:/Projects/VexTheater/src/order.rs:26) returns `()` after a failed save; [window.rs:1470](C:/Projects/VexTheater/src/window.rs:1470) mutates memory first, and lines 1394–1404 finish the normal drop interaction. | A failed write/rename silently loses the user's new arrangement on restart. It does not establish corruption of an already saved layout. |
| BUG-05 | [launch.rs:411](C:/Projects/VexTheater/src/launch.rs:411) signals before setting local `termed`; [launch.rs:277](C:/Projects/VexTheater/src/launch.rs:277), lines 277–319, saves the resulting records only after the close tasks; lines 692–695 discard save failures. | A crash or failed save can leave `termed: false` durable and permit a second SIGTERM to the same surviving app. The session-loss consequence is conditional on that app's shutdown behavior, as documented in the lifecycle code. |
| BUG-03 | [gnome.rs:80](C:/Projects/VexTheater/src/gnome.rs:80), lines 80–98, skips snapshot creation whenever a file exists but still disables all managed keys. Lines 125–127 skip unsaved keys; the older-file fixture at lines 244–262 lacks `overlay-key`. | The supported older snapshot can leave the original Super-key value unrecorded and the key disabled after returning to desktop. |
| BUG-04 | [gnome.rs:121](C:/Projects/VexTheater/src/gnome.rs:121) calls `take()` before restoration; [gnome.rs:179](C:/Projects/VexTheater/src/gnome.rs:179) removes the snapshot immediately, including after an unsuccessful read. | Failure/interruption during restoration can destroy the only recovery copy. These are verified control-flow consequences, not newly reproduced crashes. |

### Deduplication boundaries

- `ARCH-02` + `BUG-07`: one implementation and regression-test effort; retain both IDs on its checkbox.
- `ARCH-03` + `FEAT-03`: share the asynchronous check runner, implemented once. Preserve two outcomes: responsive CLI diagnostics, then an on-screen health page.
- `BUG-03` + `BUG-04`: share GNOME snapshot handling, but cover different transitions—capture/migration versus restoration. Both require completion evidence.
- `BUG-01` + `FEAT-02`, and `BUG-06` + `FEAT-04`: related queue/desktop-app work, not duplicates. Finish the existing behavior correction before extending it.
- `ARCH-01` is not a prerequisite for all other work. Use small existing helpers for the fixes; do not turn this plan into a mandatory full UI rewrite.

## High priority — protect user state and recovery data

### Batch 1 — Report persistence failures consistently · S

- [x] **BUG-07 + ARCH-02 — Preserve truthful save status.** Return a useful error from order/visibility persistence, handle it at the UI boundary, and retain an explicit unsaved state with retry or revert to the last durable order. Preserve reset's rule of leaving state unchanged if deletion fails. Cover [order.rs:26](C:/Projects/VexTheater/src/order.rs:26), [tiles.rs:37](C:/Projects/VexTheater/src/tiles.rs:37), and [window.rs:1394](C:/Projects/VexTheater/src/window.rs:1394), lines 1394–1404, 1453–1475, 1491–1499, 2047–2050.

**Done when:** Injected save/rename failures reach the UI; a drop does not claim durable success; the documented restart behavior matches the saved file; add/remove/reset feedback follows the same contract. This one item closes both original IDs.

### Batch 2 — Make Flatpak termination intent recoverable · M

- [x] **BUG-05 — Persist termination intent before signaling.** Coordinate per-instance state updates across concurrent close tasks, propagate record-write failures, and preserve at-most-once SIGTERM behavior across restart. Use [launch.rs:269](C:/Projects/VexTheater/src/launch.rs:269), lines 269–319, 387–455, and 688–695. Keep the PID/start-time ownership checks and existing escalation limits.

**Done when:** Resume the lifecycle harness from the durable record captured around signaling and demonstrate no repeated SIGTERM. Test record-save failure and concurrent records without lost updates. Explicitly define the unavoidable crash ambiguity between recording intent and delivering the signal; do not claim exactly-once delivery from a file write plus `kill`. Existing completed-close tests must continue to pass.

### Batch 3 — Preserve GNOME snapshots through migration and restoration · M

- [x] **BUG-03 — Capture newly managed keys before changing them.** Validate an existing snapshot, keep its original values, merge missing current values such as `overlay-key`, and persist the complete snapshot before disabling newly covered settings. If saving fails, leave uncovered settings unchanged. Evidence: [gnome.rs:80](C:/Projects/VexTheater/src/gnome.rs:80), lines 80–98 and 244–262.
- [x] **BUG-04 — Remove recovery data only after successful restoration.** Read without deleting, apply/sync saved values, retain unresolved originals on failure, and preserve unreadable snapshots for diagnosis/retry. Evidence: [gnome.rs:121](C:/Projects/VexTheater/src/gnome.rs:121), lines 121–141 and 177–184; [mode.rs:28](C:/Projects/VexTheater/src/mode.rs:28), lines 28–36.

**Done when:** The old two-boolean fixture survives a full enter/restore cycle with the original Super key restored. Setter failure, missing schema, unreadable snapshot, and interrupted restoration retain recoverable originals. A successful retry restores all values and only then removes the snapshot. Implement the shared snapshot helpers once for both items.

## Medium priority — repair interactions and extend existing capabilities

### Batch 4 — Finish Home requests and expose the pending launch · S

- [x] **BUG-01 — Retry automatic reload after Home's close finishes.** Preserve the busy guard, then perform the deferred reload for the effective Home request and restore selection against rebuilt identities. Evidence: [window.rs:862](C:/Projects/VexTheater/src/window.rs:862), lines 862–879, 965–979, 1755–1793. Test changed config → Home → close completion with and without a running app.
- [x] **FEAT-02 — Show pending destination and Cancel launch.** Display an indeterminate closing status and the latest queued tile. Cancel replaces the pending launch with Home while cleanup continues normally. Clear status on every completion/error path. Reuse `Request`/`CloseQueue` at [window.rs:423](C:/Projects/VexTheater/src/window.rs:423), lines 423–463; do not add a second cancellation/process-stop mechanism.

**Done when:** A → B during one close displays and launches only B; cancel launches neither; returning Home applies changed config; unknown-scope and timeout paths leave no stale status. Depends on Batch 2's lifecycle guarantees, without repeating its persistence work.

### Batch 5 — Consume held Back through the page transition · S

- [x] **BUG-02 — Keep the reset confirmation open after the long press.** Suppress repeats of the same Back press until release even after the confirmation becomes visible, and reset held-key state appropriately on cancellation/focus loss. Evidence: [window.rs:724](C:/Projects/VexTheater/src/window.rs:724), lines 724–729, 793–813, 1212–1272, 2092–2098.

**Done when:** Press → timer → repeat → release leaves the confirmation open; a new Back press dismisses it. Compare with Enter's existing swallow handling rather than changing confirmation semantics.

### Batch 6 — Make tile management accessible to a basic remote · M

- [x] **FEAT-01 — Dropped by the owner, not implemented.** The existing tile menu (Move/Remove) is enough, no tile reset is wanted, and the Power menu stays power-only apart from Desktop Mode. Original text: Add a visible Manage tiles path. Offer main/submenu editing targets reachable with arrows/OK/Back, open existing Move/Remove options without launching the target, and expose reset through its existing cancel-first confirmation. Reuse [window.rs:1843](C:/Projects/VexTheater/src/window.rs:1843), lines 1843–1881 and 2245–2289, `options_for`, and the existing grid/persistence operations.

**Done when:** A remote using only ordinary presses can move/remove/restore main-page and submenu tiles, reach/cancel reset, and return to the correct focus. Depends on Batches 1 and 5. This adds an entry path; it does not replace ordering logic or require changes to the external CEC bridge.

### Batch 7 — Responsive diagnostics, then an on-screen health page · M

- [x] **ARCH-03 — Make helper checks asynchronous.** Keep CLI report/exit-status behavior, return control to the main loop while helpers run, and preserve explicit timeout/kill/reap behavior. Evidence: [diagnose.rs:151](C:/Projects/VexTheater/src/diagnose.rs:151), lines 151–177; [window.rs:237](C:/Projects/VexTheater/src/window.rs:237). Do not blindly copy the launch helper's different timeout cleanup policy.
- [x] **FEAT-03 — Render those same results as Setup & health.** Add refreshable session/config/browser/Home-shortcut/missing-app checks, plus Add and retry entry points from the header/empty state. Inspect shortcut state without silently rewriting it. Reuse `Check`/rendering logic at [diagnose.rs:14](C:/Projects/VexTheater/src/diagnose.rs:14), [webapp.rs:39](C:/Projects/VexTheater/src/webapp.rs:39), and [keybinding.rs:67](C:/Projects/VexTheater/src/keybinding.rs:67). Keep generated-config ownership intact.

**Done when:** A hanging helper leaves navigation and Back responsive; CLI and UI agree on underlying results; refresh resolves corrected failures; browser/shortcut/config problems have distinct actionable explanations. The async runner is shared work, not two separate refactors. ARCH-03 must land before the UI runs these checks.

### Batch 8 — Correct desktop arguments, then add installed apps · M

- [x] **BUG-06 — Expand desktop metadata field codes correctly.** Pass desktop metadata into argument construction; expand `%i`, `%c`, and `%k`, retain no-file handling of file/URL codes, and quote final arguments safely. Evidence: [launch.rs:115](C:/Projects/VexTheater/src/launch.rs:115), lines 115–124 and 840–900. Test names/paths with spaces and present/absent icons.
- [ ] **FEAT-04 — Add registered desktop applications through Add.** *(Deferred by the owner until after Batch 13, so the picker can reuse Browse all's filtering.)* Discover menu-visible desktop entries, persist a defaulted desktop-ID collection beside catalog additions, reuse desktop launch/icon/availability behavior, and focus or restore existing entries instead of duplicating their IDs. Evidence: [tiles.rs:18](C:/Projects/VexTheater/src/tiles.rs:18), lines 18–24, 99–116, 165–200; [window.rs:1951](C:/Projects/VexTheater/src/window.rs:1951), lines 1951–2032 and 2378–2391. Preserve old user-state files and the generated config schema.

**Done when:** BUG-06's argument tests pass before app-picker rollout. Added applications survive restart and support move/remove/restore; menu-hidden entries are excluded; an uninstalled app becomes unavailable. This is a picker, not a package installer. Depends on Batch 1 for reliable user-state persistence.

### Batch 9 — Portable layout export and validated restore · M

- [x] **FEAT-05 — Export, preview, and restore personalization.** Version a bundle of added/hidden tiles, per-page order, and source identity inventory; preview mismatches against current config; reject malformed/unsupported input before applying. Coordinate both state-file updates and preserve a recoverable previous snapshot on partial failure. Use [tiles.rs:18](C:/Projects/VexTheater/src/tiles.rs:18), [order.rs:10](C:/Projects/VexTheater/src/order.rs:10), [files.rs:52](C:/Projects/VexTheater/src/files.rs:52), and [window.rs:283](C:/Projects/VexTheater/src/window.rs:283).

**Done when:** Round-trip yields the same effective listing, including duplicate names and hidden tiles; changed configurations report unmatched identities; failure on the second save has a tested recovery path. Apply through the owning launcher or require it stopped. Exclude browser cookies/profiles, runtime process/mode records, GNOME recovery snapshots, and system config. Depends on Batch 1; account for Batch 8's desktop-ID state in the bundle version if that feature has landed. Individual atomic file writes are not a two-file transaction.

### Batch 10 — Activate existing tiles from local automation · M

- [x] **FEAT-06 — Add forwarded tile launch and a machine-readable listing.** Resolve page/name/occurrence selectors to visible app tiles and reuse click behavior: availability check, origin tracking, webapp preparation, and the same close queue. Evidence: [main.rs:47](C:/Projects/VexTheater/src/main.rs:47), [window.rs:198](C:/Projects/VexTheater/src/window.rs:198), lines 198–275 and 1648–1793; [order.rs:10](C:/Projects/VexTheater/src/order.rs:10). Reject stale/hidden selectors, submenu targets, and power actions.

**Done when:** Requests reach the running instance, rapid commands obey latest-request-wins, and invalid/missing apps leave the current app running. Success means acceptance, not guaranteed later launch success. Selectors are tied to the current identity convention and are not promised to survive renaming. Depends on Batches 2 and 4; adds no network listener or unrestricted command executor.

### Batch 11 — Extract only the remaining domain ownership from GTK · L

- [ ] **ARCH-01 — Stage tile-model and lifecycle-controller extraction.** First give CLI/UI a plain tile/page model for identities, visibility, and order; then have grids render model state; finally isolate running-app/queue transitions from notification/presentation. Evidence: [window.rs:483](C:/Projects/VexTheater/src/window.rs:483), lines 483–562, 896–961, 1453–1475, 1755–1808; [grid.rs:73](C:/Projects/VexTheater/src/grid.rs:73); [main.rs:105](C:/Projects/VexTheater/src/main.rs:105). Preserve JSON page keys, launch semantics, and observed focus behavior.

**Done when:** Headless listing and order mutations use the same plain model as the GUI; lifecycle transitions can be tested without GTK widgets; UI maps no longer define domain order. Split the three stages into independently reviewable changes under this original ID. Incorporate small helpers already introduced by earlier batches instead of replacing them wholesale. This is deferred maintenance, not a gate for the preceding fixes/features.

## Low priority — maintenance and larger-collection usability

### Batch 12 — Unify configuration validation knowledge · M

- [ ] **ARCH-04 — Produce structured field errors from shared validation.** Centralize action metadata and semantic checks so error formatting consumes their results rather than maintaining a second schema in `locate*`. Evidence: [config.rs:164](C:/Projects/VexTheater/src/config.rs:164), lines 164–168 and 223–346. Preserve strict acceptance rules, null handling, and precise field paths; no current schema disagreement is asserted.

**Done when:** Existing strict-schema tests still pass, and rule changes cannot require independent updates to two semantic validators. Keep the external configuration contract unchanged; a new schema feature would require separate coordinated work with the consumer.

### Batch 13 — Browse/filter the visible tile graph · M

- [ ] **FEAT-07 — Add Browse all with local metadata filtering.** Build results from visible, reachable app tiles, retain original page/identity and duplicate-name context, use a visited-page set for cycles, and support keyboard filtering plus remote alphabet groups. Evidence: [config.rs:187](C:/Projects/VexTheater/src/config.rs:187), [tiles.rs:123](C:/Projects/VexTheater/src/tiles.rs:123), [window.rs:2321](C:/Projects/VexTheater/src/window.rs:2321), and [grid.rs:29](C:/Projects/VexTheater/src/grid.rs:29).

**Done when:** Nested/cyclic submenus, hidden openers, duplicate names, missing apps, empty results, and Back/focus restoration behave correctly; filtering never mutates saved order or unhides tiles. Reuse Batch 11's model if available, but do not force the full extraction merely to ship this optional view. Search tile metadata, not remote streaming catalogs.

## Verification and completion rules

For implementation, each checkbox requires its stated regression/acceptance evidence plus the project's normal checks. GUI batches also need real remote/input and visual verification at the documented logical sizes. The existing 178-test suite is a baseline, not proof of the new behaviors or crash windows. Keep real GNOME/systemd/Flatpak/DRM checks distinct from the lifecycle harness's stub programs.

This merge re-read all originally high-priority paths and all issues raised to high master priority. It did not perform new physical-device or crash-injection experiments. The three source reports retain the detailed original evidence and coverage limits. Their recorded build/test/audit results describe the same unchanged source commit; no new vulnerability audit is claimed here.

Plan verification: all 18 source IDs accounted for; one exact duplicate merged; all implementation checkboxes initially unchecked; batches ordered by master priority and per-item effort; all 49 file references checked; source-report hashes and tracked-source diff checked.

Fresh verification on the unchanged source: `bash scripts/preflight.sh` completed with exit code 0 and `Preflight passed.` This includes formatting, Clippy with warnings denied, development tests, desktop-file validation, AppStream validation, and the Nix package build. Development tests reported `178 passed; 0 failed` in 5.45s. The package build reported `Finished release profile [optimized]` in 4m 55s; its release tests reported `178 passed; 0 failed` in 3.33s. Release-test output also contained a stub-program `cat: .../run.sh: No such file or directory` message despite the successful test result; no new defect is inferred from that output alone. This paragraph records the completed run; it does not change the tested source.
