import {
  Database, File, FileArchive, FileAudio, FileBox, FileClock, FileCode2,
  FileCog, FileImage, FileJson2, FileKey2, FileQuestion, FileSpreadsheet,
  FileSymlink, FileTerminal, FileText, FileType, FileType2, FileVideo,
  Folder, Presentation,
} from "lucide-vue-next";
import type { Component } from "vue";

import type { SftpPaneEntry } from "../../views/sftpPaneState";

// Use only for visual classification; it grants no preview, execution, or file-operation capability.
const extensionGroups: ReadonlyArray<readonly [Component, string]> = [
  [FileText, "txt text md markdown mdx rst rtf adoc asciidoc org tex bib doc docx odt pages epub mobi"],
  [FileType, "pdf ps eps"],
  [FileSpreadsheet, "csv tsv xls xlsx xlsm xlsb ods numbers parquet arrow feather"],
  [Presentation, "ppt pptx pps ppsx odp"],
  [FileCode2, "js jsx mjs cjs ts tsx mts cts vue svelte astro html htm xhtml css scss sass less styl php phtml py pyw pyi rb erb go rs c h cc cpp cxx hpp hxx cs java kt kts swift m mm scala clj cljs ex exs erl hrl hs lhs lua pl pm r rmd jl dart groovy gradle sql graphql gql proto wasm wat"],
  [FileJson2, "json jsonc json5 jsonl ndjson ipynb xml xsl xslt xsd dtd plist"],
  [FileCog, "yaml yml toml ini conf config cfg cnf properties env editorconfig service socket timer target mount automount desktop reg manifest lock"],
  [FileTerminal, "sh bash zsh fish ksh csh tcsh ps1 psm1 psd1 bat cmd awk sed"],
  [FileImage, "png jpg jpeg gif webp avif apng svg ico icns bmp tif tiff heic heif raw dng cr2 nef arw psd psb ai sketch fig xcf"],
  [FileAudio, "mp3 wav flac aac m4a ogg oga opus aiff aif wma mid midi amr"],
  [FileVideo, "mp4 m4v mov mkv webm avi wmv flv mpg mpeg m2ts vob ogv 3gp"],
  [FileArchive, "zip zipx tar gz gzip tgz bz bz2 tbz tbz2 xz txz zst zstd tzst lz lzma lzo br rar 7z cab ar cpio iso img dmg vhd vhdx vmdk qcow2 bak backup"],
  [FileKey2, "key pem crt cer der csr p12 pfx p7b p7c pub asc gpg pgp keystore jks"],
  [Database, "db db3 sqlite sqlite3 mdb accdb rdb aof dump bson"],
  [FileType2, "ttf otf woff woff2 eot ttc"],
  [FileBox, "exe msi msix appx appimage deb rpm apk aab ipa pkg bin dll so dylib o obj a lib class jar war ear whl gem nupkg"],
  [FileClock, "log out err trace"],
  [FileSymlink, "lnk url webloc"],
];
const extensionIcons = new Map(extensionGroups.flatMap(([icon, extensions]) =>
  extensions.split(" ").map((extension) => [extension, icon] as const),
));
const namedIcons = new Map<string, Component>([
  ...["readme", "license", "licence", "copying", "notice", "authors", "changelog", "changes", "todo"].map((name) => [name, FileText] as const),
  ...["dockerfile", "containerfile", "makefile", "gnumakefile", "cmakelists.txt", "justfile", "rakefile", "gemfile", "procfile", "vagrantfile"].map((name) => [name, FileCode2] as const),
  ...[".gitignore", ".gitattributes", ".gitmodules", ".dockerignore", ".editorconfig", ".npmrc", ".yarnrc", ".env", ".prettierrc", ".eslintrc", "config"].map((name) => [name, FileCog] as const),
  ...[".bashrc", ".bash_profile", ".bash_login", ".profile", ".zshrc", ".zprofile", ".zshenv"].map((name) => [name, FileTerminal] as const),
  ...["id_rsa", "id_dsa", "id_ecdsa", "id_ed25519", "authorized_keys", "known_hosts"].map((name) => [name, FileKey2] as const),
]);

export function sftpEntryIcon(entry: Pick<SftpPaneEntry, "kind" | "displayName">): Component {
  if (entry.kind === "directory") return Folder;
  if (entry.kind === "symlink") return FileSymlink;
  if (entry.kind !== "file") return FileQuestion;

  const name = entry.displayName.toLowerCase();
  const named = namedIcons.get(name);
  if (named) return named;
  if (name.startsWith(".env.")) return FileCog;
  if (name.startsWith("dockerfile.") || name.startsWith("containerfile.")) return FileCode2;
  if (/\.log(?:\.\d+)+$/.test(name)) return FileClock;
  if (/\.so(?:\.\d+)+$/.test(name)) return FileBox;
  const dot = name.lastIndexOf(".");
  return dot < 0 ? File : extensionIcons.get(name.slice(dot + 1)) ?? File;
}
