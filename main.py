#!/usr/bin/env python3
"""
Sound Multiplexer - A Linux audio multiplexer with GUI interface
Author: Nicolas Filimonov
"""

import sys
import os
from PyQt6.QtWidgets import QApplication
from src.audio_manager import AudioManager
from src.gui.main_window import MainWindow


def main():
    app = QApplication(sys.argv)
    app.setApplicationName("Sound Multiplexer")
    app.setApplicationVersion("1.0.0")
    
    audio_manager = AudioManager()
    window = MainWindow(audio_manager)
    window.show()
    
    sys.exit(app.exec())


if __name__ == "__main__":
    main()