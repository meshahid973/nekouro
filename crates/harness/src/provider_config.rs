//! Device-local configuration for native HTTP model providers.
//!
//! API keys are deliberately excluded from synced/settings JSON. RPC surfaces
//! expose only whether a secret exists; they never return the secret itself.

use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

use zeron_proto::HarnessId;

static ROOT: OnceLock<RwLock<Option<PathBuf>>> = OnceLock::new();

fn root_slot() -> &'static RwLock<Option<PathBuf>> {
    ROOT.get_or_init(|| RwLock::new(None))
}

pub fn set_root(root: &Path) {
    *root_slot()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(root.to_path_buf());
}

pub fn initialized() -> bool {
    root_slot()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .is_some()
}

fn root() -> Result<PathBuf, crate::HarnessError> {
    root_slot()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
        .ok_or_else(|| crate::HarnessError::Protocol("provider configuration root is not initialized".into()))
}

fn provider_slug(id: HarnessId) -> Result<&'static str, crate::HarnessError> {
    match id {
        HarnessId::OpenRouter => Ok("openrouter"),
        HarnessId::OpenAiCompatible => Ok("openai-compatible"),
        HarnessId::Ollama => Ok("ollama"),
        HarnessId::LmStudio => Ok("lm-studio"),
        _ => Err(crate::HarnessError::Protocol(format!(
            "{id:?} is not a native HTTP provider"
        ))),
    }
}

fn key_env(id: HarnessId) -> Option<&'static str> {
    match id {
        HarnessId::OpenRouter => Some("OPENROUTER_API_KEY"),
        HarnessId::OpenAiCompatible => Some("NEKOURO_OPENAI_API_KEY"),
        HarnessId::Ollama => Some("OLLAMA_API_KEY"),
        HarnessId::LmStudio => Some("LM_STUDIO_API_KEY"),
        _ => None,
    }
}

fn secret_path(id: HarnessId) -> Result<PathBuf, crate::HarnessError> {
    Ok(root()?
        .join("provider-secrets")
        .join(format!("{}.key", provider_slug(id)?)))
}

pub fn api_key(id: HarnessId) -> Option<String> {
    if let Some(env) = key_env(id)
        && let Ok(value) = std::env::var(env)
        && !value.trim().is_empty()
    {
        return Some(value.trim().to_owned());
    }
    std::fs::read_to_string(secret_path(id).ok()?)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

pub fn has_api_key(id: HarnessId) -> bool {
    api_key(id).is_some()
}

pub fn set_api_key(id: HarnessId, value: Option<&str>) -> Result<(), crate::HarnessError> {
    let path = secret_path(id)?;
    let value = value.map(str::trim).filter(|value| !value.is_empty());
    if value.is_none() {
        return match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        };
    }
    let parent = path.parent().expect("secret path has parent");
    std::fs::create_dir_all(parent)?;
    let tmp = path.with_extension(format!("key.{}.tmp", uuid::Uuid::new_v4()));
    std::fs::write(&tmp, value.unwrap().as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(tmp, path)?;
    Ok(())
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct NativeProviderFile {
    openai_compatible_base_url: Option<String>,
}

fn config_path() -> Result<PathBuf, crate::HarnessError> {
    Ok(root()?.join("native-providers.json"))
}

fn read_config() -> NativeProviderFile {
    config_path()
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn write_config(config: &NativeProviderFile) -> Result<(), crate::HarnessError> {
    let path = config_path()?;
    let parent = path.parent().expect("provider config path has parent");
    std::fs::create_dir_all(parent)?;
    let tmp = path.with_extension(format!("json.{}.tmp", uuid::Uuid::new_v4()));
    let bytes = serde_json::to_vec_pretty(config)
        .map_err(|error| crate::HarnessError::Protocol(format!("serialize provider config: {error}")))?;
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

pub fn custom_base_url() -> Option<String> {
    std::env::var("NEKOURO_OPENAI_BASE_URL")
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_owned())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            read_config()
                .openai_compatible_base_url
                .map(|value| value.trim().trim_end_matches('/').to_owned())
                .filter(|value| !value.is_empty())
        })
}

pub fn set_custom_base_url(value: Option<&str>) -> Result<(), crate::HarnessError> {
    let mut config = read_config();
    config.openai_compatible_base_url = value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_end_matches('/').to_owned());
    write_config(&config)
}

pub fn session_load(provider: HarnessId, session_id: &str) -> Option<Vec<serde_json::Value>> {
    let slug = provider_slug(provider).ok()?;
    if !valid_session_id(session_id) {
        return None;
    }
    let path = root()
        .ok()?
        .join("provider-sessions")
        .join(slug)
        .join(format!("{session_id}.json"));
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
}

pub fn session_save(
    provider: HarnessId,
    session_id: &str,
    messages: &[serde_json::Value],
) -> Result<(), crate::HarnessError> {
    if !valid_session_id(session_id) {
        return Err(crate::HarnessError::Protocol("invalid native provider session id".into()));
    }
    let dir = root()?.join("provider-sessions").join(provider_slug(provider)?);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{session_id}.json"));
    let tmp = dir.join(format!("{session_id}.{}.tmp", uuid::Uuid::new_v4()));
    let bytes = serde_json::to_vec(messages)
        .map_err(|error| crate::HarnessError::Protocol(format!("serialize provider session: {error}")))?;
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

fn valid_session_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}
