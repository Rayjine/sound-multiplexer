"""
Theme Manager - Handles light/dark mode theming for Sound Multiplexer

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

import json
import os
from typing import Dict, Any
from PyQt6.QtCore import QObject, pyqtSignal, QSettings
from PyQt6.QtGui import QPalette, QColor
from PyQt6.QtWidgets import QApplication


class ThemeManager(QObject):
    theme_changed = pyqtSignal(str)  # Emits theme name when changed
    
    def __init__(self):
        super().__init__()
        self.settings = QSettings("SoundMultiplexer", "Theme")
        self.themes = self._define_themes()
        self.current_theme = self._get_saved_theme()
    
    def _define_themes(self) -> Dict[str, Dict[str, Any]]:
        """Define color schemes for different themes"""
        return {
            "light": {
                "name": "Light Mode",
                "window_bg": "#ffffff",
                "card_bg": "#fafafa",
                "card_border": "#e0e0e0",
                "card_hover_bg": "#f0f0f0",
                "card_hover_border": "#b0b0b0",
                "card_selected_bg": "#e3f2fd",
                "card_selected_border": "#2196F3",
                "text_primary": "#333333",
                "text_secondary": "#666666",
                "text_disabled": "#999999",
                "accent_color": "#2196F3",
                "accent_hover": "#1976D2",
                "success_color": "#4CAF50",
                "error_color": "#f44336",
                "slider_groove": "#ffffff",
                "slider_groove_border": "#bbbbbb",
                "slider_handle": "#2196F3",
                "group_box_bg": "#f8f9fa",
                "group_box_border": "#dee2e6",
                "scroll_bg": "transparent"
            },
            "dark": {
                "name": "Dark Mode", 
                "window_bg": "#1e1e1e",
                "card_bg": "#2d2d2d",
                "card_border": "#404040",
                "card_hover_bg": "#353535",
                "card_hover_border": "#505050",
                "card_selected_bg": "#1a237e",
                "card_selected_border": "#3f51b5",
                "text_primary": "#ffffff",
                "text_secondary": "#cccccc",
                "text_disabled": "#666666",
                "accent_color": "#3f51b5",
                "accent_hover": "#303f9f",
                "success_color": "#66bb6a",
                "error_color": "#ef5350",
                "slider_groove": "#404040",
                "slider_groove_border": "#555555",
                "slider_handle": "#3f51b5",
                "group_box_bg": "#252525",
                "group_box_border": "#404040",
                "scroll_bg": "transparent"
            }
        }
    
    def _get_saved_theme(self) -> str:
        """Get saved theme preference or detect system theme"""
        saved_theme = self.settings.value("theme", "system")
        
        if saved_theme == "system":
            return self._detect_system_theme()
        elif saved_theme in self.themes:
            return saved_theme
        else:
            return self._detect_system_theme()
    
    def _detect_system_theme(self) -> str:
        """Detect system theme preference"""
        try:
            # Try to detect system theme on Linux
            import subprocess
            
            # Check GNOME theme
            try:
                result = subprocess.run(
                    ["gsettings", "get", "org.gnome.desktop.interface", "gtk-theme"],
                    capture_output=True, text=True, check=True
                )
                theme_name = result.stdout.strip().strip("'\"").lower()
                if "dark" in theme_name:
                    return "dark"
            except (subprocess.CalledProcessError, FileNotFoundError):
                pass
            
            # Check KDE theme
            try:
                result = subprocess.run(
                    ["kreadconfig5", "--group", "General", "--key", "ColorScheme"],
                    capture_output=True, text=True, check=True
                )
                theme_name = result.stdout.strip().lower()
                if "dark" in theme_name:
                    return "dark"
            except (subprocess.CalledProcessError, FileNotFoundError):
                pass
            
            # Check Qt palette as fallback
            palette = QApplication.palette()
            window_color = palette.color(QPalette.ColorRole.Window)
            if window_color.lightness() < 128:
                return "dark"
            else:
                return "light"
                
        except Exception:
            # Default to light if detection fails
            return "light"
    
    def get_current_theme(self) -> str:
        """Get current theme name"""
        return self.current_theme
    
    def get_theme_colors(self, theme_name: str = None) -> Dict[str, str]:
        """Get color scheme for specified theme or current theme"""
        if theme_name is None:
            theme_name = self.current_theme
        return self.themes.get(theme_name, self.themes["light"])
    
    def set_theme(self, theme_name: str, save: bool = True):
        """Set theme and optionally save preference"""
        if theme_name == "system":
            actual_theme = self._detect_system_theme()
        elif theme_name in self.themes:
            actual_theme = theme_name
        else:
            actual_theme = "light"
        
        if actual_theme != self.current_theme:
            self.current_theme = actual_theme
            if save:
                self.settings.setValue("theme", theme_name)
            self.theme_changed.emit(actual_theme)
    
    def get_available_themes(self) -> Dict[str, str]:
        """Get list of available themes with display names"""
        themes = {"system": "System Default"}
        for key, value in self.themes.items():
            themes[key] = value["name"]
        return themes
    
    def get_main_window_style(self) -> str:
        """Get main window stylesheet"""
        colors = self.get_theme_colors()
        return f"""
            QMainWindow {{
                background-color: {colors['window_bg']};
                color: {colors['text_primary']};
            }}
            QWidget {{
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
            QPushButton {{
                background-color: {colors['card_bg']};
                color: {colors['text_primary']};
                border: 2px solid {colors['card_border']};
                border-radius: 6px;
                padding: 8px 16px;
                font-weight: bold;
            }}
            QPushButton:hover {{
                background-color: {colors['card_hover_bg']};
                border-color: {colors['card_hover_border']};
            }}
            QPushButton:pressed {{
                background-color: {colors['accent_color']};
                color: white;
            }}
            QScrollArea {{
                background-color: {colors['scroll_bg']};
                border: none;
            }}
            QScrollBar:vertical {{
                background-color: {colors['card_bg']};
                width: 12px;
                border-radius: 6px;
            }}
            QScrollBar::handle:vertical {{
                background-color: {colors['card_border']};
                border-radius: 6px;
                min-height: 20px;
            }}
            QScrollBar::handle:vertical:hover {{
                background-color: {colors['card_hover_border']};
            }}
        """
    
    def get_card_style(self, is_selected: bool = False) -> str:
        """Get card widget stylesheet"""
        colors = self.get_theme_colors()
        
        if is_selected:
            bg_color = colors['card_selected_bg']
            border_color = colors['card_selected_border']
        else:
            bg_color = colors['card_bg']
            border_color = colors['card_border']
        
        return f"""
            QFrame {{
                border: 2px solid {border_color};
                border-radius: 10px;
                background-color: {bg_color};
                margin: 5px;
                padding: 10px;
            }}
            QFrame:hover {{
                border-color: {colors['card_hover_border']};
                background-color: {colors['card_hover_bg']};
            }}
            QCheckBox {{
                font-size: 14px;
                font-weight: bold;
                color: {colors['text_primary']};
                background-color: transparent;
                spacing: 8px;
            }}
            QCheckBox::indicator {{
                width: 20px;
                height: 20px;
                border-radius: 4px;
                border: 2px solid {colors['card_border']};
                background-color: {colors['card_bg']};
            }}
            QCheckBox::indicator:hover {{
                border-color: {colors['accent_color']};
                background-color: {colors['card_hover_bg']};
            }}
            QCheckBox::indicator:checked {{
                border-color: {colors['accent_color']};
                background-color: {colors['accent_color']};
            }}
            QCheckBox:checked {{
                color: {colors['accent_color']};
            }}
            QLabel {{
                color: {colors['text_primary']};
                background-color: transparent;
            }}
            QSlider::groove:horizontal {{
                border: 1px solid {colors['slider_groove_border']};
                background: {colors['slider_groove']};
                height: 8px;
                border-radius: 4px;
            }}
            QSlider::handle:horizontal {{
                background: {colors['slider_handle']};
                border: 1px solid {colors['slider_groove_border']};
                width: 18px;
                margin: -5px 0;
                border-radius: 9px;
            }}
            QSlider::handle:horizontal:hover {{
                background: {colors['accent_hover']};
            }}
            QPushButton.mute-button {{
                background-color: {colors['card_bg']};
                color: {colors['text_primary']};
                border: 2px solid {colors['card_border']};
                border-radius: 6px;
                padding: 6px;
                font-size: 16px;
                min-width: 40px;
                max-width: 40px;
                min-height: 32px;
                max-height: 32px;
            }}
            QPushButton.mute-button:hover {{
                background-color: {colors['card_hover_bg']};
                border-color: {colors['card_hover_border']};
            }}
            QPushButton.mute-button:pressed {{
                background-color: {colors['accent_color']};
                color: white;
            }}
            QPushButton.mute-button.muted {{
                background-color: {colors['error_color']};
                color: white;
                border-color: {colors['error_color']};
            }}
            QPushButton.mute-button.muted:hover {{
                background-color: #d32f2f;
                border-color: #d32f2f;
            }}
        """