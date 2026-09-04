# Vivida agent automation

Vivida starts one owner-only local automation endpoint on Windows, Linux, and macOS. Every shell
created inside the process inherits `VIVIDO_SOCKET` and its own `VIVIDO_WINDOW_ID`, so an agent can
use `vivida msg` without a separately installed `vivido` program.

## Agent loop

Discover capabilities once per live instance and reuse its public window IDs until a restart or
stale-ID error. `layout` reads the caller's inherited `VIVIDO_WINDOW_ID`, so its top-level `caller`
object identifies the agent's workspace, one-based workspace and tab positions, stable IDs, pane
ID, rectangle, host selection, and visibility. In PowerShell:

```powershell
vivida msg capabilities
vivida msg layout
```

On Linux or macOS:

```sh
vivida msg capabilities
vivida msg layout
```

For a complete multi-step interaction, prefer one `vivida msg run-plan --file plan.json` process.
Vivida inherits the standard plan runner and `capture` composite from Vivido, while advertising its
layout methods on the same connection. A plan can call `vivida_resolve_pane`, bind
`/target/window_id`, call `vivida_activate_pane`, then use ordinary screenshot, mouse, wait, and
verification steps without rediscovery or another client process. Use `--dry-run` for static
validation or `--preflight` to execute observation steps while skipping mutations.

Read the JSON layout and choose the target's globally unique `window_id`. Workspace, tab, and pane
IDs describe the hierarchy; local pane IDs are meaningful only together with their workspace and
tab IDs. Pass `--window-id` on every target operation because an agent pane continues to inherit
its own ID even while it is hidden. If endpoint inheritance is unavailable, use `vivida list
--all --json` rather than reading registry files directly.

Do not infer Vivida split relationships from `list-windows`: that standard Vivido response is flat
and does not contain workspace, tab, or split ownership. Every pane in `layout` includes its
one-based `split_path` from the tab root and topology-aware `left`, `right`, `up`, and `down`
neighbors. Resolve a directional route against the host model instead:

```sh
vivida msg resolve-pane --path left
vivida msg resolve-pane --path down
vivida msg resolve-pane --tab 2 --path down
vivida msg resolve-pane --workspace 2 --tab 1 --path right,down
vivida msg resolve-pane --workspace-name "Project A" --tab-name "Logs" --pane-id 3
```

Workspace and tab numbers are one-based displayed positions. With no workspace or tab selector,
the caller's own workspace and tab are used, including when that pane is currently hidden. Without
a caller, an omitted workspace is accepted only when exactly one exists. `--workspace-name` and
`--tab-name` match unique display names case-insensitively; `--pane-id` directly selects a stable
pane within that named scope. Every pane's `locator` object reports this canonical triple. A route
starts at the caller when it belongs to the selected tab, otherwise at that tab's focused pane;
`--from-pane-id` selects another explicit starting point. Each `left`, `right`, `up`, or `down`
step chooses the nearest pane in that direction using overlap, distance, and stable pane ID as
tie-breakers. A direction is one navigation step, so nested targets may require repeated or mixed
steps. The response records every step and the final `target.window_id`.

```sh
vivida msg inspect --window-id 42
vivida msg activate-pane --window-id 42
vivida msg typing "status" --window-id 42 --report
vivida msg key Enter --window-id 42 --report
vivida msg wait screen-stable --window-id 42 --quiet 250ms
```

Explicitly targeted application and UI input can operate on a background pane. `activate-pane`
selects and reveals a hosted pane without requesting foreground focus. Capture the frame sequence
and geometry with the screenshot, wait for a newer frame after acting, then verify once:

```sh
vivida msg screenshot --json --window-id 42
vivida msg mouse path --point 100,100 110,105 120,115 --route application --window-id 42
vivida msg wait frame --window-id 42 --after-frame 7
vivida msg screenshot --json --window-id 42
```

The equivalent activation-and-capture composite is:

```sh
vivida msg capture --activate --stable --window-id 42
```

`screenshot --json` returns the private PNG path, captured frame sequence, physical dimensions,
scale factor, and cell metrics. Open that file before choosing coordinates. `mouse path` sends one
bounded two-to-1,000 point press/move/release gesture and preserves exact physical pixels under
SGR pixel mouse mode. `focus` remains the explicit request for real OS activation; Windows
foreground-lock or compositor policy can deny it, and targeted background input does not need it.
Mouse positions also accept `--relative-x` and `--relative-y` fractions. Dense drawing or drag
gestures can use `mouse path --duration 250ms --wait-frame`; Vivido schedules the reports, keeps
the request correlated through PTY completion, and guarantees a final button release.

## Layout management

The standard `create-window` command creates a tab in the active workspace. Vivida also exposes
its complete workspace and split model:

```sh
vivida msg create-workspace --working-directory /path/to/project
vivida msg create-workspace --name "Project A" --working-directory /path/to/project
vivida msg activate-pane --window-id 42
vivida msg create-tab --workspace-name "Project A" --name "Logs"
vivida msg split-pane --window-id 42 --axis horizontal
vivida msg close-pane --window-id 42
vivida msg rename-workspace --workspace-name "Project A" --name "Project Alpha"
vivida msg rename-tab --workspace-name "Project Alpha" --tab-name "Logs" --name "Server"
vivida msg reset-tab-title --workspace-name "Project Alpha" --tab-name "Server"
vivida msg close-tab --workspace-name "Project Alpha" --tab-name "Server"
vivida msg close-workspace --workspace-name "Project Alpha"
```

`split-pane` takes `--window-id` for the pane being split and `--axis horizontal|vertical`. The
optional `--new-window-id` names the IPC ID for the pane it creates; it is spelled differently here
because `--window-id` already names the target, unlike `create-tab` and `create-workspace` where the
flattened window options own that name.

Creation commands return the new workspace, tab, pane, and window IDs. Close replies acknowledge
that shutdown was requested; use `wait exit` or `subscribe` when the agent must observe process
termination. IDs must be rediscovered after Vivida restarts.

Workspace names are globally unique and tab names are unique within a workspace. User-provided
names are trimmed, bounded to 128 characters, and reject control characters. Context-driven titles
that collide are assigned deterministic suffixes such as `pwsh (2)`. Renaming a tab pins its title;
`reset-tab-title` or the UI's **Use Automatic Title** action resumes terminal-context updates.

The endpoint accepts connections only from the same operating-system user. Capability material
used by Vivid presentation is never included in layout, inspection, or diagnostic replies.
