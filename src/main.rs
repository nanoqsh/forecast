use {
    askama_axum::Template,
    axum::{
        extract::{FromRequestParts, Query, State},
        headers::{authorization::Basic, Authorization},
        http::{header, request::Parts, StatusCode},
        response::{IntoResponse, Response},
        routing, Router, Server, TypedHeader,
    },
    serde::Deserialize,
    sqlx::{Executor, FromRow, PgPool},
    std::net::SocketAddr,
};

#[tokio::main]
async fn main() {
    let app = match App::connect().await {
        Ok(app) => app,
        Err(err) => {
            eprintln!("database error: {err}");
            return;
        }
    };

    let router = Router::new()
        .route("/", routing::get(index))
        .route("/weather", routing::get(weather))
        .route("/stats", routing::get(stats))
        .with_state(app);

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    let server = Server::bind(&addr);
    if let Err(err) = server.serve(router.into_make_service()).await {
        eprintln!("server error: {err}");
    }
}

#[derive(Clone)]
struct App {
    pool: PgPool,
}

impl App {
    async fn connect() -> Result<Self, sqlx::Error> {
        const DATABASE_URL: &str =
            "postgres://forecast:forecast@localhost:5432/forecast?sslmode=disable";

        let pool = PgPool::connect(DATABASE_URL).await?;
        pool.execute(include_str!("../schema.sql")).await?;
        Ok(Self { pool })
    }
}

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
    State(app): State<App>,
) -> Result<WeatherView, Error> {
    let ll = get_lat_long(&app.pool, &city).await?;
    let weather = fetch_weather(ll).await.ok_or(Error::FetchWeather)?;
    Ok(WeatherView::new(city, weather))
}

async fn stats(_: User) -> &'static str {
    "We're authorized!"
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
    Unauthorized,
    Database(sqlx::Error),
}

impl From<sqlx::Error> for Error {
    fn from(v: sqlx::Error) -> Self {
        Self::Database(v)
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        const AUTH_SCHEME_VALUE: &str = "Basic realm=\"Please enter your credentials\"";

        match self {
            Self::NoResultsFound => (StatusCode::NOT_FOUND, "no results found").into_response(),
            Self::FetchWeather => {
                (StatusCode::METHOD_NOT_ALLOWED, "failed to fetch weather").into_response()
            }
            Self::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                [(header::WWW_AUTHENTICATE, AUTH_SCHEME_VALUE)],
                "unauthorized",
            )
                .into_response(),
            Self::Database(err) => {
                eprintln!("database error: {err}");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response()
            }
        }
    }
}

struct User;

#[async_trait::async_trait]
impl FromRequestParts<App> for User {
    type Rejection = Error;

    async fn from_request_parts(parts: &mut Parts, app: &App) -> Result<Self, Self::Rejection> {
        let auth: TypedHeader<Authorization<Basic>> = TypedHeader::from_request_parts(parts, app)
            .await
            .map_err(|_| Error::Unauthorized)?;

        match (auth.username(), auth.password()) {
            ("forecast", "forecast") => Ok(User),
            _ => Err(Error::Unauthorized),
        }
    }
}
