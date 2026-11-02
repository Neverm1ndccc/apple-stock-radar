use crate::{Product, Result, Store};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Serialize, Deserialize)]
pub struct Catalog {
    pub products: Vec<Product>,
    pub stores: Vec<Store>,
    pub version: String,
}

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

fn model_name(raw: &str) -> String {
    let mut name = raw.replace("iphone", "iPhone ").replace("ipad", "iPad ");
    for (from, to) in [
        ("promax", " Pro Max"),
        ("pro", " Pro"),
        ("plus", " Plus"),
        ("mini", " mini"),
        ("air", " Air"),
        ("duo", " Duo"),
    ] {
        name = name.replace(from, to);
    }
    name.replace("  ", " ").trim().to_string()
}

fn color_name(raw: &str) -> String {
    match raw {
        "black" => "黑色",
        "white" => "白色",
        "silver" => "银色",
        "blue" => "蓝色",
        "burgundy" => "酒红色",
        "glacier" => "冰川蓝色",
        "green" => "绿色",
        "pink" => "粉色",
        "purple" => "紫色",
        "teal" => "深青色",
        "ultramarine" => "群青色",
        "deserttitanium" => "沙漠色钛金属",
        "naturaltitanium" => "原色钛金属",
        "blacktitanium" => "黑色钛金属",
        "whitetitanium" => "白色钛金属",
        "skyblue" => "天蓝色",
        "lightgold" => "浅金色",
        "spaceblack" => "深空黑色",
        "cloudwhite" => "云白色",
        _ => raw,
    }
    .into()
}

impl Catalog {
    pub fn load() -> Result<Self> {
        let mut products = Vec::new();
        for (locale, text, base) in [
            (
                "zh_CN",
                include_str!("../data/products_zh_CN.json"),
                "https://www.apple.com.cn",
            ),
            (
                "zh_HK",
                include_str!("../data/products_zh_HK.json"),
                "https://www.apple.com/hk",
            ),
        ] {
            let pages: Vec<Value> = serde_json::from_str(text).map_err(|e| e.to_string())?;
            for page in pages {
                // First release verifies the iPhone selection schema only.
                if s(&page, "category") != "iphone" {
                    continue;
                }
                let family = s(&page, "family");
                let Some(items) = page.pointer("/data/products").and_then(Value::as_array) else {
                    continue;
                };
                for item in items {
                    let sku = s(item, "partNumber");
                    let raw_model = s(item, "familyType");
                    let capacity = s(item, "dimensionCapacity");
                    let color = s(item, "dimensionColor");
                    if [sku, raw_model, capacity, color]
                        .iter()
                        .any(|s| s.is_empty())
                    {
                        continue;
                    }
                    let id = format!("{locale}:{sku}");
                    if products.iter().any(|p: &Product| p.id == id) {
                        continue;
                    }
                    products.push(Product {
                        id,
                        locale: locale.into(),
                        category: "iphone".into(),
                        model: model_name(raw_model),
                        capacity: capacity.to_uppercase(),
                        color: color_name(color),
                        sku: sku.into(),
                        official_url: format!("{base}/shop/buy-iphone/{family}"),
                    });
                }
            }
        }
        let regions: Vec<Value> =
            serde_json::from_str(include_str!("../data/stores.json")).map_err(|e| e.to_string())?;
        let mut stores = Vec::new();
        for region in regions {
            let locale = s(&region, "locale");
            if !["zh_CN", "zh_HK"].contains(&locale) {
                continue;
            }
            // Mainland snapshots contain grouped states; Hong Kong has a flat store list.
            // Some snapshots retain both normalized and raw arrays, so merge by region/id.
            for key in ["state", "states", "store"] {
                if let Some(items) = region.get(key).and_then(Value::as_array) {
                    for item in items {
                        let entries = if s(item, "id").is_empty() {
                            item.get("store")
                                .and_then(Value::as_array)
                                .map(Vec::as_slice)
                                .unwrap_or(&[])
                        } else {
                            std::slice::from_ref(item)
                        };
                        for store in entries {
                            let id = s(store, "id");
                            if id.is_empty()
                                || s(store, "name").trim().is_empty()
                                || stores
                                    .iter()
                                    .any(|s: &Store| s.locale == locale && s.id == id)
                            {
                                continue;
                            }
                            stores.push(Store {
                                id: id.into(),
                                locale: locale.into(),
                                city: s(&store["address"], "city").into(),
                                name: s(store, "name").trim().into(),
                            });
                        }
                    }
                }
            }
        }
        if products.is_empty() || stores.is_empty() {
            return Err("离线目录为空，无法建立关注".into());
        }
        Ok(Self {
            products,
            stores,
            version: "上游离线快照 · d00eb0c · 未在线核验".into(),
        })
    }

    pub fn product(&self, id: &str) -> Result<&Product> {
        self.products
            .iter()
            .find(|p| p.id == id)
            .ok_or_else(|| "SKU 不在当前目录中".into())
    }

    pub fn store(&self, locale: &str, id: &str) -> Result<&Store> {
        self.stores
            .iter()
            .find(|s| s.locale == locale && s.id == id)
            .ok_or_else(|| "门店不存在或与商品地区不符".into())
    }
}
