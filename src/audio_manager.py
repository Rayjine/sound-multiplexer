"""
AudioManager - Handles PulseAudio operations for sound multiplexing
"""

import pulsectl
import threading
import time
from typing import List, Dict, Optional
from PyQt6.QtCore import QObject, pyqtSignal, QTimer


class AudioDevice:
    def __init__(self, index: int, name: str, description: str):
        self.index = index
        self.name = name
        self.description = description
        self.enabled = False
        self.volume = 1.0
        self.muted = False
    
    def get_device_type(self) -> str:
        """Determine device type based on name and description"""
        name_lower = self.name.lower()
        desc_lower = self.description.lower()
        
        # Check for specific device types in order of priority
        # Bluetooth devices (check before headphones since many BT devices are headphones)
        if any(keyword in name_lower or keyword in desc_lower for keyword in ['bluetooth', 'bt', 'wireless', 'bluez']):
            return 'bluetooth'
        # HDMI/DisplayPort audio
        elif any(keyword in name_lower or keyword in desc_lower for keyword in ['hdmi', 'displayport', 'dp']):
            return 'monitor'
        # Headphones/headsets (check for common headphone model patterns)
        elif any(keyword in name_lower or keyword in desc_lower for keyword in ['headphone', 'headset', 'head', 'wh-', 'airpods', 'beats', 'bose', 'sennheiser']):
            return 'headphones'
        # USB audio devices
        elif any(keyword in name_lower or keyword in desc_lower for keyword in ['usb', 'usb-audio']):
            return 'usb'
        # Digital audio
        elif 'digital' in name_lower or 'digital' in desc_lower:
            return 'digital'
        # Analog speakers
        elif any(keyword in name_lower or keyword in desc_lower for keyword in ['speaker', 'analog']):
            return 'speakers'
        else:
            return 'speakers'  # Default fallback
    
    def get_estimated_latency_ms(self) -> int:
        """Estimate audio latency in milliseconds based on device type"""
        device_type = self.get_device_type()
        latency_map = {
            'bluetooth': 300,   # Bluetooth Classic ~100-200ms
            'usb': 5,          # USB audio ~1-10ms  
            'speakers': 2,     # Analog speakers ~1-3ms
            'headphones': 2,   # Analog headphones ~1-3ms
            'monitor': 8,      # HDMI audio ~5-15ms
            'digital': 5,      # Digital audio ~2-10ms
        }
        return latency_map.get(device_type, 5)  # Default 5ms for unknown types


class AudioManager(QObject):
    devices_changed = pyqtSignal()
    device_volume_changed = pyqtSignal(int, float)  # device_index, new_volume
    device_mute_changed = pyqtSignal(int, bool)     # device_index, is_muted
    device_added = pyqtSignal()                     # New device detected
    device_removed = pyqtSignal()                   # Device removed
    
    def __init__(self):
        super().__init__()
        self.pulse = pulsectl.Pulse('sound-multiplexer')
        self.devices: List[AudioDevice] = []
        self.virtual_sink_name = "sound_multiplexer_combined"
        self.null_sink_name = "sound_multiplexer_null"
        self._updating_from_system = False  # Flag to prevent loops
        self._monitoring_enabled = True
        self._last_device_count = 0  # Track device count for add/remove detection
        
        # Sync compensation settings
        from PyQt6.QtCore import QSettings
        self._settings = QSettings("SoundMultiplexer", "AudioSync")
        self._sync_compensation_enabled = self._settings.value("sync_compensation", True, type=bool)
        self._delayed_sinks = []  # Track created delay modules for cleanup
        
        self.refresh_devices()
        self._set_current_device_as_default()
        
        # Start system monitoring
        self._start_system_monitoring()
    
    def refresh_devices(self) -> None:
        """Refresh the list of available audio output devices"""
        try:
            sinks = self.pulse.sink_list()
            
            # Preserve existing device states
            old_device_states = {}
            for device in self.devices:
                old_device_states[device.name] = {
                    'enabled': device.enabled,
                    'volume': device.volume,
                    'muted': device.muted
                }
            
            self.devices.clear()
            
            for sink in sinks:
                if not sink.name.startswith('sound_multiplexer'):
                    device = AudioDevice(
                        index=sink.index,
                        name=sink.name,
                        description=sink.description
                    )
                    
                    # Restore previous state if device existed before
                    if device.name in old_device_states:
                        device.enabled = old_device_states[device.name]['enabled']
                        device.volume = old_device_states[device.name]['volume']
                        device.muted = old_device_states[device.name]['muted']
                    else:
                        # Get current volume and mute state for new devices
                        try:
                            device.volume = sink.volume.value_flat
                            device.muted = sink.mute
                        except:
                            device.volume = 1.0
                            device.muted = False
                    
                    self.devices.append(device)
            
            # Check for device count changes (addition/removal)
            current_count = len(self.devices)
            if self._last_device_count != 0 and current_count != self._last_device_count:
                if current_count > self._last_device_count:
                    self.device_added.emit()
                else:
                    self.device_removed.emit()
            self._last_device_count = current_count
            
            self.devices_changed.emit()
        except Exception as e:
            print(f"Error refreshing devices: {e}")
    
    def get_devices(self) -> List[AudioDevice]:
        """Get list of available audio devices"""
        return self.devices
    
    def set_device_enabled(self, device_index: int, enabled: bool) -> None:
        """Enable or disable an audio device"""
        for device in self.devices:
            if device.index == device_index:
                device.enabled = enabled
                break
        self._update_virtual_sink()
    
    def set_device_volume(self, device_index: int, volume: float) -> None:
        """Set volume for a specific device (0.0 to 1.0)"""
        if self._updating_from_system:
            return  # Prevent loop when updating from system
            
        for device in self.devices:
            if device.index == device_index:
                device.volume = max(0.0, min(1.0, volume))
                break
        self._update_device_volumes()
    
    def set_device_muted(self, device_index: int, muted: bool) -> None:
        """Set mute state for a specific device"""
        if self._updating_from_system:
            return  # Prevent loop when updating from system
            
        for device in self.devices:
            if device.index == device_index:
                device.muted = muted
                break
        self._update_device_mute_states()
    
    def is_sync_compensation_enabled(self) -> bool:
        """Check if sync compensation is enabled"""
        return self._sync_compensation_enabled
    
    def set_sync_compensation_enabled(self, enabled: bool) -> None:
        """Enable or disable sync compensation"""
        if self._sync_compensation_enabled != enabled:
            self._sync_compensation_enabled = enabled
            self._settings.setValue("sync_compensation", enabled)
            # Recreate virtual sink with new settings
            self._update_virtual_sink()
    
    
    def _update_virtual_sink(self) -> None:
        """Update the virtual combined sink based on enabled devices"""
        enabled_devices = [d for d in self.devices if d.enabled]
        
        try:
            self._remove_virtual_sink()
            self._remove_null_sink()
            self._remove_delay_modules()
            
            if enabled_devices:
                self._create_virtual_sink(enabled_devices)
                self._set_as_default_sink(self.virtual_sink_name)
            else:
                # No devices selected - create null sink for complete silence
                self._create_null_sink()
                self._set_as_default_sink(self.null_sink_name)
        except Exception as e:
            print(f"Error updating virtual sink: {e}")
    
    def _create_virtual_sink(self, enabled_devices: List[AudioDevice]) -> None:
        """Create a combined sink with enabled devices, optionally with sync compensation"""
        if not enabled_devices:
            return
        
        import subprocess
        
        # Prepare slave devices (with or without delay compensation)
        slaves = []
        
        # Note: Advanced sync compensation with delay modules is not implemented
        # as module-delay doesn't exist in standard PulseAudio
        # For now, we use devices directly for simplicity
        slaves = [device.name for device in enabled_devices]
        
        # Create a descriptive name
        device_descriptions = [d.description for d in enabled_devices]
        if len(device_descriptions) == 1:
            combined_description = f"Sound-Multiplexer ({device_descriptions[0]})"
        else:
            combined_description = f"Sound-Multiplexer ({len(device_descriptions)} devices)"
        
        # Create the combine sink
        slaves_str = ",".join(slaves)
        cmd = [
            "pactl", "load-module", "module-combine-sink",
            f"sink_name={self.virtual_sink_name}",
            f"slaves={slaves_str}",
            f"sink_properties=device.description='{combined_description}',device.class='sound',device.intended_roles='music'"
        ]
        
        try:
            subprocess.run(cmd, check=True, capture_output=True)
            print(f"Created combine sink with {len(enabled_devices)} devices")
        except subprocess.CalledProcessError as e:
            print(f"Error creating virtual sink: {e}")
    
    def _create_null_sink(self) -> None:
        """Create a null sink that discards all audio (complete silence)"""
        import subprocess
        try:
            cmd = [
                "pactl", "load-module", "module-null-sink",
                f"sink_name={self.null_sink_name}",
                f"sink_properties=device.description='Sound-Multiplexer-Null (No Output)'"
            ]
            subprocess.run(cmd, check=True, capture_output=True)
        except subprocess.CalledProcessError as e:
            print(f"Error creating null sink: {e}")
    
    def _remove_virtual_sink(self) -> None:
        """Remove the existing virtual sink"""
        import subprocess
        try:
            result = subprocess.run(
                ["pactl", "list", "short", "modules"],
                capture_output=True, text=True, check=True
            )
            
            for line in result.stdout.split('\n'):
                if 'module-combine-sink' in line and self.virtual_sink_name in line:
                    module_id = line.split()[0]
                    subprocess.run(["pactl", "unload-module", module_id], check=True)
                    break
        except subprocess.CalledProcessError:
            pass
    
    def _remove_null_sink(self) -> None:
        """Remove the existing null sink"""
        import subprocess
        try:
            result = subprocess.run(
                ["pactl", "list", "short", "modules"],
                capture_output=True, text=True, check=True
            )
            
            for line in result.stdout.split('\n'):
                if 'module-null-sink' in line and self.null_sink_name in line:
                    module_id = line.split()[0]
                    subprocess.run(["pactl", "unload-module", module_id], check=True)
                    break
        except subprocess.CalledProcessError:
            pass
    
    def _remove_delay_modules(self) -> None:
        """Remove all delay modules created for sync compensation"""
        import subprocess
        
        for delayed_sink_name in self._delayed_sinks:
            try:
                result = subprocess.run(
                    ["pactl", "list", "short", "modules"],
                    capture_output=True, text=True, check=True
                )
                
                for line in result.stdout.split('\n'):
                    if 'module-delay' in line and delayed_sink_name in line:
                        module_id = line.split()[0]
                        subprocess.run(["pactl", "unload-module", module_id], check=True)
                        break
            except subprocess.CalledProcessError:
                pass  # Module might already be removed
        
        self._delayed_sinks.clear()
    
    def _set_as_default_sink(self, sink_name: str) -> None:
        """Set the specified sink as the default audio output"""
        import subprocess
        try:
            subprocess.run(
                ["pactl", "set-default-sink", sink_name],
                check=True, capture_output=True
            )
        except subprocess.CalledProcessError as e:
            print(f"Error setting default sink: {e}")
    
    def _update_device_volumes(self) -> None:
        """Update individual device volumes"""
        for device in self.devices:
            if device.enabled:
                try:
                    self.pulse.volume_set_all_chans(
                        self.pulse.get_sink_by_name(device.name),
                        device.volume
                    )
                except Exception as e:
                    print(f"Error setting volume for {device.name}: {e}")
    
    def _update_device_mute_states(self) -> None:
        """Update individual device mute states"""
        for device in self.devices:
            try:
                sink = self.pulse.get_sink_by_name(device.name)
                self.pulse.mute(sink, device.muted)
            except Exception as e:
                print(f"Error setting mute state for {device.name}: {e}")
    
    def _set_current_device_as_default(self) -> None:
        """Set the current default device as enabled on startup"""
        try:
            default_sink_info = self.pulse.server_info().default_sink_name
            for device in self.devices:
                if device.name == default_sink_info:
                    device.enabled = True
                    break
        except Exception as e:
            print(f"Error setting current device as default: {e}")
    
    def _start_system_monitoring(self) -> None:
        """Start monitoring system audio changes in a separate thread"""
        def monitor_events():
            try:
                # Create a separate pulse connection for monitoring
                monitor_pulse = pulsectl.Pulse('sound-multiplexer-monitor')
                
                # Set up event listener
                def event_callback(event):
                    if not self._monitoring_enabled:
                        return
                    
                    try:
                        if event.facility == pulsectl.PulseEventFacilityEnum.sink:
                            # For now, just handle all sink events as potential volume/mute changes
                            # We'll use the _check_system_changes method to determine what actually changed
                            QTimer.singleShot(100, self._check_system_changes)
                    except Exception as e:
                        # Log any issues but don't crash the monitoring
                        print(f"Event callback error: {e}")
                
                monitor_pulse.event_mask_set('sink')
                monitor_pulse.event_callback_set(event_callback)
                
                # Start the event loop
                while self._monitoring_enabled:
                    try:
                        monitor_pulse.event_listen(timeout=1.0)
                    except pulsectl.PulseLoopStop:
                        break
                    except Exception as e:
                        print(f"Event monitoring error: {e}")
                        time.sleep(1)  # Brief pause before retrying
                
                monitor_pulse.close()
                
            except Exception as e:
                print(f"Failed to start system monitoring: {e}")
        
        # Start monitoring in a background thread
        self._monitor_thread = threading.Thread(target=monitor_events, daemon=True)
        self._monitor_thread.start()
    
    def _check_system_changes(self) -> None:
        """Check for system volume/mute changes and device addition/removal"""
        if self._updating_from_system:
            return
            
        try:
            self._updating_from_system = True
            sinks = self.pulse.sink_list()
            
            # Create a mapping of current system state
            system_sinks = {}
            current_device_indices = set()
            
            for sink in sinks:
                if not sink.name.startswith('sound_multiplexer'):
                    system_sinks[sink.index] = {
                        'name': sink.name,
                        'description': sink.description,
                        'volume': sink.volume.value_flat,
                        'muted': sink.mute
                    }
                    current_device_indices.add(sink.index)
            
            # Check if device count changed (addition/removal)
            tracked_device_indices = {device.index for device in self.devices}
            
            if current_device_indices != tracked_device_indices:
                # Device list changed - trigger full refresh
                QTimer.singleShot(50, self.refresh_devices)
                return
            
            # Check for changes in our tracked devices
            for device in self.devices:
                if device.index in system_sinks:
                    system_sink = system_sinks[device.index]
                    
                    # Check volume changes
                    new_volume = system_sink['volume']
                    if abs(device.volume - new_volume) > 0.01:  # Small threshold for float comparison
                        device.volume = new_volume
                        self.device_volume_changed.emit(device.index, new_volume)
                    
                    # Check mute changes
                    new_muted = system_sink['muted']
                    if device.muted != new_muted:
                        device.muted = new_muted
                        self.device_mute_changed.emit(device.index, new_muted)
                        
        except Exception as e:
            print(f"Error checking system changes: {e}")
        finally:
            self._updating_from_system = False
    
    def cleanup(self) -> None:
        """Clean up resources"""
        try:
            self._monitoring_enabled = False
            self._remove_virtual_sink()
            self._remove_null_sink()
            self._remove_delay_modules()
            self.pulse.close()
        except Exception as e:
            print(f"Error during cleanup: {e}")