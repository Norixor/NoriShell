#!/usr/bin/env python3
"""Validate and deterministically package the three pure-data NoriShell themes."""

import hashlib
import json
from pathlib import Path
import re
import tempfile
import zipfile


ROOT = Path(__file__).resolve().parent
REPOSITORY = ROOT.parents[1]
OUTPUT = REPOSITORY / "output" / "theme-plugins"
THEMES = (("clear", "NoriShell-Theme-Moss-1.1.0.zip"),
          ("midnight", "NoriShell-Theme-Mulberry-1.1.0.zip"),
          ("sand", "NoriShell-Theme-Sand-1.0.0.zip"))
MANIFEST_KEYS = {"pluginId", "name", "publisher", "version", "protocolMajor", "protocolMinor",
                 "platform", "architectures", "capabilities", "minimumAppVersion"}
THEME_KEYS = {"schemaVersion", "id", "name", "appearance", "colors", "fontFamily", "fontSize",
              "radius", "borderWidth", "density", "shadow", "terminalPalette"}
COLOR_KEYS = {"bgCanvas", "bgSurface", "bgSubtle", "bgHover", "border", "borderStrong", "textPrimary",
              "textSecondary", "textTertiary", "accent", "accentHover", "onAccent", "accentSoft",
              "success", "successSoft", "warning", "warningSoft", "danger", "onDanger", "dangerSoft",
              "focusRing", "terminalPaneActiveBorder", "brandMarkPrimary", "brandMarkSecondary", "selection",
              "selectionText"}
TERMINAL_KEYS = {"background", "foreground", "muted", "cursor", "selection", "black", "red", "green",
                 "yellow", "blue", "magenta", "cyan", "white", "brightBlack", "brightRed", "brightGreen",
                 "brightYellow", "brightBlue", "brightMagenta", "brightCyan", "brightWhite"}
HEX = re.compile(r"#[0-9a-fA-F]{6}$")
SLUG = re.compile(r"[a-z0-9]+(?:[._-][a-z0-9]+)*$")


def contrast(left: str, right: str) -> float:
    def luminance(color: str) -> float:
        channels = [int(color[index:index + 2], 16) / 255 for index in (1, 3, 5)]
        linear = [item / 12.92 if item <= 0.04045 else ((item + 0.055) / 1.055) ** 2.4 for item in channels]
        return linear[0] * 0.2126 + linear[1] * 0.7152 + linear[2] * 0.0722
    first, second = luminance(left), luminance(right)
    return (max(first, second) + 0.05) / (min(first, second) + 0.05)


def read_json(path: Path) -> dict:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"{path}: expected an object")
    return value


def validate(manifest: dict, theme: dict, source: Path) -> None:
    if set(manifest) != MANIFEST_KEYS:
        raise ValueError(f"{source}: manifest fields must exactly match the plugin wire contract")
    if not (isinstance(manifest["pluginId"], str) and manifest["pluginId"] == f"com.norishell.theme.{theme.get('id')}"):
        raise ValueError(f"{source}: manifest pluginId must bind the theme id")
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", manifest["version"]) or manifest["protocolMajor"] != 1 or manifest["protocolMinor"] != 14:
        raise ValueError(f"{source}: theme package must target protocol 1.14 with a numeric semantic version")
    if manifest["platform"] != "desktop" or manifest["architectures"] != ["universal"] or manifest["capabilities"] != [] or manifest["minimumAppVersion"] != "0.1.0":
        raise ValueError(f"{source}: theme packages are universal desktop data packages without capabilities")
    if set(theme) != THEME_KEYS or len(json.dumps(theme, ensure_ascii=False).encode("utf-8")) > 32 * 1024:
        raise ValueError(f"{source}: invalid or oversized theme definition")
    if theme["schemaVersion"] != 1 or not isinstance(theme["id"], str) or len(theme["id"]) > 64 or not SLUG.fullmatch(theme["id"]):
        raise ValueError(f"{source}: theme id must be a bounded slug")
    if not isinstance(theme["name"], dict) or set(theme["name"]) != {"zhCN", "en"} or any(not isinstance(label, str) or not label.strip() or len(label) > 80 for label in theme["name"].values()):
        raise ValueError(f"{source}: theme names must have bounded Chinese and English labels")
    if theme["appearance"] not in {"light", "dark"} or theme["fontFamily"] not in {"system", "sans", "mono"} or theme["fontSize"] not in range(12, 19) or theme["radius"] not in range(13) or theme["borderWidth"] not in {1, 2} or theme["density"] not in {"compact", "standard", "comfortable"} or theme["shadow"] not in {"none", "soft", "standard"}:
        raise ValueError(f"{source}: invalid bounded visual metadata")
    colors = theme["colors"]
    palette = theme["terminalPalette"]
    if not isinstance(colors, dict) or set(colors) != COLOR_KEYS or not all(isinstance(value, str) and HEX.fullmatch(value) for value in colors.values()):
        raise ValueError(f"{source}: colors must be the complete fixed token map")
    if not isinstance(palette, dict) or set(palette) != TERMINAL_KEYS or not all(isinstance(value, str) and HEX.fullmatch(value) for value in palette.values()):
        raise ValueError(f"{source}: terminalPalette must be the complete existing terminal map")
    for background in (colors["bgCanvas"], colors["bgSurface"], colors["bgSubtle"]):
        if contrast(colors["textPrimary"], background) < 4.5 or contrast(colors["textSecondary"], background) < 4.5:
            raise ValueError(f"{source}: primary and secondary text must meet AA")
    if (contrast(colors["onAccent"], colors["accent"]) < 4.5 or contrast(colors["onAccent"], colors["accentHover"]) < 4.5 or contrast(colors["onDanger"], colors["danger"]) < 4.5 or contrast(colors["focusRing"], colors["bgCanvas"]) < 3 or contrast(colors["selectionText"], colors["selection"]) < 4.5):
        raise ValueError(f"{source}: action, focus, or selection contrast is insufficient")
    if any(contrast(colors["textPrimary"], colors[key]) < 4.5 for key in ("accentSoft", "successSoft", "warningSoft", "dangerSoft")):
        raise ValueError(f"{source}: text must remain readable over semantic soft backgrounds")
    if any(contrast(colors[color], colors[soft]) < 3 for color, soft in (("success", "successSoft"), ("warning", "warningSoft"), ("danger", "dangerSoft"))):
        raise ValueError(f"{source}: semantic status colors must remain distinguishable")


def package(slug: str, filename: str) -> dict:
    source = ROOT / slug
    manifest_path, theme_path = source / "manifest.json", source / "assets" / "theme.json"
    manifest, theme = read_json(manifest_path), read_json(theme_path)
    validate(manifest, theme, source)
    if not filename.endswith(f"-{manifest['version']}.zip"):
        raise ValueError(f"{source}: archive filename must match manifest version")
    OUTPUT.mkdir(parents=True, exist_ok=True)
    destination = OUTPUT / filename
    with tempfile.NamedTemporaryFile(dir=OUTPUT, prefix=f".{slug}-", suffix=".zip", delete=False) as temporary:
        staging = Path(temporary.name)
    try:
        with zipfile.ZipFile(staging, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
            for name, path in (("manifest.json", manifest_path), ("assets/theme.json", theme_path)):
                entry = zipfile.ZipInfo(name, date_time=(2026, 9, 14, 0, 0, 0))
                entry.compress_type = zipfile.ZIP_DEFLATED
                entry.external_attr = 0o100644 << 16
                archive.writestr(entry, path.read_bytes())
        with zipfile.ZipFile(staging) as archive:
            if archive.namelist() != ["manifest.json", "assets/theme.json"]:
                raise ValueError(f"{slug}: ZIP contains files outside the two-file data-package contract")
            if archive.read("manifest.json") != manifest_path.read_bytes() or archive.read("assets/theme.json") != theme_path.read_bytes():
                raise ValueError(f"{slug}: ZIP payload differs from validated sources")
        staging.replace(destination)
    finally:
        staging.unlink(missing_ok=True)
    digest = hashlib.sha256(destination.read_bytes()).hexdigest()
    checksum = destination.with_suffix(destination.suffix + ".sha256")
    checksum.write_text(f"{digest}  {destination.name}\n", encoding="ascii")
    return {"file": str(destination.relative_to(REPOSITORY)), "bytes": destination.stat().st_size, "sha256": digest, "sha256File": str(checksum.relative_to(REPOSITORY))}


if __name__ == "__main__":
    print(json.dumps({"themes": [package(slug, filename) for slug, filename in THEMES]}, indent=2))
