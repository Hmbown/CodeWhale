use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::Mutex;

pub use super::SkillReadiness;

/// A registry of named readiness probes. Owned by the skill system;
/// tests can create a fresh instance for isolation.
pub struct ProbeRegistry {
    probes: HashMap<String, Box<dyn ReadinessProbe>>,
}

impl ProbeRegistry {
    pub fn new() -> Self {
        Self { probes: HashMap::new() }
    }
    pub fn register(&mut self, name: &str, probe: Box<dyn ReadinessProbe>) {
        self.probes.insert(name.to_string(), probe);
    }
    pub fn probe(&self, name: &str) -> Option<SkillReadiness> {
        self.probes.get(name).map(|p| p.probe())
    }
    pub fn query_tools(&self, name: &str) -> Vec<String> {
        self.probes.get(name).map(|p| p.required_tools()).unwrap_or_default()
    }
}

pub trait ReadinessProbe: Send + Sync {
    fn probe(&self) -> SkillReadiness;
    fn required_tools(&self) -> Vec<String> {
        vec![]
    }
}

// 全局注册表：skill 名 → probe
static GLOBAL_PROBES: LazyLock<Mutex<ProbeRegistry>> =
    LazyLock::new(|| Mutex::new(ProbeRegistry::new()));

pub fn register(name: &str, probe: Box<dyn ReadinessProbe>) {
    GLOBAL_PROBES.lock().unwrap().register(name, probe);
}

pub fn probe(name: &str) -> Option<SkillReadiness> {
    GLOBAL_PROBES.lock().unwrap().probe(name)
}

pub fn query_tools(name: &str) -> Vec<String> {
    GLOBAL_PROBES.lock().unwrap().query_tools(name)
}

// ── Built-in probe helpers ─────────────────────────────────────

pub fn has_python() -> bool {
    std::process::Command::new("python3")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn has_python_pptx() -> bool {
    std::process::Command::new("python3")
        .args(["-c", "import pptx"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn has_openpyxl() -> bool {
    std::process::Command::new("python3")
        .args(["-c", "import openpyxl"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// ── Test-only probe: always fails ─────────────────────────────

/// A `ReadinessProbe` that always reports `NeedsSetup`.
/// Used to verify that the TUI and `doctor --json` surface
/// unavailability correctly.
pub struct NeverReadyProbe;
impl ReadinessProbe for NeverReadyProbe {
    fn probe(&self) -> SkillReadiness {
        SkillReadiness::NeedsSetup
    }
    fn required_tools(&self) -> Vec<String> {
        vec!["python3".into(), "node".into(), "libreoffice".into()]
    }
}

/// A `ReadinessProbe` that returns a canned result.
/// Use in tests to avoid executing real OS commands.
pub struct MockProbe {
    result: SkillReadiness,
    tools: Vec<String>,
}

impl MockProbe {
    pub fn new(result: SkillReadiness) -> Self {
        Self { result, tools: vec![] }
    }
    pub fn with_tools(mut self, tools: Vec<String>) -> Self {
        self.tools = tools;
        self
    }
}

impl ReadinessProbe for MockProbe {
    fn probe(&self) -> SkillReadiness { self.result }
    fn required_tools(&self) -> Vec<String> { self.tools.clone() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_ready_probe_returns_needs_setup() {
        let probe = NeverReadyProbe;
        assert_eq!(probe.probe(), SkillReadiness::NeedsSetup);
    }

    #[test]
    fn never_ready_probe_reports_three_missing_tools() {
        let probe = NeverReadyProbe;
        assert_eq!(probe.required_tools().len(), 3);
    }

    #[test]
    fn registered_probe_can_be_queried() {
        let mut registry = ProbeRegistry::new();
        registry.register("test-query", Box::new(NeverReadyProbe));
        assert_eq!(registry.probe("test-query"), Some(SkillReadiness::NeedsSetup));
    }

    #[test]
    fn mock_probe_returns_canned_result() {
        let probe = MockProbe::new(SkillReadiness::Ready);
        assert_eq!(probe.probe(), SkillReadiness::Ready);
    }

    #[test]
    fn probe_registry_isolation() {
        let mut r1 = ProbeRegistry::new();
        r1.register("a", Box::new(MockProbe::new(SkillReadiness::Ready)));
        let r2 = ProbeRegistry::new();
        assert_eq!(r2.probe("a"), None);
    }
}
