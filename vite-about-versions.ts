import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL(".", import.meta.url));

// Open-source components acknowledged on the About page; `url` is the upstream repository declared by the
// package itself (Cargo/npm metadata, vendor/README.md for vendored crates). `cargo` entries resolve against Cargo.lock and
// `npm` entries against package.json, so the page follows the pinned dependencies without a second
// hand-maintained list.
const OPEN_SOURCE = [
  { label: "russh", url: "https://github.com/warp-tech/russh", source: "cargo", name: "russh" },
  { label: "russh-sftp", url: "https://github.com/AspectUnk/russh-sftp", source: "cargo", name: "russh-sftp" },
  // Pinned upstream snapshot recorded in vendor/README.md; the crate version alone is ambiguous.
  { label: "IronRDP", url: "https://github.com/Devolutions/IronRDP", source: "cargo", name: "ironrdp", note: "9b151c4" },
  { label: "vnc-rs", url: "https://github.com/HsuJv/vnc-rs", source: "cargo", name: "vnc-rs" },
  { label: "sspi", url: "https://github.com/Devolutions/sspi-rs", source: "cargo", name: "sspi" },
  { label: "picky", url: "https://github.com/Devolutions/picky-rs", source: "cargo", name: "picky" },
  { label: "OpenH264", url: "https://github.com/ralfbiedert/openh264-rs", source: "cargo", name: "openh264" },
  { label: "Tauri", url: "https://github.com/tauri-apps/tauri", source: "cargo", name: "tauri" },
  { label: "Tokio", url: "https://github.com/tokio-rs/tokio", source: "cargo", name: "tokio" },
  { label: "rusqlite", url: "https://github.com/rusqlite/rusqlite", source: "cargo", name: "rusqlite" },
  { label: "Argon2", url: "https://github.com/RustCrypto/password-hashes", source: "cargo", name: "argon2" },
  { label: "ring", url: "https://github.com/briansmith/ring", source: "cargo", name: "ring" },
  { label: "Vue", url: "https://github.com/vuejs/core", source: "npm", name: "vue" },
  { label: "Vue Router", url: "https://github.com/vuejs/router", source: "npm", name: "vue-router" },
  { label: "Pinia", url: "https://github.com/vuejs/pinia", source: "npm", name: "pinia" },
  { label: "vue-i18n", url: "https://github.com/intlify/vue-i18n", source: "npm", name: "vue-i18n" },
  { label: "xterm.js", url: "https://github.com/xtermjs/xterm.js", source: "npm", name: "@xterm/xterm" },
  { label: "Vite", url: "https://github.com/vitejs/vite", source: "npm", name: "vite" },
  { label: "TypeScript", url: "https://github.com/microsoft/TypeScript", source: "npm", name: "typescript" },
] as const;

// Own components whose labels are localized on the page.
const COMPONENTS = [
  { id: "core", crate: "norishell-core-api" },
  { id: "sshTransport", crate: "norishell-ssh-transport" },
  { id: "vault", crate: "norishell-secret-vault" },
  { id: "rdp", crate: "norishell-rdp-client" },
  { id: "vnc", crate: "norishell-vnc-client" },
] as const;

function cargoVersion(lock: string, name: string) {
  return new RegExp(`^name = "${name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}"\\nversion = "([^"]+)"`, "m").exec(lock)?.[1];
}

function stripRange(value: string) {
  return value.replace(/^[\^~=]/, "");
}

/** Build-time only (runs inside Vite's config): returns the JSON injected as `__NVX_ABOUT_VERSIONS__`. */
export function collectAboutVersions() {
  const lock = readFileSync(`${root}Cargo.lock`, "utf8").replace(/\r\n/g, "\n");
  const pkg = JSON.parse(readFileSync(`${root}package.json`, "utf8")) as {
    dependencies?: Record<string, string>;
    devDependencies?: Record<string, string>;
  };
  const npm = { ...pkg.devDependencies, ...pkg.dependencies };
  const toolchain = /^channel = "([^"]+)"/m.exec(readFileSync(`${root}rust-toolchain.toml`, "utf8"))?.[1];

  // Widened on purpose: the toolchain entry below is not one of the crate-backed component ids.
  const components: { id: string; version: string }[] = COMPONENTS.flatMap(({ id, crate }) => {
    const version = cargoVersion(lock, crate);
    return version ? [{ id, version }] : [];
  });
  if (toolchain) components.push({ id: "rust", version: toolchain });

  const openSource = OPEN_SOURCE.flatMap((item) => {
    const version = item.source === "cargo" ? cargoVersion(lock, item.name) : npm[item.name] && stripRange(npm[item.name]);
    return version ? [{ label: item.label, url: item.url, version, note: "note" in item ? item.note : undefined }] : [];
  });

  return JSON.stringify({ components, openSource });
}
