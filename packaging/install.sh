#!/bin/bash
# Installation script for Sound Multiplexer

set -e

PACKAGE_NAME="sound-multiplexer"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Logging functions
log_info() { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[SUCCESS]${NC} $1"; }
log_warning() { echo -e "${YELLOW}[WARNING]${NC} $1"; }
log_error() { echo -e "${RED}[ERROR]${NC} $1"; }

# Check if running as root for system install
check_root() {
    if [[ $EUID -eq 0 ]]; then
        log_info "Running as root - will install system-wide"
        return 0
    else
        log_info "Running as user - will install for current user only"
        return 1
    fi
}

# Detect distribution
detect_distro() {
    if command -v dnf &> /dev/null; then
        echo "fedora"
    elif command -v yum &> /dev/null; then
        echo "rhel"
    elif command -v apt &> /dev/null; then
        echo "debian"
    elif command -v pacman &> /dev/null; then
        echo "arch"
    elif command -v zypper &> /dev/null; then
        echo "suse"
    else
        echo "unknown"
    fi
}

# Install dependencies based on distribution
install_dependencies() {
    local distro=$(detect_distro)
    log_info "Detected distribution: $distro"
    
    case $distro in
        "fedora")
            log_info "Installing dependencies with dnf..."
            sudo dnf install -y python3 python3-pip python3-PyQt6 pulseaudio pulseaudio-utils
            ;;
        "rhel")
            log_info "Installing dependencies with yum..."
            sudo yum install -y python3 python3-pip python3-PyQt6 pulseaudio pulseaudio-utils
            ;;
        "debian")
            log_info "Installing dependencies with apt..."
            sudo apt update
            sudo apt install -y python3 python3-pip python3-pyqt6 pulseaudio pulseaudio-utils
            ;;
        "arch")
            log_info "Installing dependencies with pacman..."
            sudo pacman -S --noconfirm python python-pip python-pyqt6 pulseaudio
            ;;
        "suse")
            log_info "Installing dependencies with zypper..."
            sudo zypper install -y python3 python3-pip python3-PyQt6 pulseaudio pulseaudio-utils
            ;;
        *)
            log_warning "Unknown distribution. Please install dependencies manually:"
            echo "  - Python 3.8+"
            echo "  - PyQt6"
            echo "  - PulseAudio"
            echo "  - python3-pulsectl (via pip)"
            ;;
    esac
}

# Install Python package
install_package() {
    cd "$PROJECT_DIR"
    
    # Install pulsectl via pip (not always available in system packages)
    log_info "Installing Python dependencies..."
    if check_root; then
        pip3 install pulsectl
        log_info "Installing Sound Multiplexer system-wide..."
        make install
    else
        pip3 install --user pulsectl
        log_info "Installing Sound Multiplexer for current user..."
        make install-user
    fi
}

# Main installation function
main() {
    log_info "Starting Sound Multiplexer installation..."
    
    # Check for required commands
    if ! command -v python3 &> /dev/null; then
        log_error "Python 3 is required but not installed."
        exit 1
    fi
    
    if ! command -v pip3 &> /dev/null; then
        log_error "pip3 is required but not installed."
        exit 1
    fi
    
    # Ask user if they want to install dependencies
    read -p "Install system dependencies? [Y/n]: " -n 1 -r
    echo
    if [[ $REPLY =~ ^[Yy]$ ]] || [[ -z $REPLY ]]; then
        install_dependencies
    fi
    
    # Install the package
    install_package
    
    log_success "Installation completed successfully!"
    log_info "You can now run 'sound-multiplexer' from the command line"
    log_info "Or find 'Sound Multiplexer' in your applications menu"
    
    # Check if user installation needs PATH update
    if ! check_root && [[ ":$PATH:" != *":$HOME/.local/bin:"* ]]; then
        log_warning "~/.local/bin is not in your PATH"
        log_info "Add this line to your ~/.bashrc or ~/.zshrc:"
        echo "export PATH=\"\$HOME/.local/bin:\$PATH\""
    fi
}

# Show usage
usage() {
    echo "Usage: $0 [OPTIONS]"
    echo ""
    echo "Options:"
    echo "  -h, --help     Show this help message"
    echo "  --deps-only    Only install dependencies"
    echo "  --no-deps      Skip dependency installation"
    echo ""
}

# Parse command line arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        -h|--help)
            usage
            exit 0
            ;;
        --deps-only)
            install_dependencies
            exit 0
            ;;
        --no-deps)
            SKIP_DEPS=1
            shift
            ;;
        *)
            log_error "Unknown option: $1"
            usage
            exit 1
            ;;
    esac
done

main