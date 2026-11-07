# Glint

Glint — a GNOME image viewer with editing (Adjust, Markup, Resize, multi-image strip).

Based on Loupe by the GNOME project (https://gitlab.gnome.org/GNOME/loupe), GPL-3.0-or-later.

## Build

```sh
meson setup builddir -Dprofile=dev -Dapp_id_suffix=.Devel && ninja -C builddir
./builddir/src/glint
```
