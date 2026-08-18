#!/usr/bin/env bash
# LiteDroid Linux Development Setup
# Installs system dependencies and builds the project.
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

echo -e "${GREEN}LiteDroid Linux Setup${NC}"
echo "========================"

# ── 1. Detect package manager ──────────────────────────────────────
if command -v apt-get &>/dev/null; then
    PKG_MANAGER="apt-get"
    PKG_INSTALL="sudo apt-get install -y"
elif command -v dnf &>/dev/null; then
    PKG_MANAGER="dnf"
    PKG_INSTALL="sudo dnf install -y"
elif command -v pacman &>/dev/null; then
    PKG_MANAGER="pacman"
    PKG_INSTALL="sudo pacman -S --noconfirm"
else
    echo -e "${RED}Unsupported package manager. Install deps manually.${NC}"
    exit 1
fi
echo -e "${GREEN}[OK]${NC} Package manager: $PKG_MANAGER"

# ── 2. Install system dependencies ─────────────────────────────────
echo ""
echo "Installing system dependencies..."
case "$PKG_MANAGER" in
    apt-get)
        sudo apt-get update
        $PKG_INSTALL build-essential libx11-dev pkg-config libpulse-dev \
            libasound2-dev iptables
        ;;
    dnf)
        $PKG_INSTALL gcc make libX11-devel pulseaudio-libs-devel \
            alsa-lib-devel iptables
        ;;
    pacman)
        $PKG_INSTALL base-devel libx11 pulseaudio alsa-lib iptables
        ;;
esac
echo -e "${GREEN}[OK]${NC} System dependencies installed"

# ── 3. Install Rust ────────────────────────────────────────────────
echo ""
if command -v rustc &>/dev/null; then
    RUST_VERSION=$(rustc --version)
    echo -e "${GREEN}[OK]${NC} Rust already installed: $RUST_VERSION"
else
    echo "Installing Rust toolchain..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    source "$HOME/.cargo/env"
    echo -e "${GREEN}[OK]${NC} Rust installed: $(rustc --version)"
fi

# ── 4. Check KVM ───────────────────────────────────────────────────
echo ""
if [ -e /dev/kvm ] && [ -w /dev/kvm ]; then
    echo -e "${GREEN}[OK]${NC} /dev/kvm is available"
    # Check CPU virtualization
    if grep -qE 'vmx|svm' /proc/cpuinfo 2>/dev/null; then
        echo -e "${GREEN}[OK]${NC} CPU virtualization (VMX/SVM) supported"
    else
        echo -e "${YELLOW}[WARN]${NC} CPU virtualization not detected — enable VT-x/AMD-V in BIOS"
    fi
else
    echo -e "${YELLOW}[WARN]${NC} /dev/kvm not available — run: sudo modprobe kvm_intel (or kvm_amd)"
    echo "  The project will still build; KVM is only needed at runtime."
fi

# ── 5. Build ───────────────────────────────────────────────────────
echo ""
echo "Building LiteDroid (release)..."
source "$HOME/.cargo/env"
cargo build --release 2>&1

echo ""
echo "========================"
echo -e "${GREEN}Build complete!${NC}"
echo ""
echo "Binaries:"
ls -lh target/release/litedroid target/release/litedroid-daemon target/release/litedroid-gui 2>/dev/null \
    || echo "  (binaries not found in target/release/)"
echo ""
echo "Quick start:"
echo "  ./target/release/litedroid doctor   # check system readiness"
echo "  ./target/release/litedroid device create PixelLite"
echo "  ./target/release/litedroid start --device PixelLite"
