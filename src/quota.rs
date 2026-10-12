//! Account quota snapshots, separate from tokens consumed by an individual chat.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UsageWindow {
    pub label: String,
    pub remaining_percent: f64,
    pub resets_at: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccountUsage {
    pub plan: Option<String>,
    pub windows: Vec<UsageWindow>,
    pub checked_at: u64,
}

impl AccountUsage {
    pub fn parse_claude(payload: &Value) -> Result<Self, String> {
        if payload["rate_limits_available"] != true {
            return Err("Claude no informó límites de suscripción para esta cuenta.".into());
        }
        let limits = &payload["rate_limits"];
        let mut windows = Vec::new();
        for (key, label) in [
            ("five_hour", "5 horas"),
            ("seven_day", "Semanal"),
            ("seven_day_opus", "Semanal · Opus"),
            ("seven_day_sonnet", "Semanal · Sonnet"),
            ("seven_day_oauth_apps", "Semanal · Aplicaciones"),
        ] {
            let Some(used) = limits[key]["utilization"]
                .as_f64()
                .filter(|n| n.is_finite() && (0.0..=100.0).contains(n))
            else {
                continue;
            };
            let resets_at = limits[key]["resets_at"]
                .as_str()
                .and_then(|value| {
                    time::OffsetDateTime::parse(
                        value,
                        &time::format_description::well_known::Rfc3339,
                    )
                    .ok()
                })
                .and_then(|at| u64::try_from(at.unix_timestamp()).ok());
            windows.push(UsageWindow {
                label: label.into(),
                remaining_percent: 100.0 - used,
                resets_at,
            });
        }
        if windows.is_empty() {
            return Err("Claude no informó porcentajes de cuota disponibles.".into());
        }
        Ok(Self {
            plan: payload["subscription_type"].as_str().map(str::to_string),
            windows,
            checked_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        })
    }
    pub fn parse(payload: &Value) -> Result<Self, String> {
        let windows = ["primary_window", "secondary_window"]
            .into_iter()
            .filter_map(|key| {
                let window = &payload["rate_limit"][key];
                let used = window["used_percent"].as_f64()?;
                if !used.is_finite() || !(0.0..=100.0).contains(&used) {
                    return None;
                }
                let label = match window["limit_window_seconds"].as_u64() {
                    Some(18000) => "5 horas".into(),
                    Some(604800) => "Semanal".into(),
                    Some(seconds) if seconds > 0 => format!("Ventana de {seconds} s"),
                    _ => if key == "primary_window" {
                        "Principal"
                    } else {
                        "Secundaria"
                    }
                    .into(),
                };
                Some(UsageWindow {
                    label,
                    remaining_percent: 100.0 - used,
                    resets_at: window["reset_at"].as_u64(),
                })
            })
            .collect::<Vec<_>>();
        if windows.is_empty() {
            return Err("La cuenta no informó porcentajes de cuota disponibles.".into());
        }
        Ok(Self {
            plan: payload["plan_type"].as_str().map(str::to_owned),
            windows,
            checked_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        })
    }
}

impl UsageWindow {
    pub fn caption(&self, checked_at: u64) -> String {
        let reset = match self.resets_at {
            Some(at) if at > checked_at => {
                let minutes = (at - checked_at).div_ceil(60);
                if minutes >= 1440 {
                    format!(
                        " · reinicia en {}d {}h",
                        minutes / 1440,
                        minutes % 1440 / 60
                    )
                } else {
                    format!(" · reinicia en {}h {}m", minutes / 60, minutes % 60)
                }
            }
            Some(_) => " · reinicio pendiente".into(),
            None => String::new(),
        };
        format!(
            "{} · {:.1}% libre{reset}",
            self.label, self.remaining_percent
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn claude_percentages_are_not_fractions_and_missing_windows_stay_unknown() {
        let usage = AccountUsage::parse_claude(
            &json!({"subscription_type":"pro","rate_limits_available":true,"rate_limits":{
                "five_hour":{"utilization":0.5,"resets_at":"2026-10-09T23:00:00Z"},
                "seven_day":{"utilization":92.0,"resets_at":null},
                "seven_day_opus":{"utilization":null},"seven_day_sonnet":{"utilization":101}
            }}),
        )
        .unwrap();
        assert_eq!(usage.windows.len(), 2);
        assert_eq!(usage.windows[0].remaining_percent, 99.5);
        assert_eq!(usage.windows[0].resets_at, Some(1791586800));
        assert_eq!(usage.windows[1].remaining_percent, 8.0);
        assert_eq!(usage.windows[1].resets_at, None);
        assert!(AccountUsage::parse_claude(&json!({"rate_limits_available":false})).is_err());
    }
    #[test]
    fn quota_uses_reported_duration_and_percent_without_guessing_missing_values() {
        let quota = AccountUsage::parse(&json!({"plan_type":"pro", "rate_limit":{
            "primary_window":{"used_percent":23.5,"limit_window_seconds":604800,"reset_at":2000000000},
            "secondary_window":{"limit_window_seconds":18000}}})).unwrap();
        assert_eq!(quota.windows.len(), 1);
        assert_eq!(quota.windows[0].label, "Semanal");
        assert_eq!(quota.windows[0].remaining_percent, 76.5);
        assert!(
            AccountUsage::parse(&json!({"rate_limit":{"primary_window":{"used_percent":101}}}))
                .is_err()
        );
        assert!(AccountUsage::parse(&json!({})).is_err());
        let empty =
            AccountUsage::parse(&json!({"rate_limit":{"primary_window":{"used_percent":100}}}))
                .unwrap();
        assert_eq!(empty.windows[0].remaining_percent, 0.0);
    }
}
