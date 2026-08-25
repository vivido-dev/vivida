# vivida

Vivido workspace and tab manager.

Vivida includes Vivido's complete local automation client. An agent running in one pane can
inspect and control every other pane without installing the separate `vivido` executable:

```sh
vivida msg capabilities
vivida msg layout
vivida msg list-windows
vivida msg activate-pane --window-id 42
vivida msg screenshot --json --window-id 42
vivida msg mouse path --point 100,100 120,120 --window-id 42
```

When endpoint inheritance is unavailable, `vivida list --all --json` discovers the registered
local instances. `activate-pane` selects and reveals a pane without requesting OS foreground focus.

See [Agent automation](docs/automation.md) for the cross-platform control loop, screenshots,
mouse input, layout management, and targeting rules.
