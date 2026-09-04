# Vivida automation commands

`vivida msg` carries Vivido's whole client surface plus Vivida's layout methods on one connection.
Every flag here comes from the shipped CLI. When something is unfamiliar, run
`vivida msg <command> --help` rather than reconstructing a wire request by hand — and
`vivida msg capabilities` rather than assuming a method exists.

## Client and targeting

```sh
vivida msg --target NAME <command>       # exact registered instance name
vivida msg --socket PATH <command>       # a filesystem path on Unix, a named pipe on Windows
vivida list --all --json                 # discover instances when nothing was inherited
```

Every shell created inside Vivida inherits `VIVIDO_SOCKET` and its own `VIVIDO_WINDOW_ID`, so an
agent in a pane normally needs neither flag. Window-targeted commands resolve `--window-id ID`,
then the inherited `VIVIDO_WINDOW_ID`, then the focused window — which is why an omitted target
silently means *the agent's own pane*.

The endpoint accepts connections only from the same operating-system user, and both ends verify the
peer's process owner in addition to the socket mode or pipe ACL. Capability material used by Vivid
presentation never appears in layout, inspection, or diagnostic replies.

## Output contract

Structured observations, waits, transcript metadata, capabilities, and subscription events print one
compact JSON object per line. Controls are silent on success. The exceptions:

| Command | Prints |
|---|---|
| `create-window` | the new numeric window ID, alone |
| `get-text` | exact text, with no added newline |
| `screenshot` | one absolute path; the full metadata only with `--json` |
| `capture` | screenshot JSON, always |
| `transcript --raw` | exact decoded bytes, no newline |

Errors go to standard error and exit nonzero. Stable codes: `unsupported_version`,
`invalid_request`, `invalid_params`, `duplicate_request_id`, `limit_exceeded`, `window_not_found`,
`no_focused_window`, `unsupported`, `invalid_state`, `timeout`, `sequence_gap`, `pty_closed`,
`resize_mismatch`, `focus_denied`, `regex_invalid`, `subscription_overflow`, `client_fault`.

## `layout`

```sh
vivida msg layout                      # --window-id identifies the caller in the result
```

The result carries `schema_version`, `active_workspace_id`, a top-level `caller`, and `workspaces`.

```
workspaces[]  workspace_id, workspace_index, label, working_directory, active,
              active_tab_id, closing, tabs[]
  tabs[]      tab_id, tab_index, title, custom_title, active, focused_pane_id,
              layout (the typed split tree), panes[]
    panes[]   pane_id, locator{workspace_name, tab_name, pane_id}, split_path,
              window_id, is_caller, title, working_directory, focused, os_focused,
              selected_in_host, visible, rect{x,y,width,height},
              neighbors{left,right,up,down}
```

`workspace_index` and `tab_index` are one-based **display positions** and change when things are
reordered; `workspace_id`, `tab_id`, and `pane_id` are stable IDs. `window_id` is the globally
unique public handle and the only thing to pass as `--window-id`. `split_path` is one-based from
the tab root. A `layout` node is `{"kind":"split","axis":…,"sizes":[…],"children":[…]}` or
`{"kind":"pane","pane_id":N,"window_id":N}`.

`scripts/panes.py` renders this as one line per pane.

**`list-windows` is not a substitute.** It is Vivido's flat response: no workspace, no tab, no split
ownership, and only the active tab reported visible.

## `resolve-pane`

```sh
vivida msg resolve-pane --path left
vivida msg resolve-pane --tab 2 --path down
vivida msg resolve-pane --workspace 2 --tab 1 --path right,down
vivida msg resolve-pane --workspace-name "Project A" --tab-name "Logs" --pane-id 3
vivida msg resolve-pane --from-pane-id 4 --path up
```

Selectors: `--workspace N` / `--workspace-id ID` / `--workspace-name NAME`, `--tab N` /
`--tab-id ID` / `--tab-name NAME`, `--pane-id ID`, `--from-pane-id ID`, `--from-window-id ID`,
`--path DIR[,DIR…]`.

With no workspace or tab selector, resolution is scoped to the caller's own workspace and tab, even
when that pane is hidden. Without a caller, an omitted workspace is accepted only when exactly one
exists — it never falls back to the active one. Names match case-insensitively and must be unique;
generated collisions get deterministic suffixes such as `pwsh (2)`.

A route starts at the caller when it belongs to the selected tab, otherwise at that tab's focused
pane. Each `left`, `right`, `up`, or `down` is **one navigation step**, choosing the nearest pane in
that direction by overlap, then distance, then stable pane ID. Nested targets need repeated or mixed
steps. The response records `scope`, `selector`, `source_pane_id`, every `step`, and the final
`target` with its `window_id`, `locator`, and `neighbors`.

## Layout mutation

```sh
vivida msg create-workspace --name "Project A" --working-directory /path/to/project
vivida msg create-tab --workspace-name "Project A" --name "Logs"
vivida msg split-pane --window-id 42 --axis horizontal
vivida msg activate-pane --window-id 42
vivida msg close-pane --window-id 42
vivida msg close-tab --workspace-name "Project A" --tab-name "Logs"
vivida msg close-workspace --workspace-name "Project A"
vivida msg rename-workspace --workspace-name "Project A" --name "Project Alpha"
vivida msg rename-tab --workspace-name "Project Alpha" --tab-name "Logs" --name "Server"
vivida msg reset-tab-title --workspace-name "Project Alpha" --tab-name "Server"
```

- `create-workspace` and `create-tab` take the full window options — `--command`, `--working-directory`,
  `--title`, `--class`, `--hold`, `--no-activate`, `--option`, `--vivid-target terminal|desktop` —
  plus `--name`. `create-tab` scopes with `--workspace-id`, `--workspace-name`, or
  `--from-window-id`.
- `create-window` remains compatible and creates a tab in the active workspace.
- `split-pane --axis` is `horizontal` or `vertical`.
- `close-tab`, `close-workspace`, `rename-*`, and `reset-tab-title` scope with
  `--workspace-id`/`--workspace-name`, `--tab-id`/`--tab-name`, or `--from-window-id`.
- Renaming a tab pins its title; `reset-tab-title` (or the UI's **Use Automatic Title**) resumes
  context-driven updates.
- Names are trimmed, bounded to 128 characters, and reject control characters. Workspace names are
  globally unique; tab names are unique within a workspace.

Creation replies carry the new workspace, tab, pane, and window IDs. A close reply acknowledges that
shutdown was *requested* — use `wait exit` or `subscribe` to observe termination. Rediscover with
`layout` after any structural change; IDs must be rediscovered after a Vivida restart.

Hosted `resize`, geometry, visibility, and level requests are applied through Vivida's layout and
top-level chrome. Hiding the active target selects a deterministic sibling when one exists.

> **`split-pane` panics in a debug build.** Two arguments claim `--window-id` (the pane target, and
> the embedded window options' `-w`), which trips a clap debug assertion: `Long option names must be
> unique for each argument`. Release builds parse it, the pane target wins, and the command works —
> but `--help` lists `--window-id` twice. Use a release build for `split-pane`.

## Focus and revealing

`activate-pane --window-id ID` selects and reveals a hosted pane **without** requesting foreground
focus. `focus --window-id ID` is the separate, explicit request for real OS activation; it succeeds
only after an actual focused event and otherwise returns `focus_denied` after two seconds. On
Windows the CLI makes a best-effort `AllowSetForegroundWindow` grant, and foreground-lock rules may
still deny it; on Wayland it uses `xdg_activation_v1` when available.

Explicitly targeted application and UI input operates on a background pane, so do not request focus
merely to send input or to capture an already-presented frame.

## Input

```sh
vivida msg typing 'cargo test' --window-id 42 --report
vivida msg key Enter --window-id 42
vivida msg key c --mods Ctrl --window-id 42
vivida msg key ArrowDown --repeat 4 --window-id 42
vivida msg paste "$(cat notes.txt)" --window-id 42
vivida msg signal INT --window-id 42
```

- `typing` writes literal UTF-8, up to 1 MiB, with no paste handling and no appended Enter. Success
  arrives only after every byte reached the PTY master, with a five-second write timeout.
- `key` accepts one Unicode scalar or a named key: `Enter`, `Escape`, `Tab`, `Backspace`, arrows,
  `Home`/`End`, `Insert`/`Delete`, `PageUp`/`PageDown`, `F1`–`F35`, `Keypad0`–`Keypad9`,
  `KeypadDecimal`, `KeypadDivide`, `KeypadMultiply`, `KeypadSubtract`, `KeypadAdd`, `KeypadEnter`,
  `KeypadEqual`. Modifiers are `Ctrl`, `Alt`, `Shift`, `Super`, comma-separated; `--repeat` is
  1–1000.
- `paste` accepts at most 1 MiB with bracketed-paste filtering and newline normalisation.
- `signal` sends exactly the named signal — `INT`, `TERM`, `HUP`, `QUIT`, `TSTP`, `CONT`, `WINCH`,
  `KILL`, `STOP` — to the foreground process group. `KILL` and `STOP` have no implicit aliases.

`--report` on `typing`, `key`, and `paste` prints the resolved window, encoded byte count, input
sequence, and PTY-write completion, and states that application consumption was **not** observed.

`--route application` (the default) bypasses Vivido's bindings, search, hints, selection, and
clipboard actions while honouring the terminal's cursor, keypad, bracketed-paste, Kitty keyboard,
and mouse modes. `--route ui` runs through the normal input processor for Vivido's own bindings and
local UI behaviour; its modifier state is scoped to the request, so it cannot leave a modifier stuck.

## Mouse

```sh
vivida msg mouse move --x 320 --y 180 --route ui --window-id 42
vivida msg mouse click --cell-column 12 --cell-row 4 --button left --window-id 42
vivida msg mouse click --relative-x 0.5 --relative-y 0.5 --button left --window-id 42
vivida msg mouse scroll --x 320 --y 180 --vertical -3 --route ui --window-id 42
vivida msg mouse path --point 100,100 110,105 120,115 \
  --button left --route application --duration 250ms --wait-frame --window-id 42
```

Actions are `move`, `click`, `double-click`, `down`, `up`, `drag`, `path`, `scroll`. A position
carries exactly one of a zero-based cell pair (`--cell-column`, `--cell-row`), a physical-pixel pair
(`--x`, `--y`), or a relative pair (`--relative-x`, `--relative-y`, each 0–1, mapped atomically to
the current client area).

`mouse path` is one bounded press/move/release gesture of 2–1,000 points in a single request, taking
**physical pixels only** — no cell or relative form. Prefer it over one invocation per point.
`--duration` (1 ms–30 s) paces it, at most one paced gesture per window, bounded by its own deadline
so it fails with `timeout` rather than blocking. Vivido always releases the held button on
completion, failure, disconnect, cancellation, or window loss. `--wait-frame` delays success until a
newer frame.

Application routing requires active terminal mouse reporting and the live-bottom viewport. SGR pixel
mouse mode preserves exact physical coordinates; other modes resolve to cells. Coordinates do not
survive a resize, layout change, scale-factor change, font change, or content change.

## Waits

CLI default timeout is 30 s; values accept bare milliseconds or `ms`, `s`, `m`, `h`, from 1 ms to
24 hours.

```sh
vivida msg wait text 'ready>' --window-id 42 --after-screen "$screen"
vivida msg wait text 'completed in [0-9.]+s' --regex --window-id 42
vivida msg wait output 'panicked at' --after-offset "$offset" --window-id 42
vivida msg wait screen-change --after-screen "$screen" --window-id 42
vivida msg wait screen-stable --quiet 250ms --window-id 42
vivida msg wait frame --after-frame "$frame" --window-id 42
vivida msg wait exit --window-id 42 --timeout 10m
```

| Counter | Advances on |
|---|---|
| `screen_sequence` | physical rows, cursor, selection, dimensions, display offset, screen swap, terminal input modes — **not** cursor blink, visual bell, overlays, or Vivid media |
| `frame_sequence` | only after successful surface acquisition, rendering, and presentation |
| `output_offset` | retained sanitized PTY bytes; never resets |
| `event_sequence` | process-wide ordering of replayable automation events |

`wait output` without an offset matches only future bytes; an evicted explicit offset returns
`sequence_gap`. `wait screen-stable --after-screen` first requires at least one newer screen. Regex
patterns are capped at 8 KiB and matched in linear time. Disconnecting cancels waits, pending tagged
input, resize and focus requests, and subscriptions immediately.

## Reading a pane

```sh
vivida msg inspect --window-id 42
vivida msg get-text --window-id 42
vivida msg get-text --rows 200 --window-id 42
vivida msg get-grid --window-id 42
vivida msg get-grid --since-screen "$screen" --window-id 42
vivida msg get-grid --start-line -40 --row-count 40 --window-id 42
vivida msg transcript --after-offset "$offset" --max-bytes 65536 --window-id 42
```

`inspect` returns the window summary plus cell metrics, scale factor, scrollback size, display
offset, primary/alternate screen, terminal mode names, cursor, selection, shell PID, foreground
process group, executable basename, current directory, echo state, exit status, event sequence, and
effective limits. It never returns process arguments or environment values. Useful paths:

```
.window.sequences.screen   .window.sequences.frame   .window.sequences.output
.window.grid.columns       .window.pixels.width      .window.padding.x
.cell.width                .scale_factor             .current_directory
```

`get-text` excludes styling, cursor, media, search, and message overlays; `--rows` is 1–1000 and
reads the newest physical rows including scrollback.

`get-grid` returns positioned cells with text (combining characters included), width 0/1/2, a kind
of `character`/`continuation`/`leading_wide_spacer`, and a style ID, plus a deduplicated style table
with resolved RGBA colours, attributes, and hyperlinks, along with cursor, selection, wrap flags,
and mode names. `--start-line` and `--row-count` (1–1000) go together; `--since-screen` is mutually
exclusive with them. A delta coalesces intermediate states; scrollback older than the retained 1,024
screen changes, a resize or reflow, a screen swap, or a scroll-position change returns a full
viewport with gap metadata. Replies over 16 MiB fail with `limit_exceeded` and are never truncated.

`transcript` reads the 1 MiB sanitized byte-exact PTY ring with Vivid marker envelopes removed —
use it for transient output a text snapshot would miss.

## Screenshots

```sh
vivida msg screenshot --json --window-id 42
vivida msg capture --activate --stable --window-id 42
vivida msg capture --window-id 42 --after-frame "$frame" --stable --timeout 30s
```

Both return `window_id`, `frame_sequence`, physical `width`/`height`, `scale_factor`,
`cell:{width,height}`, `padding:{x,y}`, and the PNG `path`. `capture` adds `--activate` (which calls
Vivida's pane activation and requires `--window-id`), `--after-frame N`, and `--stable[=DURATION]`,
defaulting to 250 ms.

The PNG is the last successfully presented client-area frame at physical resolution — terminal
rendering, cursor, selection, overlays, and Vivid media, straight alpha preserved, OS decorations
and desktop content excluded. Mode `0600` on Unix, per-user temp directory on Windows; **the caller
owns cleanup.** One readback per window at a time, raw allocation capped at 256 MiB. A resize
invalidates the stored frame until another is presented.

`padding` is the origin of the grid inside the capture and cannot be derived from the other fields —
see SKILL.md, and `scripts/geometry.py`.

## Plans

`run-plan` reads JSON from stdin or `--file PATH` and emits compact NDJSON for the plan, each step,
and the final status, over one connection.

```json
{
  "version": 1,
  "steps": [
    {"id": "find", "method": "vivida_resolve_pane", "params": {"path": ["right"]},
     "bind": {"target": "/target/window_id"}},
    {"id": "reveal", "method": "vivida_activate_pane",
     "params": {"window_id": {"$ref": "target"}}},
    {"id": "click", "method": "mouse",
     "params": {"action": {"click": {"button": "left", "position": {
       "relative_x": 0.5, "relative_y": 0.5, "mods": [], "route": "ui",
       "target": {"window_id": {"$ref": "target"}}}}}},
     "verify": {"window_id": {"$ref": "target"}, "frame_changed": true,
                "screenshot": true, "timeout": 30000}}
  ]
}
```

1–256 bounded linear steps. `bind` maps a plan-local alias to a JSON Pointer into that step's result;
a later value consisting only of `{"$ref":"alias"}` substitutes it. `when` compares one alias against
an exact JSON `equals` value. `on_error` is `abort` by default, or `continue`. No loops, scripts,
persistent aliases, or forward references. `--dry-run` validates without executing; `--preflight`
runs observation methods only and reports mutations as skipped.

## Events

```sh
vivida msg subscribe --window-id 42 --events screen_changed,output
vivida msg subscribe --all --since-event "$sequence"
```

The handshake's `event_kinds` is also the `--events` allowlist, so take the list from `capabilities`
rather than prose. As of this build: `screen_changed`, `output`, `frame_presented`, `title_changed`,
`focus_changed`, `resized`, `moved`, `bell`, `child_exit`, `window_created`, `window_closed`,
`client_fault`, `client_recovered`, `overflow`.

Vivido's `docs/ipc.md` additionally documents `directory_changed` carrying `{"directory":"/path"}`
from OSC 7. The event loop emits it, but it is missing from the advertised kinds — and since that
list is also the filter allowlist, `--events directory_changed` is rejected with `invalid_params:
unknown event kind`. Watch the working directory through `layout` or `inspect` instead.

Up to 32 subscriptions per process, 256 queued events each; the replay ring is bounded by 4 MiB and
4,096 events. `--since-event` atomically replays retained matching events before live delivery; if
history is gone the first event is `overflow` with the gap, and the recovery is `layout` plus
`get-grid`. A slow client never blocks the UI or PTY thread. `--all` bypasses target resolution.

## Diagnostics

```sh
vivida msg diagnose --window-id 42 --trace-limit 128
vivida msg vivid trace --tail --limit 64
vivida msg vivid trace --after 40 --follow --recovery-only
vivida msg vivid sessions | vivid surfaces | vivid tracks | vivid scene-status --session-id S
```

`diagnose` captures window, renderer, presenter, track, flow, connection-health, and bounded recent
trace metadata in one event-loop turn; it does not wait for rendering or transport, so asynchronous
metrics carry an age, and its trace is the *newest* `trace_limit` events. `vivid trace` reads the
bounded metadata journal (4,096 events or 2 MiB) and never contains credentials, media bytes, or
frame hashes.
