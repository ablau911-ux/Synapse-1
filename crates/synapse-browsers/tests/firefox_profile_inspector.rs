//! MY TDD contract for the gecko profile inspector: `profiles.ini` /
//! `installs.ini` default-profile resolution, `extensions.json` install state,
//! the installed worker's code stamp (unpacked dir or XPI) and the
//! `moz-extension://<uuid>` origin from `prefs.js`.
//! must_pass_*: a ready install is found and its origin accepted;
//! must_fail_*: missing, disabled, unsigned or unknown state is reported and
//! every origin is refused (fail closed).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde_json::json;
use synapse_browsers::gecko::{GeckoProfileInspector, GeckoProfileSet, xpi};
use synapse_browsers::{
    BrowserProfileInspector, BrowserRegistry, BrowserRow, OriginRule, sha256_hex_lower,
};

const GECKO_ID: &str = "synapse-bridge@synapse.local";
const UUID: &str = "0b6a3f0e-1c2d-4e5f-8a9b-0c1d2e3f4a5b";
const DEFAULT_RELEASE: &str = "87ieum3w.default-release";
const LEGACY_DEFAULT: &str = "fq57p52c.default";
const WORKER: &str = "const BRIDGE_CODE_STAMP = \"stamp-abc123\";\nconsole.log(1);\n";

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixture(name: &str) -> String {
    std::fs::read_to_string(
        manifest_dir()
            .join("tests")
            .join("fixtures")
            .join("firefox")
            .join(name),
    )
    .unwrap()
    .replace("\r\n", "\n")
}

fn firefox_row() -> BrowserRow {
    let text =
        std::fs::read_to_string(manifest_dir().join("browsers").join("firefox.json")).unwrap();
    BrowserRegistry::from_json_sources(&[("firefox.json", text.as_str())])
        .unwrap()
        .by_id("firefox")
        .unwrap()
        .clone()
}

/// A `%APPDATA%\Mozilla\Firefox` lookalike in a temp dir.
struct FirefoxRoot {
    dir: tempfile::TempDir,
}

impl FirefoxRoot {
    fn new(installs_ini: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("profiles.ini"), fixture("profiles.ini")).unwrap();
        if installs_ini {
            std::fs::write(dir.path().join("installs.ini"), fixture("installs.ini")).unwrap();
        }
        let root = Self { dir };
        root.profile(DEFAULT_RELEASE);
        root.profile(LEGACY_DEFAULT);
        root
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn profile(&self, name: &str) -> PathBuf {
        let dir = self.path().join("Profiles").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn unpacked_extension(&self) -> PathBuf {
        let dir = self.path().join("unpacked-bridge");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("service_worker.js"), WORKER).unwrap();
        dir
    }
}

fn addon(path: &Path, overrides: serde_json::Value) -> serde_json::Value {
    let mut record = json!({
        "id": GECKO_ID,
        "version": "0.1.0",
        "type": "extension",
        "location": "app-profile",
        "active": true,
        "userDisabled": false,
        "appDisabled": false,
        "signedState": 2,
        "path": path.display().to_string(),
    });
    for (key, value) in overrides.as_object().unwrap() {
        record[key] = value.clone();
    }
    record
}

fn write_extensions_json(profile: &Path, addons: &[serde_json::Value]) {
    let other = json!({"id": "other@example.org", "active": true, "path": "C:\\x.xpi"});
    let mut all = vec![other];
    all.extend_from_slice(addons);
    std::fs::write(
        profile.join("extensions.json"),
        json!({"schemaVersion": 37, "addons": all}).to_string(),
    )
    .unwrap();
}

fn write_prefs(profile: &Path, uuid_map: &serde_json::Value) {
    let literal = serde_json::to_string(&uuid_map.to_string()).unwrap();
    let mut file = std::fs::File::create(profile.join("prefs.js")).unwrap();
    writeln!(file, "// Mozilla User Preferences").unwrap();
    writeln!(file, "user_pref(\"browser.startup.page\", 3);").unwrap();
    writeln!(
        file,
        "user_pref(\"extensions.webextensions.uuids\", {literal});"
    )
    .unwrap();
}

/// Minimal zip writer for XPI fixtures (deflated or stored entries).
fn zip_with(entries: &[(&str, &[u8], bool)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data, deflate) in entries {
        let body = if *deflate {
            miniz_oxide::deflate::compress_to_vec(data, 6)
        } else {
            data.to_vec()
        };
        let method: u16 = if *deflate { 8 } else { 0 };
        let offset = u32::try_from(out.len()).unwrap();
        let name_len = u16::try_from(name.len()).unwrap();
        let comp = u32::try_from(body.len()).unwrap();
        let raw = u32::try_from(data.len()).unwrap();
        out.extend_from_slice(&0x0403_4b50_u32.to_le_bytes());
        for value in [20_u16, 0, method, 0, 0] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        for value in [0_u32, comp, raw] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&0_u16.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&body);

        central.extend_from_slice(&0x0201_4b50_u32.to_le_bytes());
        for value in [20_u16, 20, 0, method, 0, 0] {
            central.extend_from_slice(&value.to_le_bytes());
        }
        for value in [0_u32, comp, raw] {
            central.extend_from_slice(&value.to_le_bytes());
        }
        for value in [name_len, 0, 0, 0, 0] {
            central.extend_from_slice(&value.to_le_bytes());
        }
        central.extend_from_slice(&0_u32.to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let central_offset = u32::try_from(out.len()).unwrap();
    let central_len = u32::try_from(central.len()).unwrap();
    let count = u16::try_from(entries.len()).unwrap();
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50_u32.to_le_bytes());
    for value in [0_u16, 0, count, count] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(&central_len.to_le_bytes());
    out.extend_from_slice(&central_offset.to_le_bytes());
    out.extend_from_slice(&0_u16.to_le_bytes());
    out
}

#[test]
fn must_pass_firefox_default_profile_from_installs_ini() {
    let root = FirefoxRoot::new(true);
    let unpacked = root.unpacked_extension();
    for name in [DEFAULT_RELEASE, LEGACY_DEFAULT] {
        write_extensions_json(&root.profile(name), &[addon(&unpacked, json!({}))]);
    }
    let state = GeckoProfileInspector::scan_root(&firefox_row(), root.path());
    assert!(state.scanned);
    assert!(state.installed);
    assert_eq!(
        state.active_profile.as_deref(),
        Some(DEFAULT_RELEASE),
        "installs.ini Default= wins over profiles.ini Default=1"
    );
    assert_eq!(
        state.installed_profiles,
        vec![DEFAULT_RELEASE.to_owned(), LEGACY_DEFAULT.to_owned()]
    );
    assert_eq!(state.user_data_root.as_deref(), Some(root.path()));
}

#[test]
fn must_pass_firefox_profiles_ini_default_flag_without_installs_ini() {
    let set = GeckoProfileSet::parse(Path::new("C:\\FF"), &fixture("profiles.ini"), None);
    let names: Vec<&str> = set.profiles().iter().map(|p| p.name()).collect();
    assert_eq!(names, vec![LEGACY_DEFAULT, DEFAULT_RELEASE]);
    assert_eq!(
        set.default_candidates(),
        &[DEFAULT_RELEASE.to_owned(), LEGACY_DEFAULT.to_owned()],
        "[Install*] Default= first, then the [Profile*] Default=1 profile"
    );
    let without_install_section = fixture("profiles.ini").replace(
        "[Install308046B0AF4A39CB]\nDefault=Profiles/87ieum3w.default-release\nLocked=1\n",
        "",
    );
    let set = GeckoProfileSet::parse(Path::new("C:\\FF"), &without_install_section, None);
    assert_eq!(set.default_candidates(), &[LEGACY_DEFAULT.to_owned()]);
}

#[test]
fn must_pass_firefox_profiles_ini_relative_and_absolute_paths() {
    let ini = "[Profile0]\nName=rel\nIsRelative=1\nPath=Profiles/aaaa.rel\nDefault=1\n\n\
               [Profile1]\nName=abs\nIsRelative=0\nPath=D:\\Elsewhere\\bbbb.abs\n";
    let set = GeckoProfileSet::parse(Path::new("C:\\FF"), ini, None);
    let dirs: Vec<PathBuf> = set
        .profiles()
        .iter()
        .map(|p| p.dir().to_path_buf())
        .collect();
    assert_eq!(
        dirs,
        vec![
            Path::new("C:\\FF").join("Profiles").join("aaaa.rel"),
            PathBuf::from("D:\\Elsewhere\\bbbb.abs"),
        ]
    );
    assert_eq!(set.profiles()[1].name(), "bbbb.abs");
}

#[test]
fn must_pass_firefox_unlisted_profile_dir_with_the_only_install_wins() {
    let root = FirefoxRoot::new(true);
    let unpacked = root.unpacked_extension();
    let grouped = root.profile("uwg4a7Sy.Profile 1");
    write_extensions_json(&grouped, &[addon(&unpacked, json!({}))]);
    let state = GeckoProfileInspector::scan_root(&firefox_row(), root.path());
    assert!(state.installed);
    assert_eq!(
        state.active_profile.as_deref(),
        Some("uwg4a7Sy.Profile 1"),
        "profile-group dirs absent from profiles.ini are still scanned"
    );
}

#[test]
fn must_fail_firefox_profiles_ini_missing_reports_not_scanned() {
    let dir = tempfile::tempdir().unwrap();
    let state = GeckoProfileInspector::scan_root(&firefox_row(), dir.path());
    assert!(!state.scanned);
    assert!(!state.installed);
    assert_eq!(state.reason, "profiles_ini_unreadable");
    assert!(matches!(
        GeckoProfileInspector::origin_for_root(&firefox_row(), dir.path()),
        OriginRule::Unknown { .. }
    ));
}

#[test]
fn must_fail_firefox_extension_absent_everywhere() {
    let root = FirefoxRoot::new(true);
    write_extensions_json(&root.profile(DEFAULT_RELEASE), &[]);
    let state = GeckoProfileInspector::scan_root(&firefox_row(), root.path());
    assert!(state.scanned);
    assert!(!state.installed);
    assert_eq!(state.reason, "extension_id_absent_from_extensions_json");
    assert_eq!(state.active_profile.as_deref(), Some(DEFAULT_RELEASE));
}

#[test]
fn must_pass_firefox_extensions_json_ready_state() {
    let root = FirefoxRoot::new(true);
    let unpacked = root.unpacked_extension();
    write_extensions_json(
        &root.profile(DEFAULT_RELEASE),
        &[addon(&unpacked, json!({}))],
    );
    let state = GeckoProfileInspector::scan_root(&firefox_row(), root.path());
    assert!(state.installed);
    assert_eq!(state.reason, "extension_ready:signed_state=2");
    assert_eq!(state.extension_path.as_deref(), Some(unpacked.as_path()));
    assert_eq!(
        state.service_worker_sha256.as_deref(),
        Some(sha256_hex_lower(WORKER.as_bytes()).as_str())
    );
    assert_eq!(state.code_stamp.as_deref(), Some("stamp-abc123"));
    assert!(
        state
            .detail(&firefox_row())
            .contains("browser_kind=firefox")
    );
}

#[test]
fn must_pass_firefox_code_stamp_and_sha_from_installed_xpi() {
    let root = FirefoxRoot::new(true);
    let profile = root.profile(DEFAULT_RELEASE);
    let extensions = profile.join("extensions");
    std::fs::create_dir_all(&extensions).unwrap();
    let xpi_path = extensions.join(format!("{GECKO_ID}.xpi"));
    std::fs::write(
        &xpi_path,
        zip_with(&[
            ("manifest.json", b"{}", false),
            ("service_worker.js", WORKER.as_bytes(), true),
        ]),
    )
    .unwrap();
    write_extensions_json(&profile, &[addon(&xpi_path, json!({}))]);
    let state = GeckoProfileInspector::scan_root(&firefox_row(), root.path());
    assert!(state.installed);
    assert_eq!(state.extension_path.as_deref(), Some(xpi_path.as_path()));
    assert_eq!(
        state.service_worker_sha256.as_deref(),
        Some(sha256_hex_lower(WORKER.as_bytes()).as_str())
    );
    assert_eq!(state.code_stamp.as_deref(), Some("stamp-abc123"));
}

#[test]
fn must_pass_xpi_reader_stored_and_deflated_entries() {
    let archive = zip_with(&[
        ("a.txt", b"stored", false),
        ("b.txt", b"deflated text", true),
    ]);
    assert_eq!(xpi::read_entry(&archive, "a.txt").unwrap(), b"stored");
    assert_eq!(
        xpi::read_entry(&archive, "b.txt").unwrap(),
        b"deflated text"
    );
}

#[test]
fn must_fail_xpi_reader_missing_entry_or_corrupt_archive() {
    let archive = zip_with(&[("a.txt", b"stored", false)]);
    let missing = xpi::read_entry(&archive, "service_worker.js").unwrap_err();
    assert!(missing.contains("entry_absent"), "{missing}");
    for corrupt in [
        Vec::new(),
        b"not a zip at all".to_vec(),
        archive[..archive.len() - 4].to_vec(),
    ] {
        assert!(xpi::read_entry(&corrupt, "a.txt").is_err());
    }
    let mut lying = archive.clone();
    // Central-directory local-header offset pointing past the archive.
    let central = lying.len() - 22 - (46 + "a.txt".len());
    lying[central + 42..central + 46].copy_from_slice(&0xFFFF_0000_u32.to_le_bytes());
    assert!(xpi::read_entry(&lying, "a.txt").is_err());
}

#[test]
fn must_fail_firefox_app_disabled_unsigned_not_ready() {
    let root = FirefoxRoot::new(true);
    let unpacked = root.unpacked_extension();
    write_extensions_json(
        &root.profile(DEFAULT_RELEASE),
        &[addon(
            &unpacked,
            json!({"active": false, "appDisabled": true, "signedState": 0}),
        )],
    );
    let state = GeckoProfileInspector::scan_root(&firefox_row(), root.path());
    assert!(state.scanned);
    assert!(!state.installed);
    assert_eq!(state.reason, "extension_app_disabled:signed_state=0");
    assert!(matches!(
        GeckoProfileInspector::origin_for_root(&firefox_row(), root.path()),
        OriginRule::Unknown { .. }
    ));
}

#[test]
fn must_fail_firefox_broken_signature_or_user_disabled_not_ready() {
    for (overrides, reason) in [
        (
            json!({"signedState": -2}),
            "extension_signature_broken:signed_state=-2",
        ),
        (
            json!({"userDisabled": true, "active": false}),
            "extension_user_disabled:signed_state=2",
        ),
        (
            json!({"active": false}),
            "extension_inactive:signed_state=2",
        ),
    ] {
        let root = FirefoxRoot::new(true);
        let unpacked = root.unpacked_extension();
        write_extensions_json(
            &root.profile(DEFAULT_RELEASE),
            &[addon(&unpacked, overrides)],
        );
        let state = GeckoProfileInspector::scan_root(&firefox_row(), root.path());
        assert!(!state.installed, "{reason}");
        assert_eq!(state.reason, reason);
    }
}

#[test]
fn must_pass_firefox_unsigned_active_where_signatures_are_not_enforced() {
    let root = FirefoxRoot::new(true);
    let unpacked = root.unpacked_extension();
    write_extensions_json(
        &root.profile(DEFAULT_RELEASE),
        &[addon(&unpacked, json!({"signedState": 0}))],
    );
    let state = GeckoProfileInspector::scan_root(&firefox_row(), root.path());
    assert!(
        state.installed,
        "Firefox itself reports it active (Dev/Nightly/ESR)"
    );
    assert_eq!(
        state.reason,
        "extension_ready_signature_not_enforced:signed_state=0"
    );
}

#[test]
fn must_fail_firefox_extensions_json_corrupt_reports_reason() {
    let root = FirefoxRoot::new(true);
    std::fs::write(
        root.profile(DEFAULT_RELEASE).join("extensions.json"),
        "{ not json",
    )
    .unwrap();
    let state = GeckoProfileInspector::scan_root(&firefox_row(), root.path());
    assert!(state.scanned);
    assert!(!state.installed);
    assert_eq!(state.reason, "active_profile_extensions_json_invalid");
}

#[test]
fn must_pass_firefox_origin_from_prefs_uuid_map() {
    let root = FirefoxRoot::new(true);
    let unpacked = root.unpacked_extension();
    let profile = root.profile(DEFAULT_RELEASE);
    write_extensions_json(&profile, &[addon(&unpacked, json!({}))]);
    write_prefs(
        &profile,
        &json!({"other@example.org": "11111111-2222-4333-8444-555555555555", GECKO_ID: UUID}),
    );
    let rule = GeckoProfileInspector::origin_for_root(&firefox_row(), root.path());
    assert_eq!(rule, OriginRule::Exact(format!("moz-extension://{UUID}")));
    assert!(rule.accepts(&format!("moz-extension://{UUID}/")));
    assert!(!rule.accepts("moz-extension://11111111-2222-4333-8444-555555555555/"));
    assert!(!rule.accepts(&format!("chrome-extension://{UUID}/")));
}

#[test]
fn must_fail_firefox_origin_unknown_uuid_rejected() {
    for uuid_map in [
        json!({"other@example.org": UUID}),
        json!({GECKO_ID: "not-a-uuid"}),
        json!({GECKO_ID: "0B6A3F0E-1C2D-4E5F-8A9B-0C1D2E3F4A5B/../x"}),
        json!({GECKO_ID: 42}),
    ] {
        let root = FirefoxRoot::new(true);
        let unpacked = root.unpacked_extension();
        let profile = root.profile(DEFAULT_RELEASE);
        write_extensions_json(&profile, &[addon(&unpacked, json!({}))]);
        write_prefs(&profile, &uuid_map);
        let rule = GeckoProfileInspector::origin_for_root(&firefox_row(), root.path());
        assert!(matches!(rule, OriginRule::Unknown { .. }), "{uuid_map}");
        assert!(!rule.accepts(&format!("moz-extension://{UUID}/")));
    }
    let root = FirefoxRoot::new(true);
    let unpacked = root.unpacked_extension();
    write_extensions_json(
        &root.profile(DEFAULT_RELEASE),
        &[addon(&unpacked, json!({}))],
    );
    let rule = GeckoProfileInspector::origin_for_root(&firefox_row(), root.path());
    assert_eq!(
        rule,
        OriginRule::Unknown {
            reason: "prefs_js_unreadable".to_owned()
        }
    );
}

#[test]
fn must_pass_gecko_inspector_refuses_non_gecko_rows() {
    let chrome = BrowserRegistry::embedded_baseline()
        .legacy_default()
        .unwrap()
        .clone();
    let state = GeckoProfileInspector.install_state(&chrome);
    assert!(!state.scanned);
    assert_eq!(state.reason, "row_family_not_gecko");
    assert!(matches!(
        GeckoProfileInspector.expected_origin(&chrome),
        OriginRule::Unknown { .. }
    ));
}
