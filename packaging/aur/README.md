# Feline on Arch Linux (AUR-style package)

`PKGBUILD` here builds `feline-git`: the launcher, Feline's own runtime and
its login window, from the newest source.

```sh
cd packaging/aur
makepkg -si        # build, then install with pacman
```

Remove it again with `sudo pacman -R feline-git`.

Notes:

- This recipe hasn't been built from start to finish yet. Until it has,
  the README's from-source steps are the way to install Feline.
- A packaged Feline is updated by building the package again, not by
  About → Updates (that replaces a `feline` you installed yourself).
- If you used `feline --install-desktop-entry` before, run
  `feline --uninstall-desktop-entry` once so the menu shows one Feline.

## Publishing to the AUR

Not done yet. Once the recipe has been built and tried, with an AUR account
and SSH key:

```sh
git clone ssh://aur@aur.archlinux.org/feline-git.git aur-feline
cp PKGBUILD aur-feline/ && cd aur-feline
makepkg --printsrcinfo > .SRCINFO
git add PKGBUILD .SRCINFO && git commit -m "feline-git: first version" && git push
```
