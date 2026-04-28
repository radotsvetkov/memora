use crate::note::Privacy;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrivacyConfig {
    pub default_note_privacy: Privacy,
    pub redact_secret_in_cloud: bool,
    pub warn_on_secret_query: bool,
}

impl Default for PrivacyConfig {
    fn default() -> Self {
        Self {
            default_note_privacy: Privacy::Private,
            redact_secret_in_cloud: true,
            warn_on_secret_query: true,
        }
    }
}
