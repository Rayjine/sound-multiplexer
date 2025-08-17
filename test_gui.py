#!/usr/bin/env python3
"""
Test version of Sound Multiplexer to debug GUI issues
"""

import sys
import os
import signal

# Add the src directory to Python path for imports
sys.path.insert(0, os.path.join(os.path.dirname(__file__), 'src'))

from PyQt6.QtWidgets import QApplication
from PyQt6.QtCore import QTimer
from main import main as app_main

# Alternative: import the components directly
from audio_manager import AudioManager
from gui.main_window import MainWindow

def timeout_handler(signum, frame):
    print("Timeout reached - GUI test complete")
    sys.exit(0)

def main():
    try:
        print("=== Sound Multiplexer GUI Test ===")
        print(f"Python version: {sys.version}")
        
        # Set up timeout
        signal.signal(signal.SIGALRM, timeout_handler)
        signal.alarm(10)  # 10 second timeout
        
        print("Creating QApplication...")
        app = QApplication(sys.argv)
        app.setApplicationName("Sound Multiplexer Test")
        app.setApplicationVersion("1.0.0")
        print("✓ QApplication created")
        
        print("Creating AudioManager...")
        audio_manager = AudioManager()
        print("✓ AudioManager created")
        
        print("Creating MainWindow...")
        window = MainWindow(audio_manager)
        print("✓ MainWindow created")
        
        print("Showing window...")
        window.show()
        window.raise_()  # Bring to front
        window.activateWindow()  # Give focus
        print("✓ Window shown and activated")
        
        print("Window info:")
        print(f"  - Visible: {window.isVisible()}")
        print(f"  - Size: {window.size().width()}x{window.size().height()}")
        print(f"  - Position: ({window.x()}, {window.y()})")
        
        # Set up a timer to print status
        def print_status():
            print(f"App still running... Window visible: {window.isVisible()}")
        
        timer = QTimer()
        timer.timeout.connect(print_status)
        timer.start(2000)  # Print status every 2 seconds
        
        print("Starting event loop (will timeout in 10 seconds)...")
        app.exec()
        
    except Exception as e:
        print(f"ERROR: {e}")
        import traceback
        traceback.print_exc()
        sys.exit(1)
    finally:
        if 'audio_manager' in locals():
            audio_manager.cleanup()

if __name__ == "__main__":
    main()