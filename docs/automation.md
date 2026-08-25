# Vivida agent automation

Vivida starts one owner-only local automation endpoint on Windows, Linux, and macOS. Every shell
created inside the process inherits `VIVIDO_SOCKET` and its own `VIVIDO_WINDOW_ID`, so an agent can
use `vivida msg` without a separately installed `vivido` program.

## Agent loop

Discover capabilities once per live instance and reuse its public window IDs until a restart or
stale-ID error. In PowerShell:

```powershell
vivida msg capabilities
vivida msg layout
```

On Linux or macOS:

```sh
vivida msg capabilities
vivida msg layout
```

Read the JSON layout and choose the target's globally unique `window_id`. Workspace, tab, and pane
IDs describe the hierarchy; local pane IDs are meaningful only together with their workspace and
tab IDs. Pass `--window-id` on every target operation because an agent pane continues to inherit
its own ID even while it is hidden. If endpoint inheritance is unavailable, use `vivida list
--all --json` rather than reading registry files directly.

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

`screenshot --json` returns the private PNG path, captured frame sequence, physical dimensions,
scale factor, and cell metrics. Open that file before choosing coordinates. `mouse path` sends one
bounded two-to-1,000 point press/move/release gesture and preserves exact physical pixels under
SGR pixel mouse mode. `focus` remains the explicit request for real OS activation; Windows
foreground-lock or compositor policy can deny it, and targeted background input does not need it.

## Layout management

The standard `create-window` command creates a tab in the active workspace. Vivida also exposes
its complete workspace and split model:

```sh
vivida msg create-workspace --working-directory /path/to/project
vivida msg activate-pane --window-id 42
vivida msg create-tab --workspace-id 2
vivida msg split-pane --window-id 42 --axis horizontal
vivida msg close-pane --window-id 42
vivida msg close-tab --workspace-id 2 --tab-id 3
vivida msg close-workspace --workspace-id 2
```

Creation commands return the new workspace, tab, pane, and window IDs. Close replies acknowledge
that shutdown was requested; use `wait exit` or `subscribe` when the agent must observe process
termination. IDs must be rediscovered after Vivida restarts.

The endpoint accepts connections only from the same operating-system user. Capability material
used by Vivid presentation is never included in layout, inspection, or diagnostic replies.
