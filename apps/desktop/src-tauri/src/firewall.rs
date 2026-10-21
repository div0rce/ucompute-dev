//! Windows Firewall consent step. Pairing started on Windows listens on a random UDP port per
//! window, which the firewall drops by default (no rule, and Windows never asks for a console
//! program). The app offers one rule for the bundled daemon only, inbound UDP, Private profile only,
//! added through the standard elevation prompt: the user's click on that prompt is the
//! authorization. Nothing here runs without that click, and nothing applies to other programs,
//! ports or profiles. The status also names connected networks Windows treats as Public, where
//! that rule does not apply. Other systems report `supported: false`.

/// The one rule this app manages, replaced as a whole on every allow.
pub const RULE_NAME: &str = "Common (paired devices)";

#[derive(serde::Serialize, Debug, Default, PartialEq)]
pub struct Status {
    pub supported: bool,
    /// An enabled inbound allow rule for UDP on the Private profile names the bundled daemon.
    pub allowed: bool,
    /// The program the matching rule (else the first rule of that name) names.
    pub rule_program: Option<String>,
    /// Connected networks Windows treats as Public: the rule does not apply there, so other
    /// devices still cannot reach this computer on them. Changing that is the user's call
    /// (Settings > Network & internet); the app never widens the rule to Public.
    pub public_networks: Vec<String>,
}

/// `cmd.exe` parameters that delete any rule of that name (it may name an older install path)
/// and add ours. `cmd /c a & b` exits with `b`'s code, so a missing old rule is not an error.
/// Inside double quotes cmd takes everything literally except `%` (delayed `!` is off with
/// `/v:off`); a Windows path cannot hold `"` or control characters, so anything else is refused.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn allow_params(program: &str) -> Result<String, String> {
    if program.is_empty() || program.chars().any(|c| c == '%' || c == '"' || c.is_control()) {
        return Err(format!(
            "the daemon's path cannot be passed to netsh safely ({program}); add the rule in Windows Defender Firewall instead"
        ));
    }
    let netsh = "netsh advfirewall firewall";
    Ok(format!(
        "/d /v:off /c {netsh} delete rule name=\"{RULE_NAME}\" & {netsh} add rule name=\"{RULE_NAME}\" dir=in action=allow program=\"{program}\" protocol=UDP profile=private enable=yes"
    ))
}

/// Reads `netsh advfirewall firewall show rule name=... verbose`: is any rule of that name an
/// enabled inbound allow rule for UDP on the Private profile naming exactly `program`?
// ponytail: netsh prints English keys only on English Windows, in the OEM code page; a localized
// Windows or a non-ASCII path reads as not allowed, so the (idempotent) offer stays visible.
// INetFwPolicy2 over COM if that matters.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn parse_rules(output: &str, program: &str) -> Status {
    let mut rules: Vec<Vec<(&str, &str)>> = Vec::new();
    for line in output.lines() {
        let Some((k, v)) = line.split_once(':') else { continue };
        let (k, v) = (k.trim(), v.trim());
        if k == "Rule Name" {
            rules.push(Vec::new());
        }
        if let Some(r) = rules.last_mut() {
            r.push((k, v));
        }
    }
    fn get<'a>(r: &[(&str, &'a str)], key: &str) -> &'a str {
        r.iter().find(|(k, _)| *k == key).map_or("", |(_, v)| v)
    }
    let matches = |r: &&Vec<(&str, &str)>| {
        get(r, "Enabled") == "Yes"
            && get(r, "Direction") == "In"
            && get(r, "Profiles").split(',').any(|p| p.trim() == "Private")
            && get(r, "Protocol").eq_ignore_ascii_case("UDP")
            && get(r, "Action") == "Allow"
            && get(r, "Program").eq_ignore_ascii_case(program)
    };
    let hit = rules.iter().find(matches);
    Status {
        supported: true,
        allowed: hit.is_some(),
        rule_program: hit
            .or(rules.first())
            .map(|r| get(r, "Program"))
            .filter(|p| !p.is_empty())
            .map(str::to_string),
        public_networks: Vec::new(),
    }
}

/// PowerShell's `Get-NetConnectionProfile`, one connected network per line: its category name, a
/// tab, its name (see `PROFILES`). Category names are not localized.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn parse_public(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|l| l.trim_end().split_once('\t'))
        .filter(|(category, _)| category.trim() == "Public")
        .map(|(_, name)| name.trim().to_string())
        .collect()
}

/// Read-only. No double quotes: Rust quotes the argument for powershell.exe's own parser.
#[cfg(windows)]
const PROFILES: &str = "[Console]::OutputEncoding = [Text.Encoding]::UTF8; \
    Get-NetConnectionProfile | ForEach-Object { $_.NetworkCategory.ToString() + [char]9 + $_.Name }";

#[cfg(not(windows))]
pub fn status() -> Status {
    Status::default()
}

#[cfg(not(windows))]
pub fn allow() -> Result<(), String> {
    Err("Windows Firewall rules apply only on Windows".into())
}

/// `common.exe` next to the app: where Tauri puts the `binaries/common` sidecar.
#[cfg(windows)]
fn daemon_program() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot locate the app: {e}"))?;
    let dir = exe.parent().ok_or("cannot locate the app's folder")?;
    Ok(dir.join("common.exe").to_string_lossy().into_owned())
}

/// `C:\Windows\System32`: cmd and netsh are never looked up on PATH or in the app's folder.
#[cfg(windows)]
fn system32() -> std::path::PathBuf {
    use std::os::windows::ffi::OsStringExt;
    let mut buf = [0u16; 260];
    // SAFETY: the buffer and its length in u16 units.
    let n = unsafe {
        windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32)
    };
    match n as usize {
        1..=259 => std::ffi::OsString::from_wide(&buf[..n as usize]).into(),
        _ => r"C:\Windows\System32".into(),
    }
}

/// Read-only, no elevation.
#[cfg(windows)]
pub fn status() -> Status {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let Ok(program) = daemon_program() else {
        return Status {
            supported: true,
            ..Status::default()
        };
    };
    let out = std::process::Command::new(system32().join("netsh.exe"))
        .raw_arg("advfirewall firewall show rule")
        .raw_arg(format!("name=\"{RULE_NAME}\""))
        .raw_arg("verbose")
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    let mut status = match out {
        // No rule of that name: netsh exits 1 with "No rules match", which parses as not allowed.
        Ok(o) => parse_rules(&String::from_utf8_lossy(&o.stdout), &program),
        Err(e) => {
            eprintln!("common-desktop: could not read the firewall rules: {e}");
            Status {
                supported: true,
                ..Status::default()
            }
        }
    };
    // ponytail: PowerShell starts in about a second, once per Peers visit; INetworkListManager over
    // COM if that matters. A failure reads as no Public network (the rule notice still works).
    let ps = system32().join(r"WindowsPowerShell\v1.0\powershell.exe");
    match std::process::Command::new(ps)
        .args(["-NoProfile", "-NonInteractive", "-Command", PROFILES])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    {
        Ok(o) => status.public_networks = parse_public(&String::from_utf8_lossy(&o.stdout)),
        Err(e) => eprintln!("common-desktop: could not read the network categories: {e}"),
    }
    status
}

/// Runs [`allow_params`] in an elevated `cmd.exe` through the standard elevation prompt and waits
/// for it. `Err("cancelled")` when the user declines the prompt.
#[cfg(windows)]
pub fn allow() -> Result<(), String> {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_CANCELLED, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
    use windows_sys::Win32::UI::Shell::{
        ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    };
    let wide = |s: &std::ffi::OsStr| {
        use std::os::windows::ffi::OsStrExt;
        s.encode_wide().chain([0]).collect::<Vec<u16>>()
    };
    let params = wide(allow_params(&daemon_program()?)?.as_ref());
    let sys = system32();
    let file = wide(sys.join("cmd.exe").as_os_str());
    // cmd looks in its working directory before PATH: System32, so `netsh` is the system's own.
    let dir = wide(sys.as_os_str());
    let verb = wide("runas".as_ref());
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        lpVerb: verb.as_ptr(),
        lpFile: file.as_ptr(),
        lpParameters: params.as_ptr(),
        lpDirectory: dir.as_ptr(),
        nShow: 0, // SW_HIDE: netsh's console never shows
        ..Default::default()
    };
    // SAFETY: `info` is fully initialised and its strings outlive the call; the process handle is
    // waited on and closed below.
    unsafe {
        if ShellExecuteExW(&mut info) == 0 {
            let err = GetLastError();
            return Err(if err == ERROR_CANCELLED {
                "cancelled".into()
            } else {
                format!("Windows could not start the elevation prompt (error {err})")
            });
        }
        if info.hProcess.is_null() {
            return Err("Windows did not report the netsh process".into());
        }
        let waited = WaitForSingleObject(info.hProcess, 60_000);
        let mut code = 1u32;
        let read = GetExitCodeProcess(info.hProcess, &mut code);
        CloseHandle(info.hProcess);
        if waited != WAIT_OBJECT_0 {
            return Err("netsh did not finish within 60 s".into());
        }
        if read == 0 || code != 0 {
            return Err(format!("netsh could not add the rule (exit code {code})"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROGRAM: &str = r"C:\Users\Nasr\AppData\Local\Common\common.exe";

    fn rule(enabled: &str, dir: &str, profiles: &str, protocol: &str, program: &str, action: &str) -> String {
        format!(
            "\r\nRule Name:                            {RULE_NAME}\r\n----------------------------------------------------------------------\r\n\
             Enabled:                              {enabled}\r\nDirection:                            {dir}\r\nProfiles:                             {profiles}\r\n\
             Grouping:                             \r\nLocalIP:                              Any\r\nRemoteIP:                             Any\r\n\
             Protocol:                             {protocol}\r\nLocalPort:                            Any\r\nRemotePort:                           Any\r\n\
             Edge traversal:                       No\r\nProgram:                              {program}\r\nInterfaceTypes:                       Any\r\n\
             Security:                             NotRequired\r\nRule source:                          Local Setting\r\nAction:                               {action}\r\n"
        )
    }

    #[test]
    fn allow_params_scope_the_rule_to_the_daemon_inbound_udp_private() {
        let p = allow_params(r"C:\Program Files\Common (beta)\common.exe").unwrap();
        assert_eq!(
            p,
            "/d /v:off /c netsh advfirewall firewall delete rule name=\"Common (paired devices)\" & \
             netsh advfirewall firewall add rule name=\"Common (paired devices)\" dir=in action=allow \
             program=\"C:\\Program Files\\Common (beta)\\common.exe\" protocol=UDP profile=private enable=yes"
        );
        assert!(!p.contains("public") && !p.contains("domain") && !p.contains("profile=any"));
        for bad in [
            "",
            r"C:\100%\common.exe",
            "C:\\a\"b\\common.exe",
            "C:\\a\r\nnetsh\\common.exe",
        ] {
            assert!(allow_params(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn only_an_enabled_inbound_udp_private_allow_rule_for_the_daemon_counts() {
        let ok = rule("Yes", "In", "Private", "UDP", PROGRAM, "Allow");
        assert_eq!(
            parse_rules(&ok, PROGRAM),
            Status {
                supported: true,
                allowed: true,
                rule_program: Some(PROGRAM.into()),
                public_networks: vec![]
            }
        );
        // Windows paths are case-insensitive.
        assert!(parse_rules(&ok, &PROGRAM.to_uppercase()).allowed);
        assert!(parse_rules(&rule("Yes", "In", "Domain,Private", "UDP", PROGRAM, "Allow"), PROGRAM).allowed);
        for (bad, why) in [
            (rule("No", "In", "Private", "UDP", PROGRAM, "Allow"), "disabled"),
            (rule("Yes", "Out", "Private", "UDP", PROGRAM, "Allow"), "outbound"),
            (rule("Yes", "In", "Public", "UDP", PROGRAM, "Allow"), "public only"),
            (rule("Yes", "In", "Private", "TCP", PROGRAM, "Allow"), "tcp"),
            (rule("Yes", "In", "Private", "UDP", PROGRAM, "Block"), "block"),
            (
                rule("Yes", "In", "Private", "UDP", r"C:\Old\common.exe", "Allow"),
                "another install",
            ),
        ] {
            assert!(!parse_rules(&bad, PROGRAM).allowed, "{why}");
        }
        let moved = parse_rules(
            &rule("Yes", "In", "Private", "UDP", r"C:\Old\common.exe", "Allow"),
            PROGRAM,
        );
        assert_eq!(moved.rule_program.as_deref(), Some(r"C:\Old\common.exe"));
        // Two rules of that name: the matching one decides.
        let both = rule("Yes", "Out", "Private", "UDP", PROGRAM, "Allow") + &ok;
        assert!(parse_rules(&both, PROGRAM).allowed);
        assert_eq!(
            parse_rules("\r\nNo rules match the specified criteria.\r\n", PROGRAM),
            Status {
                supported: true,
                allowed: false,
                rule_program: None,
                public_networks: vec![]
            }
        );
    }

    /// A rule for Private networks does nothing on a Public one (Windows' default for a new Wi-Fi
    /// network or hotspot): those are reported by name so the app keeps saying so.
    #[test]
    fn public_networks_are_reported_by_name() {
        let out =
            "Private\tHome Wi-Fi\r\nPublic\teduroam\r\nDomainAuthenticated\tcorp.example\r\nPublic\tIdentifying...\r\n";
        assert_eq!(
            parse_public(out),
            vec!["eduroam".to_string(), "Identifying...".to_string()]
        );
        assert!(parse_public("Private\tHome Wi-Fi\r\n").is_empty());
        assert!(parse_public("").is_empty());
    }
}
