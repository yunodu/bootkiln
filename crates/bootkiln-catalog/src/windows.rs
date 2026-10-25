//! Windows 11 from Microsoft's download page, using the same requests the page makes in a
//! browser (as Rufus's Fido script does). Microsoft has no official API for this, so it may
//! stop working; the error then says Microsoft refused.

use std::time::{SystemTime, UNIX_EPOCH};

use regex_lite::Regex;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::{Error, Release, http};

const PAGE: &str = "https://www.microsoft.com/en-us/software-download/windows11";
const REFERER: &str = "https://www.microsoft.com/software-download/windows11";
const API: &str = "https://www.microsoft.com/software-download-connector/api";
const PROFILE: &str = "606624d44113";
const ORG_ID: &str = "y6jn8c31";
const INSTANCE_ID: &str = "560dc9f3-1aa5-4a2f-b63c-9e18f8d0e175";
/// `DownloadType` of the x64 ISO in the link list.
const DOWNLOAD_X64: u32 = 1;

/// Microsoft's language name used when no other is chosen.
pub const WINDOWS_LANGUAGE_FALLBACK: &str = "English";

/// Microsoft's language names whose checksum rows are named differently.
const CHECKSUM_ROW_ALIASES: &[(&str, &str)] = &[
    ("English (United Kingdom)", "English International"),
    ("Chinese (Simplified)", "Chinese Simplified"),
    ("Chinese (Traditional)", "Chinese Traditional"),
];

/// Microsoft's language name for a Windows locale such as `de-DE`, if it offers one.
pub fn default_windows_language(locale: &str) -> Option<&'static str> {
    let lower = locale.to_ascii_lowercase();
    let (language, region) = lower.split_once('-').unwrap_or((&lower, ""));
    let name = match (language, region) {
        ("en", "us") => "English",
        ("en", _) => "English (United Kingdom)",
        ("pt", "br") => "Brazilian Portuguese",
        ("pt", _) => "Portuguese",
        ("fr", "ca") => "French Canadian",
        ("fr", _) => "French",
        ("es", "es") => "Spanish",
        ("es", _) => "Spanish (Mexico)",
        ("zh", "tw" | "hk" | "mo") => "Chinese (Traditional)",
        ("zh", _) => "Chinese (Simplified)",
        ("sr", _) => "Serbian Latin",
        ("nb" | "nn" | "no", _) => "Norwegian",
        _ => match language {
            "ar" => "Arabic",
            "bg" => "Bulgarian",
            "hr" => "Croatian",
            "cs" => "Czech",
            "da" => "Danish",
            "nl" => "Dutch",
            "et" => "Estonian",
            "fi" => "Finnish",
            "de" => "German",
            "el" => "Greek",
            "he" => "Hebrew",
            "hu" => "Hungarian",
            "it" => "Italian",
            "ja" => "Japanese",
            "ko" => "Korean",
            "lv" => "Latvian",
            "lt" => "Lithuanian",
            "pl" => "Polish",
            "ro" => "Romanian",
            "ru" => "Russian",
            "sk" => "Slovak",
            "sl" => "Slovenian",
            "sv" => "Swedish",
            "th" => "Thai",
            "tr" => "Turkish",
            "uk" => "Ukrainian",
            _ => return None,
        },
    };
    Some(name)
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Skus {
    skus: Option<Vec<Sku>>,
    validation_container: Option<Validation>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Sku {
    id: String,
    language: String,
    product_display_name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Links {
    product_download_options: Option<Vec<Link>>,
    validation_container: Option<Validation>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Link {
    uri: String,
    download_type: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Validation {
    errors: Vec<ValidationError>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ValidationError {
    value: Option<String>,
}

pub(crate) fn windows11(language: &str) -> Result<Release, Error> {
    let page = http::text(PAGE, &[])?;
    let edition = Regex::new(r#"<option value="(\d+)">Windows 11 \(multi-edition ISO for x64 devices\)</option>"#)
        .expect("valid regex")
        .captures(&page)
        .map(|c| c[1].to_owned())
        .ok_or_else(|| Error::Format("Microsoft's page lists no x64 multi-edition ISO".into()))?;

    let session = session_id();
    whitelist_session(&session)?;
    let skus: Skus = json(&format!(
        "{API}/getskuinformationbyproductedition?profile={PROFILE}&productEditionId={edition}\
         &SKU=undefined&friendlyFileName=undefined&Locale=en-US&sessionID={session}"
    ))?;
    refused(&skus.validation_container)?;
    let skus = skus.skus.unwrap_or_default();
    let mut languages: Vec<String> = skus.iter().map(|s| s.language.clone()).collect();
    languages.sort();
    languages.dedup();
    let sku = skus
        .iter()
        .find(|s| s.language == language)
        .ok_or_else(|| Error::Format(format!("Microsoft offers no {language} Windows 11 ISO")))?;
    let version = sku.product_display_name.rsplit(" - ").next().unwrap_or(&sku.product_display_name).to_owned();

    let links: Links = json_with_referer(&format!(
        "{API}/GetProductDownloadLinksBySku?profile={PROFILE}&productEditionId=undefined&SKU={}\
         &friendlyFileName=undefined&Locale=en-US&sessionID={session}",
        sku.id
    ))?;
    refused(&links.validation_container)?;
    let url = links
        .product_download_options
        .unwrap_or_default()
        .into_iter()
        .find(|l| l.download_type == DOWNLOAD_X64)
        .map(|l| l.uri)
        .ok_or_else(|| Error::MicrosoftRefused("no x64 download link was returned".into()))?;
    let file_name = url.split('?').next().and_then(|p| p.rsplit('/').next()).unwrap_or("Windows11.iso").to_owned();
    let sha256 = checksum(&page, language)
        .ok_or_else(|| Error::Format(format!("Microsoft's page publishes no checksum for {language}")))?;

    Ok(Release {
        entry: "windows11".into(),
        version,
        size: http::content_length(&url),
        file_name,
        url,
        sha256,
        language: Some(language.to_owned()),
        languages,
    })
}

/// The checksum table row "<language> 64-bit" on the download page.
fn checksum(page: &str, language: &str) -> Option<String> {
    let row_name = CHECKSUM_ROW_ALIASES.iter().find(|(from, _)| *from == language).map_or(language, |(_, to)| to);
    let rows = Regex::new(r"<tr><td>([^<]+)</td><td>([0-9A-Fa-f]{64})</td></tr>").expect("valid regex");
    rows.captures_iter(page)
        .find(|c| c[1].trim() == format!("{row_name} 64-bit"))
        .map(|c| c[2].to_ascii_lowercase())
}

/// Microsoft only answers sessions that were "seen" by its fraud-protection endpoints.
fn whitelist_session(session: &str) -> Result<(), Error> {
    http::text(&format!("https://vlscppe.microsoft.com/tags?org_id={ORG_ID}&session_id={session}"), &[])?;
    let script = http::text(
        &format!("https://ov-df.microsoft.com/mdt.js?instanceId={INSTANCE_ID}&PageId=si&session_id={session}"),
        &[],
    )?;
    let w = Regex::new(r"[?&]w=([A-F0-9]+)").expect("valid regex").captures(&script).map(|c| c[1].to_owned());
    let rticks = Regex::new(r#"rticks="\+?(\d+)"#).expect("valid regex").captures(&script).map(|c| c[1].to_owned());
    let (Some(w), Some(rticks)) = (w, rticks) else {
        return Err(Error::MicrosoftRefused("the session check script changed".into()));
    };
    let millis = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
    http::text(
        &format!(
            "https://ov-df.microsoft.com/?session_id={session}&CustomerId={INSTANCE_ID}&PageId=si\
             &w={w}&mdt={millis}&rticks={rticks}"
        ),
        &[],
    )?;
    Ok(())
}

fn refused(validation: &Option<Validation>) -> Result<(), Error> {
    match validation.as_ref().and_then(|v| v.errors.first()) {
        Some(e) => Err(Error::MicrosoftRefused(e.value.clone().unwrap_or_else(|| "unknown reason".into()))),
        None => Ok(()),
    }
}

fn json<T: for<'de> Deserialize<'de>>(url: &str) -> Result<T, Error> {
    serde_json::from_str(&http::text(url, &[])?).map_err(|e| Error::Format(format!("Microsoft API: {e}")))
}

fn json_with_referer<T: for<'de> Deserialize<'de>>(url: &str) -> Result<T, Error> {
    serde_json::from_str(&http::text(url, &[("Referer", REFERER)])?)
        .map_err(|e| Error::Format(format!("Microsoft API: {e}")))
}

/// A random GUID-shaped session id (Microsoft only checks the format).
fn session_id() -> String {
    let seed = format!("{:?}{:?}", SystemTime::now(), std::thread::current().id());
    let h = Sha256::digest(seed.as_bytes());
    let hex: String = h.iter().take(16).map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-4{}-a{}-{}", &hex[..8], &hex[8..12], &hex[13..16], &hex[17..20], &hex[20..32])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_rows_by_language_and_alias() {
        let page = "<tr><td>English International 64-bit</td><td>7E3F373BD3C2321B5D5125DFCF718DFBF1A0ABC50A267118669951890DF98A5D</td></tr>\
                    <tr><td>English 64-bit</td><td>BD4307DF32BC8AF33B39CCECB1174AEB345386630F89A2B86C7A4E36B55EA650</td></tr>\
                    <tr><td>Chinese Simplified 64-bit</td><td>1111111111111111111111111111111111111111111111111111111111111111</td></tr>";
        assert_eq!(checksum(page, "English").unwrap(), "bd4307df32bc8af33b39ccecb1174aeb345386630f89a2b86c7a4e36b55ea650");
        assert!(checksum(page, "English (United Kingdom)").unwrap().starts_with("7e3f"));
        assert!(checksum(page, "Chinese (Simplified)").is_some());
        assert_eq!(checksum(page, "German"), None);
    }

    #[test]
    fn locales_map_to_microsoft_language_names() {
        assert_eq!(default_windows_language("en-US"), Some("English"));
        assert_eq!(default_windows_language("en-GB"), Some("English (United Kingdom)"));
        assert_eq!(default_windows_language("de-DE"), Some("German"));
        assert_eq!(default_windows_language("pt-BR"), Some("Brazilian Portuguese"));
        assert_eq!(default_windows_language("zh-TW"), Some("Chinese (Traditional)"));
        assert_eq!(default_windows_language("xx-YY"), None);
    }

    #[test]
    fn session_ids_are_guid_shaped_and_distinct() {
        let a = session_id();
        assert_eq!(a.len(), 36);
        assert_eq!(a.matches('-').count(), 4);
        std::thread::sleep(std::time::Duration::from_millis(2));
        assert_ne!(a, session_id());
    }
}
