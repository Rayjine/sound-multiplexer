"""
MainWindow - GUI interface for Sound Multiplexer

Copyright (C) 2025  Nicolas Filimonov

This program is free software: you can redistribute it and/or modify
it under the terms of the GNU General Public License as published by
the Free Software Foundation, either version 3 of the License, or
(at your option) any later version.

This program is distributed in the hope that it will be useful,
but WITHOUT ANY WARRANTY; without even the implied warranty of
MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
GNU General Public License for more details.

You should have received a copy of the GNU General Public License
along with this program.  If not, see <https://www.gnu.org/licenses/>.
"""

from PyQt6.QtWidgets import (
    QMainWindow, QWidget, QVBoxLayout, QHBoxLayout, QCheckBox, 
    QSlider, QLabel, QPushButton, QScrollArea, QFrame, QGroupBox, QGridLayout
)
from PyQt6.QtCore import Qt, QTimer, pyqtSignal
from PyQt6.QtGui import QIcon, QFont, QPalette
from typing import Dict
from ..audio_manager import AudioManager, AudioDevice
from ..theme_manager import ThemeManager
from .settings_dialog import SettingsDialog


class DeviceCardWidget(QFrame):
    device_enabled_changed = pyqtSignal()  # Signal for when device enabled state changes
    
    def __init__(self, device: AudioDevice, audio_manager: AudioManager, theme_manager: ThemeManager):
        super().__init__()
        self.device = device
        self.audio_manager = audio_manager
        self.theme_manager = theme_manager
        self.setup_ui()
    
    def get_device_icon(self) -> str:
        """Get Unicode icon based on device type"""
        device_type = self.device.get_device_type()
        icons = {
            'headphones': '🎧',
            'speakers': '🔊',
            'monitor': '🖥️',
            'bluetooth': '🔗',
            'usb': '🔌',
            'digital': '🎛️'
        }
        return icons.get(device_type, '🔊')
    
    def setup_ui(self):
        self.setFrameStyle(QFrame.Shape.Box)
        self.apply_theme()
        
        layout = QVBoxLayout(self)
        layout.setContentsMargins(15, 15, 15, 15)
        layout.setSpacing(10)
        
        # Header with icon and checkbox
        header_layout = QHBoxLayout()
        
        icon_label = QLabel(self.get_device_icon())
        icon_label.setStyleSheet("font-size: 24px; margin-right: 10px;")
        
        self.checkbox = QCheckBox(self.device.description)
        self.checkbox.setChecked(self.device.enabled)
        self.checkbox.stateChanged.connect(self.on_enabled_changed)
        # Hide the default checkbox indicator and remove its spacing since we use a custom one
        self.checkbox.setStyleSheet(
            """
            QCheckBox::indicator {
                width: 0px;
                height: 0px;
                margin: 0px;
                padding: 0px;
            }
            QCheckBox {
                spacing: 0px; /* remove space reserved for the (now hidden) indicator */
            }
            """
        )
        
        # Add custom checkmark indicator
        self.checkbox_indicator = QLabel()
        # Slightly smaller box with no internal padding for a crisper checkmark
        self.checkbox_indicator.setFixedSize(28, 28)
        self.checkbox_indicator.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self.checkbox_indicator.mousePressEvent = self.on_checkbox_indicator_clicked
        self.checkbox_indicator.setCursor(Qt.CursorShape.PointingHandCursor)
        self.update_checkbox_indicator()
        
        header_layout.addWidget(icon_label)
        header_layout.addWidget(self.checkbox_indicator)
        header_layout.addWidget(self.checkbox, 1)
        
        layout.addLayout(header_layout)
        
        # Volume control section
        volume_layout = QVBoxLayout()
        volume_layout.setSpacing(5)
        
        volume_header = QHBoxLayout()
        self.volume_label = QLabel("Volume:")
        self.volume_value_label = QLabel(f"{int(self.device.volume * 100)}%")
        self.volume_value_label.setStyleSheet("font-weight: bold;")
        self.volume_value_label.setMinimumWidth(40)
        
        # Mute button
        self.mute_button = QPushButton()
        self.mute_button.setProperty("class", "mute-button")
        self.mute_button.setToolTip("Toggle Mute")
        self.mute_button.clicked.connect(self.on_mute_toggled)
        self.update_mute_button()
        
        volume_header.addWidget(self.volume_label)
        volume_header.addStretch()
        volume_header.addWidget(self.volume_value_label)
        volume_header.addWidget(self.mute_button)
        
        self.volume_slider = QSlider(Qt.Orientation.Horizontal)
        self.volume_slider.setRange(0, 100)
        self.volume_slider.setValue(int(self.device.volume * 100))
        self.volume_slider.valueChanged.connect(self.on_volume_changed)
        
        volume_layout.addLayout(volume_header)
        volume_layout.addWidget(self.volume_slider)
        
        layout.addLayout(volume_layout)
        
        self.update_enabled_state()
    
    def update_checkbox_indicator(self):
        """Update the custom checkbox indicator"""
        colors = self.theme_manager.get_theme_colors()
        if self.checkbox.isChecked():
            self.checkbox_indicator.setText("✓")
            self.checkbox_indicator.setStyleSheet(f"""
                QLabel {{
                    background-color: {colors['accent_color']};
                    color: white;
                    border: 2px solid {colors['accent_color']};
                    border-radius: 4px;
                    font-weight: bold;
                    font-size: 16px;
                    padding: 0px;
                    margin: 0px;
                }}
            """)
        else:
            self.checkbox_indicator.setText("")
            self.checkbox_indicator.setStyleSheet(f"""
                QLabel {{
                    background-color: {colors['card_bg']};
                    border: 2px solid {colors['card_border']};
                    border-radius: 4px;
                    padding: 0px;
                    margin: 0px;
                }}
            """)
    
    def on_checkbox_indicator_clicked(self, event):
        """Handle clicks on the custom checkbox indicator"""
        # Toggle the checkbox state
        self.checkbox.setChecked(not self.checkbox.isChecked())
    
    def on_enabled_changed(self, state):
        enabled = state == Qt.CheckState.Checked.value
        self.device.enabled = enabled
        self.audio_manager.set_device_enabled(self.device.index, enabled)
        self.update_checkbox_indicator()
        self.update_enabled_state()
        self.update_card_style()
        self.device_enabled_changed.emit()  # Notify main window of change
    
    def on_volume_changed(self, value):
        volume = value / 100.0
        self.device.volume = volume
        self.audio_manager.set_device_volume(self.device.index, volume)
        self.volume_value_label.setText(f"{value}%")
    
    def on_mute_toggled(self):
        """Toggle mute state"""
        self.device.muted = not self.device.muted
        self.audio_manager.set_device_muted(self.device.index, self.device.muted)
        self.update_mute_button()
        self.update_enabled_state()
    
    def update_mute_button(self):
        """Update mute button appearance based on mute state"""
        if self.device.muted:
            self.mute_button.setText("🔇")
            self.mute_button.setProperty("class", "mute-button muted")
            self.mute_button.setToolTip("Unmute")
        else:
            self.mute_button.setText("🔊")
            self.mute_button.setProperty("class", "mute-button")
            self.mute_button.setToolTip("Mute")
        
        # Refresh stylesheet to apply new properties
        self.mute_button.style().unpolish(self.mute_button)
        self.mute_button.style().polish(self.mute_button)
    
    def update_enabled_state(self):
        enabled = self.checkbox.isChecked()
        muted = self.device.muted
        
        # Enable/disable volume controls based on enabled state and mute state
        volume_controls_enabled = enabled and not muted
        
        self.volume_slider.setEnabled(volume_controls_enabled)
        self.volume_label.setEnabled(enabled)
        self.volume_value_label.setEnabled(enabled)
        self.mute_button.setEnabled(enabled)
        
        # Update volume label appearance when muted
        if muted:
            self.volume_value_label.setStyleSheet("font-weight: bold; color: #666;")
        else:
            colors = self.theme_manager.get_theme_colors()
            self.volume_value_label.setStyleSheet(f"font-weight: bold; color: {colors['accent_color']};")
        
        # Update mute button when card theme changes
        self.update_mute_button()
    
    def apply_theme(self):
        """Apply current theme to the card"""
        is_selected = hasattr(self, 'checkbox') and self.checkbox.isChecked()
        self.setStyleSheet(self.theme_manager.get_card_style(is_selected))
        
        # Update checkbox indicator, mute button and volume label colors
        if hasattr(self, 'checkbox_indicator'):
            self.update_checkbox_indicator()
        if hasattr(self, 'mute_button'):
            self.update_mute_button()
        if hasattr(self, 'volume_value_label'):
            self.update_enabled_state()
    
    def update_card_style(self):
        """Update card styling based on enabled state"""
        self.apply_theme()


class MainWindow(QMainWindow):
    def __init__(self, audio_manager: AudioManager):
        super().__init__()
        self.audio_manager = audio_manager
        self.theme_manager = ThemeManager()
        self.device_widgets: Dict[int, DeviceCardWidget] = {}
        self.setup_ui()
        self.setup_connections()
        
        # Apply initial theme
        self.apply_theme()
        
        self.refresh_timer = QTimer()
        self.refresh_timer.timeout.connect(self.refresh_devices)
        self.refresh_timer.start(5000)
    
    def setup_ui(self):
        self.setWindowTitle("Sound Multiplexer")
        self.setMinimumSize(700, 450)
        self.resize(900, 600)
        
        central_widget = QWidget()
        self.setCentralWidget(central_widget)
        main_layout = QVBoxLayout(central_widget)
        main_layout.setSpacing(10)
        main_layout.setContentsMargins(15, 15, 15, 15)
        
        # Header with title and settings button
        header_layout = QHBoxLayout()
        
        title_label = QLabel("Sound Multiplexer")
        title_font = QFont()
        title_font.setPointSize(16)
        title_font.setBold(True)
        title_label.setFont(title_font)
        
        self.settings_button = QPushButton("⚙️")
        self.settings_button.setToolTip("Settings")
        self.settings_button.setFixedSize(40, 40)
        self.settings_button.setStyleSheet("""
            QPushButton {
                font-size: 18px;
                border-radius: 20px;
                text-align: center;
                padding: 4px;
            }
        """)
        self.settings_button.clicked.connect(self.open_settings)
        
        header_layout.addWidget(title_label, 1)
        header_layout.addWidget(self.settings_button)
        
        main_layout.addLayout(header_layout)
        
        subtitle_label = QLabel("Select multiple audio output devices to play simultaneously")
        subtitle_label.setAlignment(Qt.AlignmentFlag.AlignCenter)
        subtitle_label.setStyleSheet("color: gray;")
        main_layout.addWidget(subtitle_label)
        
        
        devices_group = QGroupBox("Audio Output Devices")
        devices_group_layout = QVBoxLayout(devices_group)
        
        scroll_area = QScrollArea()
        scroll_area.setWidgetResizable(True)
        scroll_area.setFrameStyle(QFrame.Shape.NoFrame)
        scroll_area.setStyleSheet("QScrollArea { background-color: transparent; }")
        
        self.devices_widget = QWidget()
        self.devices_layout = QGridLayout(self.devices_widget)
        self.devices_layout.setSpacing(10)
        self.devices_layout.setContentsMargins(10, 10, 10, 10)
        # Set alignment to top-left to prevent widgets from moving down
        self.devices_layout.setAlignment(Qt.AlignmentFlag.AlignTop | Qt.AlignmentFlag.AlignLeft)
        
        scroll_area.setWidget(self.devices_widget)
        devices_group_layout.addWidget(scroll_area)
        
        main_layout.addWidget(devices_group, 1)
        
        button_layout = QHBoxLayout()
        
        self.refresh_button = QPushButton("Refresh Devices")
        self.refresh_button.clicked.connect(self.refresh_devices)
        
        self.select_all_button = QPushButton("Select All")
        self.select_all_button.clicked.connect(self.select_all_devices)
        
        self.deselect_all_button = QPushButton("Deselect All")
        self.deselect_all_button.clicked.connect(self.deselect_all_devices)
        
        button_layout.addWidget(self.refresh_button)
        button_layout.addStretch()
        button_layout.addWidget(self.select_all_button)
        button_layout.addWidget(self.deselect_all_button)
        
        main_layout.addLayout(button_layout)
        
        self.status_label = QLabel("Ready")
        self.status_label.setStyleSheet("color: green;")
        main_layout.addWidget(self.status_label)
        
        self.populate_devices()
    
    def setup_connections(self):
        self.audio_manager.devices_changed.connect(self.populate_devices)
        self.audio_manager.device_volume_changed.connect(self.on_system_volume_changed)
        self.audio_manager.device_mute_changed.connect(self.on_system_mute_changed)
        self.theme_manager.theme_changed.connect(self.apply_theme)
    
    def populate_devices(self):
        # Clear existing widgets
        for widget in self.device_widgets.values():
            widget.deleteLater()
        self.device_widgets.clear()
        
        # Clear the grid layout
        for i in reversed(range(self.devices_layout.count())):
            self.devices_layout.itemAt(i).widget().setParent(None)
        
        devices = self.audio_manager.get_devices()
        
        if not devices:
            no_devices_label = QLabel("No audio devices found")
            no_devices_label.setAlignment(Qt.AlignmentFlag.AlignCenter)
            no_devices_label.setStyleSheet("color: gray; font-style: italic; font-size: 16px; padding: 50px;")
            self.devices_layout.addWidget(no_devices_label, 0, 0, 1, 2)
            self.status_label.setText("No audio devices available")
            self.status_label.setStyleSheet("color: red;")
        else:
            # Arrange cards in a 2-column grid
            for i, device in enumerate(devices):
                device_card = DeviceCardWidget(device, self.audio_manager, self.theme_manager)
                device_card.device_enabled_changed.connect(self.update_status_message)
                row = i // 2
                col = i % 2
                self.devices_layout.addWidget(device_card, row, col)
                self.device_widgets[device.index] = device_card
            
            # Check if any devices are enabled
            enabled_count = sum(1 for device in devices if device.enabled)
            if enabled_count == 0:
                self.status_label.setText(f"Found {len(devices)} audio device(s) - No output selected (Silent mode)")
                self.status_label.setStyleSheet("color: orange;")
            else:
                self.status_label.setText(f"Found {len(devices)} audio device(s) - {enabled_count} device(s) enabled")
                self.status_label.setStyleSheet("color: green;")
    
    def refresh_devices(self):
        self.audio_manager.refresh_devices()
    
    def select_all_devices(self):
        for widget in self.device_widgets.values():
            widget.checkbox.setChecked(True)
        self.update_status_message()
    
    def deselect_all_devices(self):
        for widget in self.device_widgets.values():
            widget.checkbox.setChecked(False)
        self.update_status_message()
    
    
    def apply_theme(self):
        """Apply current theme to main window"""
        self.setStyleSheet(self.theme_manager.get_main_window_style())
        
        # Update all device cards
        for device_widget in self.device_widgets.values():
            device_widget.apply_theme()
    
    def on_system_volume_changed(self, device_index: int, new_volume: float):
        """Handle volume changes from system"""
        if device_index in self.device_widgets:
            widget = self.device_widgets[device_index]
            # Update slider without triggering its signal
            widget.volume_slider.blockSignals(True)
            widget.volume_slider.setValue(int(new_volume * 100))
            widget.volume_slider.blockSignals(False)
            # Update volume label
            widget.volume_value_label.setText(f"{int(new_volume * 100)}%")
    
    def on_system_mute_changed(self, device_index: int, is_muted: bool):
        """Handle mute changes from system"""
        if device_index in self.device_widgets:
            widget = self.device_widgets[device_index]
            # Update mute button and enabled state
            widget.update_mute_button()
            widget.update_enabled_state()
    
    def update_status_message(self):
        """Update status message based on enabled devices"""
        devices = self.audio_manager.get_devices()
        if not devices:
            return
            
        enabled_count = sum(1 for device in devices if device.enabled)
        if enabled_count == 0:
            self.status_label.setText(f"Found {len(devices)} audio device(s) - No output selected (Silent mode)")
            self.status_label.setStyleSheet("color: orange;")
        else:
            self.status_label.setText(f"Found {len(devices)} audio device(s) - {enabled_count} device(s) enabled")
            self.status_label.setStyleSheet("color: green;")
    
    def open_settings(self):
        """Open settings dialog"""
        dialog = SettingsDialog(self.theme_manager, self.audio_manager, self)
        dialog.theme_changed.connect(self.apply_theme)
        dialog.sync_compensation_changed.connect(self.on_sync_compensation_changed)
        dialog.exec()
    
    def on_sync_compensation_changed(self, enabled: bool):
        """Handle sync compensation setting change"""
        # The audio manager already applies the change, we just need to update status if needed
        self.update_status_message()
    
    def closeEvent(self, event):
        self.audio_manager.cleanup()
        event.accept()