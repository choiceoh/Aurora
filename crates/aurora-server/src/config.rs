use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Z.AI default settings
const CODING_PLAN_BASE_URL: &str = "https://api.z.ai/api/coding/paas/v4";
const CODING_PLAN_MODEL: &str = "glm-5-turbo";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub api_key: String,
    #[serde(default = "default_base_url")]
    pub base_url: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deneb_url: Option<String>,
    #[serde(default = "default_server_host")]
    pub server_host: String,
    #[serde(default = "default_server_port")]
    pub server_port: u16,
}

fn default_base_url() -> String {
    CODING_PLAN_BASE_URL.to_string()
}

fn default_model() -> String {
    CODING_PLAN_MODEL.to_string()
}

fn default_server_host() -> String {
    "0.0.0.0".to_string()
}

fn default_server_port() -> u16 {
    3710
}

impl Default for Config {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            base_url: default_base_url(),
            model: default_model(),
            deneb_url: None,
            server_host: default_server_host(),
            server_port: default_server_port(),
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".aurora")
            .join("config.json")
    }

    pub fn load() -> Option<Self> {
        let path = Self::path();
        let data = fs::read_to_string(&path).ok()?;
        let mut config: Config = serde_json::from_str(&data).ok()?;

        if let Ok(key) = std::env::var("ZHIPUAI_API_KEY") {
            if !key.is_empty() {
                config.api_key = key;
            }
        }

        if let Ok(url) = std::env::var("DENEB_URL") {
            if !url.is_empty() {
                config.deneb_url = Some(url);
            }
        }

        if config.api_key.is_empty() {
            return None;
        }
        Some(config)
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("디렉토리 생성 실패: {e}"))?;
        }
        let json =
            serde_json::to_string_pretty(self).map_err(|e| format!("직렬화 실패: {e}"))?;
        fs::write(&path, json).map_err(|e| format!("파일 쓰기 실패: {e}"))?;
        Ok(())
    }

    pub fn init_with_key(api_key: String) -> Result<Self, String> {
        let config = Self {
            api_key,
            base_url: default_base_url(),
            model: default_model(),
            deneb_url: None,
            server_host: default_server_host(),
            server_port: default_server_port(),
        };
        config.save()?;
        Ok(config)
    }

    pub fn display_url(&self) -> &str {
        if self.base_url.contains("z.ai/api/coding") {
            "Z.AI 코딩플랜"
        } else if self.base_url.contains("z.ai") {
            "Z.AI"
        } else if self.base_url.contains("bigmodel.cn") {
            "智谱AI"
        } else {
            &self.base_url
        }
    }

    pub fn listen_addr(&self) -> String {
        format!("{}:{}", self.server_host, self.server_port)
    }
}
