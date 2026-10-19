//! Agreement IPC reconstructed from original 1.44.0 runtime and isolated SQLite evidence.
use crate::storage::Store;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const VERSION: &str = "2026-10-03.1";
const SETTING: &str = "agreement_acceptance";

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Status {
    version: String,
    required: bool,
    accepted_version: Option<String>,
    accepted_at: Option<String>,
}

pub fn status(store: &Store) -> Result<Value, String> {
    let previous = store.setting(SETTING)?
        .and_then(|value| serde_json::from_str::<Status>(&value).ok());
    let (accepted_version, accepted_at) = previous
        .map(|value| (value.accepted_version, value.accepted_at))
        .unwrap_or((None, None));
    serde_json::to_value(Status {
        version: VERSION.into(),
        required: accepted_version.as_deref() != Some(VERSION),
        accepted_version,
        accepted_at,
    }).map_err(|error| error.to_string())
}

pub fn accept(store: &Store, args: &Value) -> Result<Value, String> {
    let input = args.get("version").ok_or_else(||
        "invalid args `version` for command `accept_agreement`: command accept_agreement missing required key version".to_owned())?;
    let version: String = serde_json::from_value(input.clone()).map_err(|error|
        format!("invalid args `version` for command `accept_agreement`: {error}"))?;
    if version != VERSION {
        return Err("Редакция соглашения изменилась. Откройте актуальный текст.".into());
    }
    let previous = status(store)?;
    if previous["required"] == false {
        return Ok(previous);
    }
    let accepted = Status {
        version: VERSION.into(),
        required: false,
        accepted_version: Some(VERSION.into()),
        accepted_at: Some(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Nanos, false)),
    };
    let encoded = serde_json::to_string(&accepted).map_err(|error| error.to_string())?;
    store.set_setting(SETTING, &encoded)?;
    serde_json::to_value(accepted).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn acceptance_validates_version_and_persists_idempotently() {
        let dir = std::env::temp_dir().join(format!("sfery-agreement-test-{}", uuid::Uuid::new_v4()));
        let store = Store::new(&dir).unwrap();
        let initial = json!({"version":VERSION,"required":true,"accepted_version":null,"accepted_at":null});
        assert_eq!(status(&store).unwrap(), initial);
        for invalid in ["wrong", " 2026-10-03.1 "] {
            assert_eq!(accept(&store, &json!({"version":invalid})).unwrap_err(),
                "Редакция соглашения изменилась. Откройте актуальный текст.");
        }
        assert_eq!(accept(&store, &json!({})).unwrap_err(),
            "invalid args `version` for command `accept_agreement`: command accept_agreement missing required key version");
        assert_eq!(accept(&store, &json!({"version":123})).unwrap_err(),
            "invalid args `version` for command `accept_agreement`: invalid type: integer `123`, expected a string");
        assert_eq!(status(&store).unwrap(), initial);
        let accepted = accept(&store, &json!({"version":VERSION})).unwrap();
        assert_eq!(accepted["required"], false);
        assert_eq!(accepted["accepted_version"], VERSION);
        let timestamp = accepted["accepted_at"].as_str().unwrap();
        assert!(timestamp.ends_with("+00:00"));
        chrono::DateTime::parse_from_rfc3339(timestamp).unwrap();
        assert_eq!(accept(&store, &json!({"version":VERSION})).unwrap(), accepted);
        drop(store);
        let reopened = Store::new(&dir).unwrap();
        assert_eq!(status(&reopened).unwrap(), accepted);
        assert_eq!(serde_json::from_str::<Value>(&reopened.setting(SETTING).unwrap().unwrap()).unwrap(), accepted);
        drop(reopened);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
