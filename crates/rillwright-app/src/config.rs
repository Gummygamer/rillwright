use std::{path::PathBuf, time::Duration};

use rillwright_agent::policy::Mode;
use rillwright_engine::{CacheMode, EngineOptions, GenParams, LoadOptions, PrefillSchedule};

#[derive(Clone, Debug)]
pub struct LoadConfig {
    pub model: PathBuf,
    pub loops: u32,
    pub cache_mode: CacheMode,
    pub context: u32,
    pub batch: u32,
    pub micro_batch: u32,
    pub gpu_layers: i32,
    pub threads: i32,
    pub flash_attention: rillwright_engine::model::FlashAttn,
    pub prefix_reuse: bool,
    pub swa_full: bool,
}

impl LoadConfig {
    pub fn engine_options(&self) -> Result<EngineOptions, String> {
        self.engine_options_with_vocab_only(false)
    }

    pub fn engine_options_with_vocab_only(
        &self,
        vocab_only: bool,
    ) -> Result<EngineOptions, String> {
        if !(1..=4).contains(&self.loops) {
            return Err("loops must be between 1 and 4".into());
        }
        if self.context == 0 || self.batch == 0 || self.micro_batch == 0 || self.threads <= 0 {
            return Err("context, batch, micro-batch, and thread counts must be nonzero".into());
        }
        if self.micro_batch > self.batch || self.batch > self.context {
            return Err(
                "micro-batch must not exceed batch, and batch must not exceed context".into(),
            );
        }
        if !self.model.is_file() {
            return Err(format!(
                "model file does not exist: {}",
                self.model.display()
            ));
        }
        let mut options = EngineOptions::new(LoadOptions {
            path: self.model.clone(),
            loops: self.loops,
            cache_mode: self.cache_mode,
            n_gpu_layers: self.gpu_layers,
            vocab_only,
        });
        options.n_ctx = self.context;
        options.n_batch = self.batch;
        options.n_ubatch = self.micro_batch;
        options.threads = self.threads;
        options.prefix_reuse = self.prefix_reuse;
        options.swa_full = self.swa_full;
        options.flash_attn = self.flash_attention;
        Ok(options)
    }
}

impl Default for LoadConfig {
    fn default() -> Self {
        Self {
            model: model_env().map(PathBuf::from).unwrap_or_default(),
            loops: 2,
            cache_mode: CacheMode::LastLoop,
            context: 32768,
            batch: 2048,
            micro_batch: 512,
            gpu_layers: -1,
            threads: 8,
            flash_attention: rillwright_engine::model::FlashAttn::Auto,
            prefix_reuse: false,
            swa_full: false,
        }
    }
}

/// Canonical model setting wins; keep the old name as a compatibility alias.
pub fn model_env() -> Option<std::ffi::OsString> {
    select_model_env(
        std::env::var_os("RILLWRIGHT_MODEL"),
        std::env::var_os("DORY_MODEL"),
    )
}

fn select_model_env(
    canonical: Option<std::ffi::OsString>,
    legacy: Option<std::ffi::OsString>,
) -> Option<std::ffi::OsString> {
    canonical
        .filter(|v| !v.is_empty())
        .or_else(|| legacy.filter(|v| !v.is_empty()))
}

#[derive(Clone, Debug)]
pub struct GenerationConfig {
    pub params: GenParams,
    pub thinking: bool,
}

impl Default for GenerationConfig {
    fn default() -> Self {
        Self {
            params: GenParams::default(),
            thinking: true,
        }
    }
}

impl GenerationConfig {
    pub fn validate(&self) -> Result<(), String> {
        let p = &self.params;
        if !p.temp.is_finite() || p.temp < 0.0 {
            return Err("temperature must be finite and nonnegative".into());
        }
        if !p.top_p.is_finite() || !(0.0..=1.0).contains(&p.top_p) {
            return Err("top-p must be finite and between 0 and 1".into());
        }
        if p.max_tokens == 0 {
            return Err("maximum output tokens must be nonzero".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct AgentSettings {
    pub workspace: PathBuf,
    pub permission: Mode,
    pub max_rounds: usize,
    pub max_duration: Duration,
    pub command_timeout: Duration,
    pub sandbox: String,
    pub allow_commands: Vec<String>,
    pub network: bool,
    pub allow_web: bool,
    pub env_pass: Vec<String>,
    pub writable: Vec<PathBuf>,
    pub web_provider: String,
    pub web_enabled: bool,
    pub web_budget: usize,
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self {
            workspace: PathBuf::new(),
            permission: Mode::Ask,
            max_rounds: 30,
            max_duration: Duration::from_secs(1800),
            command_timeout: Duration::from_secs(120),
            sandbox: "auto".into(),
            allow_commands: Vec::new(),
            network: false,
            allow_web: false,
            env_pass: Vec::new(),
            writable: Vec::new(),
            web_provider: "tavily".into(),
            web_enabled: false,
            web_budget: 3,
        }
    }
}

#[derive(Clone, Debug)]
pub struct WebSettings {
    pub provider: Option<String>,
    pub enabled: bool,
    pub search_budget: usize,
    pub allow_without_approval: bool,
}

impl Default for WebSettings {
    fn default() -> Self {
        Self {
            provider: None,
            enabled: false,
            search_budget: 3,
            allow_without_approval: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_defaults_match_the_cli_generation_and_agent_defaults() {
        let generation = GenerationConfig::default();
        assert!(generation.validate().is_ok());
        assert_eq!(generation.params.seed, 42);
        assert_eq!(generation.params.temp, 1.0);
        assert_eq!(generation.params.top_p, 0.95);
        assert_eq!(generation.params.max_tokens, 8192);
        let agent = AgentSettings::default();
        assert_eq!(agent.permission.as_str(), "ask");
        assert_eq!(agent.max_rounds, 30);
        assert_eq!(agent.max_duration, Duration::from_secs(1800));
        assert_eq!(agent.command_timeout, Duration::from_secs(120));
    }

    #[test]
    fn canonical_model_environment_wins_and_empty_values_fall_back() {
        use std::ffi::OsString;
        assert_eq!(
            select_model_env(Some("new.gguf".into()), Some("old.gguf".into())),
            Some(OsString::from("new.gguf"))
        );
        assert_eq!(
            select_model_env(Some(OsString::new()), Some("old.gguf".into())),
            Some(OsString::from("old.gguf"))
        );
        assert_eq!(select_model_env(None, Some(OsString::new())), None);
    }

    #[test]
    fn invalid_generation_numbers_are_rejected() {
        let mut generation = GenerationConfig::default();
        generation.params.temp = f32::NAN;
        assert!(generation.validate().is_err());
        generation.params.temp = 1.0;
        generation.params.top_p = 1.1;
        assert!(generation.validate().is_err());
        generation.params.top_p = 0.95;
        generation.params.max_tokens = 0;
        assert!(generation.validate().is_err());
    }

    #[test]
    fn load_options_validate_batch_context_and_path() {
        let dir = tempfile::tempdir().unwrap();
        let model = dir.path().join("model.gguf");
        std::fs::write(&model, b"model").unwrap();
        let mut load = LoadConfig::default();
        load.model = model;
        assert!(load.engine_options().is_ok());
        load.micro_batch = load.batch + 1;
        assert!(load.engine_options().is_err());
        load.micro_batch = 512;
        load.context = 0;
        assert!(load.engine_options().is_err());
    }
}

#[allow(dead_code)]
fn _prefill_default_is_stable() -> PrefillSchedule {
    PrefillSchedule::default()
}
