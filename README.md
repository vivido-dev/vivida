# vivida

Vivido workspace and tab manager.

Vivida includes Vivido's complete local automation client. An agent running in one pane can
inspect and control every other pane without installing the separate `vivido` executable:

```sh
vivida msg capabilities
vivida msg layout
vivida msg list-windows
```

See [Agent automation](docs/automation.md) for the cross-platform control loop, screenshots,
mouse input, layout management, and targeting rules.
