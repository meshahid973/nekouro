# NekoUro

Based on the upstream Zeron project and distributed under its MIT license.

Control your coding agents (Claude Code, Codex, Cursor, Devin, Grok, Hermes, Pi, Antigravity) locally by default, with optional multi-device sync.

*English | [简体中文](README.zh-CN.md)*

![NekoUro driving a Claude Code session with a live branch diff sidebar](apps/landing/public/assets/app-screenshot.jpg)

Every device runs a small engine that stores sessions on that device. A new installation starts in local-only mode without an account or a network connection.

## Install and run locally (Linux)

```bash
git clone https://github.com/meshahid973/nekouro.git
cd nekouro
cargo run --locked -p nekouro
```

To create an installable Linux tarball, run `scripts/package-linux.sh`, extract the generated archive under `target/package`, and run its `install.sh`. The packaged installer adds NekoUro to your application launcher with a per-user `nekouro.desktop` entry and icon. Linux requires the system ALSA runtime (`libasound.so.2`), including for headless mode because it shares the desktop executable.

The desktop sidebar browser also needs the [Linux browser runtime](docs/reference/linux-browser.md).

Day-to-day:

```bash
nekouro status      # local/synced mode and engine status
nekouro update      # update to the latest release
nekouro daemon start|stop|restart|status
```

## Optional multi-device sync

Sign in only when you want to open your account's synced workspace. Authentication changes the profile selected by the next engine start, so stop the daemon before changing it:

```bash
nekouro daemon stop
nekouro login
nekouro daemon start
```

You can then start an agent on one synced device and follow or drive it from another. An always-on machine such as a VPS can keep those agents working after you close your laptop.

Devices signed in to the same synced account are trusted with remote workspace access. A device controlling a workspace on another device can list, read, and write its files; enabling `Show ignored files` also makes gitignored files such as `.env` available remotely. `.git` is always excluded. Only sign in devices you trust with the full contents of your workspaces.

Signing in does not upload, move, or import existing local sessions. Local sessions and their attachments remain under the local profile and reappear when you return to local-only mode:

```bash
nekouro daemon stop
nekouro logout
nekouro daemon start
```

`nekouro login` and `nekouro logout` refuse to modify credentials while an engine owns the data directory. The desktop app follows the same next-restart profile boundary.

On macOS: use the desktop release, or build `nekouro` from source and run `nekouro daemon install` to install the launchd service.

On Windows: run the `nekouro-<version>-windows-x86_64-setup.exe` installer from the [latest release](https://github.com/meshahid973/nekouro/releases/latest). It installs for your user without administrator rights, adds NekoUro to the Start menu, and appears in Settings → Apps for uninstalling. A portable ZIP is also published; keep `nekouro-update.json` beside `nekouro.exe` for in-app updates. See the [development notes](docs/reference/windows-development.md) for source builds.

## Updates

The desktop app checks for a new release when it starts, every hour while it runs, and when you come back to it after the machine slept. A new version downloads in the background; the sidebar then offers **Update ready — restart to apply**, and if you don't restart, it installs the next time you quit NekoUro. Check by hand with **NekoUro → Check for Updates…** on macOS, or **Check for updates** in the account menu (bottom of the sidebar) on Windows and Linux. Set `ZERON_AUTO_UPDATE=0` to be notified without the background download.

Linux desktop installs from the release tarball's `install.sh` use the same `~/.nekouro/app` layout as the packaged installer, so they update in place too. A daemon installed as a service restarts into a newer installed version once no agent run or terminal is active; `nekouro update` updates headless installs on demand.

## Upstream and attribution

NekoUro is based on [Zeron](https://github.com/zeronsh/zeron) and retains Zeron's MIT license notice. The upstream Zeron project credits [The Context Company](https://www.thecontextcompany.com/) as a sponsor; its sponsorship page is maintained by the upstream project.

---

Developing or curious how it works? Check out [ARCHITECTURE.md](ARCHITECTURE.md). For upstream Zeron documentation, see [its repository](https://github.com/zeronsh/zeron).

Licensed under the [MIT License](LICENSE).
