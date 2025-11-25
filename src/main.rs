#[macro_use]
extern crate rocket;

use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use dotenvy::dotenv;
use reqwest::Client;
use rocket::State;
use rocket::http::Status;
use rocket::response::status;
use rocket::serde::Serialize;
use rocket::serde::json::Json;

mod tfl;

use tfl::Arrival;

#[derive(Clone)]
struct ApiConfig {
    stop_id: String,
    app_id: Option<String>,
    app_key: Option<String>,
    cache_ttl: Duration,
}

impl ApiConfig {
    fn build_tfl_url(&self) -> String {
        let stop_id = &self.stop_id;
        let app_id = self.app_id.as_deref();
        let app_key = self.app_key.as_deref();
        tfl::build_tfl_url(stop_id, app_id, app_key)
    }
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

impl From<tfl::TflError> for ErrorResponse {
    fn from(err: tfl::TflError) -> Self {
        match err {
            tfl::TflError::UpstreamError(code, text) => ErrorResponse {
                error: "TFL_UPSTREAM_ERROR".into(),
                message: format!("TfL returned HTTP {}", code),
                details: Some(text),
            },
            tfl::TflError::ParseError(e) => ErrorResponse {
                error: "TFL_PARSE_ERROR".into(),
                message: "Failed to parse TfL response JSON".into(),
                details: Some(e.to_string()),
            },
        }
    }
}

impl From<reqwest::Error> for ErrorResponse {
    fn from(err: reqwest::Error) -> Self {
        ErrorResponse {
            error: "TFL_PARSE_ERROR".into(),
            message: "Failed to parse TfL response JSON".into(),
            details: Some(err.to_string()),
        }
    }
}

impl<T> From<PoisonError<T>> for ErrorResponse {
    fn from(_: PoisonError<T>) -> Self {
        ErrorResponse {
            error: "CACHE_LOCK_ERROR".into(),
            message: "Failed to acquire cache lock".into(),
            details: None,
        }
    }
}

async fn check_arrivals_cache(
    state: &State<AppState>,
) -> Result<Option<Vec<Arrival>>, status::Custom<Json<ErrorResponse>>> {
    let cache_guard = state
        .cache
        .lock()
        .map_err(|e| status::Custom(Status::InternalServerError, Json(ErrorResponse::from(e))))?;

    if let Some(cache) = cache_guard.as_ref() {
        if cache.last_fetch.elapsed() < state.config.cache_ttl {
            return Ok(Some(cache.data.clone()));
        }
    }

    Ok(None)
}

async fn update_arrivals_cache(
    state: &State<AppState>,
    arrivals: Vec<Arrival>,
) -> Result<(), status::Custom<Json<ErrorResponse>>> {
    let mut cache_guard = state
        .cache
        .lock()
        .map_err(|e| status::Custom(Status::InternalServerError, Json(ErrorResponse::from(e))))?;

    *cache_guard = Some(Cache {
        last_fetch: Instant::now(),
        data: arrivals.clone(),
    });

    Ok(())
}

async fn fetch_arrivals_from_tfl(
    state: &State<AppState>,
) -> Result<Vec<Arrival>, status::Custom<Json<ErrorResponse>>> {
    // Check Cache
    if let Some(cached) = check_arrivals_cache(state).await? {
        return Ok(cached);
    }

    // Fetch from TfL
    let arrivals = tfl::fetch_arrivals(&state.config, &state.client)
        .await
        .map_err(|e| status::Custom(Status::BadGateway, Json(ErrorResponse::from(e))))?;

    // Update Cache
    update_arrivals_cache(state, arrivals.clone()).await?;

    // Return arrivals
    Ok(arrivals)
}

fn filter_arrivals_by_route(arrivals: Vec<Arrival>, routes: &str) -> Vec<Arrival> {
    let routes_lc = routes.to_lowercase();
    let routes_split = routes_lc.split(',').collect::<Vec<&str>>();
    let routes_closure = |a: &Arrival| {
        routes_split
            .iter()
            .any(|r| a.line_name.trim().to_lowercase() == *r)
    };
    arrivals.into_iter().filter(routes_closure).collect()
}

#[get("/next-bus?<routes>")]
async fn next_bus(
    routes: Option<String>,
    state: &State<AppState>,
) -> Result<Json<Vec<Arrival>>, status::Custom<Json<ErrorResponse>>> {
    // Fetch arrivals
    let arrivals = fetch_arrivals_from_tfl(state).await?;

    // Filter by route if provided
    let mut filtered = if let Some(ref routes_str) = routes {
        filter_arrivals_by_route(arrivals, routes_str)
    } else {
        arrivals
    };

    // Construct empty response if no arrivals
    if filtered.is_empty() {
        return Err(status::Custom(
            Status::NoContent,
            Json(ErrorResponse {
                error: "NO_ARRIVALS".into(),
                message: format!(
                    "No upcoming buses found for stop {}{}",
                    state.config.stop_id,
                    routes
                        .as_ref()
                        .map(|r| format!(" on route(s) {}", r))
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

#[derive(Serialize)]
struct SummaryBus {
    route: String,
    destination: String,
    minutes: i64,
}

#[derive(Serialize)]
struct SummaryResponse {
    stop_id: String,
    stop_name: String,
    last_updated: String,
    services: Vec<SummaryBus>,
}

#[get("/next-bus/summary?<routes>&<limit>")]
async fn next_bus_summary(
    routes: Option<String>,
    limit: Option<usize>,
    state: &State<AppState>,
) -> Result<Json<SummaryResponse>, status::Custom<Json<ErrorResponse>>> {
    // Fetch arrivals
    let arrivals = fetch_arrivals_from_tfl(state).await?;

    // Filter by route if provided
    let mut filtered = if let Some(ref routes_str) = routes {
        filter_arrivals_by_route(arrivals, routes_str)
    } else {
        arrivals
    };

    // Construct empty response if no arrivals
    if filtered.is_empty() {
        return Ok(Json(SummaryResponse {
            stop_id: state.config.stop_id.clone(),
            stop_name: "NA".into(),
            last_updated: chrono::Utc::now().to_rfc3339(),
            services: vec![],
        }));
    }

    // Sort by soonest
    filtered.sort_by_key(|a| a.time_to_station);

    let limit = limit.unwrap_or(100);
    let services: Vec<SummaryBus> = filtered
        .iter()
        .take(limit)
        .map(|a| SummaryBus {
            route: a.line_name.clone(),
            destination: a.destination_name.clone(),
            minutes: (a.time_to_station / 60).max(0),
        })
        .collect();

    let first = &filtered[0];

    let resp = SummaryResponse {
        stop_id: first.naptan_id.clone(),
        stop_name: first.station_name.clone(),
        last_updated: first.timestamp.clone(),
        services,
    };

    Ok(Json(resp))
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

    rocket::build()
        .manage(state)
        .mount("/", routes![next_bus, next_bus_summary])
}
