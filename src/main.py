#!/usr/bin/env python3
"""
Sound Multiplexer - A Linux audio multiplexer with GUI interface

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

import sys
import os

from PyQt6.QtWidgets import QApplication
from .audio_manager import AudioManager
from .gui.main_window import MainWindow


def main():
    try:
        app = QApplication(sys.argv)
        app.setApplicationName("Sound Multiplexer")
        app.setApplicationVersion("1.0.0")
        
        audio_manager = AudioManager()
        window = MainWindow(audio_manager)
        window.show()
        
        sys.exit(app.exec())
    except Exception as e:
        print(f"ERROR: {e}")
        import traceback
        traceback.print_exc()
        sys.exit(1)


if __name__ == "__main__":
    main()