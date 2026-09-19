#!/usr/bin/env bash
# M1/M6 verification: inventory what GNOME's display manager knows right now.
# Run before starting a USBra session, and again during one — the extra
# connector in the second run is the virtual Display 2 (docs/05-milestones.md).
set -euo pipefail

echo "== org.gnome.Mutter.DisplayConfig: current monitors =="
RAW="$(gdbus call --session \
    --dest org.gnome.Mutter.DisplayConfig \
    --object-path /org/gnome/Mutter/DisplayConfig \
    --method org.gnome.Mutter.DisplayConfig.GetResources 2>/dev/null || true)"
if [ -n "$RAW" ]; then
    echo "$RAW" | tr ',' '\n' | grep -oE "'[A-Za-z0-9]+-[0-9]+'" | sort -u \
        | sed 's/^/  connector: /'
else
    echo "  (DisplayConfig not reachable — is this a GNOME Wayland session?)"
fi

echo
echo "== org.gnome.Mutter.ScreenCast: virtual-monitor API (used from M6 on) =="
if gdbus introspect --session \
        --dest org.gnome.Mutter.ScreenCast \
        --object-path /org/gnome/Mutter/ScreenCast >/dev/null 2>&1; then
    echo "  available ✓ (RecordVirtual will create the virtual monitor)"
    gdbus introspect --session \
        --dest org.gnome.Mutter.ScreenCast \
        --object-path /org/gnome/Mutter/ScreenCast 2>/dev/null \
        | grep -E "method|interface" | head -12 | sed 's/^/  /'
else
    echo "  not on the bus (need GNOME 40+ / Ubuntu 21.04+, Wayland session)"
fi

echo
echo "== session context =="
echo "  XDG_SESSION_TYPE=${XDG_SESSION_TYPE:-unknown}   (USBra M6 targets: wayland)"
echo "  mutter/gnome-shell version: $(gnome-shell --version 2>/dev/null || echo '?')"
echo
echo "Tip: run this again while a USBra session is active — one more connector = Display 2."
