use super::error::Error;
use super::error::Result;

pub struct PolicyParser;

impl Default for PolicyParser {
    fn default() -> Self {
        Self::new()
    }
}

impl PolicyParser {
    pub fn new() -> Self {
        Self
    }

    pub fn parse(&mut self, _policy_identifier: &str, _policy_file_contents: &str) -> Result<()> {
        Err(Error::UnsupportedPlatform(
            "Starlark execpolicy files are not supported on HarmonyOS/OpenHarmony yet because upstream starlark-rust still depends on a rustyline/nix chain that does not compile for OHOS.".to_string(),
        ))
    }

    pub fn build(self) -> super::policy::Policy {
        super::policy::Policy::empty()
    }
}
