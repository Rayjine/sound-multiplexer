#!/usr/bin/env python3
"""
Test script to verify Sound Multiplexer installation
"""

import subprocess
import sys
import time
import signal
import os

def test_command_exists(command):
    """Test if a command exists in PATH"""
    try:
        result = subprocess.run(['which', command], capture_output=True, text=True)
        return result.returncode == 0
    except:
        return False

def test_gui_launch():
    """Test launching the GUI"""
    try:
        print("Testing GUI launch...")
        proc = subprocess.Popen(['sound-multiplexer-gui'], 
                               stdout=subprocess.DEVNULL, 
                               stderr=subprocess.DEVNULL)
        
        # Wait 3 seconds for the app to start
        time.sleep(3)
        
        # Check if process is still running
        if proc.poll() is None:
            print("✓ GUI launched successfully")
            proc.terminate()
            proc.wait(timeout=5)
            return True
        else:
            print("✗ GUI failed to launch")
            return False
    except Exception as e:
        print(f"✗ GUI launch error: {e}")
        return False

def test_desktop_entry():
    """Test if desktop entry exists"""
    desktop_file = os.path.expanduser("~/.local/share/applications/sound-multiplexer.desktop")
    exists = os.path.exists(desktop_file)
    print(f"{'✓' if exists else '✗'} Desktop entry: {desktop_file}")
    return exists

def main():
    print("=== Sound Multiplexer Installation Test ===\n")
    
    tests = []
    
    # Test commands
    print("Testing installed commands:")
    for cmd in ['sound-multiplexer', 'sound-multiplexer-gui']:
        exists = test_command_exists(cmd)
        print(f"{'✓' if exists else '✗'} Command '{cmd}' in PATH")
        tests.append(exists)
    
    print()
    
    # Test desktop entry
    tests.append(test_desktop_entry())
    
    print()
    
    # Test GUI launch
    tests.append(test_gui_launch())
    
    print()
    
    # Summary
    passed = sum(tests)
    total = len(tests)
    
    print(f"=== Results: {passed}/{total} tests passed ===")
    
    if passed == total:
        print("✓ Installation is working correctly!")
        print("\nTo launch Sound Multiplexer:")
        print("1. Run 'sound-multiplexer-gui' from terminal")
        print("2. Find 'Sound Multiplexer' in your applications menu")
        print("3. Search for 'sound' or 'audio' in activities")
    else:
        print("✗ Some tests failed - installation may have issues")
        return 1
    
    return 0

if __name__ == "__main__":
    sys.exit(main())