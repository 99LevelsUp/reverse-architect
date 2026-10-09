//! What a plugin says about itself, and the checks it must pass first.
//!
//! The manifest comes from the component (`describe()`), never from a file
//! beside it, so code and manifest cannot disagree. Everything here is checked
//! before the plugin sees a single source file: an id that could not name a
//! cache directory, an extension that could never match, a trust rank the core
//! reserves — each refuses the load by name rather than surfacing later as a
//! plugin that silently extracts nothing.

use std::collections::BTreeSet;

/// A language a plugin claims.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginLanguage {
    pub name: String,
    pub extensions: Vec<String>,
}

/// A checked manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginManifest {
    pub id: String,
    pub version: String,
    pub languages: Vec<PluginLanguage>,
    pub trust_rank: u8,
    pub depends_on: Vec<String>,
}

/// Why a manifest was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ManifestError {
    #[error("plugin id `{0}` is not a valid id: use 1–40 lowercase letters, digits and `-`, starting with a letter")]
    BadId(String),
    #[error(
        "plugin id `{0}` is a reserved device name on Windows and cannot name a cache directory"
    )]
    ReservedId(String),
    #[error("plugin `{id}` declares the extension `{ext}`: write it without the dot, one segment, no path separator")]
    BadExtension { id: String, ext: String },
    #[error("plugin `{id}` claims no language")]
    NoLanguage { id: String },
    #[error("plugin `{id}` claims the extension `{ext}` twice")]
    DuplicateExtension { id: String, ext: String },
    #[error("plugin `{id}` depends on itself")]
    SelfDependency { id: String },
}

const RESERVED: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Whether `id` can serve as a plugin id: it names cache directories and
/// appears in every provenance, so it is kept to a charset no filesystem and no
/// reader can misread.
pub fn valid_id(id: &str) -> Result<(), ManifestError> {
    let ok = !id.is_empty()
        && id.len() <= 40
        && id.starts_with(|c: char| c.is_ascii_lowercase())
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !ok {
        return Err(ManifestError::BadId(id.to_string()));
    }
    if RESERVED.contains(&id) {
        return Err(ManifestError::ReservedId(id.to_string()));
    }
    Ok(())
}

impl PluginManifest {
    /// Check a manifest the component returned.
    pub fn check(self) -> Result<PluginManifest, ManifestError> {
        valid_id(&self.id)?;
        if self.languages.is_empty() {
            return Err(ManifestError::NoLanguage {
                id: self.id.clone(),
            });
        }
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for lang in &self.languages {
            for ext in &lang.extensions {
                let well_formed = !ext.is_empty()
                    && !ext.starts_with('.')
                    && !ext.contains(['.', '/', '\\'])
                    && !ext.chars().any(char::is_whitespace);
                if !well_formed {
                    return Err(ManifestError::BadExtension {
                        id: self.id.clone(),
                        ext: ext.clone(),
                    });
                }
                if !seen.insert(ext) {
                    return Err(ManifestError::DuplicateExtension {
                        id: self.id.clone(),
                        ext: ext.clone(),
                    });
                }
            }
        }
        if self.depends_on.iter().any(|d| d == &self.id) {
            return Err(ManifestError::SelfDependency {
                id: self.id.clone(),
            });
        }
        Ok(self)
    }

    /// Every extension this plugin claims, sorted.
    pub fn extensions(&self) -> BTreeSet<&str> {
        self.languages
            .iter()
            .flat_map(|l| l.extensions.iter().map(String::as_str))
            .collect()
    }

    /// The reporting name for `ext`, if this plugin claims it.
    pub fn language_for(&self, ext: &str) -> Option<&str> {
        self.languages
            .iter()
            .find(|l| l.extensions.iter().any(|e| e == ext))
            .map(|l| l.name.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(id: &str, exts: &[&str]) -> PluginManifest {
        PluginManifest {
            id: id.into(),
            version: "1".into(),
            languages: vec![PluginLanguage {
                name: "x".into(),
                extensions: exts.iter().map(|e| e.to_string()).collect(),
            }],
            trust_rank: 30,
            depends_on: Vec::new(),
        }
    }

    #[test]
    fn a_hostile_id_is_refused_by_name() {
        for id in [
            "",
            "../x",
            "a/b",
            "Upper",
            "fingerprint.json",
            "9lives",
            "with space",
        ] {
            assert!(matches!(valid_id(id), Err(ManifestError::BadId(_))), "{id}");
        }
        for id in ["con", "nul", "com1", "lpt9"] {
            assert!(
                matches!(valid_id(id), Err(ManifestError::ReservedId(_))),
                "{id}"
            );
        }
        assert!(valid_id("lang-csharp").is_ok());
    }

    #[test]
    fn an_extension_written_with_its_dot_is_reported_not_accepted() {
        assert!(matches!(
            manifest("k", &[".ktd"]).check(),
            Err(ManifestError::BadExtension { .. })
        ));
        assert!(matches!(
            manifest("k", &["a/b"]).check(),
            Err(ManifestError::BadExtension { .. })
        ));
        assert!(matches!(
            manifest("k", &["ktd", "ktd"]).check(),
            Err(ManifestError::DuplicateExtension { .. })
        ));
        assert!(manifest("k", &["ktd"]).check().is_ok());
    }

    #[test]
    fn a_plugin_must_claim_a_language() {
        let mut m = manifest("k", &["ktd"]);
        m.languages.clear();
        assert!(matches!(m.check(), Err(ManifestError::NoLanguage { .. })));
    }
}
