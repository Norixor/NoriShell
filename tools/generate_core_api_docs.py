#!/usr/bin/env python3
"""Regenerate docs/guides/core-api/{types,commands,handlers}.generated.md.

Single sources of truth:
  types:    `X::decl(&config)` list in crates/core-api/src/bin/export_core_api.rs
            (module = file under crates/core-api/src declaring the type)
  commands: `pub const COMMAND_*` in crates/core-api/src/lib.rs
  events:   `pub const EVENT_*` in crates/core-api/src/lib.rs plus exported types
            whose names end in Event/OutputGap/OutputItem/TerminalGap. Other
            Tauri event names are emitted ad hoc from src-tauri and have no
            central definition, so they are not catalogued.
  handlers: `production_invoke_handler!` macro list plus the Wry-only list in
            src-tauri/src/lib.rs (cfg markers preserved), ACL from
            src-tauri/permissions/app.toml `main-window-commands`.

Usage: tools/generate_core_api_docs.py [--check]
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "docs/guides/core-api"
CORE_SRC = ROOT / "crates/core-api/src"


def gen_commands():
    text = (CORE_SRC / "lib.rs").read_text()
    return re.findall(r'pub const (COMMAND_[A-Z0-9_]+): &str =\s*"([^"]+)";', text)


def gen_types():
    exporter = (CORE_SRC / "bin/export_core_api.rs").read_text()
    names = re.findall(r"^\s*([A-Za-z0-9_]+)::decl\(&config\),", exporter, re.M)
    owners = {}
    for f in sorted(CORE_SRC.glob("*.rs")):
        src = f.read_text()
        # ID newtypes are declared by macros such as `uuid_id!(Name, ...)`.
        for m in re.finditer(r"^(?:pub (?:struct|enum|type)|[a-z_]+!\()\s*([A-Za-z0-9_]+)", src, re.M):
            owners.setdefault(m.group(1), []).append(f.stem)
    rows = []
    for n in names:
        mods = owners.get(n)
        if not mods or len(mods) != 1:
            sys.exit(f"cannot resolve a unique source module for type {n}: {mods}")
        rows.append((n, mods[0]))
    return rows


def split_handlers(body):
    """Yield (name, cfg_or_None) for each `module::name` entry in a handler list."""
    cfg = None
    for raw in body.split("\n"):
        line = raw.strip()
        if not line or line.startswith("//"):
            continue
        m = re.match(r"#\[cfg\((.*)\)\]$", line)
        if m:
            cfg = m.group(1)
            continue
        m = re.match(r"(?:[A-Za-z0-9_]+::)*([A-Za-z0-9_]+),?$", line)
        if m:
            yield m.group(1), cfg
            cfg = None


def gen_handlers():
    text = (ROOT / "src-tauri/src/lib.rs").read_text()
    macro = re.search(r"generate_handler!\[(.*?)\$\(, \$runtime_command\)\*", text, re.S).group(1)
    wry = re.search(r"production_invoke_handler!\(\s*builder,(.*?)\n        \)", text, re.S).group(1)
    entries = list(split_handlers(macro)) + list(split_handlers(wry))
    toml = (ROOT / "src-tauri/permissions/app.toml").read_text()
    block = re.search(r'identifier = "main-window-commands".*?commands\.allow = \[(.*?)\]', toml, re.S).group(1)
    allowed = set(re.findall(r'"([^"]+)"', block))
    return entries, allowed


def cfg_label(cfg):
    if cfg is None:
        return ""
    labels = {'target_os = "android"': "Android only"}
    return f" ({labels.get(cfg, 'cfg: ' + cfg)})"


def build():
    commands = gen_commands()
    by_cmd = {v: k for k, v in commands}
    types = gen_types()
    handlers, allowed = gen_handlers()
    modules = {m for _, m in types}

    t = [
        "# Generated Core API type directory",
        "",
        "Generated from `crates/core-api/src/bin/export_core_api.rs`. It lists every TypeScript declaration emitted by that exporter: "
        f"**{len(types)} types** across **{len(modules)} public modules**. This is an internal application contract, never a guest SDK permission surface.",
        "",
        "| Type | Source module |",
        "| --- | --- |",
    ] + [f"| `{n}` | `{m}` |" for n, m in types]

    c = [
        "# Generated stable Core command directory",
        "",
        f"Generated from the `COMMAND_*` declarations in `crates/core-api/src/lib.rs`: **{len(commands)} stable command symbols**. "
        "Registration and window ACL are separately listed in [the runtime handler directory](handlers.generated.md).",
        "",
        "| Symbol | Command |",
        "| --- | --- |",
    ] + [f"| `{k}` | `{v}` |" for k, v in commands]

    h = [
        "# Generated Tauri handler and main-window ACL directory",
        "",
        "Generated from `production_invoke_handler!` in `src-tauri/src/lib.rs` and the trusted-main allowlist in `src-tauri/permissions/app.toml`. "
        f"It lists **{len(handlers)} registered command handlers**. Registration does not grant plugin access. "
        "Handlers marked with a platform exist only in builds for that platform.",
        "",
        "| Handler | Stable Core symbol | Main-window ACL |",
        "| --- | --- | --- |",
    ]
    for name, cfg in handlers:
        sym = by_cmd.get(name)
        h.append(
            f"| `{name}`{cfg_label(cfg)} | {'`' + sym + '`' if sym else 'internal'} | "
            f"{'allowed' if name in allowed else 'not allowed'} |"
        )
    events = re.findall(r'pub const (EVENT_[A-Z0-9_]+): &str =\s*"([^"]+)";', (CORE_SRC / "lib.rs").read_text())
    payloads = [n for n, _ in types if re.search(r"(Event|OutputGap|OutputItem|TerminalGap)$", n)]
    e = [
        "# Generated Core event directory",
        "",
        "Generated from the `EVENT_*` constants in `crates/core-api/src/lib.rs` and the exported TypeScript declarations whose names denote event or output payloads. "
        "Other Tauri event names are emitted directly from `src-tauri` without a central definition and are not listed here. "
        "Event payloads remain generation-scoped projections, not permissions.",
        "",
        f"## Named Core event symbols ({len(events)})",
        "",
        "| Symbol | Event |",
        "| --- | --- |",
    ] + [f"| `{k}` | `{v}` |" for k, v in events] + [
        "",
        f"## Event and output payload types ({len(payloads)})",
        "",
    ] + [f"- `{n}`" for n in payloads]
    return {
        "events.generated.md": "\n".join(e) + "\n",
        "types.generated.md": "\n".join(t) + "\n",
        "commands.generated.md": "\n".join(c) + "\n",
        "handlers.generated.md": "\n".join(h) + "\n",
    }


def main():
    files = build()
    check = "--check" in sys.argv
    bad = 0
    for name, content in files.items():
        p = OUT / name
        if check:
            if not p.exists() or p.read_text() != content:
                print(f"out of date: {p}")
                bad = 1
        else:
            p.write_text(content)
    return bad


if __name__ == "__main__":
    sys.exit(main())
