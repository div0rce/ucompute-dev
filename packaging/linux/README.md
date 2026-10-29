# UCompute on Linux: user-level install (experimental)

UCompute: the binary is `ucompute`, the unit `ucompute.service`. Daemon only: no desktop
app, CPU engine only (llama.cpp b11609 Ubuntu x64 build, pinned and hash-checked, D-024). Tested on Ubuntu 26.04
x86_64 only. No root needed.

Install (from the unpacked tarball):

    sha256sum -c SHA256SUMS
    install -Dm755 ucompute ~/.local/bin/ucompute
    install -Dm644 ucompute.service ~/.config/systemd/user/ucompute.service
    systemctl --user daemon-reload
    systemctl --user enable --now ucompute.service
    ~/.local/bin/ucompute setup          # downloads and verifies the pinned engine
    curl -s 127.0.0.1:8192/health

The API listens on 127.0.0.1:8192 only. Paired devices (Mesh) stay off until you turn networking on
(`PUT /api/network` with `{"enabled": true}`, or `network_enabled` in `~/.local/share/ucompute/config.json`, then
`systemctl --user restart ucompute`). The service runs while you are logged
in; to keep it running after logout, `loginctl enable-linger` (may need an administrator).

Remove:

    systemctl --user disable --now ucompute.service
    rm ~/.local/bin/ucompute ~/.config/systemd/user/ucompute.service
    systemctl --user daemon-reload

Your models, device identity and pairings stay in `~/.local/share/ucompute` (or `$XDG_DATA_HOME/ucompute`); delete that
folder to remove them too.
