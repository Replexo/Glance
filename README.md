# Glint

Glint — a GNOME image viewer with editing (Adjust, Markup, Resize, multi-image strip).

Based on Loupe by the GNOME project (https://gitlab.gnome.org/GNOME/loupe), GPL-3.0-or-later.

## Credits

- Icons: [MingCute Icons](https://www.mingcute.com/), Apache-2.0 (see `data/COPYING.mingcute`), with the fill colour set to black so GTK can recolour them.
- Markup tools follow the set offered by [Gradia](https://github.com/AlexanderVanhee/Gradia).

## Build

```sh
meson setup builddir -Dprofile=dev -Dapp_id_suffix=.Devel && ninja -C builddir
./builddir/src/glint
```
