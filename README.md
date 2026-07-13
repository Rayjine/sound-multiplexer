# Sound Multiplexer

Play audio through several output devices at once on Linux — speakers, headphones and Bluetooth in any combination.

Sound Multiplexer is a small PyQt6 app that builds a combined PulseAudio/PipeWire sink from the devices you select. Each device has its own volume and mute, kept in sync with the system mixer, and devices are picked up as you plug them in.

## Requirements

- Linux with PulseAudio, or PipeWire with `pipewire-pulse` (the default on most current distros)
- Python 3.8+, PyQt6, pulsectl

## Install

```bash
git clone https://github.com/rayjine/sound-multiplexer.git
cd sound-multiplexer
./packaging/install.sh
```

The script installs your distro's system dependencies (dnf, apt, pacman or zypper) and installs the app for the current user into `~/.local/bin`. Then launch `sound-multiplexer-gui`, or find "Sound Multiplexer" in your application menu.

To install manually instead, get PyQt6 from your distro (`python3-PyQt6` on Fedora/openSUSE, `python3-pyqt6` on Debian/Ubuntu, `python-pyqt6` on Arch), then:

```bash
pip3 install --user pulsectl
make install-user
```

To run from a source checkout without installing:

```bash
pip install -r requirements.txt
python3 main.py
```

## Usage

Check the devices you want audio to play through. Unchecking everything switches to silence — a null sink, with no fallback to the default device. Each device card has a volume slider and a mute button; both follow and update the system mixer. The gear button opens settings (light/dark/system theme).

The settings dialog also has an "audio sync compensation" toggle, but per-device delay is not implemented yet — the setting currently has no effect.

## How it works

The app loads a `module-combine-sink` named `sound_multiplexer_combined` with the selected devices as slaves and sets it as the default sink; with nothing selected it loads a `module-null-sink` instead. Closing the app unloads its modules.

If the app crashes and leaves a stale sink behind:

```bash
pactl list short modules | grep sound_multiplexer
pactl unload-module <id>
```

Settings are stored under `~/.config/SoundMultiplexer/`.

## Development

```bash
make deps        # runtime dependencies
make dev-deps    # + black, flake8, mypy, isort
make lint
make format
make dev-install # editable install
```

There is no automated test suite yet.

## License

GPL-3.0 — see [LICENSE](LICENSE).

Bugs and ideas: [GitHub Issues](https://github.com/rayjine/sound-multiplexer/issues).
