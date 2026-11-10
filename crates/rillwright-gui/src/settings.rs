use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use rillwright_agent::policy::Mode;
use rillwright_app::config::{AgentSettings, GenerationConfig, LoadConfig};
use rillwright_engine::{CacheMode, GenParams, PrefillSchedule, model::FlashAttn};
use serde::{Deserialize, Serialize};

const VERSION: u32 = 1;

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UserSettings {
    version: u32,
    pub model_path: String,
    #[serde(default)]
    pub workspace_path: String,
    pub loops: u32,
    pub cache_mode: String,
    pub flash_attention: String,
    pub prefix_reuse: bool,
    pub swa_full: bool,
    pub context: u32,
    pub batch: u32,
    pub micro_batch: u32,
    pub gpu_layers: i32,
    pub threads: i32,
    pub seed: u32,
    pub temperature: f32,
    pub top_p: f32,
    pub max_tokens: u32,
    pub thinking: bool,
    pub max_rounds: usize,
    pub max_seconds: u64,
    pub command_timeout: u64,
    pub permission: String,
    pub sandbox: String,
    pub network: bool,
    pub writable_dirs: String,
    pub env_names: String,
    pub allow_commands: String,
    pub show_reasoning: bool,
    pub font_scale: f32,
    pub theme: String,
    pub web_provider: String,
    pub web_enabled: bool,
    pub web_budget: usize,
    pub allow_web: bool,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            version: VERSION,
            model_path: String::new(),
            workspace_path: String::new(),
            loops: 2,
            cache_mode: "last_loop".into(),
            flash_attention: "auto".into(),
            prefix_reuse: false,
            swa_full: false,
            context: 32768,
            batch: 2048,
            micro_batch: 512,
            gpu_layers: -1,
            threads: 8,
            seed: 42,
            temperature: 1.0,
            top_p: 0.95,
            max_tokens: 8192,
            thinking: true,
            max_rounds: 30,
            max_seconds: 1800,
            command_timeout: 120,
            permission: "ask".into(),
            sandbox: "auto".into(),
            network: false,
            writable_dirs: String::new(),
            env_names: String::new(),
            allow_commands: String::new(),
            show_reasoning: true,
            font_scale: 1.0,
            theme: "system".into(),
            web_provider: "tavily".into(),
            web_enabled: false,
            web_budget: 3,
            allow_web: false,
        }
    }
}

impl UserSettings {
    pub fn load_with_warning() -> (Self, Option<String>) {
        let (canonical, legacy) = config_paths();
        Self::load_from_paths(&canonical, &legacy)
    }

    fn load_from_paths(canonical: &Path, legacy: &Path) -> (Self, Option<String>) {
        let migrating = !canonical.exists() && legacy.exists();
        let path = if migrating { legacy } else { canonical };
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !migrating => {
                return (Self::default(), None);
            }
            Err(e) => {
                return (
                    Self::default(),
                    Some(format!("Could not read settings {}: {e}", path.display())),
                );
            }
        };
        let Ok(mut settings) = serde_json::from_slice::<Self>(&bytes) else {
            return (
                Self::default(),
                Some(format!(
                    "Settings file is malformed; using defaults: {}",
                    path.display()
                )),
            );
        };
        if settings.version != VERSION {
            return (
                Self::default(),
                Some("Settings version is unsupported; using defaults".into()),
            );
        }
        settings.version = VERSION;
        let warning = if migrating {
            match settings.save_to(canonical) {
                Ok(()) => None,
                Err(e) => Some(format!(
                    "Loaded legacy settings, but could not migrate them: {e}"
                )),
            }
        } else {
            None
        };
        (settings, warning)
    }

    pub fn save(&self) -> Result<(), String> {
        let path = config_paths().0;
        self.save_to(&path)
    }

    fn save_to(&self, path: &Path) -> Result<(), String> {
        let parent = path.parent().ok_or("settings directory has no parent")?;
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let tmp = parent.join(format!(".settings-{}.tmp", std::process::id()));
        let mut bytes = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
        fs::rename(&tmp, &path).map_err(|e| {
            let _ = fs::remove_file(&tmp);
            e.to_string()
        })
    }

    pub fn load_config(&self, path: PathBuf) -> LoadConfig {
        let mut c = LoadConfig::default();
        c.model = path;
        c.loops = self.loops;
        c.cache_mode = match self.cache_mode.as_str() {
            "per_loop" => CacheMode::PerLoop,
            _ => CacheMode::LastLoop,
        };
        c.flash_attention = match self.flash_attention.as_str() {
            "on" => FlashAttn::On,
            "off" => FlashAttn::Off,
            _ => FlashAttn::Auto,
        };
        c.prefix_reuse = self.prefix_reuse;
        c.swa_full = self.swa_full;
        c.context = self.context;
        c.batch = self.batch;
        c.micro_batch = self.micro_batch;
        c.gpu_layers = self.gpu_layers;
        c.threads = self.threads;
        c
    }

    pub fn generation_config(&self) -> Result<GenerationConfig, String> {
        let prefill: PrefillSchedule = "server".parse().map_err(|e: String| e)?;
        let c = GenerationConfig {
            thinking: self.thinking,
            params: GenParams {
                seed: self.seed,
                temp: self.temperature,
                top_p: self.top_p,
                max_tokens: self.max_tokens,
                prefill,
                ..GenParams::default()
            },
        };
        c.validate()?;
        Ok(c)
    }

    pub fn agent_settings(&self, workspace: PathBuf) -> Result<AgentSettings, String> {
        let permission = self.permission.parse::<Mode>().map_err(|e| e.to_string())?;
        if !matches!(self.sandbox.as_str(), "auto" | "bwrap" | "none") {
            return Err("sandbox must be auto, bwrap, or none".into());
        }
        if self.max_rounds == 0 || self.max_seconds == 0 || self.command_timeout == 0 {
            return Err("agent round and time limits must be nonzero".into());
        }
        if !matches!(self.web_provider.as_str(), "tavily" | "brave")
            || !(1..=20).contains(&self.web_budget)
        {
            return Err("web provider or search budget is invalid".into());
        }
        for path in paths(&self.writable_dirs) {
            if path.as_os_str().is_empty() {
                return Err("writable directory entries cannot be empty".into());
            }
            if path.exists() && !path.is_dir() {
                return Err(format!(
                    "writable path is not a directory: {}",
                    path.display()
                ));
            }
        }
        Ok(AgentSettings {
            workspace,
            permission,
            max_rounds: self.max_rounds,
            max_duration: Duration::from_secs(self.max_seconds),
            command_timeout: Duration::from_secs(self.command_timeout),
            sandbox: self.sandbox.clone(),
            allow_commands: words(&self.allow_commands),
            network: self.network,
            allow_web: self.allow_web,
            env_pass: words(&self.env_names),
            writable: paths(&self.writable_dirs),
            web_provider: self.web_provider.clone(),
            web_enabled: self.web_enabled,
            web_budget: self.web_budget,
        })
    }
}

fn words(s: &str) -> Vec<String> {
    s.lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}
fn paths(s: &str) -> Vec<PathBuf> {
    words(s).into_iter().map(PathBuf::from).collect()
}

fn config_paths() -> (PathBuf, PathBuf) {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(std::env::temp_dir);
    (
        base.join("rillwright").join("settings.json"),
        base.join("dory-harness").join("settings.json"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_without_secret_values() {
        let settings = UserSettings::default();
        let bytes = serde_json::to_vec(&settings).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["permission"], "ask");
        assert_eq!(value["max_seconds"], 1800);
        assert!(value.get("api_key").is_none());
        assert!(value.get("TAVILY_API_KEY").is_none());
        assert!(value.get("BRAVE_SEARCH_API_KEY").is_none());
    }

    #[test]
    fn a_valid_legacy_settings_file_is_copied_to_the_canonical_path() {
        let temp = tempfile::tempdir().unwrap();
        let canonical = temp.path().join("new/settings.json");
        let legacy = temp.path().join("old/settings.json");
        fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        let mut expected = UserSettings::default();
        expected.model_path = "saved.gguf".into();
        fs::write(&legacy, serde_json::to_vec(&expected).unwrap()).unwrap();

        let (actual, warning) = UserSettings::load_from_paths(&canonical, &legacy);
        assert_eq!(actual.model_path, "saved.gguf");
        assert!(warning.is_none());
        assert!(canonical.exists());
        assert!(legacy.exists());
    }

    #[test]
    fn malformed_canonical_settings_do_not_fall_back_to_legacy() {
        let temp = tempfile::tempdir().unwrap();
        let canonical = temp.path().join("new/settings.json");
        let legacy = temp.path().join("old/settings.json");
        fs::create_dir_all(canonical.parent().unwrap()).unwrap();
        fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        fs::write(&canonical, b"not json").unwrap();
        fs::write(
            &legacy,
            serde_json::to_vec(&UserSettings::default()).unwrap(),
        )
        .unwrap();

        let (settings, warning) = UserSettings::load_from_paths(&canonical, &legacy);
        assert!(settings.model_path.is_empty());
        assert!(warning.unwrap().contains("malformed"));
        assert_eq!(
            fs::read(&legacy).unwrap(),
            serde_json::to_vec(&UserSettings::default()).unwrap()
        );
    }

    #[test]
    fn absent_settings_use_defaults_without_a_warning() {
        let temp = tempfile::tempdir().unwrap();
        let (settings, warning) = UserSettings::load_from_paths(
            &temp.path().join("new/settings.json"),
            &temp.path().join("old/settings.json"),
        );
        assert!(settings.model_path.is_empty());
        assert!(warning.is_none());
    }

    #[test]
    fn failed_migration_write_keeps_the_legacy_settings_loaded() {
        let temp = tempfile::tempdir().unwrap();
        let blocked_parent = temp.path().join("not-a-directory");
        fs::write(&blocked_parent, b"x").unwrap();
        let canonical = blocked_parent.join("settings.json");
        let legacy = temp.path().join("old/settings.json");
        fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        let mut saved = UserSettings::default();
        saved.model_path = "kept.gguf".into();
        fs::write(&legacy, serde_json::to_vec(&saved).unwrap()).unwrap();

        let (loaded, warning) = UserSettings::load_from_paths(&canonical, &legacy);
        assert_eq!(loaded.model_path, "kept.gguf");
        assert!(warning.unwrap().contains("could not migrate"));
        assert!(legacy.exists());
        assert!(!canonical.exists());
    }

    #[test]
    fn older_settings_without_workspace_keep_the_saved_model_preference() {
        let old =
            serde_json::json!({"version": 1, "model_path": "saved.gguf", "permission": "ask"});
        let settings: UserSettings = serde_json::from_value(old).unwrap();
        assert_eq!(settings.model_path, "saved.gguf");
        assert!(settings.workspace_path.is_empty());
    }

    #[test]
    fn loop_preference_round_trips_and_older_files_default_to_two() {
        for loops in 1..=4 {
            let mut settings = UserSettings::default();
            settings.loops = loops;
            let bytes = serde_json::to_vec(&settings).unwrap();
            let decoded: UserSettings = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(decoded.loops, loops);
        }
        let old = serde_json::json!({"version": 1, "model_path": "saved.gguf"});
        let decoded: UserSettings = serde_json::from_value(old).unwrap();
        assert_eq!(decoded.loops, 2);
    }

    #[test]
    fn invalid_loop_preferences_are_rejected_by_load_configuration() {
        for loops in [0, 5] {
            let mut settings = UserSettings::default();
            settings.loops = loops;
            assert!(
                settings
                    .load_config(PathBuf::from("model.gguf"))
                    .engine_options()
                    .is_err()
            );
        }
    }

    #[test]
    fn configuration_conversion_rejects_invalid_permission_and_limits() {
        let mut settings = UserSettings::default();
        settings.permission = "invalid".into();
        assert!(settings.agent_settings(PathBuf::new()).is_err());
        settings.permission = "ask".into();
        settings.max_seconds = 0;
        assert!(settings.agent_settings(PathBuf::new()).is_err());
        settings.max_seconds = 1800;
        settings.top_p = f32::NAN;
        assert!(settings.generation_config().is_err());
    }
}
