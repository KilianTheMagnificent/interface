//! The model catalogue `model/list` reports, and the Fast service tier each
//! model advertises.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use interface_common::sync::lock;
use interface_contracts::engine::ModelOption;
use interface_engine::EngineError;
use serde_json::{Value, json};

use crate::{protocol::CodexMethod, transport::CodexTransport};

#[derive(Clone, Debug, Default)]
pub(super) struct FastTierCatalogue {
    pub(super) loaded: bool,
    pub(super) default_model: Option<String>,
    by_model: HashMap<String, String>,
}

impl FastTierCatalogue {
    pub(super) fn tier_for(&self, model: Option<&str>) -> Option<String> {
        let selected = model.or(self.default_model.as_deref())?;
        self.by_model.get(selected).cloned()
    }
}

pub(super) fn model_catalog(response: &Value) -> (Vec<ModelOption>, FastTierCatalogue) {
    let mut models = Vec::new();
    let mut catalogue = FastTierCatalogue {
        loaded: true,
        ..FastTierCatalogue::default()
    };
    for model in response
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if model.get("hidden").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let Some(value) = model
            .get("id")
            .or_else(|| model.get("model"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
        else {
            continue;
        };
        let model_id = model
            .get("model")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty());
        let display_name = model
            .get("displayName")
            .and_then(Value::as_str)
            .unwrap_or(&value)
            .to_owned();
        let mut effort_levels = Vec::new();
        for effort in model
            .get("supportedReasoningEfforts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(level) = effort
                .get("reasoningEffort")
                .and_then(Value::as_str)
                .filter(|level| !level.is_empty())
            else {
                continue;
            };
            if !effort_levels.iter().any(|known| known == level) {
                effort_levels.push(level.to_owned());
            }
        }
        let fast_service_tier = fast_service_tier_from_model(model);
        if let Some(tier) = &fast_service_tier {
            catalogue.by_model.insert(value.clone(), tier.clone());
            if let Some(model_id) = model_id {
                catalogue.by_model.insert(model_id.to_owned(), tier.clone());
            }
        }
        let is_default = model.get("isDefault").and_then(Value::as_bool);
        if is_default == Some(true) {
            catalogue.default_model = Some(value.clone());
        }
        models.push(ModelOption {
            value,
            display_name,
            description: model
                .get("description")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            supports_effort: !effort_levels.is_empty(),
            effort_levels,
            is_default,
            supports_fast_mode: fast_service_tier.is_some().then_some(true),
        });
    }
    (models, catalogue)
}

fn fast_service_tier_from_model(model: &Value) -> Option<String> {
    let advertised = model
        .get("serviceTiers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .find_map(|tier| {
            let id = tier
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())?;
            let name = tier.get("name").and_then(Value::as_str).unwrap_or_default();
            (id.eq_ignore_ascii_case("fast") || name.trim().eq_ignore_ascii_case("fast"))
                .then(|| id.to_owned())
        });
    advertised.or_else(|| {
        model
            .get("additionalSpeedTiers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .find(|tier| tier.eq_ignore_ascii_case("fast"))
            .map(ToOwned::to_owned)
    })
}

pub(super) async fn fast_tier_for(
    transport: &Arc<CodexTransport>,
    cache: &Arc<Mutex<FastTierCatalogue>>,
    model: Option<&str>,
) -> Result<Option<String>, EngineError> {
    {
        let catalogue = lock(cache);
        if catalogue.loaded {
            return Ok(catalogue.tier_for(model));
        }
    }
    let response = transport
        .request(CodexMethod::MODEL_LIST, &json!({}))
        .await
        .into_result()?;
    let (_, catalogue) = model_catalog(&response);
    let tier = catalogue.tier_for(model);
    *lock(cache) = catalogue;
    Ok(tier)
}
