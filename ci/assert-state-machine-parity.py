#!/usr/bin/env python3
"""R2 — the state diagram and the transition table must agree.

Legal edges live in `message_state_transitions` / `job_state_transitions` /
`attempt_state_transitions`, and triggers reject everything else with
SQLSTATE SM001. The diagrams in the design doc are what people actually read.
When the two drift, the first symptom is a production SM001 on a transition
that looks perfectly legal in the diagram — a failure that is confusing
precisely because the documentation is wrong.

This compares them in both directions and fails on any asymmetry.

Usage: ci/assert-state-machine-parity.py
"""

from __future__ import annotations

import pathlib
import re
import sys

DOC = pathlib.Path("docs/architecture.md")
SQL = pathlib.Path("schema/migrations/postgres/0002_bootstrap/up.sql")

# `[*]` is mermaid's start/end pseudo-state. Entry into the initial state and
# exit from a terminal one are not rows in the table — terminality is expressed
# as "no outgoing rows" — so both are dropped before comparing.
PSEUDO = "[*]"

Edge = tuple[str, str]


def mermaid_state_diagrams(markdown: str) -> list[tuple[set[str], set[Edge]]]:
    """Every ```mermaid stateDiagram-v2``` block, as (states, edges)."""
    out = []
    for block in re.findall(r"```mermaid\n(.*?)```", markdown, re.S):
        if "stateDiagram" not in block.splitlines()[0]:
            continue
        edges: set[Edge] = set()
        states: set[str] = set()
        for line in block.splitlines():
            m = re.match(r"\s*(\S+)\s*-->\s*([^:\n]+?)\s*(?::.*)?$", line)
            if not m:
                continue
            src, dst = m.group(1), m.group(2)
            states.update({src, dst} - {PSEUDO})
            if PSEUDO in (src, dst):
                continue
            edges.add((src, dst))
        if edges:
            out.append((states, edges))
    return out


def sql_transitions(sql: str, table: str) -> set[Edge]:
    """Rows of a `VALUES` list in the INSERT into `table`."""
    m = re.search(
        rf"INSERT\s+INTO\s+{re.escape(table)}\s*\([^)]*\)\s*VALUES(.*?);",
        sql,
        re.S | re.I,
    )
    if not m:
        sys.exit(f"could not find an INSERT INTO {table} in {SQL}")
    return set(re.findall(r"\(\s*'(\w+)'\s*,\s*'(\w+)'\s*\)", m.group(1)))


def report(name: str, diagram: set[Edge], table: set[Edge]) -> bool:
    """Print any asymmetry. Returns True when the two agree."""
    only_diagram = sorted(diagram - table)
    only_table = sorted(table - diagram)
    if not only_diagram and not only_table:
        print(f"  {name}: {len(table)} edges, diagram and table agree")
        return True

    print(f"  {name}: MISMATCH", file=sys.stderr)
    for src, dst in only_diagram:
        print(
            f"    {src} -> {dst}: in the diagram, missing from the table. "
            f"Rust would propose it and Postgres would raise SM001.",
            file=sys.stderr,
        )
    for src, dst in only_table:
        print(
            f"    {src} -> {dst}: in the table, missing from the diagram. "
            f"Legal but undocumented.",
            file=sys.stderr,
        )
    return False


def terminal_states(edges: set[Edge], states: set[str]) -> set[str]:
    return {s for s in states if not any(src == s for src, _ in edges)}


def main() -> int:
    if not DOC.exists() or not SQL.exists():
        sys.exit(f"run from the repository root; expected {DOC} and {SQL}")

    markdown = DOC.read_text()
    sql = SQL.read_text()

    diagrams = mermaid_state_diagrams(markdown)
    if len(diagrams) < 3:
        sys.exit(f"expected at least three stateDiagram-v2 blocks, found {len(diagrams)}")

    machines = {
        "message": sql_transitions(sql, "message_state_transitions"),
        "job": sql_transitions(sql, "job_state_transitions"),
        "attempt": sql_transitions(sql, "attempt_state_transitions"),
    }

    # Match each diagram to a machine by state overlap rather than by position,
    # so reordering the document does not silently compare the wrong pair.
    print("state machine parity:")
    ok = True
    for name, table in machines.items():
        table_states = {s for edge in table for s in edge}
        best = max(diagrams, key=lambda d: len(d[0] & table_states))
        states, edges = best
        if not (states & table_states):
            sys.exit(f"no diagram matches the {name} transition table")
        ok &= report(name, edges, table)

        # Terminality is data: a state with no outgoing rows is terminal, and
        # the diagram must not draw an exit from it.
        sql_terminal = terminal_states(table, table_states)
        diagram_terminal = terminal_states(edges, states)
        if sql_terminal != diagram_terminal:
            print(
                f"    terminal states disagree: table {sorted(sql_terminal)} "
                f"vs diagram {sorted(diagram_terminal)}",
                file=sys.stderr,
            )
            ok = False

    if not ok:
        print(
            "\nR2: legal edges are the transition table. Fix whichever side is "
            "wrong — the doc if the table is right, the migration if it is not.",
            file=sys.stderr,
        )
        return 1
    print("state machine parity OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
