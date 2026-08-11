#!/usr/bin/env bash
# fix-appimage.sh — Remove infra libs from a Tauri-produced AppImage that crash
# on Mesa 25+ / GLib 2.88 distros (Ubuntu 26.04, Fedora 42+, etc.).
#
# Usage: fix-appimage.sh <path-to.AppImage>
#
# Set APPIMAGETOOL_RUNTIME_FILE to a pre-downloaded AppImage type2 runtime to
# avoid appimagetool fetching one from its mutable `continuous` tag (CI pins
# this; unset is fine for local testing).
#
# Root cause — three interlocking failures (upstream: https://github.com/tauri-apps/tauri/issues/15665):
#
#  1. EGL crash: linuxdeploy bundles libwayland-client.so.0 (1.22) alongside
#     the app. Mesa 25's libEGL calls the bundled version at runtime; the version
#     skew causes eglGetDisplay to return EGL_BAD_PARAMETER under Wayland, which
#     WebKitWebProcess treats as fatal and aborts before the window ever appears.
#
#  2. GStreamer crash: linuxdeploy also bundles libgst*.so* (GStreamer core libs).
#     AppRun unconditionally sets GST_PLUGIN_SYSTEM_PATH_1_0 to a dir inside the
#     AppImage that the bundler never populates (bundleMediaFramework is false by
#     default), so GStreamer's plugin discovery yields an empty registry. The
#     "GStreamer element appsink not found" error kills the render process; as a
#     side effect the broken run poisons ~/.cache/gstreamer-1.0/registry.x86_64.bin.
#
#  3. WebKit helper mismatch (latent): the bundled WebKit helpers
#     (WebKitNetworkProcess/WebKitWebProcess) have RUNPATH=$ORIGIN only, and
#     linuxdeploy string-patches /usr -> ././ inside libwebkit2gtk so the helper
#     dir is resolved relative to the process cwd. AppRun's chdir($APPDIR/usr)
#     makes this work; any launch that bypasses AppRun (extracted-AppDir usage,
#     repack workflows, dbus/systemd activation with cwd=/) resolves the helpers
#     wrong -- spawning nothing, dying on unresolved bundled libs, or spawning
#     the system helpers -- and the window never appears.
#
# Fix: remove the offending libs so the app uses the system copies (which are
# newer and ABI-compatible on any distro shipping glib >= 2.72 / Ubuntu 22.04+),
# and symlink the system GStreamer plugin directory so discovery works correctly.
# No tauri.conf.json knob can do this — bundle.linux.appimage only exposes
# bundleMediaFramework, files (copy-only, no remove/symlink), and bundleXdgOpen.

set -euo pipefail

if [[ $# -lt 1 ]]; then
  echo "Usage: fix-appimage.sh <path-to.AppImage>" >&2
  exit 1
fi

if [[ ! -f "$1" ]]; then
  echo "Error: file not found: $1" >&2
  exit 1
fi

APPIMAGE_ABS="$(realpath "$1")"
APPIMAGE_NAME="$(basename "$APPIMAGE_ABS")"

# Detect multiarch triplet for GStreamer plugin path.
case "$(uname -m)" in
  x86_64)  MULTIARCH="x86_64-linux-gnu" ;;
  aarch64) MULTIARCH="aarch64-linux-gnu" ;;
  *)
    echo "Error: unsupported architecture: $(uname -m)" >&2
    exit 1
    ;;
esac

WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT

echo "==> Extracting $APPIMAGE_NAME"
(cd "$WORKDIR" && APPIMAGE_EXTRACT_AND_RUN=1 "$APPIMAGE_ABS" --appimage-extract)

LIBDIR="$WORKDIR/squashfs-root/usr/lib"

# Guard against a bundler layout change: if the primary offending lib is not
# where we expect it, the rm globs below would silently no-op and we'd ship
# an unfixed artifact. Fail loudly instead so a tauri/linuxdeploy upgrade
# that changes the bundled lib set gets noticed here, not by users.
if ! compgen -G "$LIBDIR/libwayland-client.so*" > /dev/null; then
  echo "Error: libwayland-client not found in $LIBDIR — bundler layout changed; update fix-appimage.sh" >&2
  exit 1
fi

echo "==> Removing infra libs that conflict with system Mesa / GLib / GStreamer"
rm -f \
  "$LIBDIR"/libwayland-client.so* \
  "$LIBDIR"/libwayland-cursor.so* \
  "$LIBDIR"/libwayland-egl.so* \
  "$LIBDIR"/libwayland-server.so* \
  "$LIBDIR"/libglib-2.0.so* \
  "$LIBDIR"/libgio-2.0.so* \
  "$LIBDIR"/libgobject-2.0.so* \
  "$LIBDIR"/libgmodule-2.0.so* \
  "$LIBDIR"/libmount.so* \
  "$LIBDIR"/libblkid.so* \
  "$LIBDIR"/libselinux.so* \
  "$LIBDIR"/libpcre2-8.so* \
  "$LIBDIR"/libgst*.so* \
  "$LIBDIR"/libzstd.so* \
  "$LIBDIR"/libelf.so* \
  "$LIBDIR"/libffi.so*

echo "==> Symlinking system GStreamer plugin directory"
# On distros without the Debian multiarch layout (e.g. Arch), this symlink
# dangles — GStreamer then falls back to its default plugin discovery, which
# is a safe degradation (unlike the original empty in-bundle dir).
rm -rf "$LIBDIR/gstreamer-1.0"
ln -s "/usr/lib/$MULTIARCH/gstreamer-1.0" "$LIBDIR/gstreamer-1.0"

echo "==> Repacking AppImage"
# Pass a pinned type2 runtime when provided (CI sets APPIMAGETOOL_RUNTIME_FILE);
# without it appimagetool downloads the runtime from its mutable `continuous`
# tag at repack time — acceptable for local testing, not for release builds.
RUNTIME_ARGS=()
if [[ -n "${APPIMAGETOOL_RUNTIME_FILE:-}" ]]; then
  RUNTIME_ARGS=(--runtime-file "$APPIMAGETOOL_RUNTIME_FILE")
fi
APPIMAGE_EXTRACT_AND_RUN=1 ARCH="$(uname -m)" appimagetool \
  "${RUNTIME_ARGS[@]}" \
  "$WORKDIR/squashfs-root" "$APPIMAGE_ABS"

echo "==> Done: $APPIMAGE_ABS (unsigned community artifact)"
