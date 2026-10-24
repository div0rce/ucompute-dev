# UCompute on Linux: user-level install (experimental)

UCompute was formerly called Common; the binary, unit and data folder still say `common`. Daemon only: no desktop
app, CPU engine only (llama.cpp b11609 Ubuntu x64 build, pinned and hash-checked, D-024). Tested on Ubuntu 26.04
x86_64 only. No root needed.

Install (from the unpacked tarball):

    sha256sum -c SHA256SUMS
    install -Dm755 common ~/.local/bin/common
    install -Dm644 common.service ~/.config/systemd/user/common.service
    systemctl --user daemon-reload
    systemctl --user enable --now common.service
    ~/.local/bin/common setup          # downloads and verifies the pinned engine
    curl -s 127.0.0.1:8192/health

The API listens on 127.0.0.1:8192 only. Paired devices (Mesh) stay off until you turn networking on
(`PUT /api/network` with `{"enabled": true}`, or `network_enabled` in `~/.local/share/common/config.json`, then
`systemctl --user restart common`). The service runs while you are logged
in; to keep it running after logout, `loginctl enable-linger` (may need an administrator).

Remove:

    systemctl --user disable --now common.service
    rm ~/.local/bin/common ~/.config/systemd/user/common.service
    systemctl --user daemon-reload

Your models, device identity and pairings stay in `~/.local/share/common` (or `$XDG_DATA_HOME/common`); delete that
folder to remove them too.
