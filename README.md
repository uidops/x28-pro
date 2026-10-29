# X28 PRO

A macOS menu bar app and command-line tool for the **ZLT X28 PRO** 4G/5G router.
It shows the live connection and lets you change the main radio settings without
opening the router's web page.

## Features

- **Menu bar icon**: network type (`4G`, `4G+`, `5G`) and five signal bars,
  matching the router's own signal level, with optional temperature text.
- **Popover** (click the icon):
  - operator, band chips and a per-carrier table for carrier aggregation
  - RSRP / RSRQ / SINR meters, plus 5G readings when attached to a 5G cell
  - temperature, CPU and uptime
  - data connection: APN, IPv4, IPv6 and DNS
  - a pause button that stops background polling
- **Quick settings** from the popover or the app window:
  - network mode
  - 4G and 5G band lock (only the bands the router reports as supported)
  - operator: automatic or a fixed network
  - router restart
- **App window**: Status, Network, Device and App tabs; follows the system
  light/dark mode.
- **Menu bar app**: Cmd+Q / Cmd+W close the window and keep the icon running.
  Quit fully from the popover or with ⌥⌘Q.
- **CLI**: `x28-pro` prints the temperature (and more with `--details`).

## Requirements

- macOS 11 or later (Apple Silicon builds by default)
- Rust (stable)
- A ZLT X28 PRO reachable from your Mac, by default at `https://192.168.1.1`

## Build and run

```sh
# menu bar app
cargo run --release --bin x28-tray

# open the app window at launch
cargo run --release --bin x28-tray -- --open-panel

# command-line tool only (no GUI dependencies)
cargo build --release --no-default-features --bin x28-pro
./target/release/x28-pro --details
```

## Package as a .app and .dmg

```sh
packaging/macos/build-dmg.sh
```

This builds the binaries, renders the icon, signs the app ad-hoc and writes:

- `target/macos/X28 PRO.app`
- `target/macos/X28-PRO-<version>-<arch>.dmg`

To sign for distribution, set `SIGN_IDENTITY` to your Developer ID. The ad-hoc
build is not notarized, so on another Mac you must right-click → Open the app
the first time, or run:

```sh
xattr -dr com.apple.quarantine "/Applications/X28 PRO.app"
```

## Configuration

Settings are stored in `~/.config/x28-pro/config.json` and can be edited in the
app's **App** tab:

| Setting      | Default               |
| ------------ | --------------------- |
| Router       | `https://192.168.1.1` |
| User         | `superadmin`          |
| Password     | `superadmin`          |
| Poll every   | 30 seconds            |

The CLI also reads `X28_HOST`, `X28_USER` and `X28_PASS`, or the `--host`,
`--user` and `--pass` flags.

## CLI

```text
x28-pro [--host URL] [--user NAME] [--pass PASS] [--details] [--time]
x28-pro --capture-ui
```

`--capture-ui` saves the router's web UI and an index of the API commands it
uses to `~/.config/x28-pro/webui/`. It is useful for mapping more settings
(Wi-Fi, APN, LAN, SMS). The app's **Device** tab has the same action.

## Notes

- The router's API is undocumented. The command numbers come from the open-source
  [ZLT-X28-Pro](https://github.com/Ariiima/ZLT-X28-Pro) and
  [modempatch](https://github.com/ehsanecc/modempatch) projects. Your firmware
  may differ, so check the result after changing a setting.
- Changing the network mode, band lock or operator makes the modem re-attach,
  which drops the internet for up to a minute.
- The router uses a self-signed certificate, which the app accepts on purpose.
  Only point it at your own router.

## License

[MIT](LICENSE) © 2026 Javad
