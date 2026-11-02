pub mod apple;
pub mod catalog;
pub mod feishu;
pub mod history;
pub mod model;
pub mod request_budget;
pub mod scan_health;
pub mod scanner;
pub mod storage;

pub use catalog::Catalog;
pub use model::*;
pub use storage::Database;

pub type Result<T> = std::result::Result<T, String>;

pub fn official_url(value: &str) -> Result<url::Url> {
    let parsed = url::Url::parse(value).map_err(|_| "链接格式错误")?;
    let host = parsed.host_str().unwrap_or_default();
    if parsed.scheme() != "https"
        || !matches!(
            host,
            "www.apple.com" | "www.apple.com.cn" | "apple.com" | "apple.com.cn"
        )
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || !(parsed.path() == "/shop"
            || parsed.path().starts_with("/shop/")
            || parsed.path() == "/hk/shop"
            || parsed.path().starts_with("/hk/shop/"))
    {
        return Err("只允许 Apple 官方 HTTPS 页面".into());
    }
    Ok(parsed)
}

pub fn bark_url(value: &str) -> Result<url::Url> {
    let url = url::Url::parse(value.trim()).map_err(|_| "Bark 地址格式错误")?;
    if url.scheme() != "https"
        || url.host_str() != Some("api.day.app")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url
            .path_segments()
            .map(|s| s.filter(|p| !p.is_empty()).count())
            != Some(1)
        || !url
            .path()
            .trim_matches('/')
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err("首版仅支持 https://api.day.app/你的Key；地址只保存在 Keychain".into());
    }
    Ok(url)
}
