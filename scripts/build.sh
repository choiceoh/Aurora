#!/bin/bash
set -e

VERSION="0.1.0"
NAME="aurora"

echo "═══════════════════════════════════════"
echo "  Aurora Build Script v${VERSION}"
echo "═══════════════════════════════════════"

case "${1:-release}" in
  release)
    echo "▸ Building release binary..."
    cargo build --release
    echo "✓ Binary: target/release/${NAME}"
    ls -lh "target/release/${NAME}"
    ;;

  deb)
    echo "▸ Building .deb package..."
    cargo deb
    echo "✓ Package: target/debian/"
    ls -lh target/debian/*.deb
    echo ""
    echo "Install with: sudo dpkg -i target/debian/${NAME}_${VERSION}-1_amd64.deb"
    ;;

  bundle)
    echo "▸ Building app bundle (macOS .app / Windows .exe)..."
    cargo bundle --release
    echo "✓ Bundle created in target/release/bundle/"
    ;;

  all)
    $0 release
    echo ""
    $0 deb
    ;;

  *)
    echo "Usage: $0 {release|deb|bundle|all}"
    echo ""
    echo "  release  - Build optimized release binary"
    echo "  deb      - Build Debian/Ubuntu .deb package"
    echo "  bundle   - Build macOS .app or Windows bundle"
    echo "  all      - Build release + deb"
    exit 1
    ;;
esac

echo ""
echo "═══ Done! ═══"
