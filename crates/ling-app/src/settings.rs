//! 模型 API 配置：加载 / 保存 / 就绪判定 / 展示脱敏。
//!
//! 凭据只保存在用户本机数据目录的 `settings.json`，绝不写入源码或示例。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// OpenAI 兼容网关配置
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApiSettings {
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub model: String,
}

impl ApiSettings {
    /// 「已配置」判定：接口地址与模型必填，API Key 允许为空（匿名网关）。
    /// 与 `quick_validate` 及设置页占位文案「留空表示匿名网关」保持一致。
    pub fn is_ready(&self) -> bool {
        !self.base_url.trim().is_empty() && !self.model.trim().is_empty()
    }

    /// 保存前的轻量校验：非空 + http/https 前缀。
    /// 完整校验（协议、禁用网段、DNS）由 ling-core::ai 在每次请求时执行。
    pub fn quick_validate(&self) -> Result<(), String> {
        let base = self.base_url.trim();
        if base.is_empty() {
            return Err("接口地址不能为空".to_string());
        }
        if !base.starts_with("http://") && !base.starts_with("https://") {
            return Err("接口地址必须以 http:// 或 https:// 开头".to_string());
        }
        if self.model.trim().is_empty() {
            return Err("模型名称不能为空".to_string());
        }
        Ok(())
    }

    /// 从数据目录加载；文件缺失或损坏时返回默认值（首次启动 / 手工删除）
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// 保存为 JSON（目录不存在则创建）
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self).unwrap_or_default())
    }
}

/// API Key 展示脱敏：仅保留前 2 与后 2 个字符
pub fn mask_key(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= 4 {
        return "•".repeat(chars.len().max(1));
    }
    let head: String = chars[..2].iter().collect();
    let tail: String = chars[chars.len() - 2..].iter().collect();
    let dots = "•".repeat(6);
    format!("{head}{dots}{tail}")
}

/// 应用数据目录（数据库 / 配置 / 导出文件都放这里）。
/// 优先级：环境变量 LING_DATA_DIR → Windows %APPDATA%\ling →
/// Unix $XDG_DATA_HOME/ling 或 $HOME/.local/share/ling → 当前目录 ./ling-data
pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("LING_DATA_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    #[cfg(windows)]
    {
        if let Some(base) = std::env::var_os("APPDATA") {
            if !base.is_empty() {
                return PathBuf::from(base).join("ling");
            }
        }
    }
    #[cfg(not(windows))]
    {
        if let Some(base) = std::env::var_os("XDG_DATA_HOME") {
            if !base.is_empty() {
                return PathBuf::from(base).join("ling");
            }
        }
        if let Some(home) = std::env::var_os("HOME") {
            if !home.is_empty() {
                return PathBuf::from(home).join(".local").join("share").join("ling");
            }
        }
    }
    PathBuf::from("ling-data")
}

#[cfg(test)]
mod tests {
    use super::*;

    // 测试用的「密钥」由程序拼接生成，源码中不出现任何真实凭据字面量
    fn synthetic_key(len: usize) -> String {
        "k".repeat(len)
    }

    #[test]
    fn ready_requires_base_and_model_only() {
        // 匿名网关：key 留空也应就绪（与占位文案 / quick_validate 三处一致）
        assert!(!ApiSettings::default().is_ready());
        let mut s = ApiSettings {
            base_url: "https://gateway.example/v1".into(),
            api_key: String::new(),
            model: "demo-model".into(),
        };
        assert!(s.is_ready(), "空 key（匿名网关）应视为已配置");
        s.model = "  ".into();
        assert!(!s.is_ready());
        s.model = "demo-model".into();
        s.base_url = " ".into();
        assert!(!s.is_ready());
        // 带 key 的常规网关同样就绪
        s.base_url = "https://gateway.example/v1".into();
        s.api_key = synthetic_key(24);
        assert!(s.is_ready());
    }

    #[test]
    fn quick_validate_checks_scheme_and_model() {
        let mut s = ApiSettings::default();
        assert!(s.quick_validate().is_err());
        s.base_url = "ftp://gateway.example/v1".into();
        assert!(s.quick_validate().is_err());
        s.base_url = "https://gateway.example/v1".into();
        assert!(s.quick_validate().is_err(), "缺少模型名应报错");
        s.model = "demo-model".into();
        assert!(s.quick_validate().is_ok());
    }

    #[test]
    fn mask_key_hides_middle() {
        let key = synthetic_key(12);
        let masked = mask_key(&key);
        assert!(masked.starts_with("kk"));
        assert!(masked.ends_with("kk"));
        assert!(masked.contains('•'));
        assert!(!masked.contains(&key));
        // 短键整体打码
        assert_eq!(mask_key(&synthetic_key(3)), "•••");
    }

    #[test]
    fn settings_roundtrip_through_file() {
        let dir = std::env::temp_dir().join(format!("ling-test-{}", std::process::id()));
        let path = dir.join("settings.json");
        let s = ApiSettings {
            base_url: "https://gateway.example/v1".into(),
            api_key: synthetic_key(20),
            model: "demo-model".into(),
        };
        s.save(&path).unwrap();
        let loaded = ApiSettings::load(&path);
        assert_eq!(loaded.base_url, s.base_url);
        assert_eq!(loaded.model, s.model);
        assert_eq!(loaded.api_key, s.api_key);
        std::fs::remove_dir_all(dir).ok();
    }
}
