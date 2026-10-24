use std::net::SocketAddr;
use std::time::Duration;

use axum::extract::DefaultBodyLimit;
use axum::http::{header, Method};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use tl360_backend::{
    mailer::Mailer, push::PushSender, recordatorios, resumen_semanal, routes, state::AppState,
};

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let database_url = std::env::var("DATABASE_URL").expect("falta DATABASE_URL en .env");
    let jwt_secret = std::env::var("JWT_SECRET").unwrap_or_default();
    let produccion = std::env::var("APP_ENV").is_ok_and(|v| v == "production");
    let secreto_debil = jwt_secret.len() < 32 || jwt_secret == "supersecreto";
    if secreto_debil {
        // Con un secreto conocido cualquiera puede firmar un token de superadmin.
        assert!(
            !produccion,
            "JWT_SECRET falta o es débil (mínimo 32 caracteres): no se arranca en producción"
        );
        tracing::warn!("JWT_SECRET débil o ausente: solo válido para desarrollo. Definí uno largo y aleatorio antes de desplegar.");
    }
    let jwt_secret = if jwt_secret.is_empty() {
        "supersecreto".to_string()
    } else {
        jwt_secret
    };
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3001);

    let opciones: PgConnectOptions = database_url
        .parse()
        .expect("DATABASE_URL inválida (formato: postgres://usuario:clave@host:puerto/base)");
    let destino = format!("{}:{}", opciones.get_host(), opciones.get_port());
    // El pooler de transacciones de Supabase (pgbouncer, puerto 6543) no admite sentencias preparadas
    // con nombre: sin esto las consultas fallan con "prepared statement already exists".
    let via_pooler = opciones.get_port() == 6543 || database_url.contains("pgbouncer=true");
    let opciones = if via_pooler {
        opciones.statement_cache_capacity(0)
    } else {
        opciones
    };

    let pool_opts = PgPoolOptions::new()
        .max_connections(
            std::env::var("DB_MAX_CONNECTIONS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(10),
        )
        .acquire_timeout(Duration::from_secs(15));

    // Reintenta: en Render/Supabase la BD puede tardar unos segundos en aceptar conexiones (cold start).
    let mut db = None;
    for intento in 1..=5 {
        match pool_opts.clone().connect_with(opciones.clone()).await {
            Ok(p) => {
                db = Some(p);
                break;
            }
            Err(e) => {
                tracing::error!("no se pudo conectar a la base de datos en {destino} (intento {intento}/5): {e}");
                if intento < 5 {
                    tokio::time::sleep(Duration::from_secs(3)).await;
                }
            }
        }
    }
    let db = db.unwrap_or_else(|| {
        panic!(
            "no se pudo conectar a la base de datos en {destino}. Revisá que esté encendida y que DATABASE_URL \
             sea correcta. En Supabase, el host directo (db.<ref>.supabase.co) es solo IPv6 y Render no lo alcanza: \
             usá la cadena del pooler (aws-0-<región>.pooler.supabase.com)."
        )
    });

    // Las migraciones usan un lock de sesión que el pooler de transacciones no soporta: si hace falta,
    // definí MIGRATE_DATABASE_URL (conexión directa o session pooler) o RUN_MIGRATIONS=false.
    if std::env::var("RUN_MIGRATIONS").map_or(true, |v| v != "false") {
        let migrar_con = match std::env::var("MIGRATE_DATABASE_URL") {
            Ok(url) => PgPoolOptions::new()
                .max_connections(1)
                .acquire_timeout(Duration::from_secs(15))
                .connect(&url)
                .await
                .expect("no se pudo conectar con MIGRATE_DATABASE_URL"),
            Err(_) => db.clone(),
        };
        sqlx::migrate!("./migrations")
            .run(&migrar_con)
            .await
            .unwrap_or_else(|e| panic!("error corriendo migraciones: {e}"));
    }

    let frontend_url =
        std::env::var("FRONTEND_URL").unwrap_or_else(|_| "http://localhost:5173".into());
    let mut state = AppState::new(db, jwt_secret, frontend_url);
    state.trust_proxy = std::env::var("TRUST_PROXY").is_ok_and(|v| v == "true");
    state.mailer = Mailer::desde_entorno();
    state.push = PushSender::desde_entorno();
    if state.push.configurado() {
        recordatorios::bucle(state.clone());
        resumen_semanal::bucle(state.clone());
    }

    // Solo el front puede llamar a la API desde un navegador (CORS_ORIGINS admite varios, separados por coma).
    let origenes: Vec<axum::http::HeaderValue> = std::env::var("CORS_ORIGINS")
        .unwrap_or_else(|_| state.frontend_url.clone())
        .split(',')
        .filter_map(|o| o.trim().parse().ok())
        .collect();
    let cors = CorsLayer::new()
        .allow_origin(origenes)
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION]);

    let app = routes::build(state)
        .layer(DefaultBodyLimit::max(256 * 1024))
        .layer(TraceLayer::new_for_http())
        .layer(cors);

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
        .await
        .unwrap();
    tracing::info!("✅ Servidor escuchando en http://localhost:{port}");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .unwrap();
}
