# Next steps (temporary)

Working notes for the next session. Delete this file once the items below land.

## Status

- Done: three commits removed 13 tests and two files (`574b06b`, `74ec2ad`, `6dec329`).
  Suite went from 263 to 250 tests, 799 to 761 assertions, `alpha-build` runtime
  39.8s to 12.4s.
- Remaining: the four items below.

## 1. Continue trimming the test suite

Low-risk deletions/merges, coverage preserved:

- `crates/alpha-module-permission/tests/permission.rs`: merge the three mode sweeps
  (`read_mode_allows_reads_and_denies_mutations`,
  `auto_mode_allows_sandboxed_requests_without_a_channel`,
  `free_mode_allows_everything_without_a_channel`) into one table-driven test.
  -2 tests, same assertions.
- `crates/alpha-module-tools/tests/tools.rs::free_mode_shell_runs_outside_the_workspace`:
  duplicates the policy boundary already covered by
  `free_mode_writes_outside_the_workspace`. Delete. -1 test.
- `crates/alpha-front-tui/src/lib.rs::queued_steers_render_as_user_messages`:
  the semantics are covered by
  `crates/alpha-front-runtime/src/chat.rs::queued_steers_are_visible_until_they_settle`
  plus the TUI user-message rendering test. Delete. -1 test.

Larger refactor, separate commit:

- `crates/alpha-module-agent-host/tests/actor.rs` is 872 lines for 10 tests; roughly
  450 lines are ten copies of the same `create_session` scaffolding. Extract a
  session helper, a `wait_for_turn_end` helper, and a notification-collector helper.
  No test is removed, but the file should shrink by about 40%. While there, replace
  the `sleep(50ms)` + `assert_eq!(session_count(), 0)` eviction checks (lines 302,
  348, 490, 618) with `#[tokio::test(start_paused = true)]` and `tokio::time::advance`
  so the tests stop depending on wall-clock timing.

Rule for all of the above: delete rather than add, and keep an assertion only when
something else does not already fail without it.

## 2. Fix the status toast reporting errors as success

Symptom: the toast shows a green check, the title "Updated", and a remote error as
the subtitle, e.g. `remote error: invalid session request: cannot confirm steers
during an active turn`.

Cause:

- `crates/alpha-front-desktop/src/shell.rs:173`: `render_status_toast` hardcodes
  `IconName::CircleCheck` and the title `"Updated"` for every status.
- `crates/alpha-front-desktop/src/lib.rs:99`: `local_status: Option<String>` has no
  notion of success or failure, and about 39 call sites write both outcomes into it.

Fix:

- Store the outcome, not just the text: `local_status: Option<StatusMessage>` with
  `{ tone: StatusTone::{Success, Error}, text: String }`.
- Render tone-dependent icon and color (`CircleCheck` + `ACCENT`, `CircleX` + error
  color) and a title that matches the tone (`"Updated"` / `"Failed"`).
- Errors should stay longer than successes (see item 3).
- Give the truncated line a tooltip or wrap to two lines so the full remote error is
  readable.

Acceptance: submitting a steer during an active turn shows an error-toned toast with
the full message, and a successful action still shows the accent check.

## 3. Let the toast dismiss itself

Today the toast is a pure function of `local_status` and only disappears when some
later action clears the field, so stale text can stay on screen indefinitely.

- Add a generation counter plus a scheduled clear: on set, bump
  `status_generation`, capture it in `cx.spawn`/`window.request_animation_frame`
  based timer, and clear `local_status` only if the generation still matches.
- Successes auto-dismiss after about 3s, errors after about 8s or on click. Hover
  should not be required for dismissal to work.
- Clicking the toast dismisses it immediately.
- Keep the eviction idempotent: a new status resets the timer instead of racing it.

Acceptance: a status message disappears on its own and a rapid second status does not
get cleared by the first timer.

## 4. Paste images with plain Ctrl+V

Pasting is bound in the capture phase at
`crates/alpha-front-desktop/src/command_palette.rs:139`
(`handle_desktop_key`, registered through `.capture_key_down` at
`crates/alpha-front-desktop/src/shell.rs:302`) and already checks
`event.keystroke.modifiers.secondary()` with `key == "v"`.

Reported behaviour says Shift is still required. Reproduce first, then fix:

- Confirm which modifier `secondary()` maps to on the reporter's platform, and whether
  the desktop window binds a competing `ctrl-v` action that consumed the key before
  the capture handler.
- Check `cx.read_from_clipboard()` entry types for real clipboard sources: a
  screenshot tool may deliver `ClipboardEntry::String` or
  `ExternalPaths` rather than `Image`, in which case `attached` stays 0 and the paste
  falls through to the text area. Handle those cases (read the file behind the path,
  sniff the image format) instead of silently pasting nothing.
- If Shift+V is genuinely what works today, keep it as an alias rather than the only
  path.

Acceptance: copying a screenshot and pressing Ctrl+V with the composer focused
attaches the image and shows an "attached N images" status; Ctrl+Shift+V keeps
working; pasting plain text is unchanged.

## 5. Remove the steer confirmation buttons

The desktop queued-steer card (`crates/alpha-front-desktop/src/shell.rs:120-170`)
renders two actions next to the summary: `Discard` (`discard-steers`, line 137) and
`Run next` (`run-steers`, line 153), both calling
`FrontRuntime::confirm_steers` (`crates/alpha-front-runtime/src/lib.rs:573`).

`Run next` is unusable during the only situation in which the card is visible:

- `crates/alpha-module-agent-host/src/lib.rs:1096` rejects `accept: true` with
  `cannot confirm steers during an active turn`, which is the error in the toast
  described above.
- A steer submitted during a running turn already starts the next turn on its own
  (`running_steer_starts_the_next_turn_after_completion`,
  `crates/alpha-module-agent-host/tests/actor.rs:661`), so the button adds nothing.

Change:

- Drop the `Run next` button from the desktop card. Keep the card as a passive
  "Queued steer" indicator with the steer text.
- Keep `Discard` (the agent host accepts discards at any time) so a queued steer can
  still be cancelled, or drop it too if the queued indicator should be purely
  informational. Do not leave a disabled button in place.
- Keep the `/confirm` and `/discard` slash commands
  (`crates/alpha-front-runtime/src/command.rs:50-58`) as the explicit escape hatch;
  they are the only path for steers recovered after a restart
  (`recovered_steers`).
- The TUI renders queued steers as plain user messages and needs no change.

Acceptance: submitting a steer during a running turn shows it in the transcript and
runs automatically when the turn ends, with no confirmation button anywhere in the
desktop UI, and `/confirm`/`/discard` still work.

## Verification before each commit

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The desktop binary needs a display for manual checks; the toast and paste changes
should be eyeballed in a running `alpha desktop` session.
