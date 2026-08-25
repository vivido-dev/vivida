# Vivida agent automation

Vivida starts one owner-only local automation endpoint on Windows, Linux, and macOS. Every shell
created inside the process inherits `VIVIDO_SOCKET` and its own `VIVIDO_WINDOW_ID`, so an agent can
use `vivida msg` without a separately installed `vivido` program.

## Agent loop

Save the controller pane before selecting another pane. In PowerShell:

```powershell
$controller = $env:VIVIDO_WINDOW_ID
vivida msg capabilities
vivida msg layout
vivida msg list-windows
```

On Linux or macOS:

```sh
controller=$VIVIDO_WINDOW_ID
vivida msg capabilities
vivida msg layout
vivida msg list-windows
```

Read the JSON layout and choose the target's globally unique `window_id`. Workspace, tab, and pane
IDs describe the hierarchy; local pane IDs are meaningful only together with their workspace and
tab IDs. Pass `--window-id` on every target operation because an agent pane continues to inherit
its own ID even while it is hidden.

```sh
vivida msg inspect --window-id 42
vivida msg focus --window-id 42
vivida msg typing "status" --window-id 42 --report
vivida msg key Enter --window-id 42 --report
vivida msg wait screen-stable --window-id 42 --quiet 250ms
```

Application-routed typing, paste, keys, and mouse input can target a background pane. Focus the
target first for UI-routed mouse input or a fresh visual frame. Capture the current frame sequence
with `inspect`, wait for a newer frame after focusing or acting, then take the screenshot:

```sh
vivida msg wait frame --window-id 42 --after-frame 7
vivida msg screenshot --window-id 42
```

`screenshot` prints an absolute path to a private PNG of the target pane. Claude Code, Codex CLI,
OpenCode, or another vision-capable agent should open that file with its image-reading tool before
choosing pixel or cell coordinates for `vivida msg mouse`. Vivida does not perform OCR or image
recognition itself.

Restore the controller when needed:

```powershell
vivida msg focus --window-id $controller
```

```sh
vivida msg focus --window-id "$controller"
```

## Layout management

The standard `create-window` command creates a tab in the active workspace. Vivida also exposes
its complete workspace and split model:

```sh
vivida msg create-workspace --working-directory /path/to/project
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
