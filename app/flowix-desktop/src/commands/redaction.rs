//! F6 IPC：脱敏配置读写（`~/.flowix/redaction/config.json`）+ 自定义词表加载
//! （`~/.flowix/redaction/words.txt`，用户手改）+ L1 规则预览。
//!
//! M1 切片：`redaction_preview` 只跑 L1 规则引擎（入参直接给文本）；
//! 完整管线（L0 取材 + L2 蒸馏 + L3 确认门）在 M2/M3 接入，届时
//! `redaction_preview` 升级为 (question, scope, targetLevel) 口径。

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use flowix_redaction::{self, CustomWord, RedactionOptions, RedactionReport};
use serde::{Deserialize, Serialize};

pub const REDACTION_LEVELS: &[&str] = &["r1", "r2", "r3"];
const SCHEMA_VERSION: u32 = 1;

fn default_level() -> String {
    "r2".to_string()
}

fn default_payload_limit_kb() -> u32 {
    64
}

fn default_confirm_timeout_secs() -> u32 {
    120
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactionConfig {
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default = "default_level")]
    pub default_level: String,
    #[serde(default)]
    pub level_overrides: BTreeMap<String, String>,
    #[serde(default = "default_payload_limit_kb")]
    pub payload_limit_kb: u32,
    #[serde(default = "default_confirm_timeout_secs")]
    pub confirm_timeout_secs: u32,
    #[serde(default)]
    pub strict_luhn: bool,
    #[serde(default = "default_true")]
    pub rewrite_absolute_time: bool,
}

impl Default for RedactionConfig {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            default_level: default_level(),
            level_overrides: BTreeMap::new(),
            payload_limit_kb: default_payload_limit_kb(),
            confirm_timeout_secs: default_confirm_timeout_secs(),
            strict_luhn: false,
            rewrite_absolute_time: true,
        }
    }
}

fn redaction_dir() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or_else(|| "HOME_UNAVAILABLE".to_string())?;
    Ok(home.join(".flowix").join("redaction"))
}

fn config_file(dir: &Path) -> PathBuf {
    dir.join("config.json")
}

fn words_file(dir: &Path) -> PathBuf {
    dir.join("words.txt")
}

pub(crate) fn read_config_from(dir: &Path) -> RedactionConfig {
    fs::read_to_string(config_file(dir))
        .ok()
        .and_then(|raw| serde_json::from_str::<RedactionConfig>(&raw).ok())
        .unwrap_or_default()
}

pub(crate) fn write_config_to(dir: &Path, config: &RedactionConfig) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("CONFIG_DIR_CREATE_FAILED: {e}"))?;
    let raw = serde_json::to_string_pretty(config)
        .map_err(|e| format!("CONFIG_SERIALIZE_FAILED: {e}"))?;
    let target = config_file(dir);
    let tmp = dir.join("config.json.tmp");
    {
        let mut f = fs::File::create(&tmp).map_err(|e| format!("CONFIG_WRITE_FAILED: {e}"))?;
        f.write_all(raw.as_bytes())
            .map_err(|e| format!("CONFIG_WRITE_FAILED: {e}"))?;
        f.sync_all().map_err(|e| format!("CONFIG_WRITE_FAILED: {e}"))?;
    }
    fs::rename(&tmp, &target).map_err(|e| format!("CONFIG_WRITE_FAILED: {e}"))?;
    Ok(())
}

pub(crate) fn read_words_from(dir: &Path) -> Vec<CustomWord> {
    fs::read_to_string(words_file(dir))
        .map(|raw| {
            raw.lines()
                .filter_map(flowix_redaction::parse_word_line)
                .collect()
        })
        .unwrap_or_default()
}

fn validate_config(config: &RedactionConfig) -> Result<(), String> {
    if !REDACTION_LEVELS.contains(&config.default_level.as_str()) {
        return Err("INVALID_DEFAULT_LEVEL".to_string());
    }
    if let Some((skill, level)) = config
        .level_overrides
        .iter()
        .find(|(_, level)| !REDACTION_LEVELS.contains(&level.as_str()))
    {
        return Err(format!("INVALID_LEVEL_OVERRIDE:{skill}:{level}"));
    }
    if !(1..=4096).contains(&config.payload_limit_kb) {
        return Err("INVALID_PAYLOAD_LIMIT".to_string());
    }
    if !(30..=600).contains(&config.confirm_timeout_secs) {
        return Err("INVALID_CONFIRM_TIMEOUT".to_string());
    }
    Ok(())
}

fn load_config() -> Result<RedactionConfig, String> {
    Ok(read_config_from(&redaction_dir()?))
}

fn load_words() -> Result<Vec<CustomWord>, String> {
    Ok(read_words_from(&redaction_dir()?))
}

#[tauri::command]
pub fn redaction_config_get() -> Result<RedactionConfig, String> {
    load_config()
}

#[tauri::command]
pub fn redaction_config_set(config: RedactionConfig) -> Result<RedactionConfig, String> {
    validate_config(&config)?;
    let mut normalized = config;
    normalized.schema_version = SCHEMA_VERSION;
    let dir = redaction_dir()?;
    write_config_to(&dir, &normalized)?;
    Ok(normalized)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactionPreview {
    pub report: RedactionReport,
    pub word_count: usize,
}

#[tauri::command]
pub fn redaction_preview(text: String) -> Result<RedactionPreview, String> {
    if text.trim().is_empty() {
        return Err("TEXT_REQUIRED".to_string());
    }
    let config = load_config()?;
    let words = load_words()?;
    let options = RedactionOptions {
        strict_luhn: config.strict_luhn,
        rewrite_absolute_time: config.rewrite_absolute_time,
        now: config
            .rewrite_absolute_time
            .then(chrono::Local::now).map(|now| now.date_naive()),
    };
    let report = flowix_redaction::redact(&text, &options, &words);
    Ok(RedactionPreview {
        report,
        word_count: words.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_when_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        let config = read_config_from(dir.path());
        assert_eq!(config, RedactionConfig::default());
        assert_eq!(config.default_level, "r2");
        assert_eq!(config.payload_limit_kb, 64);
        assert_eq!(config.confirm_timeout_secs, 120);
        assert!(!config.strict_luhn);
        assert!(config.rewrite_absolute_time);
    }

    #[test]
    fn config_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let config = RedactionConfig {
            default_level: "r3".to_string(),
            level_overrides: BTreeMap::from([("skill-a".to_string(), "r1".to_string())]),
            payload_limit_kb: 128,
            confirm_timeout_secs: 90,
            strict_luhn: true,
            rewrite_absolute_time: false,
            schema_version: 0,
        };
        write_config_to(dir.path(), &config).unwrap();
        assert!(config_file(dir.path()).is_file());
        let loaded = read_config_from(dir.path());
        assert_eq!(loaded, config);
    }

    #[test]
    fn config_rejects_invalid_values() {
        let base = RedactionConfig::default();
        assert_eq!(
            validate_config(&RedactionConfig {
                default_level: "r9".to_string(),
                ..base.clone()
            }),
            Err("INVALID_DEFAULT_LEVEL".to_string())
        );
        assert_eq!(
            validate_config(&RedactionConfig {
                level_overrides: BTreeMap::from([("s".to_string(), "x".to_string())]),
                ..base.clone()
            })
            .err()
            .map(|e| e.split(':').next().unwrap().to_string()),
            Some("INVALID_LEVEL_OVERRIDE".to_string())
        );
        assert!(validate_config(&RedactionConfig {
            payload_limit_kb: 0,
            ..base.clone()
        })
        .is_err());
        assert!(validate_config(&RedactionConfig {
            confirm_timeout_secs: 10,
            ..base
        })
        .is_err());
    }

    #[test]
    fn words_file_parsing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_words_from(dir.path()).is_empty());

        fs::write(
            words_file(dir.path()),
            "张三=>张工\n\n李四\n  王五 => 老王 \n",
        )
        .unwrap();
        let words = read_words_from(dir.path());
        assert_eq!(words.len(), 3);
        assert_eq!(words[0].original, "张三");
        assert_eq!(words[0].replacement.as_deref(), Some("张工"));
        assert_eq!(words[1].original, "李四");
        assert_eq!(words[1].replacement, None);
        assert_eq!(words[2].replacement.as_deref(), Some("老王"));
    }

    #[test]
    fn preview_uses_config_and_words() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(words_file(dir.path()), "张三=>张工\n").unwrap();
        let config = RedactionConfig {
            rewrite_absolute_time: false,
            ..RedactionConfig::default()
        };
        write_config_to(dir.path(), &config).unwrap();

        let options = RedactionOptions {
            strict_luhn: config.strict_luhn,
            rewrite_absolute_time: config.rewrite_absolute_time,
            now: None,
        };
        let words = read_words_from(dir.path());
        let report = flowix_redaction::redact("张三的电话13800138000", &options, &words);
        assert_eq!(report.text, "张工的电话⟦PHONE-1⟧");
    }

    #[test]
    fn preview_rejects_empty_text() {
        let err = redaction_preview("   ".to_string()).unwrap_err();
        assert_eq!(err, "TEXT_REQUIRED");
    }
}
