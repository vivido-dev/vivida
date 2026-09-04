#!/usr/bin/env python3
"""Summarize `vivida msg layout` as one line per pane.

    vivida msg layout | ./panes.py

The layout document is the authoritative view of Vivida's workspace/tab/split tree, but it carries
rectangles, neighbor graphs, and a typed split tree for every pane. When the question is only "which
pane do I target", this folds it down:

    ADDRESS  WS/TAB/PANE  WINDOW_ID  FLAGS  SPLIT  TITLE  CWD
    s1t1w42  1/1/1        42         cf-vs  1      zsh    /srv/app
    s1t1w43  1/1/2        43         ---v-  2.1    codex  /srv/app

FLAGS are `c` caller, `f` focused, `o` OS-focused, `v` visible, `s` selected in host; `-` otherwise.
ADDRESS is the agent-mesh position derived the same way the watcher's reconcile pass derives it, so
a pane whose window id cannot be one is reported rather than silently skipped.

`--json` prints the same rows as JSON. Everything is read-only; nothing is sent to Vivida.
"""

from __future__ import annotations

import argparse
import json
import sys

# An agent-mesh address index is a u32, and a positive one: see agent-mesh-core's Address.
MAX_ADDRESS_INDEX = 0xFFFFFFFF


class LayoutError(Exception):
    """A message that is useful to the caller rather than a traceback."""


def rows(layout: dict) -> list[dict]:
    """Flatten the layout into one record per pane, in workspace/tab/pane order."""
    workspaces = layout.get("workspaces")
    if not isinstance(workspaces, list):
        raise LayoutError(
            "input has no 'workspaces' array; expected the JSON from `vivida msg layout`"
        )
    out = []
    for workspace in workspaces:
        space = workspace.get("workspace_index")
        for tab in workspace.get("tabs", []):
            tab_index = tab.get("tab_index")
            for pane in tab.get("panes", []):
                window_id = pane.get("window_id")
                locator = pane.get("locator", {})
                out.append(
                    {
                        "address": _address(space, tab_index, window_id),
                        "workspace_index": space,
                        "tab_index": tab_index,
                        "pane_id": pane.get("pane_id"),
                        "window_id": window_id,
                        "workspace_name": locator.get("workspace_name"),
                        "tab_name": locator.get("tab_name"),
                        "title": pane.get("title"),
                        "working_directory": pane.get("working_directory"),
                        "split_path": pane.get("split_path") or [],
                        "is_caller": bool(pane.get("is_caller")),
                        "focused": bool(pane.get("focused")),
                        "os_focused": bool(pane.get("os_focused")),
                        "visible": bool(pane.get("visible")),
                        "selected_in_host": bool(pane.get("selected_in_host")),
                        "neighbors": {
                            direction: value.get("window_id")
                            for direction, value in (pane.get("neighbors") or {}).items()
                            if isinstance(value, dict)
                        },
                    }
                )
    return out


def _address(space, tab, window_id) -> str | None:
    """The `sNtNwN` a reconcile pass would derive, or None when it could not."""
    segments = []
    for letter, index in (("s", space), ("t", tab), ("w", window_id)):
        if index is None:
            if letter == "w":
                return None  # a pane with no window id is skipped, not guessed at
            continue
        if not isinstance(index, int) or index <= 0 or index > MAX_ADDRESS_INDEX:
            return None
        segments.append(f"{letter}{index}")
    return "".join(segments) or None


def flags(row: dict) -> str:
    return "".join(
        letter if row[key] else "-"
        for letter, key in (
            ("c", "is_caller"),
            ("f", "focused"),
            ("o", "os_focused"),
            ("v", "visible"),
            ("s", "selected_in_host"),
        )
    )


def _shorten(value, width: int) -> str:
    text = "" if value is None else str(value)
    return text if len(text) <= width else text[: width - 1] + "…"


def render(records: list[dict]) -> str:
    if not records:
        return "no panes"
    header = ("ADDRESS", "WS/TAB/PANE", "WINDOW_ID", "FLAGS", "SPLIT", "TITLE", "CWD")
    lines = [
        (
            record["address"] or "-",
            f"{record['workspace_index']}/{record['tab_index']}/{record['pane_id']}",
            str(record["window_id"]),
            flags(record),
            ".".join(str(step) for step in record["split_path"]) or "-",
            _shorten(record["title"], 24),
            _shorten(record["working_directory"], 40),
        )
        for record in records
    ]
    widths = [max(len(row[column]) for row in [header, *lines]) for column in range(len(header))]
    out = [
        "  ".join(cell.ljust(width) for cell, width in zip(row, widths)).rstrip()
        for row in [header, *lines]
    ]

    unaddressable = [record for record in records if record["address"] is None]
    if unaddressable:
        out.append("")
        out.append(
            f"no mesh address for {len(unaddressable)} of {len(records)} panes: an address index is"
        )
        out.append(
            "a one-based u32 and those ids are outside it. Vivida's own window ids are small, so"
        )
        out.append(
            "such an id was claimed with `create-window --window-id N`. The pane still binds, is"
        )
        out.append(
            "reachable by alias, and takes --window-id as usual; it has no position, so reconcile"
        )
        out.append("leaves it alone.")
    return "\n".join(out)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument(
        "--from",
        dest="source",
        default="-",
        metavar="PATH",
        help="layout JSON; '-' (the default) reads stdin",
    )
    parser.add_argument("--workspace", type=int, help="only this one-based workspace position")
    parser.add_argument("--tab", type=int, help="only this one-based tab position")
    parser.add_argument("--caller", action="store_true", help="only the calling pane")
    parser.add_argument("--json", action="store_true", help="print records as JSON")
    arguments = parser.parse_args(argv)

    try:
        if arguments.source == "-":
            text = sys.stdin.read()
        else:
            with open(arguments.source, encoding="utf-8") as handle:
                text = handle.read()
        if not text.strip():
            raise LayoutError("no input; pipe `vivida msg layout` in, or pass --from PATH")
        try:
            layout = json.loads(text)
        except json.JSONDecodeError as error:
            raise LayoutError(f"input is not JSON: {error}") from error
        if not isinstance(layout, dict):
            raise LayoutError("input must be one JSON object")
        records = rows(layout)
    except (LayoutError, OSError) as error:
        json.dump({"error": str(error)}, sys.stdout)
        sys.stdout.write("\n")
        return 2

    if arguments.workspace is not None:
        records = [r for r in records if r["workspace_index"] == arguments.workspace]
    if arguments.tab is not None:
        records = [r for r in records if r["tab_index"] == arguments.tab]
    if arguments.caller:
        records = [r for r in records if r["is_caller"]]

    if arguments.json:
        json.dump(records, sys.stdout, indent=2)
        sys.stdout.write("\n")
    else:
        sys.stdout.write(render(records) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
