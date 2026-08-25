# vivida

Vivido workspace and tab manager.

Vivida includes Vivido's complete local automation client. An agent running in one pane can
inspect and control every other pane without installing the separate `vivido` executable:

```sh
vivida msg capabilities
vivida msg layout
vivida msg resolve-pane --path left
vivida msg resolve-pane --tab 2 --path down
vivida msg list-windows
vivida msg activate-pane --window-id 42
vivida msg screenshot --json --window-id 42
vivida msg mouse path --point 100,100 120,120 --window-id 42
```

`layout` uses the pane's inherited `VIVIDO_WINDOW_ID` to mark the caller and report its workspace,
tab, pane, one-based split-tree path, rectangle, visibility, and directional neighbors.
`resolve-pane` walks an explicit directional path from the caller—or from the selected tab's
focused pane—and returns an automation `window_id`. Repeated steps traverse deeply nested splits.

When endpoint inheritance is unavailable, `vivida list --all --json` discovers the registered
local instances. `activate-pane` selects and reveals a pane without requesting OS foreground focus.

See [Agent automation](docs/automation.md) for the cross-platform control loop, screenshots,
mouse input, layout management, and targeting rules.
