# Glint

Glint — a GNOME image viewer with editing (Adjust, Markup, Resize, multi-image strip).

Based on Loupe by the GNOME project (https://gitlab.gnome.org/GNOME/loupe), GPL-3.0-or-later.

## Credits

- Editor icons: [coolicons](https://github.com/krystonschwarze/coolicons) by Kryston Schwarze, MIT (see `data/COPYING.coolicons`), with strokes converted to fills for GTK.
- Markup tools follow the set offered by [Gradia](https://github.com/AlexanderVanhee/Gradia).

## Build

```sh
meson setup builddir -Dprofile=dev -Dapp_id_suffix=.Devel && ninja -C builddir
./builddir/src/glint
```
