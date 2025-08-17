"""
Settings Dialog - Theme and preferences management

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
    QDialog, QVBoxLayout, QHBoxLayout, QLabel, QComboBox, 
    QPushButton, QGroupBox, QDialogButtonBox, QCheckBox
)
from PyQt6.QtCore import Qt, pyqtSignal
from PyQt6.QtGui import QFont
from ..theme_manager import ThemeManager


class SettingsDialog(QDialog):
    theme_changed = pyqtSignal(str)
    sync_compensation_changed = pyqtSignal(bool)
    
    def __init__(self, theme_manager: ThemeManager, audio_manager, parent=None):
        super().__init__(parent)
        self.theme_manager = theme_manager
        self.audio_manager = audio_manager
        self.original_theme = theme_manager.get_current_theme()
        self.original_sync_compensation = audio_manager.is_sync_compensation_enabled()
        self.setup_ui()
    
    def setup_ui(self):
        self.setWindowTitle("Settings")
        self.setModal(True)
        self.setFixedSize(450, 320)
        
        layout = QVBoxLayout(self)
        layout.setSpacing(20)
        layout.setContentsMargins(20, 20, 20, 20)
        
        # Title
        title_label = QLabel("Settings")
        title_font = QFont()
        title_font.setPointSize(16)
        title_font.setBold(True)
        title_label.setFont(title_font)
        title_label.setAlignment(Qt.AlignmentFlag.AlignCenter)
        layout.addWidget(title_label)
        
        # Theme settings group
        theme_group = QGroupBox("Appearance")
        theme_layout = QVBoxLayout(theme_group)
        theme_layout.setSpacing(10)
        
        # Theme selection
        theme_selection_layout = QHBoxLayout()
        theme_label = QLabel("Theme:")
        theme_label.setMinimumWidth(80)
        
        self.theme_combo = QComboBox()
        available_themes = self.theme_manager.get_available_themes()
        
        for theme_key, theme_name in available_themes.items():
            self.theme_combo.addItem(theme_name, theme_key)
        
        # Set current theme in combo box
        current_theme_setting = self.theme_manager.settings.value("theme", "system")
        current_index = self.theme_combo.findData(current_theme_setting)
        if current_index >= 0:
            self.theme_combo.setCurrentIndex(current_index)
        
        self.theme_combo.currentTextChanged.connect(self.on_theme_preview)
        
        theme_selection_layout.addWidget(theme_label)
        theme_selection_layout.addWidget(self.theme_combo, 1)
        
        theme_layout.addLayout(theme_selection_layout)
        
        # Theme description
        self.theme_description = QLabel()
        self.theme_description.setWordWrap(True)
        self.theme_description.setStyleSheet("color: gray; font-style: italic;")
        self.update_theme_description()
        theme_layout.addWidget(self.theme_description)
        
        layout.addWidget(theme_group)
        
        # Audio settings group
        audio_group = QGroupBox("Audio Synchronization")
        audio_layout = QVBoxLayout(audio_group)
        audio_layout.setSpacing(10)
        
        # Sync compensation checkbox
        self.sync_compensation_checkbox = QCheckBox("Enable audio sync compensation")
        self.sync_compensation_checkbox.setChecked(self.audio_manager.is_sync_compensation_enabled())
        self.sync_compensation_checkbox.stateChanged.connect(self.on_sync_compensation_changed)
        
        # Sync compensation description
        sync_description = QLabel()
        sync_description.setText(
            "Automatically adds delays to faster devices (wired audio) to synchronize "
            "with slower devices (Bluetooth). This reduces audio echo when using multiple "
            "output types simultaneously."
        )
        sync_description.setWordWrap(True)
        sync_description.setStyleSheet("color: gray; font-style: italic; font-size: 11px;")
        
        audio_layout.addWidget(self.sync_compensation_checkbox)
        audio_layout.addWidget(sync_description)
        
        layout.addWidget(audio_group)
        
        layout.addStretch()
        
        # Dialog buttons
        button_box = QDialogButtonBox(
            QDialogButtonBox.StandardButton.Ok | 
            QDialogButtonBox.StandardButton.Cancel |
            QDialogButtonBox.StandardButton.Apply
        )
        
        button_box.accepted.connect(self.accept_changes)
        button_box.rejected.connect(self.reject_changes)
        button_box.button(QDialogButtonBox.StandardButton.Apply).clicked.connect(self.apply_changes)
        
        layout.addWidget(button_box)
        
        # Apply current theme to dialog
        self.apply_theme_to_dialog()
    
    def update_theme_description(self):
        """Update theme description based on selection"""
        current_data = self.theme_combo.currentData()
        descriptions = {
            "system": "Automatically follows your system's light/dark mode setting",
            "light": "Light theme with bright colors and dark text",
            "dark": "Dark theme with dark colors and light text"
        }
        self.theme_description.setText(descriptions.get(current_data, ""))
    
    def on_theme_preview(self):
        """Preview theme selection"""
        self.update_theme_description()
        selected_theme = self.theme_combo.currentData()
        if selected_theme:
            # Apply theme temporarily for preview
            self.theme_manager.set_theme(selected_theme, save=False)
            self.theme_changed.emit(selected_theme)
            self.apply_theme_to_dialog()
    
    def on_sync_compensation_changed(self, state):
        """Handle sync compensation checkbox change"""
        enabled = state == Qt.CheckState.Checked.value
        # Apply change immediately for preview
        self.audio_manager.set_sync_compensation_enabled(enabled)
        self.sync_compensation_changed.emit(enabled)
    
    def apply_theme_to_dialog(self):
        """Apply current theme to this dialog"""
        colors = self.theme_manager.get_theme_colors()
        
        dialog_style = f"""
            QDialog {{
                background-color: {colors['window_bg']};
                color: {colors['text_primary']};
            }}
            QGroupBox {{
                font-weight: bold;
                border: 2px solid {colors['group_box_border']};
                border-radius: 8px;
                margin-top: 1ex;
                padding-top: 10px;
                background-color: {colors['group_box_bg']};
                color: {colors['text_primary']};
            }}
            QGroupBox::title {{
                subcontrol-origin: margin;
                left: 10px;
                padding: 0 5px 0 5px;
                color: {colors['text_primary']};
            }}
            QLabel {{
                color: {colors['text_primary']};
                background-color: transparent;
            }}
            QComboBox {{
                background-color: {colors['card_bg']};
                color: {colors['text_primary']};
                border: 2px solid {colors['card_border']};
                border-radius: 6px;
                padding: 6px 12px;
                min-width: 120px;
            }}
            QComboBox:hover {{
                border-color: {colors['card_hover_border']};
                background-color: {colors['card_hover_bg']};
            }}
            QComboBox::drop-down {{
                border: none;
                width: 20px;
            }}
            QComboBox::down-arrow {{
                image: none;
                border-left: 5px solid transparent;
                border-right: 5px solid transparent;
                border-top: 5px solid {colors['text_primary']};
                margin-right: 5px;
            }}
            QComboBox QAbstractItemView {{
                background-color: {colors['card_bg']};
                color: {colors['text_primary']};
                border: 1px solid {colors['card_border']};
                selection-background-color: {colors['accent_color']};
            }}
            QPushButton {{
                background-color: {colors['card_bg']};
                color: {colors['text_primary']};
                border: 2px solid {colors['card_border']};
                border-radius: 6px;
                padding: 8px 16px;
                font-weight: bold;
                min-width: 80px;
            }}
            QPushButton:hover {{
                background-color: {colors['card_hover_bg']};
                border-color: {colors['card_hover_border']};
            }}
            QPushButton:pressed {{
                background-color: {colors['accent_color']};
                color: white;
            }}
        """
        
        self.setStyleSheet(dialog_style)
    
    def apply_changes(self):
        """Apply theme changes without closing dialog"""
        selected_theme = self.theme_combo.currentData()
        if selected_theme:
            self.theme_manager.set_theme(selected_theme, save=True)
    
    def accept_changes(self):
        """Accept and apply changes, then close dialog"""
        self.apply_changes()
        self.accept()
    
    def reject_changes(self):
        """Reject changes and restore original settings"""
        # Restore original theme
        self.theme_manager.set_theme(self.original_theme, save=False)
        self.theme_changed.emit(self.original_theme)
        
        # Restore original sync compensation setting
        self.audio_manager.set_sync_compensation_enabled(self.original_sync_compensation)
        self.sync_compensation_changed.emit(self.original_sync_compensation)
        
        self.reject()