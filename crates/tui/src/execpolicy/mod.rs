#![allow(dead_code)]
#![allow(unused_imports)]

pub mod decision;
pub mod error;
pub mod execpolicycheck;
pub mod matcher;
#[cfg(not(target_env = "ohos"))]
pub mod parser;
#[cfg(target_env = "ohos")]
pub mod parser_ohos;
pub mod policy;
pub mod rule;
pub mod rules;

pub use decision::Decision;
pub use error::Error;
pub use error::Result;
pub use execpolicycheck::ExecPolicyCheckCommand;
#[cfg(not(target_env = "ohos"))]
pub use parser::PolicyParser;
#[cfg(target_env = "ohos")]
pub use parser_ohos::PolicyParser;
pub use policy::Evaluation;
pub use policy::Policy;
pub use rule::Rule;
pub use rule::RuleMatch;
pub use rule::RuleRef;
pub use rules::{
    ExecPolicyConfig, ExecPolicyDecision, default_execpolicy_path, load_default_policy,
};
