#!/usr/bin/env python3
"""
Setup script for Sound Multiplexer
"""

from setuptools import setup, find_packages
import os

# Read the README file
def read_readme():
    with open("README.md", "r", encoding="utf-8") as fh:
        return fh.read()

# Read requirements
def read_requirements():
    with open("requirements.txt", "r", encoding="utf-8") as fh:
        return [line.strip() for line in fh if line.strip() and not line.startswith("#")]

setup(
    name="sound-multiplexer",
    version="1.0.0",
    author="Sound Multiplexer Contributors",
    author_email="",
    description="A GUI application for multiplexing audio output to multiple devices simultaneously",
    long_description=read_readme(),
    long_description_content_type="text/markdown",
    url="https://github.com/rayjine/sound-multiplexer",
    packages=find_packages(),
    classifiers=[
        "Development Status :: 4 - Beta",
        "Intended Audience :: End Users/Desktop",
        "License :: OSI Approved :: GNU General Public License v3 (GPLv3)",
        "Operating System :: POSIX :: Linux",
        "Programming Language :: Python :: 3",
        "Programming Language :: Python :: 3.8",
        "Programming Language :: Python :: 3.9",
        "Programming Language :: Python :: 3.10",
        "Programming Language :: Python :: 3.11",
        "Programming Language :: Python :: 3.12",
        "Topic :: Multimedia :: Sound/Audio",
        "Topic :: Desktop Environment",
        "Environment :: X11 Applications :: Qt",
    ],
    python_requires=">=3.8",
    install_requires=read_requirements(),
    extras_require={
        "dev": [
            "pytest",
            "black",
            "flake8",
            "mypy",
        ],
    },
    entry_points={
        "console_scripts": [
            "sound-multiplexer=src.main:main",
        ],
        "gui_scripts": [
            "sound-multiplexer-gui=src.main:main",
        ],
    },
    data_files=[
        ("share/applications", ["packaging/sound-multiplexer.desktop"]),
        ("share/pixmaps", ["packaging/sound-multiplexer.png"]),
        ("share/doc/sound-multiplexer", ["README.md", "docs/USER_GUIDE.md", "docs/TECHNICAL.md"]),
    ],
    include_package_data=True,
    package_data={
        "src": ["*.py"],
        "src.gui": ["*.py"],
    },
    zip_safe=False,
)