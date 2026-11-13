# Feline on Arch Linux (AUR-style package)

`PKGBUILD` here builds `feline-git`: the launcher, Feline's own runtime and
its login window, from the newest source.

```sh
cd packaging/aur
makepkg -si        # build, then install with pacman
```

Remove it again with `sudo pacman -R feline-git`.

Notes:

- While the repository is private, git needs your GitHub login to fetch the
  source; `gh auth setup-git` sets that up once.
- A packaged Feline is updated by building the package again, not by
  About → Updates (that replaces a `feline` you installed yourself).
- If you used `feline --install-desktop-entry` before, run
  `feline --uninstall-desktop-entry` once so the menu shows one Feline.

## Publishing to the AUR

The AUR only takes packages whose source anyone can fetch, so this waits
until the repository is public. Then, with an AUR account and SSH key:

```sh
git clone ssh://aur@aur.archlinux.org/feline-git.git aur-feline
cp PKGBUILD aur-feline/ && cd aur-feline
makepkg --printsrcinfo > .SRCINFO
git add PKGBUILD .SRCINFO && git commit -m "feline-git: first version" && git push
```
