#[macro_use]
extern crate rocket;

use std::sync::Mutex;
use std::time::{Duration, Instant};

use dotenvy::dotenv;
use reqwest::Client;
use rocket::State;
use rocket::http::Status;
use rocket::response::status;
use rocket::serde::json::Json;
use rocket::serde::{Deserialize, Serialize};

#[derive(Clone)]
struct ApiConfig {
    stop_id: String,
    app_id: Option<String>,
    app_key: Option<String>,
    cache_ttl: Duration,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Timing {
    #[serde(rename = "countdownServerAdjustment")]
    pub countdown_server_adjustment: String,
    pub source: String,
    pub insert: String,
    pub read: String,
    pub sent: String,
    pub received: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Arrival {
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

struct Cache {
    last_fetch: Instant,
    data: Vec<Arrival>,
}

struct AppState {
    client: Client,
    config: ApiConfig,
    cache: Mutex<Option<Cache>>,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
    message: String,
    details: Option<String>,
}

fn build_tfl_url(config: &ApiConfig) -> String {
    let mut base = format!(
        "https://api.tfl.gov.uk/StopPoint/{}/Arrivals",
        urlencoding::encode(&config.stop_id)
    );

    let mut params = vec![];
    if let Some(app_id) = &config.app_id {
        params.push(format!("app_id={}", urlencoding::encode(app_id)));
    }
    if let Some(app_key) = &config.app_key {
        params.push(format!("app_key={}", urlencoding::encode(app_key)));
    }
    if !params.is_empty() {
        base.push('?');
        base.push_str(&params.join("&"));
    }

    base
}

async fn fetch_arrivals(
    state: &AppState,
) -> Result<Vec<Arrival>, status::Custom<Json<ErrorResponse>>> {
    {
        // check cache first
        let cache_guard = state.cache.lock().map_err(|_| {
            status::Custom(
                Status::InternalServerError,
                Json(ErrorResponse {
                    error: "CACHE_LOCK_ERROR".into(),
                    message: "Failed to acquire cache lock".into(),
                    details: None,
                }),
            )
        })?;

        if let Some(cache) = cache_guard.as_ref() {
            if cache.last_fetch.elapsed() < state.config.cache_ttl {
                return Ok(cache.data.clone());
            }
        }
    }

    let url = build_tfl_url(&state.config);

    let resp = state
        .client
        .get(&url)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| {
            status::Custom(
                Status::BadGateway,
                Json(ErrorResponse {
                    error: "TFL_UPSTREAM_ERROR".into(),
                    message: "TfL API request failed".into(),
                    details: Some(e.to_string()),
                }),
            )
        })?;

    if !resp.status().is_success() {
        let code = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(status::Custom(
            Status::BadGateway,
            Json(ErrorResponse {
                error: "TFL_UPSTREAM_ERROR".into(),
                message: format!("TfL returned HTTP {}", code),
                details: Some(text),
            }),
        ));
    }

    let arrivals: Vec<Arrival> = resp.json().await.map_err(|e| {
        status::Custom(
            Status::BadGateway,
            Json(ErrorResponse {
                error: "TFL_PARSE_ERROR".into(),
                message: "Failed to parse TfL response JSON".into(),
                details: Some(e.to_string()),
            }),
        )
    })?;

    {
        let mut cache_guard = state.cache.lock().map_err(|_| {
            status::Custom(
                Status::InternalServerError,
                Json(ErrorResponse {
                    error: "CACHE_LOCK_ERROR".into(),
                    message: "Failed to acquire cache lock".into(),
                    details: None,
                }),
            )
        })?;

        *cache_guard = Some(Cache {
            last_fetch: Instant::now(),
            data: arrivals.clone(),
        });
    }

    Ok(arrivals)
}

#[get("/next-bus?<route>")]
async fn next_bus(
    route: Option<String>,
    state: &State<AppState>,
) -> Result<Json<Vec<Arrival>>, status::Custom<Json<ErrorResponse>>> {
    let arrivals = fetch_arrivals(&state).await?;

    // Filter by line_name if route specified
    let mut filtered: Vec<Arrival> = if let Some(ref route_filter) = route {
        let rf = route_filter.to_lowercase();
        arrivals
            .into_iter()
            .filter(|a| a.line_name.to_lowercase() == rf)
            .collect()
    } else {
        arrivals
    };

    if filtered.is_empty() {
        return Err(status::Custom(
            Status::ServiceUnavailable,
            Json(ErrorResponse {
                error: "NO_ARRIVALS".into(),
                message: format!(
                    "No upcoming buses found for stop {}{}",
                    state.config.stop_id,
                    route
                        .as_ref()
                        .map(|r| format!(" on route {}", r))
                        .unwrap_or_default()
                ),
                details: None,
            }),
        ));
    }

    // Sort by soonest
    filtered.sort_by_key(|a| a.time_to_station);

    Ok(Json(filtered))
}

fn load_config() -> ApiConfig {
    use std::env;

    dotenv().ok();

    let stop_id = env::var("TFL_STOP_ID").expect("TFL_STOP_ID must be set (TfL StopPoint id)");

    let app_id = env::var("TFL_APP_ID").ok().filter(|s| !s.is_empty());
    let app_key = env::var("TFL_APP_KEY").ok().filter(|s| !s.is_empty());

    ApiConfig {
        stop_id,
        app_id,
        app_key,
        cache_ttl: Duration::from_secs(10),
    }
}

#[launch]
fn rocket() -> _ {
    let config = load_config();

    let client = Client::builder()
        .user_agent("lx-tfl-api/0.1")
        .build()
        .expect("Failed to build HTTP client");

    let state = AppState {
        client,
        config,
        cache: Mutex::new(None),
    };

    rocket::build().manage(state).mount("/", routes![next_bus])
}
