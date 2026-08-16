//! Explicit local import bridge for Kimi-managed plugins.
//!
//! Listing is read-only and only considers immediate, canonical child
//! directories of `~/.kimi-code/plugins/managed`. Import requires the exact
//! content hash shown by listing, then routes the local directory through the
//! ordinary reviewed installer. The resulting Codewhale plugin still starts
//! disabled and untrusted; this module never launches or probes an external
//! Kimi application, daemon, MCP binary, or permission grant.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use super::render::{escape_review_path, escape_review_text};
use crate::commands::CommandResult;
use crate::localization::{Locale, MessageId, tr};
use crate::plugins::agent_plugin::KIMI_PLUGIN_JSON_NAME;
use crate::plugins::manifest::PluginManifest;
use crate::plugins::metadata_is_link_or_reparse;
use crate::tui::app::App;

const MAX_MANAGED_CHILDREN: usize = 128;
const LIST_COMMAND: &str = "/plugin import kimi [list]";
const APPROVE_COMMAND: &str = "/plugin import kimi approve <name> <content-hash>";

#[derive(Debug)]
struct Candidate {
    name: String,
    version: String,
    license: Option<String>,
    canonical_path: PathBuf,
    content_hash: String,
    capability_hash: String,
    inventory: String,
    applicable: bool,
}

#[derive(Debug)]
struct Scan {
    root: PathBuf,
    candidates: Vec<Candidate>,
    rejected: Vec<String>,
}

fn message(locale: Locale, id: MessageId, replacements: &[(&str, &str)]) -> String {
    let mut rendered = tr(locale, id).into_owned();
    for (placeholder, value) in replacements {
        rendered = rendered.replace(placeholder, value);
    }
    rendered
}

pub(super) fn usage(locale: Locale) -> String {
    message(
        locale,
        MessageId::PluginKimiUsage,
        &[
            ("{list_command}", LIST_COMMAND),
            ("{approve_command}", APPROVE_COMMAND),
        ],
    )
}

pub(super) fn dispatch(
    app: &mut App,
    words: &[&str],
    home_override: Option<&Path>,
) -> CommandResult {
    match words {
        [] | ["list"] => list(app.ui_locale, home_override),
        ["approve", name, content_hash] => approve(app, name, content_hash, home_override),
        _ => CommandResult::error(usage(app.ui_locale)),
    }
}

fn list(locale: Locale, home_override: Option<&Path>) -> CommandResult {
    let scan = match scan_managed_plugins(locale, home_override) {
        Ok(scan) => scan,
        Err(error) => return CommandResult::error(error),
    };
    let root = escape_review_path(&scan.root);
    let mut output = message(
        locale,
        MessageId::PluginKimiManagedRootHeading,
        &[("{root}", &root)],
    );
    output.push('\n');
    if scan.candidates.is_empty() {
        output.push_str("  ");
        output.push_str(&tr(locale, MessageId::PluginKimiNoneFound));
        output.push('\n');
    }
    for candidate in &scan.candidates {
        let name = escape_review_text(&candidate.name);
        let version = escape_review_text(&candidate.version);
        let license = candidate
            .license
            .as_deref()
            .map(escape_review_text)
            .unwrap_or_else(|| tr(locale, MessageId::PluginKimiLicenseUnspecified).into_owned());
        let applicability = tr(
            locale,
            if candidate.applicable {
                MessageId::PluginKimiApplicable
            } else {
                MessageId::PluginKimiNotApplicable
            },
        );
        let inventory = escape_review_text(&candidate.inventory);
        let summary = message(
            locale,
            MessageId::PluginKimiCandidateSummary,
            &[
                ("{name}", &name),
                ("{version}", &version),
                ("{license}", &license),
                ("{applicability}", &applicability),
                ("{inventory}", &inventory),
            ],
        );
        let _ = writeln!(output, "\n{summary}");

        let path = escape_review_path(&candidate.canonical_path);
        let approve_command = format!(
            "/plugin import kimi approve {} {}",
            candidate.name, candidate.content_hash
        );
        let details = message(
            locale,
            MessageId::PluginKimiCandidateDetails,
            &[
                ("{path}", &path),
                ("{content_hash}", &candidate.content_hash),
                ("{capability_hash}", &candidate.capability_hash),
                ("{approve_command}", &approve_command),
            ],
        );
        let _ = writeln!(output, "{details}");
    }
    if !scan.rejected.is_empty() {
        output.push('\n');
        output.push_str(&tr(locale, MessageId::PluginKimiRejectedHeading));
        output.push('\n');
        for rejection in &scan.rejected {
            let _ = writeln!(output, "  - {rejection}");
        }
    }
    output.push('\n');
    output.push_str(&tr(locale, MessageId::PluginKimiInspectionFooter));
    CommandResult::message(output)
}

fn approve(
    app: &mut App,
    name: &str,
    expected_hash: &str,
    home_override: Option<&Path>,
) -> CommandResult {
    let scan = match scan_managed_plugins(app.ui_locale, home_override) {
        Ok(scan) => scan,
        Err(error) => return CommandResult::error(error),
    };
    let Some(candidate) = scan
        .candidates
        .into_iter()
        .find(|candidate| candidate.name == name)
    else {
        let name = escape_review_text(name);
        return CommandResult::error(message(
            app.ui_locale,
            MessageId::PluginKimiCandidateMissing,
            &[("{name}", &name), ("{list_command}", "/plugin import kimi")],
        ));
    };
    if candidate.content_hash != expected_hash {
        let name = escape_review_text(name);
        let expected = escape_review_text(expected_hash);
        return CommandResult::error(message(
            app.ui_locale,
            MessageId::PluginKimiCandidateChanged,
            &[
                ("{name}", &name),
                ("{expected}", &expected),
                ("{actual}", &candidate.content_hash),
                ("{list_command}", "/plugin import kimi"),
            ],
        ));
    }

    // `install_bundle` revalidates and copies the source through the ordinary
    // local installer. Its result is always rediscovered disabled/untrusted
    // and presents the post-copy authority review before any activation.
    super::install_bundle_with_expected_hash(app, &candidate.canonical_path, expected_hash)
}

fn scan_managed_plugins(locale: Locale, home_override: Option<&Path>) -> Result<Scan, String> {
    let home = match home_override {
        Some(home) => home.to_path_buf(),
        None => crate::config::effective_home_dir()
            .ok_or_else(|| tr(locale, MessageId::PluginKimiHomeMissing).into_owned())?,
    };
    let configured_root = home.join(".kimi-code/plugins/managed");
    let metadata = match fs::symlink_metadata(&configured_root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Scan {
                root: configured_root,
                candidates: Vec::new(),
                rejected: Vec::new(),
            });
        }
        Err(error) => {
            let root = escape_review_path(&configured_root);
            let error = escape_review_text(&error.to_string());
            return Err(message(
                locale,
                MessageId::PluginKimiRootInspectFailed,
                &[("{root}", &root), ("{error}", &error)],
            ));
        }
    };
    if metadata_is_link_or_reparse(&metadata) || !metadata.is_dir() {
        let root = escape_review_path(&configured_root);
        return Err(message(
            locale,
            MessageId::PluginKimiRootMustBeDirectory,
            &[("{root}", &root)],
        ));
    }
    let canonical_root = configured_root.canonicalize().map_err(|error| {
        let root = escape_review_path(&configured_root);
        let error = escape_review_text(&error.to_string());
        message(
            locale,
            MessageId::PluginKimiRootCanonicalizeFailed,
            &[("{root}", &root), ("{error}", &error)],
        )
    })?;
    let mut entries = fs::read_dir(&canonical_root)
        .map_err(|error| {
            let root = escape_review_path(&canonical_root);
            let error = escape_review_text(&error.to_string());
            message(
                locale,
                MessageId::PluginKimiRootListFailed,
                &[("{root}", &root), ("{error}", &error)],
            )
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            let error = escape_review_text(&error.to_string());
            message(
                locale,
                MessageId::PluginKimiEntryReadFailed,
                &[("{error}", &error)],
            )
        })?;
    if entries.len() > MAX_MANAGED_CHILDREN {
        let count = entries.len().to_string();
        let max = MAX_MANAGED_CHILDREN.to_string();
        return Err(message(
            locale,
            MessageId::PluginKimiEntryLimit,
            &[("{count}", &count), ("{max}", &max)],
        ));
    }
    entries.sort_by_key(fs::DirEntry::file_name);

    let mut candidates = Vec::new();
    let mut rejected = Vec::new();
    for entry in entries {
        let path = entry.path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                let path = escape_review_path(&path);
                let error = escape_review_text(&error.to_string());
                rejected.push(message(
                    locale,
                    MessageId::PluginKimiEntryInspectFailed,
                    &[("{path}", &path), ("{error}", &error)],
                ));
                continue;
            }
        };
        if metadata_is_link_or_reparse(&metadata) {
            let path = escape_review_path(&path);
            rejected.push(message(
                locale,
                MessageId::PluginKimiEntryLinksRefused,
                &[("{path}", &path)],
            ));
            continue;
        }
        if !metadata.is_dir() {
            continue;
        }
        let canonical_path = match path.canonicalize() {
            Ok(path) if path.parent() == Some(canonical_root.as_path()) => path,
            Ok(canonical_path) => {
                let path = escape_review_path(&entry.path());
                let canonical_path = escape_review_path(&canonical_path);
                rejected.push(message(
                    locale,
                    MessageId::PluginKimiEntryOutsideRoot,
                    &[("{path}", &path), ("{canonical_path}", &canonical_path)],
                ));
                continue;
            }
            Err(error) => {
                let path = escape_review_path(&path);
                let error = escape_review_text(&error.to_string());
                rejected.push(message(
                    locale,
                    MessageId::PluginKimiEntryCanonicalizeFailed,
                    &[("{path}", &path), ("{error}", &error)],
                ));
                continue;
            }
        };
        match inspect_candidate(locale, &canonical_path) {
            Ok(candidate) => candidates.push(candidate),
            Err(error) => rejected.push(error),
        }
    }
    candidates.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(Scan {
        root: canonical_root,
        candidates,
        rejected,
    })
}

fn inspect_candidate(locale: Locale, canonical_path: &Path) -> Result<Candidate, String> {
    let manifest_path = canonical_path.join(KIMI_PLUGIN_JSON_NAME);
    let metadata = fs::symlink_metadata(&manifest_path).map_err(|error| {
        let path = escape_review_path(canonical_path);
        let error = escape_review_text(&error.to_string());
        message(
            locale,
            MessageId::PluginKimiManifestUnreadable,
            &[
                ("{path}", &path),
                ("{manifest}", KIMI_PLUGIN_JSON_NAME),
                ("{error}", &error),
            ],
        )
    })?;
    if metadata_is_link_or_reparse(&metadata) || !metadata.is_file() {
        let path = escape_review_path(canonical_path);
        return Err(message(
            locale,
            MessageId::PluginKimiManifestMustBeFile,
            &[("{path}", &path), ("{manifest}", KIMI_PLUGIN_JSON_NAME)],
        ));
    }
    let validated = PluginManifest::validate_from_path(&manifest_path).map_err(|error| {
        let path = escape_review_path(canonical_path);
        let error = escape_review_text(&error.to_string());
        message(
            locale,
            MessageId::PluginKimiManifestInvalid,
            &[("{path}", &path), ("{error}", &error)],
        )
    })?;
    let name = validated.manifest.plugin.name.clone();
    if canonical_path.file_name().and_then(|part| part.to_str()) != Some(name.as_str()) {
        let path = escape_review_path(canonical_path);
        let escaped_name = escape_review_text(&name);
        return Err(message(
            locale,
            MessageId::PluginKimiDirectoryNameMismatch,
            &[("{path}", &path), ("{name}", &escaped_name)],
        ));
    }
    Ok(Candidate {
        name,
        version: validated.manifest.plugin.version.clone(),
        license: validated.manifest.plugin.license.clone(),
        canonical_path: validated.canonical_root,
        content_hash: validated.content_hash,
        capability_hash: validated.capability_hash,
        inventory: validated.inventory.summary(),
        applicable: validated.applicable,
    })
}
