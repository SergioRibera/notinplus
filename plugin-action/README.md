# Pack a NotIn Plus plugin

Builds the `.ntp` package of a plugin with `notin-pack`, the tool every
NotIn Plus release ships. It writes into `plugin.ntp.kdl` what the plugin
adds (document types, Create menu entries, settings) and the SHA-256 of
each library, then zips the folder. The app shows those lines before
installing, checks each library against its checksum before loading it,
and refuses a plugin that adds something it did not declare.

Use the action at the release tag whose SDK the plugin is built with:
that tag picks the `notin-pack` it downloads.

```yaml
- uses: SergioRibera/notinplus/plugin-action@v0.1.0-alpha.6
  id: pack
  with:
    plugin-dir: dist/my-plugin
- run: echo "${{ steps.pack.outputs.package }}"
```

| Input | Required | Default | |
|---|---|---|---|
| `plugin-dir` | yes | | Folder with `plugin.ntp.kdl` and every library it lists |
| `output` | no | `<id>-<version>.ntp` next to `plugin-dir` | Package path |
| `version` | no | the tag the action is used at | Release whose `notin-pack` to use |

Runners: Linux x86_64 and ARM64, macOS ARM64, Windows x86_64. The folder
must hold a library for the runner's system: `notin-pack` loads it to see
what the plugin registers. Checksums cover every library listed.

## Building the libraries for every platform

Build each library on its own runner, gather them in one folder, then
pack once:

```yaml
jobs:
  build:
    strategy:
      matrix:
        include:
          - { runner: ubuntu-latest, target: linux-x86_64, file: libmy_plugin.so }
          - { runner: macos-latest, target: macos, file: libmy_plugin.dylib }
          - { runner: windows-latest, target: windows-x86_64, file: my_plugin.dll }
    runs-on: ${{ matrix.runner }}
    steps:
      - uses: actions/checkout@v4
      - run: cargo build --release
      - uses: actions/upload-artifact@v4
        with:
          name: lib-${{ matrix.target }}
          path: target/release/${{ matrix.file }}

  pack:
    needs: build
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/download-artifact@v4
        with:
          pattern: lib-*
          path: libs
      - run: |
          mkdir -p dist/my-plugin
          cp plugin.ntp.kdl dist/my-plugin/
          cp -r libs/lib-linux-x86_64 dist/my-plugin/linux
          cp -r libs/lib-macos dist/my-plugin/macos
          cp -r libs/lib-windows-x86_64 dist/my-plugin/windows
      - uses: SergioRibera/notinplus/plugin-action@v0.1.0-alpha.6
        id: pack
        with:
          plugin-dir: dist/my-plugin
      - uses: softprops/action-gh-release@v2
        with:
          files: ${{ steps.pack.outputs.package }}
```

with `plugin.ntp.kdl` listing them:

```kdl
id      "org.example.my-plugin"
name    "My plugin"
version "1.0.0"
library "linux/libmy_plugin.so"     target="linux-x86_64"
library "macos/libmy_plugin.dylib"  target="macos"
library "windows/my_plugin.dll"     target="windows-x86_64"
```
