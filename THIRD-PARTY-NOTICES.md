# Third-party notices

NoriShell's GPL-3.0-only license applies to its original code. It does not
relicense third-party code, dependencies, product names, or logos.

## Source copies distributed in this repository

The [vendor inventory](vendor/README.md) records each upstream package, version,
revision where available, and locally modified files. Preserve these notices and
the license files when redistributing the source.

| Component | License text |
| --- | --- |
| russh | [Apache-2.0](vendor/russh/LICENSE-APACHE) |
| russh-sftp | [Apache-2.0](vendor/russh-sftp/LICENSE) |
| vnc-rs | [MIT](vendor/vnc-rs/LICENSE-MIT), [Apache-2.0](vendor/vnc-rs/LICENSE-APACHE) |
| picky | [MIT](vendor/picky/LICENSE-MIT), [Apache-2.0](vendor/picky/LICENSE-APACHE) |
| sspi | [MIT](vendor/sspi/LICENSE-MIT), [Apache-2.0](vendor/sspi/LICENSE-APACHE) |
| ironrdp-connector | [MIT](vendor/ironrdp-connector/LICENSE-MIT), [Apache-2.0](vendor/ironrdp-connector/LICENSE-APACHE) |
| ironrdp-session | [MIT](vendor/ironrdp-session/LICENSE-MIT), [Apache-2.0](vendor/ironrdp-session/LICENSE-APACHE) |
| ironrdp-rdpsnd | [MIT](vendor/ironrdp-rdpsnd/LICENSE-MIT), [Apache-2.0](vendor/ironrdp-rdpsnd/LICENSE-APACHE) |

## Platform icons

Apple, Windows 11 and Linux compatibility icons come from Devicon v2.17.0,
Copyright (c) 2015 konpa, under the [MIT license](src/assets/platforms/LICENSE-DEVICON).
The [asset inventory](src/assets/platforms/README.md) links to the upstream files.
Product names and logos remain the property of their respective owners.

## Resolved dependencies and binary releases

`Cargo.lock` and `pnpm-lock.yaml` pin the resolved dependencies; each dependency
retains its own license. The table above inventories copied source and assets,
not every transitive dependency linked into a platform binary. A binary release
must carry the licenses and required notices of its actual resolved dependencies
as well as these source notices. Do not treat a successful build or this table as
proof that a binary release's license inventory is complete.
