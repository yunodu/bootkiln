//! `autounattend.xml` answer file for Windows Setup, built from opt-in customisations
//! (the same set Rufus offers).

use crate::error::Error;
use crate::windows_iso::{Arch, WindowsInfo};

/// File name Windows Setup looks for at the root of every drive.
pub const FILE_NAME: &str = "autounattend.xml";
const MAX_USER_NAME: usize = 20;
const USER_NAME_FORBIDDEN: &[char] = &['"', '/', '\\', '[', ']', ':', ';', '|', '=', ',', '+', '*', '?', '<', '>', '@'];

/// Regional settings copied from the PC running bootkiln.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    /// Keyboard as `<language id>:<layout id>`, e.g. `0407:00000407`.
    pub input_locale: String,
    /// Locale names, e.g. `de-DE`.
    pub system_locale: String,
    pub user_locale: String,
    /// Windows time zone key name, e.g. `W. Europe Standard Time`.
    pub time_zone: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Customisations {
    /// Skip Windows 11's TPM 2.0, Secure Boot and RAM checks.
    pub skip_hardware_checks: bool,
    /// Allow setup without a Microsoft account.
    pub skip_online_account: bool,
    /// Local administrator created with an empty password that must be changed at first
    /// sign-in.
    pub local_account: Option<String>,
    pub region: Option<Region>,
    /// Answer the privacy / data collection questions with the most private choices.
    pub skip_privacy_questions: bool,
    /// Prevent BitLocker's automatic device encryption.
    pub disable_bitlocker: bool,
}

impl Customisations {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// The answer file, or an error when an option does not apply to this Windows.
pub fn answer_file(options: &Customisations, windows: &WindowsInfo) -> Result<String, Error> {
    if options.skip_hardware_checks && windows.is_windows_11() == Some(false) {
        return Err(Error::WindowsOption(
            "skipping the TPM / Secure Boot / RAM checks only applies to Windows 11; this ISO is Windows 10".into(),
        ));
    }
    let user = options.local_account.as_deref().map(validate_user_name).transpose()?;
    let arch = windows.arch;
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
         <unattend xmlns=\"urn:schemas-microsoft-com:unattend\" xmlns:wcm=\"http://schemas.microsoft.com/WMIConfig/2002/State\">\n",
    );

    if options.skip_hardware_checks {
        xml.push_str("  <settings pass=\"windowsPE\">\n");
        xml.push_str(&component("Microsoft-Windows-Setup", arch));
        xml.push_str("      <UserData>\n        <ProductKey>\n          <Key />\n        </ProductKey>\n      </UserData>\n");
        xml.push_str(&command_list(
            CommandList::RunSynchronous,
            &["BypassTPMCheck", "BypassSecureBootCheck", "BypassRAMCheck"]
                .map(|value| format!("reg add HKLM\\SYSTEM\\Setup\\LabConfig /v {value} /t REG_DWORD /d 1 /f")),
        ));
        xml.push_str("    </component>\n  </settings>\n");
    }

    let mut specialize = Vec::new();
    if options.skip_online_account {
        specialize.push("reg add HKLM\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\OOBE /v BypassNRO /t REG_DWORD /d 1 /f".to_owned());
    }
    if options.disable_bitlocker {
        specialize.push("reg add HKLM\\SYSTEM\\CurrentControlSet\\Control\\BitLocker /v PreventDeviceEncryption /t REG_DWORD /d 1 /f".to_owned());
    }
    if !specialize.is_empty() {
        xml.push_str("  <settings pass=\"specialize\">\n");
        xml.push_str(&component("Microsoft-Windows-Deployment", arch));
        xml.push_str(&command_list(CommandList::RunSynchronous, &specialize));
        xml.push_str("    </component>\n  </settings>\n");
    }

    let shell_setup = user.is_some() || options.region.is_some() || options.skip_privacy_questions;
    if shell_setup {
        xml.push_str("  <settings pass=\"oobeSystem\">\n");
        if let Some(region) = &options.region {
            xml.push_str(&component("Microsoft-Windows-International-Core", arch));
            for (tag, value) in [
                ("InputLocale", &region.input_locale),
                ("SystemLocale", &region.system_locale),
                ("UserLocale", &region.user_locale),
            ] {
                xml.push_str(&format!("      <{tag}>{}</{tag}>\n", escape(value)));
            }
            xml.push_str("    </component>\n");
        }
        xml.push_str(&component("Microsoft-Windows-Shell-Setup", arch));
        if let Some(region) = &options.region {
            xml.push_str(&format!("      <TimeZone>{}</TimeZone>\n", escape(&region.time_zone)));
        }
        if options.skip_privacy_questions || user.is_some() {
            xml.push_str("      <OOBE>\n");
            if options.skip_privacy_questions {
                xml.push_str("        <ProtectYourPC>3</ProtectYourPC>\n");
            }
            if user.is_some() {
                xml.push_str("        <HideOnlineAccountScreens>true</HideOnlineAccountScreens>\n");
            }
            xml.push_str("      </OOBE>\n");
        }
        if let Some(user) = &user {
            let user = escape(user);
            xml.push_str(&format!(
                "      <UserAccounts>\n        <LocalAccounts>\n          <LocalAccount wcm:action=\"add\">\n            \
                 <Name>{user}</Name>\n            <DisplayName>{user}</DisplayName>\n            \
                 <Group>Administrators;Power Users</Group>\n            \
                 <Password>\n              <Value></Value>\n              <PlainText>true</PlainText>\n            </Password>\n          \
                 </LocalAccount>\n        </LocalAccounts>\n      </UserAccounts>\n"
            ));
            // One automatic sign-in runs the commands; the password must be set at the next.
            xml.push_str(&format!(
                "      <AutoLogon>\n        <Password>\n          <Value></Value>\n          <PlainText>true</PlainText>\n        </Password>\n        \
                 <Enabled>true</Enabled>\n        <LogonCount>1</LogonCount>\n        <Username>{user}</Username>\n      </AutoLogon>\n"
            ));
            xml.push_str(&command_list(
                CommandList::FirstLogon,
                &[format!("net user &quot;{user}&quot; /logonpasswordchg:yes"), "net accounts /maxpwage:unlimited".to_owned()],
            ));
        }
        xml.push_str("    </component>\n  </settings>\n");
    }

    xml.push_str("</unattend>\n");
    Ok(xml)
}

fn component(name: &str, arch: Arch) -> String {
    format!(
        "    <component name=\"{name}\" processorArchitecture=\"{}\" language=\"neutral\" \
         publicKeyToken=\"31bf3856ad364e35\" versionScope=\"nonSxS\">\n",
        arch.unattend_name()
    )
}

#[derive(Clone, Copy)]
enum CommandList {
    /// Runs during a setup pass; each entry is a `Path`.
    RunSynchronous,
    /// Runs at the first sign-in; each entry is a `CommandLine`.
    FirstLogon,
}

/// Commands are inserted verbatim: callers escape anything user-provided.
fn command_list(kind: CommandList, commands: &[String]) -> String {
    let (list, item, field) = match kind {
        CommandList::RunSynchronous => ("RunSynchronous", "RunSynchronousCommand", "Path"),
        CommandList::FirstLogon => ("FirstLogonCommands", "SynchronousCommand", "CommandLine"),
    };
    let mut out = format!("      <{list}>\n");
    for (i, command) in commands.iter().enumerate() {
        out.push_str(&format!(
            "        <{item} wcm:action=\"add\">\n          <Order>{}</Order>\n          <{field}>{command}</{field}>\n        </{item}>\n",
            i + 1
        ));
    }
    out.push_str(&format!("      </{list}>\n"));
    out
}

/// Windows local account names: 1-20 characters, none of `"/\[]:;|=,+*?<>@`, not only
/// dots and spaces. Surrounding whitespace is dropped.
fn validate_user_name(name: &str) -> Result<&str, Error> {
    let name = name.trim();
    let valid = !name.is_empty()
        && name.chars().count() <= MAX_USER_NAME
        && !name.chars().any(|c| c.is_control() || USER_NAME_FORBIDDEN.contains(&c))
        && !name.chars().all(|c| c == '.' || c == ' ');
    if valid {
        Ok(name)
    } else {
        Err(Error::WindowsOption(format!(
            "{name:?} is not a valid Windows account name (1-20 characters, none of \"/\\[]:;|=,+*?<>@)"
        )))
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIN11: WindowsInfo = WindowsInfo { arch: Arch::Amd64, build: Some(26300) };
    const WIN10: WindowsInfo = WindowsInfo { arch: Arch::Arm64, build: Some(19045) };

    fn all() -> Customisations {
        Customisations {
            skip_hardware_checks: true,
            skip_online_account: true,
            local_account: Some(" Pat & Co ".into()),
            region: Some(Region {
                input_locale: "0407:00000407".into(),
                system_locale: "de-DE".into(),
                user_locale: "de-DE".into(),
                time_zone: "W. Europe Standard Time".into(),
            }),
            skip_privacy_questions: true,
            disable_bitlocker: true,
        }
    }

    /// Well-formedness check without an XML dependency: every opened tag closes in order.
    fn assert_balanced(xml: &str) {
        let mut stack = Vec::new();
        for tag in xml.split('<').skip(1).map(|t| t.split('>').next().unwrap()) {
            if tag.starts_with('?') || tag.ends_with('/') {
                continue;
            }
            match tag.strip_prefix('/') {
                Some(name) => assert_eq!(stack.pop().as_deref(), Some(name), "mismatched </{name}>"),
                None => stack.push(tag.split(' ').next().unwrap().to_owned()),
            }
        }
        assert!(stack.is_empty(), "unclosed {stack:?}");
    }

    #[test]
    fn every_option_lands_in_its_setup_pass() {
        let xml = answer_file(&all(), &WIN11).unwrap();
        assert_balanced(&xml);
        let pass = |name: &str| {
            let start = xml.find(&format!("<settings pass=\"{name}\">")).unwrap_or_else(|| panic!("no {name} pass"));
            &xml[start..start + xml[start..].find("</settings>").unwrap()]
        };
        assert!(pass("windowsPE").contains("BypassTPMCheck") && pass("windowsPE").contains("BypassRAMCheck"));
        assert!(pass("specialize").contains("BypassNRO") && pass("specialize").contains("PreventDeviceEncryption"));
        let oobe = pass("oobeSystem");
        for needle in [
            "<InputLocale>0407:00000407</InputLocale>",
            "<TimeZone>W. Europe Standard Time</TimeZone>",
            "<ProtectYourPC>3</ProtectYourPC>",
            "<Name>Pat &amp; Co</Name>",
            "net user &quot;Pat &amp; Co&quot; /logonpasswordchg:yes",
        ] {
            assert!(oobe.contains(needle), "missing {needle}");
        }
        assert!(xml.matches("processorArchitecture=\"amd64\"").count() >= 4);
    }

    #[test]
    fn only_requested_passes_are_written() {
        let only_bitlocker = Customisations { disable_bitlocker: true, ..Default::default() };
        let xml = answer_file(&only_bitlocker, &WIN10).unwrap();
        assert_balanced(&xml);
        assert!(!xml.contains("windowsPE") && !xml.contains("oobeSystem"));
        assert!(xml.contains("processorArchitecture=\"arm64\""));
    }

    #[test]
    fn hardware_check_bypass_is_refused_for_windows_10() {
        let options = Customisations { skip_hardware_checks: true, ..Default::default() };
        assert!(matches!(answer_file(&options, &WIN10), Err(Error::WindowsOption(_))));
        // Unknown build: allowed, the registry values are harmless on Windows 10.
        assert!(answer_file(&options, &WindowsInfo { build: None, ..WIN10 }).is_ok());
    }

    #[test]
    fn invalid_account_names_are_refused() {
        for name in ["", "   ", "a@b", "x".repeat(21).as_str(), "..", "tab\tname"] {
            let options = Customisations { local_account: Some(name.into()), ..Default::default() };
            assert!(matches!(answer_file(&options, &WIN11), Err(Error::WindowsOption(_))), "{name:?}");
        }
    }
}
