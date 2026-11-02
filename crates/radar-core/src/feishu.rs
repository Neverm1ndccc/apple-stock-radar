//! Feishu custom group bot protocol. Credentials belong in Keychain, never SQLite.
use crate::{Result, Source, Target, official_url};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credentials {
    pub webhook: String,
    pub secret: Option<String>,
}

impl Credentials {
    pub fn validate(&self) -> Result<()> {
        webhook_url(&self.webhook)?;
        if self
            .secret
            .as_ref()
            .is_some_and(|s| s.is_empty() || s.len() > 512 || s.contains(['\n', '\r']))
        {
            return Err("飞书签名密钥格式无效".into());
        }
        Ok(())
    }
}

pub fn webhook_url(value: &str) -> Result<url::Url> {
    let u = url::Url::parse(value.trim()).map_err(|_| "飞书 Webhook 格式错误")?;
    let token = u
        .path()
        .strip_prefix("/open-apis/bot/v2/hook/")
        .unwrap_or("");
    if u.scheme() != "https"
        || u.host_str() != Some("open.feishu.cn")
        || !u.username().is_empty()
        || u.password().is_some()
        || u.port().is_some()
        || u.query().is_some()
        || u.fragment().is_some()
        || token.is_empty()
        || token.len() > 200
        || !token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("仅支持飞书群自定义机器人的官方 HTTPS Webhook".into());
    }
    Ok(u)
}

pub fn payload(
    title: &str,
    body: &str,
    url: &str,
    secret: Option<&str>,
    timestamp: i64,
) -> Result<serde_json::Value> {
    official_url(url)?;
    let mut value = serde_json::json!({
        "msg_type":"text",
        "content":{"text": format!("库存雷达\n{title}\n{body}\n{url}")}
    });
    if let Some(secret) = secret {
        let key = ring::hmac::Key::new(
            ring::hmac::HMAC_SHA256,
            format!("{timestamp}\n{secret}").as_bytes(),
        );
        value["timestamp"] = timestamp.to_string().into();
        value["sign"] = STANDARD.encode(ring::hmac::sign(&key, b"").as_ref()).into();
    }
    if serde_json::to_vec(&value)
        .map_err(|_| "飞书消息编码失败")?
        .len()
        > 20 * 1024
    {
        return Err("飞书消息超出 20 KB 限制".into());
    }
    Ok(value)
}

pub fn check_response(value: &serde_json::Value) -> Result<()> {
    let codes: Vec<_> = ["code", "StatusCode"]
        .iter()
        .filter_map(|key| value.get(key))
        .collect();
    if !codes.is_empty() && codes.iter().all(|code| code.as_i64() == Some(0)) {
        Ok(())
    } else {
        // Server messages can echo a credential. Never return their raw text.
        Err("飞书未确认接收，请检查机器人关键词、签名密钥或群权限".into())
    }
}

pub fn target_message(target: &Target) -> Result<(String, String, String)> {
    if target.source == Source::Mock {
        return Err("模拟数据不能发送到飞书".into());
    }
    let state = match target.state.as_str() {
        "IN_STOCK" => "可取货",
        "OUT_OF_STOCK" => "暂无货",
        "STALE" => "记录已过期，当前库存无法确认",
        _ => "当前库存无法确认",
    };
    Ok((
        format!("[手动发送] {} · {state}", target.product.model),
        format!(
            "{} {} · {} / {}\nSKU：{}\n来源：{}\n观测时间：{}\n请以 Apple 结账页为准",
            target.product.capacity,
            target.product.color,
            target.store.city,
            target.store.name,
            target.product.sku,
            if target.source == Source::Apple {
                "Apple 商店查询"
            } else {
                "用户手动记录"
            },
            target.observed_at.as_deref().unwrap_or("尚无记录")
        ),
        target.product.official_url.clone(),
    ))
}
