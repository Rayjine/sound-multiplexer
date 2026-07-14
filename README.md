# Sound Multiplexer

Play audio through several output devices at once — speakers, headphones and Bluetooth in any combination.

[Website](https://rayjine.github.io/sound-multiplexer/) · [Downloads](https://github.com/Rayjine/sound-multiplexer/releases/latest)

Select the devices you want sound on; the app builds a combined PulseAudio/PipeWire sink and makes it the default. Each device keeps its own volume and mute, a master row controls the combined output as a whole, and everything stays in sync with the system mixer both ways.

## Status

The app is written in Rust with a Tauri UI. Linux is tested, including a live end-to-end suite that drives a real PipeWire server. The Windows build (WASAPI loopback fan-out) passes its tests in CI but hasn't run on real hardware yet. Installers for both — AppImage, deb, rpm and a Windows setup — are on the [releases page](https://github.com/Rayjine/sound-multiplexer/releases), or build from source below.

## Build from source

You need Rust ([rustup.rs](https://rustup.rs)) and, on Linux, the Tauri system libraries:

- Fedora: `sudo dnf install webkit2gtk4.1-devel openssl-devel libappindicator-gtk3-devel librsvg2-devel dbus-devel`
- Debian/Ubuntu: `sudo apt install libwebkit2gtk-4.1-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev libdbus-1-dev build-essential`

```bash
cargo build --release
./target/release/sound-multiplexer
```

## How it works

With two or more devices enabled, the app loads a `module-combine-sink` named `sound_multiplexer_combined` with your devices as slaves and sets it as the default sink — that sink is what the app shows as the "Master volume" row, and it's also what the system volume keys control while it's active. With one device enabled there's no extra plumbing, just a plain default device. With none, a null sink gives true silence. Closing the app removes its sinks; a restarted app adopts or cleans up anything a crashed run left behind.

Manual cleanup, should you ever need it:

```bash
pactl list short modules | grep sound_multiplexer
pactl unload-module <id>
```

## Development

```bash
cargo test --workspace            # unit tests
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p sound-multiplexer-audio --test linux_live -- --ignored
                                  # live end-to-end against your audio server
                                  # (briefly reroutes audio, restores everything)
(cd ui-tests && npm test)         # frontend tests (jsdom)
```

The full technical reference — backend contracts, routing semantics, the Windows engine, testing strategy — lives in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). CI runs the whole suite on Linux (including the live E2E inside a PipeWire session) and Windows, and produces installer artifacts.

## License

GPL-3.0 — see [LICENSE](LICENSE).

Bugs and ideas: [GitHub Issues](https://github.com/rayjine/sound-multiplexer/issues).
