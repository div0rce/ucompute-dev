//! Windows Firewall consent step. Pairing started on Windows listens on a random UDP port per
//! window, which the firewall drops by default (no rule, and Windows never asks for a console
//! program). The app offers one rule for the bundled daemon only, inbound UDP, Private profile only,
//! added through the standard elevation prompt: the user's click on that prompt is the
//! authorization. Nothing here runs without that click, and nothing applies to other programs,
//! ports or profiles. The status also names connected networks that are not Private (Public or
//! domain), where that rule does not apply. Other systems report `supported: false`.

/// The one rule this app manages for its own program, replaced on every allow.
pub const RULE_NAME: &str = "Common (paired devices)";

/// Why status and Allow are not offered for a service the app did not start.
pub const NOT_OURS: &str =
    "the running service was not started by this app, so its firewall rule cannot be checked or added here";

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
    /// Connected domain networks (any other category that is not Private): the rule does not apply
    /// there either, and the user cannot switch them to Private; their administrator decides.
    pub domain_networks: Vec<String>,
    /// The app adopted a service it did not start (already serving when it opened, or serving after
    /// the app's own daemon exited). It may be another program than the bundled `common.exe`, so its
    /// rule is neither checked nor added.
    pub adopted: bool,
}

/// `cmd.exe` parameters that delete this program's own rule of that name and add it again. Other
/// Windows accounts' per-user installs have rules of the same name for their own paths, which stay.
/// `cmd /c a & b` exits with `b`'s code, so a missing old rule is not an error.
/// Inside double quotes cmd takes everything literally except `%` (delayed `!` is off with
/// `/v:off`); a Windows path cannot hold `"` or control characters, so anything else is refused.
// ponytail: a rule this account added for an older install path is left in place (Private UDP for
// a program that is gone); a per-account rule name would let the delete clean it up.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn allow_params(program: &str) -> Result<String, String> {
    if program.is_empty() || program.chars().any(|c| c == '%' || c == '"' || c.is_control()) {
        return Err(format!(
            "the daemon's path cannot be passed to netsh safely ({program}); add the rule in Windows Defender Firewall instead"
        ));
    }
    let netsh = "netsh advfirewall firewall";
    Ok(format!(
        "/d /v:off /c {netsh} delete rule name=\"{RULE_NAME}\" program=\"{program}\" & {netsh} add rule name=\"{RULE_NAME}\" dir=in action=allow program=\"{program}\" protocol=UDP profile=private enable=yes"
    ))
}

/// The read-only PowerShell script [`status`] runs: UTF-8 output and non-localized enum names
/// (netsh localizes its labels and writes the OEM code page, so it cannot be read on every
/// Windows). One line per connected network, `net<TAB>category<TAB>name`; one per rule of
/// [`RULE_NAME`], `rule<TAB>enabled<TAB>direction<TAB>action<TAB>profiles<TAB>protocol<TAB>program`;
/// then `end`. Any failure other than "nothing found" stops it before `end`, so a script that could
/// not read is never taken for "no rule" (the encoding comes first: if it cannot be set, ASCII still
/// reads). `-ErrorAction SilentlyContinue` hides every non-terminating error, and the NetSecurity
/// cmdlets report access denied or a broken provider that way too, so the errors are kept
/// (`-ErrorVariable`) and any that is not the not-found one is thrown. No double quotes: Rust quotes
/// the argument for powershell.exe's own parser.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn script() -> String {
    format!(
        "[Console]::OutputEncoding = [Text.Encoding]::UTF8; $ErrorActionPreference = 'Stop'; $t = [char]9; \
         Get-NetConnectionProfile -ErrorAction SilentlyContinue -ErrorVariable ne | ForEach-Object {{ 'net', $_.NetworkCategory, $_.Name -join $t }}; \
         Get-NetFirewallRule -DisplayName '{RULE_NAME}' -ErrorAction SilentlyContinue -ErrorVariable fe | ForEach-Object {{ \
         'rule', $_.Enabled, $_.Direction, $_.Action, $_.Profile, ($_ | Get-NetFirewallPortFilter).Protocol, \
         ($_ | Get-NetFirewallApplicationFilter).Program -join $t }}; \
         (@($ne) + @($fe)) | Where-Object {{ $_.FullyQualifiedErrorId -notlike 'CmdletizationQuery_NotFound*' }} | ForEach-Object {{ throw $_ }}; 'end'"
    )
}

/// Reads [`script`]'s output: is any rule of that name an enabled inbound allow rule for UDP on
/// the Private profile naming exactly `program`, and which connected networks are not Private?
/// `None` when the script did not reach `end`.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn parse(output: &str, program: &str) -> Option<Status> {
    let mut status = Status {
        supported: true,
        ..Status::default()
    };
    let mut rules: Vec<Vec<&str>> = Vec::new();
    let mut end = false;
    for line in output.lines() {
        // [Text.Encoding]::UTF8 may write a byte order mark first.
        let line = line.trim_start_matches('\u{feff}').trim_end();
        match line.split_once('\t') {
            Some(("net", rest)) => match rest.split_once('\t') {
                Some(("Private", _)) | None => {}
                Some(("Public", name)) => status.public_networks.push(name.trim().to_string()),
                Some((_, name)) => status.domain_networks.push(name.trim().to_string()),
            },
            Some(("rule", rest)) => {
                let f: Vec<&str> = rest.splitn(6, '\t').collect();
                if f.len() == 6 {
                    rules.push(f);
                }
            }
            _ => end |= line == "end",
        }
    }
    if !end {
        return None;
    }
    // Fields: enabled, direction, action, profiles ("Domain, Private", or "Any" for all), protocol, program.
    let hit = rules.iter().find(|f| {
        f[0] == "True"
            && f[1] == "Inbound"
            && f[2] == "Allow"
            && f[3].split(',').any(|p| matches!(p.trim(), "Private" | "Any"))
            && f[4].eq_ignore_ascii_case("UDP")
            && f[5].eq_ignore_ascii_case(program)
    });
    status.allowed = hit.is_some();
    status.rule_program = hit
        .or(rules.first())
        .map(|f| f[5])
        .filter(|p| !p.is_empty())
        .map(str::to_string);
    Some(status)
}

/// The app checks and adds the rule for the bundled `common.exe` only. A service it adopted may be
/// another program, so for it nothing is checked (`adopted`) and Allow refuses.
pub fn status_for(ours: bool) -> Result<Status, String> {
    if !ours {
        return Ok(Status {
            supported: cfg!(windows),
            adopted: true,
            ..Status::default()
        });
    }
    status()
}

/// See [`status_for`].
pub fn allow_for(ours: bool) -> Result<(), String> {
    if !ours {
        return Err(NOT_OURS.into());
    }
    allow()
}

#[cfg(not(windows))]
pub fn status() -> Result<Status, String> {
    Ok(Status::default())
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

/// Read-only, no elevation. `Err` when the rules could not be read: the app then claims nothing
/// about them (a failed read is never shown as a missing rule).
// ponytail: PowerShell starts in about a second, on each visit that shows the firewall state;
// INetFwPolicy2 and INetworkListManager over COM if that matters.
#[cfg(windows)]
pub fn status() -> Result<Status, String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let program = daemon_program()?;
    let ps = system32().join(r"WindowsPowerShell\v1.0\powershell.exe");
    let out = std::process::Command::new(ps)
        .args(["-NoProfile", "-NonInteractive", "-Command", &script()])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("could not start PowerShell to read the firewall rules: {e}"))?;
    parse(&String::from_utf8_lossy(&out.stdout), &program).ok_or_else(|| {
        let err = String::from_utf8_lossy(&out.stderr);
        format!("could not read the firewall rules: {}", err.trim())
    })
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

    /// One `rule` line as the script prints it.
    fn rule(enabled: &str, dir: &str, action: &str, profiles: &str, protocol: &str, program: &str) -> String {
        format!("rule\t{enabled}\t{dir}\t{action}\t{profiles}\t{protocol}\t{program}\r\n")
    }
    fn read(lines: &str) -> Status {
        parse(&format!("{lines}end\r\n"), PROGRAM).expect("ends with end")
    }
    fn ok() -> String {
        rule("True", "Inbound", "Allow", "Private", "UDP", PROGRAM)
    }

    #[test]
    fn allow_params_scope_the_rule_to_the_daemon_inbound_udp_private() {
        let p = allow_params(r"C:\Program Files\Common (beta)\common.exe").unwrap();
        // The delete names this program too: another Windows account's rule of the same name stays.
        assert_eq!(
            p,
            "/d /v:off /c netsh advfirewall firewall delete rule name=\"Common (paired devices)\" \
             program=\"C:\\Program Files\\Common (beta)\\common.exe\" & \
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

    /// The read names the rule in single quotes (no double quotes reach powershell.exe's parser),
    /// asks for non-localized values and ends with the `end` marker.
    #[test]
    fn the_script_reads_the_rule_by_name_without_double_quotes() {
        let s = script();
        assert!(
            s.contains("Get-NetFirewallRule -DisplayName 'Common (paired devices)'"),
            "{s}"
        );
        assert!(s.contains("[Text.Encoding]::UTF8") && s.ends_with("'end'"), "{s}");
        assert!(!s.contains('"') && !s.contains("netsh"), "{s}");
    }

    /// The finding: `-ErrorAction SilentlyContinue` also hid access denied and provider failures, so a
    /// read that failed still printed `end` and read as "no rule". Each read that silences its errors
    /// keeps them, and every kept error other than not-found is thrown before `end`.
    #[test]
    fn the_script_throws_every_error_but_not_found_before_end() {
        let s = script();
        let silenced = s.matches("-ErrorAction SilentlyContinue").count();
        assert_eq!(silenced, 2, "{s}");
        assert_eq!(
            s.matches("-ErrorAction SilentlyContinue -ErrorVariable ").count(),
            silenced,
            "{s}"
        );
        let check = "(@($ne) + @($fe)) | Where-Object { $_.FullyQualifiedErrorId -notlike 'CmdletizationQuery_NotFound*' } | ForEach-Object { throw $_ }; 'end'";
        assert!(s.ends_with(check), "{s}");
        assert!(
            s.contains("-ErrorVariable ne |") && s.contains("-ErrorVariable fe |"),
            "{s}"
        );
    }

    #[test]
    fn only_an_enabled_inbound_udp_private_allow_rule_for_the_daemon_counts() {
        assert_eq!(
            read(&ok()),
            Status {
                supported: true,
                allowed: true,
                rule_program: Some(PROGRAM.into()),
                ..Status::default()
            }
        );
        // Windows paths are case-insensitive.
        assert!(parse(&format!("{}end", ok()), &PROGRAM.to_uppercase()).unwrap().allowed);
        assert!(read(&rule("True", "Inbound", "Allow", "Domain, Private", "UDP", PROGRAM)).allowed);
        assert!(read(&rule("True", "Inbound", "Allow", "Any", "UDP", PROGRAM)).allowed);
        for (bad, why) in [
            (rule("False", "Inbound", "Allow", "Private", "UDP", PROGRAM), "disabled"),
            (rule("True", "Outbound", "Allow", "Private", "UDP", PROGRAM), "outbound"),
            (
                rule("True", "Inbound", "Allow", "Public", "UDP", PROGRAM),
                "public only",
            ),
            (rule("True", "Inbound", "Allow", "Private", "TCP", PROGRAM), "tcp"),
            (rule("True", "Inbound", "Block", "Private", "UDP", PROGRAM), "block"),
            (
                rule("True", "Inbound", "Allow", "Private", "UDP", r"C:\Old\common.exe"),
                "another install",
            ),
        ] {
            assert!(!read(&bad).allowed, "{why}");
        }
        let moved = read(&rule(
            "True",
            "Inbound",
            "Allow",
            "Private",
            "UDP",
            r"C:\Old\common.exe",
        ));
        assert_eq!(moved.rule_program.as_deref(), Some(r"C:\Old\common.exe"));
        // Two rules of that name: the matching one decides.
        assert!(read(&(rule("True", "Outbound", "Allow", "Private", "UDP", PROGRAM) + &ok())).allowed);
        // No rule of that name.
        assert_eq!(
            read(""),
            Status {
                supported: true,
                ..Status::default()
            }
        );
    }

    /// The finding: netsh's English labels and OEM code page read as "no rule" on a localized
    /// Windows or under a non-ASCII user name. The script's values are enum names and UTF-8.
    #[test]
    fn a_non_ascii_path_and_a_byte_order_mark_still_match() {
        let jose = "C:\\Users\\José\\AppData\\Local\\Common\\common.exe";
        let out = format!(
            "\u{feff}net\tPrivate\tZuhause\r\n{}end\r\n",
            rule("True", "Inbound", "Allow", "Private", "UDP", jose)
        );
        assert!(parse(&out, jose).unwrap().allowed);
        let out = format!(
            "\u{feff}{}end\r\n",
            rule("True", "Inbound", "Allow", "Private", "UDP", jose)
        );
        assert!(parse(&out, jose).unwrap().allowed);
    }

    /// A script that stopped before `end` (PowerShell blocked, a cmdlet missing) is not "no rule".
    #[test]
    fn a_read_that_did_not_finish_is_not_a_missing_rule() {
        assert_eq!(parse("", PROGRAM), None);
        assert_eq!(parse("net\tPublic\teduroam\r\n", PROGRAM), None);
        assert_eq!(parse(&ok(), PROGRAM), None);
    }

    /// A rule for Private networks does nothing on a Public one (Windows' default for a new Wi-Fi
    /// network or hotspot) or on a domain network: those are reported by name, separately, since only
    /// a Public one can be switched to Private by the user.
    #[test]
    fn networks_that_are_not_private_are_reported_by_name() {
        let s = read("net\tPrivate\tHome Wi-Fi\r\nnet\tPublic\teduroam\r\nnet\tDomainAuthenticated\tcorp.example\r\nnet\tPublic\tIdentifying...\r\n");
        assert_eq!(
            s.public_networks,
            vec!["eduroam".to_string(), "Identifying...".to_string()]
        );
        assert_eq!(s.domain_networks, vec!["corp.example".to_string()]);
        let s = read("net\tPrivate\tHome Wi-Fi\r\n");
        assert!(s.public_networks.is_empty() && s.domain_networks.is_empty());
    }

    /// The app adopted a service it did not start: its program may not be the bundled one, so the
    /// status claims nothing about a rule and Allow refuses instead of adding one for another path.
    #[test]
    fn an_adopted_service_is_neither_checked_nor_allowed() {
        let s = status_for(false).unwrap();
        assert!(s.adopted && !s.allowed);
        assert_eq!(s.supported, cfg!(windows));
        assert_eq!(allow_for(false), Err(NOT_OURS.to_string()));
        assert!(!status_for(true).unwrap().adopted);
    }
}
