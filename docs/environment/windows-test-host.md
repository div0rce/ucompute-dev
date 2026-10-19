# Windows 11 test host (HP laptop): one-time setup

The second test machine is the owner's HP laptop running Windows 11. Common is cross-compiled on the Mac and
run on the laptop; the Mac drives it over SSH. These steps are run **by the owner on the laptop**, in
**PowerShell as Administrator** (Start menu, type PowerShell, right-click, Run as administrator).

## 1. Turn on the built-in OpenSSH server

```powershell
Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0
Start-Service sshd
Set-Service -Name sshd -StartupType Automatic
```

## 2. Allow the Mac's key (and only that key)

```powershell
$key = 'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOfEuX7P8emUWyL2Al4vbDBRyv6h5Sc6LX6w9AKtkeJR common-dev-mac'
Add-Content -Path "$env:ProgramData\ssh\administrators_authorized_keys" -Value $key
icacls.exe "$env:ProgramData\ssh\administrators_authorized_keys" /inheritance:r /grant "Administrators:F" /grant "SYSTEM:F"
```

If your Windows account is not an administrator, use your own key file instead:

```powershell
New-Item -ItemType Directory -Force "$env:USERPROFILE\.ssh" | Out-Null
Add-Content -Path "$env:USERPROFILE\.ssh\authorized_keys" -Value $key
```

## 3. Let Common's peers reach the laptop on your home network only

```powershell
Get-NetConnectionProfile          # shows the Wi-Fi name and whether it is Public or Private
Set-NetConnectionProfile -InterfaceAlias 'Wi-Fi' -NetworkCategory Private
New-NetFirewallRule -DisplayName 'Common peer endpoint (QUIC)' -Direction Inbound -Protocol UDP -LocalPort 8193 -Profile Private -Action Allow
New-NetFirewallRule -DisplayName 'Common LAN discovery (mDNS)' -Direction Inbound -Protocol UDP -LocalPort 5353 -Profile Private -Action Allow
```

If `Get-NetConnectionProfile` shows a different interface name than `Wi-Fi`, use that name.

## 4. Send these back

```powershell
whoami
(Get-NetIPAddress -AddressFamily IPv4 -InterfaceAlias 'Wi-Fi').IPAddress
```

## Undo everything later

```powershell
Stop-Service sshd; Set-Service -Name sshd -StartupType Disabled
Remove-NetFirewallRule -DisplayName 'Common peer endpoint (QUIC)','Common LAN discovery (mDNS)'
# and delete the common-dev-mac line from the authorized_keys file used in step 2
```

What this grants: anyone holding the private key `~/.ssh/common_hp_ed25519` on the Mac can open a shell on the
laptop as your account while it is on the same network. Nothing else is opened to the network except UDP 8193
and 5353 on Private networks.

## Driving the laptop from the Mac

`scripts/windows-host.sh` wraps the SSH key and the laptop's name (`WIN_HOST`, default `jess-hp.local`):

```sh
scripts/windows-host.sh run 'common.exe --version'                  # cmd.exe command
scripts/windows-host.sh ps  'Get-NetConnectionProfile'              # PowerShell, no quoting trouble
scripts/windows-host.sh cp  target/x86_64-pc-windows-gnu/release/common.exe C:/Users/jessg/Common/common.exe
scripts/windows-host.sh sync                                         # HEAD -> C:/Users/jessg/src/common
```

On a network the laptop has not seen before, Windows marks it Public, and the firewall rules above (Private
only) block SSH and Common until the owner sets that network to Private (Settings > Network & internet > Wi-Fi
> the network > Network profile type). eduroam drops mDNS between clients; a phone hotspot carries it.

## Build tools installed on the laptop (2026-10-19, with the owner's go-ahead)

Installed with `winget` over SSH: Visual Studio 2022 Build Tools 17.14 with the "Desktop development with C++"
workload (Microsoft's license accepted on the owner's instruction; the installer asked for a restart, not yet done), Rustup (the repo's `rust-toolchain.toml` pins 1.98.0, fetched on first build), Node.js
LTS 24 and pnpm 10.34.4. The WebView2 runtime ships with Windows 11. The desktop app builds natively there:
`scripts/windows-host.sh sync`, then `cd apps\desktop && pnpm install --frozen-lockfile && pnpm tauri build`.

Long builds over SSH: Windows ends a session's processes when the SSH connection drops (a Wi-Fi switch is
enough), so start them detached and poll a log, for example
`scripts/windows-host.sh ps 'Invoke-CimMethod Win32_Process -MethodName Create -Arguments @{CommandLine="cmd /c cd /d C:\Users\jessg\src\common\apps\desktop && pnpm tauri build --bundles nsis > C:\Users\jessg\build.log 2>&1"}'`.
