# Linux packaging

The files that make the Prime Files a program of a Linux desktop.

| File | What it is |
| --- | --- |
| `prime-files.desktop` | The freedesktop entry: the menu item, the icon and the `inode/directory` handler. Installed at `/usr/share/applications/prime-files.desktop`. |
| `prime-files.png` | The icon at 256x256, the size the program looks for. Installed at `/usr/share/icons/hicolor/256x256/apps/prime-files.png`. |
| `install-linux.sh` | Installs the program for the current user, without root. `--uninstall` takes back exactly what it installed. |

The Debian package itself lives in `debian/`, at the root of the repository.

## The icon

`assets/icon/prime-files.png` is the master (512x512). The one here is that
master made smaller, for the size the program asks the desktop for:

```bash
ffmpeg -i assets/icon/prime-files.png -vf "scale=256:256:flags=lanczos" -y packaging/prime-files.png
```

The master is drawn in code by `assets/icon/make_icon.py`, which needs Pillow;
the smaller one is only a copy of it, so it does not have to be redrawn.

## The three ways to install

```bash
# The package, for the whole system (needs the debhelper and cargo).
dpkg-buildpackage -b -us -uc
sudo apt install ../prime-files_0.3.9-1_amd64.deb

# For the current user only (no root).
cargo build --release -p nohrs
./packaging/install-linux.sh

# The way cargo does it; the binary keeps the name it has inside the code.
cargo install --path crates/nohrs
```

`docs/build-and-display-linux.md` (§ 5) has the same list with the runtime
dependencies and the ways to start the program.
