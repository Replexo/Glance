# Contributing

Bug reports and ideas are welcome in the [issue tracker](https://github.com/Replexo/Glance/issues). Please include the version from **About Glance** and, for crashes, the steps that lead to them.

Problems that also happen in Loupe, such as an image format that fails to load, are best reported to [Loupe](https://gitlab.gnome.org/GNOME/loupe/-/issues) or [glycin](https://gitlab.gnome.org/GNOME/glycin/-/issues) directly.

## Code

- Format Rust code with `cargo fmt` and check it with `cargo clippy`.
- Run the tests with `meson test -C builddir`.
- Keep one change per pull request, with a short description of what it changes and why.

## Help pages

The help pages are written in [ducktype](http://projectmallard.org/ducktype/1.0/index.html) in `help/C/duck`. Generate the `.page` files with `make -C help/C/` and preview them with `yelp help/C/index.page`. Commit the generated `.page` files as well.
