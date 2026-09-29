export interface AboutComponentVersion {
  id: string;
  version: string;
}

export interface AboutOpenSourceVersion {
  label: string;
  url: string;
  version: string;
  note?: string;
}

interface AboutVersions {
  components: AboutComponentVersion[];
  openSource: AboutOpenSourceVersion[];
}

// Runtime side: injected by Vite from Cargo.lock, package.json and rust-toolchain.toml (collected by
// vite-about-versions.ts at the repository root). Unit-test builds without the define get an empty list.
declare const __NVX_ABOUT_VERSIONS__: AboutVersions | undefined;

export const aboutVersions: AboutVersions = typeof __NVX_ABOUT_VERSIONS__ === "undefined"
  ? { components: [], openSource: [] }
  : __NVX_ABOUT_VERSIONS__;
