use std::time::Duration;
use thiserror::Error;

use reqwest::{Client, StatusCode};
use rocket::serde::{Deserialize, Serialize};

use crate::ApiConfig;

pub fn build_tfl_url(stop_id: &str, app_id: Option<&str>, app_key: Option<&str>) -> String {
    let mut base = format!(
        "https://api.tfl.gov.uk/StopPoint/{}/Arrivals",
        urlencoding::encode(stop_id)
    );

    let mut params = vec![];
    if let Some(app_id) = app_id {
        params.push(format!("app_id={}", urlencoding::encode(app_id)));
    }
    if let Some(app_key) = app_key {
        params.push(format!("app_key={}", urlencoding::encode(app_key)));
    }
    if !params.is_empty() {
        base.push('?');
        base.push_str(&params.join("&"));
    }

    base
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Timing {
    #[serde(rename = "countdownServerAdjustment")]
    pub countdown_server_adjustment: String,
    pub source: String,
    pub insert: String,
    pub read: String,
    pub sent: String,
    pub received: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Arrival {
    pub id: String,
    #[serde(rename = "operationType")]
    pub operation_type: i32,
    #[serde(rename = "vehicleId")]
    pub vehicle_id: String,
    #[serde(rename = "naptanId")]
    pub naptan_id: String,
    #[serde(rename = "stationName")]
    pub station_name: String,
    #[serde(rename = "lineId")]
    pub line_id: String,
    #[serde(rename = "lineName")]
    pub line_name: String,
    #[serde(rename = "platformName")]
    pub platform_name: String,
    pub direction: String,
    pub bearing: String,
    #[serde(rename = "tripId")]
    pub trip_id: String,
    #[serde(rename = "baseVersion")]
    pub base_version: String,
    #[serde(rename = "destinationNaptanId")]
    pub destination_naptan_id: String,
    #[serde(rename = "destinationName")]
    pub destination_name: String,
    pub timestamp: String,
    #[serde(rename = "timeToStation")]
    pub time_to_station: i64,
    #[serde(rename = "currentLocation")]
    pub current_location: String,
    pub towards: String,
    #[serde(rename = "expectedArrival")]
    pub expected_arrival: String,
    #[serde(rename = "timeToLive")]
    pub time_to_live: String,
    #[serde(rename = "modeName")]
    pub mode_name: String,
    pub timing: Timing,
}

#[derive(Error, Debug)]
pub enum TflError {
    #[error("Upstream error")]
    UpstreamError(StatusCode, String),
    #[error("Parse error")]
    ParseError(#[from] reqwest::Error),
}

pub async fn fetch_arrivals(config: &ApiConfig, client: &Client) -> Result<Vec<Arrival>, TflError> {
    let url = config.build_tfl_url();

    let resp = client
        .get(&url)
        .timeout(Duration::from_secs(5))
        .send()
        .await?;

    if !resp.status().is_success() {
        let code = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(TflError::UpstreamError(code, text));
    }

    let arrivals: Vec<Arrival> = resp.json().await?;

    Ok(arrivals)
}
