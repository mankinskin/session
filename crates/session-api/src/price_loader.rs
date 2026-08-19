use serde::{
    Deserialize,
    Serialize,
};
use std::{
    collections::HashMap,
    path::Path,
};

use crate::SessionError;

/// Model price record from tools/model-prices/model_prices.json
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelPrice {
    pub provider_id: String,
    pub provider_name: String,
    pub model_id: String,
    #[serde(default)]
    pub input_mtok: Option<f64>,
    #[serde(default)]
    pub output_mtok: Option<f64>,
    #[serde(default)]
    pub cache_read_mtok: Option<f64>,
    #[serde(default)]
    pub cache_write_mtok: Option<f64>,
}

/// Price table wrapper with metadata
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PriceTable {
    #[serde(rename = "_meta")]
    pub meta: serde_json::Value,
    pub models: Vec<ModelPrice>,
}

/// Load price table from tools/model-prices/model_prices.json relative to repo root.
/// Repo root is resolved from the session store root by going up to workspace root,
/// which is typically the repo root.
pub fn load_price_table(store_root: &Path) -> Result<PriceTable, SessionError> {
    // Resolve repo root: session store is typically at <repo>/.session,
    // so we go up one level.
    let repo_root = store_root.parent().ok_or_else(|| {
        SessionError::InvalidStorePath(store_root.to_path_buf())
    })?;
    let price_file = repo_root
        .join("tools")
        .join("model-prices")
        .join("model_prices.json");

    let json = std::fs::read_to_string(&price_file).map_err(|source| {
        SessionError::Io {
            path: price_file.clone(),
            source,
        }
    })?;

    serde_json::from_str(&json).map_err(|source| SessionError::Deserialize {
        path: price_file,
        source,
    })
}

/// Compute USD cost from token counts and model_id.
/// Returns None if the model is not found in the price table.
pub fn compute_cost_usd(
    model_id: &str,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
    price_table: &PriceTable,
) -> Option<f64> {
    // Build a fast lookup map by model_id
    let price_map: HashMap<&str, &ModelPrice> = price_table
        .models
        .iter()
        .map(|p| (p.model_id.as_str(), p))
        .collect();

    let price = price_map.get(model_id)?;

    let input_cost = price
        .input_mtok
        .map(|rate| (input_tokens as f64 / 1_000_000.0) * rate)
        .unwrap_or(0.0);
    let output_cost = price
        .output_mtok
        .map(|rate| (output_tokens as f64 / 1_000_000.0) * rate)
        .unwrap_or(0.0);
    let cache_read_cost = price
        .cache_read_mtok
        .map(|rate| (cache_read_tokens as f64 / 1_000_000.0) * rate)
        .unwrap_or(0.0);
    let cache_write_cost = price
        .cache_write_mtok
        .map(|rate| (cache_write_tokens as f64 / 1_000_000.0) * rate)
        .unwrap_or(0.0);

    Some(input_cost + output_cost + cache_read_cost + cache_write_cost)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_cost_with_all_token_types() {
        let table = PriceTable {
            meta: serde_json::json!({}),
            models: vec![ModelPrice {
                provider_id: "anthropic".to_string(),
                provider_name: "Anthropic".to_string(),
                model_id: "claude-3-5-sonnet".to_string(),
                input_mtok: Some(3.0),
                output_mtok: Some(15.0),
                cache_read_mtok: Some(0.3),
                cache_write_mtok: Some(3.75),
            }],
        };

        let cost = compute_cost_usd(
            "claude-3-5-sonnet",
            1_000_000, // 1M input tokens = $3
            1_000_000, // 1M output tokens = $15
            1_000_000, // 1M cache read = $0.30
            1_000_000, // 1M cache write = $3.75
            &table,
        );

        assert_eq!(cost, Some(22.05));
    }

    #[test]
    fn compute_cost_unknown_model_returns_none() {
        let table = PriceTable {
            meta: serde_json::json!({}),
            models: vec![],
        };

        let cost =
            compute_cost_usd("unknown-model", 100_000, 100_000, 0, 0, &table);

        assert_eq!(cost, None);
    }

    #[test]
    fn compute_cost_with_missing_prices_uses_zero() {
        let table = PriceTable {
            meta: serde_json::json!({}),
            models: vec![ModelPrice {
                provider_id: "test".to_string(),
                provider_name: "Test".to_string(),
                model_id: "partial-model".to_string(),
                input_mtok: Some(1.0),
                output_mtok: None,
                cache_read_mtok: None,
                cache_write_mtok: None,
            }],
        };

        let cost = compute_cost_usd(
            "partial-model",
            500_000, // 0.5M input = $0.50
            500_000, // output: no price, $0
            100_000, // cache read: no price, $0
            100_000, // cache write: no price, $0
            &table,
        );

        assert_eq!(cost, Some(0.5));
    }
}
