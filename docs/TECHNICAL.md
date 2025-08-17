# 🔧 Technical Documentation

## Architecture Overview

Sound Multiplexer is built with a modular architecture that separates concerns between audio management, user interface, and system integration.

### Core Components

```
┌─────────────────┐    ┌─────────────────┐    ┌─────────────────┐
│   GUI Layer     │    │  Audio Manager  │    │ Theme Manager   │
│                 │    │                 │    │                 │
│ - main_window   │◄──►│ - PulseAudio    │    │ - Light/Dark    │
│ - settings      │    │ - Device mgmt   │    │ - System detect │
│ - device_cards  │    │ - Sync comp     │    │ - Styling       │
└─────────────────┘    └─────────────────┘    └─────────────────┘
         │                        │                        │
         └────────────────────────┼────────────────────────┘
                                  │
                        ┌─────────▼─────────┐
                        │   PulseAudio      │
                        │   System          │
                        └───────────────────┘
```

## Audio Management System

### PulseAudio Integration

The application uses three main PulseAudio modules:

#### 1. module-combine-sink
**Purpose**: Combines multiple audio devices into a single virtual sink
```bash
pactl load-module module-combine-sink \
  sink_name=sound_multiplexer_combined \
  slaves=device1,device2,device3 \
  sink_properties=device.description='Sound-Multiplexer (3 devices [Synced])'
```

#### 2. module-null-sink
**Purpose**: Creates a "black hole" sink that discards all audio
```bash
pactl load-module module-null-sink \
  sink_name=sound_multiplexer_null \
  sink_properties=device.description='Sound-Multiplexer-Null (No Output)'
```

#### 3. module-delay
**Purpose**: Adds precise time delays to compensate for device latency differences
```bash
pactl load-module module-delay \
  sink_name=sound_multiplexer_combined_delay_123_145ms \
  master=original_device_name \
  delay_time=145
```

### Audio Synchronization Algorithm

#### Device Latency Detection
```python
def get_estimated_latency_ms(self) -> int:
    """Estimate audio latency based on device type detection"""
    device_type = self.get_device_type()
    latency_map = {
        'bluetooth': 150,   # Bluetooth Classic ~100-200ms
        'usb': 5,          # USB audio ~1-10ms  
        'speakers': 2,     # Analog speakers ~1-3ms
        'headphones': 2,   # Analog headphones ~1-3ms
        'monitor': 8,      # HDMI audio ~5-15ms
        'digital': 5,      # Digital audio ~2-10ms
    }
    return latency_map.get(device_type, 5)
```

#### Sync Compensation Process
1. **Calculate Maximum Latency**: Find slowest device among selected outputs
2. **Determine Delays**: Calculate required delay for each faster device
3. **Create Delay Modules**: Use PulseAudio module-delay for precise timing
4. **Build Combined Sink**: Route all delay-compensated devices to combined sink

#### Example Calculation
```python
# Selected devices:
bluetooth_headphones = 150ms  # Slowest device
usb_speakers = 5ms           # Fast device
analog_output = 2ms          # Fastest device

# Compensation delays:
bluetooth_delay = 0ms        # No delay (reference)
usb_delay = 145ms           # 150 - 5 = 145ms
analog_delay = 148ms        # 150 - 2 = 148ms

# Result: All devices play synchronized audio
```

## Real-time System Monitoring

### PulseAudio Event Processing

The application monitors PulseAudio events in real-time using a dedicated background thread:

```python
def monitor_events():
    monitor_pulse = pulsectl.Pulse('sound-multiplexer-monitor')
    
    def event_callback(event):
        if event.facility == pulsectl.PulseEventFacilityEnum.sink:
            # Volume/mute changes detected
            QTimer.singleShot(100, self._check_system_changes)
    
    monitor_pulse.event_mask_set('sink')
    monitor_pulse.event_callback_set(event_callback)
    monitor_pulse.event_listen()
```

### Bidirectional Synchronization

#### System → Application
- **Volume changes** from system controls update UI sliders instantly
- **Mute toggles** from other applications update mute button states
- **Device plug/unplug** events refresh device list automatically

#### Application → System
- **Volume slider** changes immediately applied to PulseAudio
- **Mute button** toggles instantly update system mute state
- **Device selection** creates/destroys PulseAudio sinks in real-time

### Loop Prevention
```python
def set_device_volume(self, device_index: int, volume: float) -> None:
    if self._updating_from_system:
        return  # Prevent infinite loops
    # Apply volume change...
```

## Theme System Architecture

### Theme Detection and Management

#### System Theme Detection
```python
def _detect_system_theme(self) -> str:
    # GNOME theme detection
    result = subprocess.run([
        "gsettings", "get", "org.gnome.desktop.interface", "gtk-theme"
    ])
    if "dark" in result.stdout.lower():
        return "dark"
    
    # KDE theme detection  
    result = subprocess.run([
        "kreadconfig5", "--group", "General", "--key", "ColorScheme"
    ])
    if "dark" in result.stdout.lower():
        return "dark"
    
    # Qt palette fallback
    palette = QApplication.palette()
    if palette.color(QPalette.ColorRole.Window).lightness() < 128:
        return "dark"
    
    return "light"
```

#### Dynamic Theme Application
```python
def apply_theme(self):
    """Apply theme to all components"""
    # Main window styling
    self.setStyleSheet(self.theme_manager.get_main_window_style())
    
    # Update all device cards
    for device_widget in self.device_widgets.values():
        device_widget.apply_theme()
    
    # Settings dialog (if open)
    if hasattr(self, 'settings_dialog'):
        self.settings_dialog.apply_theme_to_dialog()
```

### Color Scheme Definition
```python
themes = {
    "light": {
        "window_bg": "#ffffff",
        "card_bg": "#fafafa", 
        "card_border": "#e0e0e0",
        "text_primary": "#333333",
        "accent_color": "#2196F3",
        # ... more colors
    },
    "dark": {
        "window_bg": "#1e1e1e",
        "card_bg": "#2d2d2d",
        "card_border": "#404040", 
        "text_primary": "#ffffff",
        "accent_color": "#3f51b5",
        # ... more colors
    }
}
```

## Device Type Detection

### Pattern Matching Algorithm
```python
def get_device_type(self) -> str:
    name_lower = self.name.lower()
    desc_lower = self.description.lower()
    
    # Priority-based detection
    if any(kw in name_lower or kw in desc_lower 
           for kw in ['bluetooth', 'bt', 'wireless']):
        return 'bluetooth'
    elif any(kw in name_lower or kw in desc_lower 
             for kw in ['hdmi', 'displayport', 'dp']):
        return 'monitor'
    elif any(kw in name_lower or kw in desc_lower 
             for kw in ['headphone', 'headset', 'head']):
        return 'headphones'
    # ... more patterns
```

### Icon Mapping
```python
def get_device_icon(self) -> str:
    device_type = self.device.get_device_type()
    icons = {
        'headphones': '🎧',
        'speakers': '🔊', 
        'monitor': '🖥️',
        'bluetooth': '🔗',
        'usb': '🔌',
        'digital': '💾'
    }
    return icons.get(device_type, '🔊')
```

## User Interface Architecture

### Card-Based Design System

Each audio device is represented by a `DeviceCardWidget` containing:

```python
class DeviceCardWidget(QFrame):
    def setup_ui(self):
        # Header: Icon + Device name checkbox
        header_layout = QHBoxLayout()
        icon_label = QLabel(self.get_device_icon())
        checkbox = QCheckBox(self.device.description)
        
        # Volume controls: Label + Slider + Percentage + Mute
        volume_layout = QVBoxLayout()
        volume_header = QHBoxLayout()
        volume_label = QLabel("Volume:")
        volume_value = QLabel("75%")
        mute_button = QPushButton("🔊")
        volume_slider = QSlider(Qt.Orientation.Horizontal)
```

### Responsive Grid Layout
```python
# Auto-arrange cards in 2-column grid
for i, device in enumerate(devices):
    device_card = DeviceCardWidget(device, audio_manager, theme_manager)
    row = i // 2
    col = i % 2
    self.devices_layout.addWidget(device_card, row, col)
```

### Signal-Slot Communication
```python
# Device state changes
device_card.device_enabled_changed.connect(main_window.update_status_message)

# System sync signals  
audio_manager.device_volume_changed.connect(main_window.on_system_volume_changed)
audio_manager.device_mute_changed.connect(main_window.on_system_mute_changed)

# Theme changes
theme_manager.theme_changed.connect(main_window.apply_theme)
```

## Settings and Persistence

### Configuration Storage
Using Qt's QSettings for cross-platform settings persistence:

```python
# Theme settings
self.theme_settings = QSettings("SoundMultiplexer", "Theme")
current_theme = self.theme_settings.value("theme", "system")

# Audio settings  
self.audio_settings = QSettings("SoundMultiplexer", "AudioSync")
sync_enabled = self.audio_settings.value("sync_compensation", True, type=bool)
```

### Settings Files Location
- **Linux**: `~/.config/SoundMultiplexer/`
  - `Theme.conf` - Theme preferences
  - `AudioSync.conf` - Audio synchronization settings

### Settings Dialog Architecture
```python
class SettingsDialog(QDialog):
    # Signals for real-time preview
    theme_changed = pyqtSignal(str)
    sync_compensation_changed = pyqtSignal(bool)
    
    def on_theme_preview(self):
        # Apply temporarily without saving
        self.theme_manager.set_theme(selected_theme, save=False)
        
    def apply_changes(self):
        # Permanently save all changes
        self.theme_manager.set_theme(selected_theme, save=True)
```

## Error Handling and Robustness

### PulseAudio Error Recovery
```python
def _create_virtual_sink(self, enabled_devices):
    try:
        subprocess.run(cmd, check=True, capture_output=True)
    except subprocess.CalledProcessError as e:
        print(f"Error creating virtual sink: {e}")
        # Graceful degradation - continue with available devices
```

### Resource Cleanup
```python
def cleanup(self):
    """Comprehensive cleanup on application exit"""
    try:
        self._monitoring_enabled = False      # Stop event monitoring
        self._remove_virtual_sink()           # Remove combine sink
        self._remove_null_sink()              # Remove null sink  
        self._remove_delay_modules()          # Remove all delay modules
        self.pulse.close()                    # Close PulseAudio connection
    except Exception as e:
        print(f"Error during cleanup: {e}")
```

### Thread Safety
- **Event monitoring** runs in dedicated background thread
- **UI updates** use `QTimer.singleShot()` for thread-safe operations
- **State flags** prevent infinite loops during system synchronization

## Performance Considerations

### Efficient Event Processing
- **Debounced updates**: `QTimer.singleShot(100ms)` prevents excessive refreshes
- **Selective monitoring**: Only monitor sink events, ignore unnecessary PulseAudio events
- **Lazy evaluation**: Only check system changes when events actually occur

### Memory Management
- **Widget cleanup**: Proper `deleteLater()` calls for removed device widgets
- **Module tracking**: Keep track of created PulseAudio modules for cleanup
- **Connection management**: Close PulseAudio connections when no longer needed

### Audio Performance
- **Low latency**: Direct PulseAudio module creation without unnecessary layers
- **Efficient routing**: Use native PulseAudio combine-sink for optimal performance
- **Minimal processing**: Delay compensation uses hardware-efficient module-delay

## Security Considerations

### PulseAudio Module Management
- **Naming conventions**: Use predictable module names for safe cleanup
- **Permission checks**: Verify PulseAudio access before attempting operations
- **Module isolation**: Only manage modules created by the application

### Settings Security
- **Safe defaults**: Sensible fallbacks when settings files are corrupted
- **Input validation**: Validate theme names and numeric settings
- **Path security**: Use Qt's standard settings locations, avoid custom paths

## Testing and Debugging

### Debug Mode
```bash
python main.py --debug
```

### Logging Output
The application provides comprehensive logging:
- PulseAudio module creation/destruction
- Device detection events
- Sync compensation calculations
- Error conditions and recovery

### Common Debug Information
```
Added 145ms delay to Built-in Audio
Created synced combine sink with 2 devices
Event monitoring error: Connection refused
Removed delay module: sound_multiplexer_combined_delay_123_145ms
```

## Future Enhancement Opportunities

### Advanced Sync Features
- **Manual delay adjustment**: Per-device delay sliders in UI
- **Acoustic calibration**: Microphone-based latency measurement
- **Codec-specific delays**: Bluetooth codec detection and compensation

### Enhanced Device Detection
- **USB device database**: Lookup known device latencies
- **Driver integration**: Direct communication with audio drivers
- **Machine learning**: Learn optimal delays based on user preferences

### Performance Optimization
- **Async operations**: Non-blocking PulseAudio operations
- **Cached device info**: Reduce system calls for known devices
- **Batch operations**: Group multiple PulseAudio changes together