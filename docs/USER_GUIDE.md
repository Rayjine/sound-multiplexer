# 📖 Sound Multiplexer User Guide

## Getting Started

### First Launch
When you first launch Sound Multiplexer, the application will:
1. **Detect your audio devices** automatically
2. **Select your current default device** (the one you're currently using)
3. **Display device cards** in a clean, organized grid layout
4. **Show current volume levels** for each device

### Main Interface Overview

```
┌─────────────────────────────────────────────────────────┐
│ Sound Multiplexer                                    ⚙️ │
│ Select multiple audio output devices simultaneously      │
├─────────────────────────────────────────────────────────┤
│ Audio Output Devices                                    │
│ ┌──────────────────┐ ┌──────────────────┐              │
│ │ 🎧 ☑ Headphones  │ │ 🔊 ☐ Speakers    │              │
│ │ Volume:      75% │ │ Volume:      50% │              │
│ │ ████████████▒▒▒  │ │ ██████▒▒▒▒▒▒▒▒▒  │              │
│ │              🔊  │ │              🔇  │              │
│ └──────────────────┘ └──────────────────┘              │
│                                                         │
│ [Refresh] [Select All] [Deselect All]                  │
│ Found 2 devices - 1 device enabled                     │
└─────────────────────────────────────────────────────────┘
```

## Basic Operations

### Selecting Audio Devices

#### ✅ **Enable a Device**
- **Click the checkbox** next to the device name to enable it
- **Result**: Audio will start playing through that device
- **Visual feedback**: Checkbox shows ✓ and card border turns blue

#### ☐ **Disable a Device** 
- **Uncheck the checkbox** to disable the device
- **Result**: Audio stops playing through that device
- **Visual feedback**: Checkbox becomes empty and card returns to normal

#### 🔄 **Multiple Devices**
- **Check multiple boxes** to play audio through several devices simultaneously
- **Perfect for**: Listening through headphones while others hear speakers
- **No limit**: Select as many devices as you want

### Volume Control

#### 🎚️ **Individual Volume Sliders**
Each device has its own volume control:
- **Drag the slider** left (quieter) or right (louder)
- **Range**: 0% (silent) to 100% (maximum)
- **Real-time**: Changes apply instantly
- **Percentage display**: Shows exact volume level

#### 🔊 **Mute Controls**
Each device has a dedicated mute button:
- **🔊 (Unmuted)**: Click to mute the device
- **🔇 (Muted)**: Click to unmute the device
- **Independent**: Mute one device while others continue playing
- **Volume preservation**: Volume setting is remembered when unmuted

### Device Management

#### 🔌 **Hot-Plugging Support**
- **Plug in headphones**: They appear automatically in the interface
- **Connect Bluetooth**: New wireless devices are detected instantly
- **Unplug devices**: Removed devices disappear from the list
- **No restart needed**: Changes happen in real-time

#### 🔍 **Device Type Recognition**
The application automatically detects and shows appropriate icons:
- **🎧 Headphones**: Wired and wireless headphones/headsets
- **🔊 Speakers**: Desktop speakers, laptop speakers, soundbars
- **🖥️ Monitors**: HDMI/DisplayPort audio from monitors and TVs
- **🔗 Bluetooth**: Wireless audio devices
- **🔌 USB**: USB audio interfaces and DACs
- **💾 Digital**: S/PDIF, optical, and other digital audio

## Advanced Features

### Audio Synchronization

#### 🎵 **The Problem: Audio Echo**
When using multiple device types simultaneously, you might hear audio echo:
- **Wired devices** (speakers, wired headphones): Very low latency (~2-5ms)
- **Bluetooth devices**: Higher latency (~150ms)
- **Result**: You hear audio from wired devices first, then Bluetooth → Echo effect

#### ⚡ **The Solution: Sync Compensation**
Sound Multiplexer can automatically fix this:

1. **Open Settings** (⚙️ button in top-right corner)
2. **Find "Audio Synchronization" section**
3. **Check "Enable audio sync compensation"**
4. **How it works**: Adds delays to faster devices to match slower ones

#### 📊 **Automatic Delay Calculation**
The system knows typical latencies for different device types:
```
Device Type          | Typical Latency
--------------------|----------------
Bluetooth           | ~150ms
HDMI/Monitor        | ~8ms  
USB Audio           | ~5ms
Analog Speakers     | ~2ms
Wired Headphones    | ~2ms
```

#### 🎯 **Example Scenario**
You select:
- **Bluetooth headphones** (150ms latency)
- **USB speakers** (5ms latency)

**Without sync compensation**: You hear USB speakers 145ms before Bluetooth
**With sync compensation**: USB speakers get +145ms delay → Perfect synchronization!

### Silent Mode

#### 🔇 **True Silence**
Unlike other audio systems, Sound Multiplexer provides true silence:
- **Normal behavior**: When you disable all devices, system falls back to default
- **Sound Multiplexer**: When no devices selected, creates complete silence
- **No audio anywhere**: System audio is routed to a "null sink" that discards it

#### 🧡 **Visual Indicator**
When in silent mode:
- **Status message**: "No output selected (Silent mode)" in orange
- **System audio list**: Shows "Sound-Multiplexer-Null (No Output)"
- **Complete control**: No unwanted audio output

### Theme Customization

#### 🎨 **Built-in Themes**
Access via Settings (⚙️) → Appearance:

**System Default** (Recommended)
- Automatically follows your desktop theme
- Dark desktop = dark app, light desktop = light app
- Updates when you change system theme

**Light Mode**
- Clean, bright interface
- Dark text on light backgrounds
- Blue accent colors
- Perfect for daytime use

**Dark Mode**  
- Easy on the eyes
- Light text on dark backgrounds
- Purple/blue accents
- Great for evening use

#### 🔄 **Live Preview**
- **Instant updates**: Theme changes apply immediately
- **No restart needed**: See changes while settings dialog is open
- **Easy comparison**: Switch between themes to find your preference

## Interface Elements Guide

### Device Cards

Each audio device is shown in its own card:

```
┌──────────────────────────────────────────┐
│ 🎧 ☑ Sony WH-1000XM4 Headphones         │ ← Device icon & checkbox
│                                          │
│ Volume:                            85% 🔊 │ ← Volume label, percentage, mute
│ ████████████████████▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒  │ ← Volume slider
└──────────────────────────────────────────┘
```

#### Card States
- **Unselected**: Gray border, standard colors
- **Selected**: Blue border, highlighted background
- **Hover**: Slightly darker background for visual feedback

### Status Bar

Bottom status bar shows current state:
- **Green**: "Found X devices - Y device(s) enabled" (normal operation)
- **Orange**: "Found X devices - No output selected (Silent mode)" (silent mode)
- **Red**: "No audio devices available" (no devices detected)

### Control Buttons

#### **Refresh Devices**
- **Purpose**: Manually refresh the device list
- **When to use**: If a device isn't appearing automatically
- **Note**: Usually not needed due to automatic detection

#### **Select All**
- **Purpose**: Enable all available audio devices at once
- **Useful for**: Testing all speakers, maximum audio coverage
- **Shortcut**: Faster than checking each device individually

#### **Deselect All**
- **Purpose**: Disable all devices for silent mode
- **Instant silence**: Immediately stops all audio output
- **Useful for**: Quick audio mute, testing silence mode

## Tips and Best Practices

### 🎯 **Optimal Setup Recommendations**

#### **Home Entertainment**
- **Main speakers** + **Wireless headphones**
- **Enable sync compensation** for echo-free experience
- **Individual volume control** for different listening preferences

#### **Content Creation**
- **Monitor headphones** for detailed audio work
- **Reference speakers** for mix checking
- **Keep sync disabled** if monitoring latency is critical

#### **Gaming**
- **Gaming headset** for game audio and voice chat
- **Desktop speakers** for background music or shared audio
- **Quick device switching** without restarting games

### ⚡ **Performance Tips**

#### **Bluetooth Optimization**
- **Use high-quality codecs** (aptX, LDAC) if available
- **Keep devices close** to reduce latency variations
- **Enable sync compensation** when mixing with wired devices

#### **System Resources**
- **Modern systems**: Can handle 4+ simultaneous devices easily
- **Older hardware**: Limit to 2-3 devices to avoid stuttering
- **CPU usage**: Sync compensation adds minimal overhead

### 🛠️ **Troubleshooting Quick Fixes**

#### **Device Not Appearing**
1. Check device is properly connected
2. Click "Refresh Devices"
3. Verify device works in system audio settings
4. Restart Sound Multiplexer if needed

#### **Audio Stuttering**
1. Reduce number of active devices
2. Disable sync compensation temporarily
3. Check system CPU usage
4. Close other audio applications

#### **Echo Between Devices**
1. Enable sync compensation in settings
2. If still present, try disabling/re-enabling devices
3. Check if devices have individual delay settings

#### **Volume Not Syncing**
1. Change volume from system settings to test sync
2. Restart the application if sync is broken
3. Check PulseAudio is running properly

### 🔧 **Advanced Usage**

#### **System Integration**
- **Audio shows in system settings**: "Sound-Multiplexer" appears as selectable output
- **Manual switching**: Can switch to/from multiplexer in system audio settings
- **Other apps work**: All applications route through the multiplexer automatically

#### **Command Line Verification**
Check active modules:
```bash
pactl list short modules | grep sound_multiplexer
```

Check active sinks:
```bash  
pactl list short sinks | grep sound_multiplexer
```

#### **Professional Audio Workflows**
- **DAW compatibility**: Works with most Digital Audio Workstations
- **Low latency**: Minimal additional latency when sync compensation disabled
- **Multiple monitors**: Perfect for multi-screen setups with individual speaker sets

## Keyboard Shortcuts

Currently, Sound Multiplexer uses mouse interaction, but future versions may include:
- **Ctrl+A**: Select all devices
- **Ctrl+D**: Deselect all devices  
- **Ctrl+R**: Refresh devices
- **Ctrl+,**: Open settings

## Accessibility Features

### Visual Accessibility
- **Large checkboxes**: Enhanced visibility for device selection
- **Clear icons**: Distinct device type indicators
- **High contrast**: Good contrast ratios in both light and dark themes
- **Readable fonts**: Clear, standard font sizes throughout

### Motor Accessibility  
- **Large click targets**: Easy-to-click buttons and sliders
- **Keyboard navigation**: Standard Qt keyboard navigation support
- **No precision required**: Forgiving interaction areas

### Audio Accessibility
- **Multiple output support**: Route to hearing aids + speakers simultaneously
- **Individual volume control**: Customize levels for different needs
- **Visual feedback**: No reliance on audio cues for operation

---

*This user guide covers all major features of Sound Multiplexer. For technical details, see [TECHNICAL.md](TECHNICAL.md). For installation and setup, see the main [README.md](../README.md).*