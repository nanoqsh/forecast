use {
    askama_axum::Template,
    axum::{
        extract::{Query, State},
        http::StatusCode,
        response::{IntoResponse, Response},
        routing, Router, Server,
    },
    serde::Deserialize,
    sqlx::{FromRow, PgPool},
    std::net::SocketAddr,
};

#[derive(Template)]
#[template(path = "index.html")]
struct IndexView;

async fn index() -> IndexView {
    IndexView
}

#[derive(Deserialize)]
struct WeatherQuery {
    city: String,
}

async fn weather(
    Query(WeatherQuery { city }): Query<WeatherQuery>,
    State(pool): State<PgPool>,
) -> Result<WeatherView, Error> {
    let ll = get_lat_long(&pool, &city).await?;
    let weather = fetch_weather(ll).await.ok_or(Error::FetchWeather)?;
    Ok(WeatherView::new(city, weather))
}

async fn stats() -> &'static str {
    "Stats"
}

#[tokio::main]
async fn main() {
    const DATABASE_URL: &str =
        "postgres://forecast:forecast@localhost:5432/forecast?sslmode=disable";

    let pool = match PgPool::connect(DATABASE_URL).await {
        Ok(pool) => pool,
        Err(err) => {
            eprintln!("failed to connect to the database: {err}");
            return;
        }
    };

    if let Err(err) = run_migration(&pool).await {
        eprintln!("failed to init database: {err}");
        return;
    }

    let app = Router::new()
        .route("/", routing::get(index))
        .route("/weather", routing::get(weather))
        .route("/stats", routing::get(stats))
        .with_state(pool);

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .expect("start server");
}

async fn run_migration(pool: &PgPool) -> Result<(), sqlx::Error> {
    const CREATE_CITIES: &str = "
        CREATE TABLE IF NOT EXISTS cities (
            id SERIAL PRIMARY KEY,
            name TEXT NOT NULL,
            lat FLOAT8 NOT NULL,
            long FLOAT8 NOT NULL
        );
    ";

    const CREATE_CITIES_INDEX: &str =
        "CREATE INDEX IF NOT EXISTS cities_name_idx ON cities (name);";

    for sql in [CREATE_CITIES, CREATE_CITIES_INDEX] {
        sqlx::query(sql).execute(pool).await?;
    }

    Ok(())
}

async fn fetch_lat_long(city: &str) -> Option<LatLong> {
    let endpoint = format!(
        "https://geocoding-api.open-meteo.com/v1/search?name={}&count=1&language=en&format=json",
        city,
    );

    let res: GeoResponse = reqwest::get(&endpoint).await.ok()?.json().await.ok()?;
    res.results.into_iter().next()
}

async fn get_lat_long(pool: &PgPool, name: &str) -> Result<LatLong, Error> {
    let ll =
        sqlx::query_as("SELECT lat AS latitude, long AS longitude FROM cities WHERE name = $1")
            .bind(name)
            .fetch_optional(pool)
            .await?;

    if let Some(ll) = ll {
        return Ok(ll);
    }

    let ll = fetch_lat_long(name).await.ok_or(Error::NoResultsFound)?;
    sqlx::query("INSERT INTO cities (name, lat, long) VALUES ($1, $2, $3)")
        .bind(name)
        .bind(ll.latitude)
        .bind(ll.longitude)
        .execute(pool)
        .await?;

    Ok(ll)
}

async fn fetch_weather(
    LatLong {
        latitude,
        longitude,
    }: LatLong,
) -> Option<WeatherResponse> {
    let endpoint = format!("https://api.open-meteo.com/v1/forecast?latitude={latitude}&longitude={longitude}&hourly=temperature_2m");
    reqwest::get(&endpoint).await.ok()?.json().await.ok()
}

#[derive(Deserialize)]
struct GeoResponse {
    results: Vec<LatLong>,
}

#[derive(Deserialize, FromRow)]
struct LatLong {
    latitude: f64,
    longitude: f64,
}

#[derive(Deserialize)]
struct WeatherResponse {
    latitude: f64,
    longitude: f64,
    timezone: String,
    hourly: Hourly,
}

#[derive(Deserialize)]
struct Hourly {
    time: Vec<String>,
    temperature_2m: Vec<f64>,
}

#[derive(Template)]
#[template(path = "weather.html")]
struct WeatherView {
    city: String,
    forecasts: Vec<Forecast>,
}

impl WeatherView {
    fn new(city: String, response: WeatherResponse) -> Self {
        Self {
            city,
            forecasts: response
                .hourly
                .time
                .into_iter()
                .zip(response.hourly.temperature_2m)
                .map(|(date, temperature)| Forecast { date, temperature })
                .collect(),
        }
    }
}

#[derive(Deserialize)]
struct Forecast {
    date: String,
    temperature: f64,
}

enum Error {
    NoResultsFound,
    FetchWeather,
    Database(sqlx::Error),
}

impl From<sqlx::Error> for Error {
    fn from(v: sqlx::Error) -> Self {
        Self::Database(v)
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let (code, payload) = match self {
            Self::NoResultsFound => (StatusCode::NOT_FOUND, "no results found"),
            Self::FetchWeather => (StatusCode::METHOD_NOT_ALLOWED, "failed to fetch weather"),
            Self::Database(err) => {
                eprintln!("database error: {err}");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal server error")
            }
        };

        (code, payload).into_response()
    }
}
