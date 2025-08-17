# 🎵 Sound Multiplexer

A modern Linux audio multiplexer with GUI interface that allows you to play audio simultaneously through multiple output devices with advanced synchronization and theming capabilities.

![Sound Multiplexer Interface](docs/screenshot.png)

## ✨ Features

### 🔊 **Multi-Device Audio Output**
- **Simultaneous Playback**: Route audio to multiple devices at once (speakers + headphones + Bluetooth, etc.)
- **Device Selection**: Easy checkbox interface to enable/disable individual audio outputs
- **True Silence Mode**: Complete audio silence when no devices are selected (no fallback to system default)
- **Real-time Device Detection**: Automatic detection of plugged/unplugged audio devices

### 🎚️ **Advanced Audio Controls**
- **Individual Volume Control**: Separate volume slider for each audio device
- **Mute Toggle**: Independent mute button for each device with visual feedback
- **System Synchronization**: Real-time sync with system audio settings (volume changes from other apps instantly reflected)
- **Audio Sync Compensation**: Intelligent delay compensation for mixed wired/wireless setups

### 🎨 **Modern User Interface**
- **Card-Based Design**: Clean, modern cards for each audio device with icons
- **Device Type Icons**: Automatic device type detection (🎧 headphones, 🔊 speakers, 🖥️ monitors, 🔗 Bluetooth)
- **Light/Dark Themes**: Full theme support with system detection
- **Enhanced Controls**: Large, visible checkboxes and intuitive volume controls
- **Real-time Status**: Live status updates showing active devices and silent mode

### ⚙️ **Intelligent Audio Processing**
- **PulseAudio Integration**: Deep integration with Linux PulseAudio system
- **Latency Compensation**: Automatic delay compensation for different device types
- **System Visibility**: Created audio sinks appear in system settings with descriptive names
- **Clean Resource Management**: Proper cleanup of audio modules on exit

## 🚀 Quick Start

### Prerequisites
- Linux with PulseAudio
- Python 3.8+
- PyQt6
- pulsectl

### Installation

```bash
# Clone the repository
git clone https://github.com/yourusername/sound-multiplexer.git
cd sound-multiplexer

# Install dependencies
pip install -r requirements.txt

# Run the application
python main.py
```

### First Run
1. **Launch the application** - Audio devices will be automatically detected
2. **Select devices** - Check the boxes for devices you want to use simultaneously
3. **Adjust volumes** - Use individual volume sliders for each device
4. **Configure sync** - Open settings (⚙️) to enable/disable audio synchronization

## 📖 User Guide

### Basic Usage

#### **Selecting Audio Devices**
- **Enable devices**: Check the box next to any audio device to include it in output
- **Multiple devices**: Select as many devices as you want for simultaneous playbook
- **No devices**: Uncheck all devices for complete silence (no system fallback)

#### **Volume Control**
- **Individual volumes**: Each device has its own volume slider (0-100%)
- **Mute control**: Click the 🔊/🔇 button to mute/unmute individual devices
- **System sync**: Volume changes in system settings automatically update the interface

#### **Device Management**
- **Auto-detection**: Plug in headphones, speakers, or Bluetooth devices - they appear automatically
- **Hot-swapping**: Devices can be connected/disconnected while the app is running
- **Device icons**: Visual identification of device types (headphones, speakers, monitors, etc.)

### Advanced Features

#### **Audio Synchronization**
When using multiple device types (e.g., wired speakers + Bluetooth headphones), enable sync compensation to eliminate audio echo:

1. **Open Settings** (⚙️ button in top-right)
2. **Enable "Audio sync compensation"**
3. **How it works**: Automatically adds delays to faster devices to match slower ones
4. **Device latencies**:
   - Bluetooth: ~150ms
   - HDMI/Monitor: ~8ms
   - USB Audio: ~5ms
   - Analog: ~2ms

#### **Theme Customization**
Access themes via Settings (⚙️):
- **System Default**: Automatically follows your desktop theme
- **Light Mode**: Clean interface with bright colors
- **Dark Mode**: Easy on the eyes for low-light environments

#### **Silent Mode**
When no devices are selected:
- **True silence**: Audio is completely muted (no fallback)
- **Null sink**: Creates a "black hole" for audio output
- **Visual indicator**: Status shows "Silent mode" in orange text

## 🔧 Technical Details

### Architecture

#### **Audio Processing**
- **PulseAudio Modules**: Uses `module-combine-sink` for multiple outputs
- **Delay Compensation**: Uses `module-delay` for audio synchronization
- **Null Sink**: Uses `module-null-sink` for silent mode
- **Event Monitoring**: Real-time PulseAudio event processing

#### **System Integration**
- **Device Detection**: Monitors PulseAudio sink events for plug/unplug
- **Volume Sync**: Bidirectional synchronization with system volume controls
- **Mute Sync**: Real-time mute state synchronization
- **Clean Shutdown**: Proper cleanup of all created audio modules

#### **Audio Latency Compensation**
The system automatically detects device types and applies appropriate delays:

```python
# Example latency compensation
bluetooth_headphones: 150ms
usb_speakers: 5ms
# Result: USB speakers delayed by 145ms for perfect sync
```

### File Structure
```
sound-multiplexer/
├── main.py                     # Application entry point
├── src/
│   ├── audio_manager.py        # PulseAudio integration & device management
│   ├── theme_manager.py        # Theme system & styling
│   └── gui/
│       ├── main_window.py      # Main interface & device cards
│       └── settings_dialog.py  # Settings & preferences
├── docs/                       # Documentation
└── requirements.txt           # Python dependencies
```

## ⚙️ Configuration

### Settings Storage
Settings are automatically saved using Qt's QSettings:
- **Theme preference**: `~/.config/SoundMultiplexer/Theme.conf`
- **Audio sync setting**: `~/.config/SoundMultiplexer/AudioSync.conf`

### Audio Module Names
The application creates the following PulseAudio modules:
- **Combined sink**: `sound_multiplexer_combined` (when devices selected)
- **Null sink**: `sound_multiplexer_null` (when no devices selected)
- **Delay modules**: `sound_multiplexer_combined_delay_*` (for sync compensation)

## 🎯 Use Cases

### **Home Entertainment**
- Play music through both TV speakers and wireless headphones
- Route game audio to speakers while voice chat goes to headphones

### **Content Creation**
- Monitor audio through headphones while recording to external speakers
- Multiple monitor setups with synchronized audio

### **Accessibility**
- Simultaneous output to hearing aids and speakers
- Visual and auditory feedback through multiple devices

### **Development/Testing**
- Test audio applications across multiple device types
- Audio debugging with multiple outputs

## 🛠️ Troubleshooting

### Common Issues

#### **No audio devices detected**
- Check PulseAudio is running: `pulseaudio --check`
- Restart PulseAudio: `pulseaudio -k && pulseaudio --start`
- Check device permissions

#### **Audio stuttering with sync compensation**
- Try disabling sync compensation in settings
- Check system CPU usage during playback
- Verify PulseAudio configuration

#### **Settings not saving**
- Check write permissions for `~/.config/SoundMultiplexer/`
- Verify Qt settings are working: test with other Qt applications

#### **Bluetooth latency issues**
- Sync compensation should handle this automatically
- Try manual Bluetooth codec selection (A2DP, aptX)
- Check Bluetooth device specifications

### Debug Mode
Run with verbose output:
```bash
python main.py --debug
```

### Log Files
Application logs PulseAudio operations to console:
- Module creation/destruction
- Device detection events
- Sync compensation calculations

## 🤝 Contributing

### Development Setup
1. Fork the repository
2. Create a virtual environment: `python -m venv venv`
3. Install development dependencies: `pip install -r requirements-dev.txt`
4. Run tests: `pytest tests/`

### Architecture Guidelines
- **Separation of concerns**: GUI, audio management, and theming are separate modules
- **Qt signals/slots**: Use for inter-component communication
- **Error handling**: Graceful degradation when PulseAudio operations fail
- **Resource cleanup**: Always clean up PulseAudio modules

## 📄 License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## 🙏 Acknowledgments

- **PulseAudio** for the flexible Linux audio system
- **PyQt6** for the modern GUI framework
- **pulsectl** for Python PulseAudio integration

## 📞 Support

- **Issues**: [GitHub Issues](https://github.com/yourusername/sound-multiplexer/issues)
- **Discussions**: [GitHub Discussions](https://github.com/yourusername/sound-multiplexer/discussions)
- **Wiki**: [Project Wiki](https://github.com/yourusername/sound-multiplexer/wiki)

---

**Made with ❤️ for the Linux audio community**

## Author

Nicolas Filimonov