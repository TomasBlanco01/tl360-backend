//! Pruebas de integración de la API. Cada test recibe una base de datos PostgreSQL nueva (con las
//! migraciones aplicadas) gracias a `#[sqlx::test]`, y llama a los endpoints reales a través del router
//! de Axum, sin abrir un puerto. Requiere `DATABASE_URL` apuntando a un servidor donde se puedan crear
//! bases (ver README): `DATABASE_URL=postgres://... cargo test`.

use axum::{
    body::Body,
    http::{header, Request, StatusCode},
    Router,
};
use chrono::{Datelike, Duration, NaiveDate, NaiveTime, Weekday};
use http_body_util::BodyExt;
use jsonwebtoken::{encode, EncodingKey, Header};
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;

use tl360_backend::agenda::{ahora_local, nombre_dia};
use tl360_backend::auth_extractor::{decode_token, Claims};
use tl360_backend::{recordatorios, routes, state::AppState};

const SECRETO: &str = "secreto-de-pruebas-de-integracion-0123456789";
/// Clave pública P-256 (65 bytes) y secreto auth (16 bytes) de ejemplo, en base64url.
const PUSH_P256DH: &str =
    "BMjQIp55pdbU8pfCBKyXcZjlmER_mXt5LqNrN1hrXbdBS5EnhIbMu3Au-RV53iIpztzNXkGI56BFB1udQ8Bq_H4";
const PUSH_AUTH: &str = "xS03Fi5ErfTNH_l9WHE9Ig";

// ------------------------------------------------------------------ helpers

fn app(pool: PgPool) -> Router {
    routes::build(AppState::new(
        pool,
        SECRETO.into(),
        "http://localhost:5173".into(),
    ))
}

async fn enviar(
    app: &Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<String>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    if body.is_some() {
        req = req.header(header::CONTENT_TYPE, "application/json");
    }
    let req = req.body(body.map_or_else(Body::empty, Body::from)).unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    enviar(app, method, uri, token, body.map(|b| b.to_string())).await
}

fn token_de(id: i32, rol: &str, secreto: &str) -> String {
    let claims = Claims {
        id,
        nombre: format!("u{id}"),
        rol: rol.into(),
        exp: (chrono::Utc::now().timestamp() + 3600) as usize,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secreto.as_bytes()),
    )
    .unwrap()
}

struct Usuario {
    id: i32,
    token: String,
}

/// Usuarios de prueba ya verificados (RF-01): equivalen a cuentas que confirmaron su correo.
/// El flujo de verificación sin verificar se prueba aparte con el registro real.
async fn crear_usuario(pool: &PgPool, email: &str, rol: &str) -> Usuario {
    crear_usuario_verificacion(pool, email, rol, true).await
}

async fn crear_usuario_verificacion(
    pool: &PgPool,
    email: &str,
    rol: &str,
    verificado: bool,
) -> Usuario {
    let hash = bcrypt::hash("clave123", 4).unwrap();
    let id: i32 = sqlx::query_scalar(
        r#"INSERT INTO usuario (nombre, email, contrasena_hash, rol, email_verificado) VALUES ($1, $2, $3, $4, $5) RETURNING id"#,
    )
    .bind(format!("Usuario {email}"))
    .bind(email)
    .bind(hash)
    .bind(rol)
    .bind(verificado)
    .fetch_one(pool)
    .await
    .unwrap();
    Usuario {
        id,
        token: token_de(id, rol, SECRETO),
    }
}

async fn crear_rubro(pool: &PgPool, nombre: &str) -> i32 {
    sqlx::query_scalar("INSERT INTO rubro (nombre) VALUES ($1) RETURNING id")
        .bind(nombre)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn crear_empresa(
    pool: &PgPool,
    nombre: &str,
    rubro: i32,
    admin: Option<i32>,
    turnos: bool,
    duracion: Option<i32>,
) -> i32 {
    sqlx::query_scalar(
        "INSERT INTO empresa (nombre, direccion, rubro_id, usuario_id, turnos_habilitados, duracion_turno_min, descripcion) \
         VALUES ($1, 'Calle 123', $2, $3, $4, $5, 'Descripción larga de prueba') RETURNING id",
    )
    .bind(nombre)
    .bind(rubro)
    .bind(admin)
    .bind(turnos)
    .bind(duracion)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn poner_horario(pool: &PgPool, empresa: i32, dia: &str, ini: (u32, u32), fin: (u32, u32)) {
    sqlx::query("INSERT INTO horario_empresa (empresa_id, dia_semana, hora_inicio, hora_fin) VALUES ($1, $2, $3, $4)")
        .bind(empresa)
        .bind(dia)
        .bind(NaiveTime::from_hms_opt(ini.0, ini.1, 0).unwrap())
        .bind(NaiveTime::from_hms_opt(fin.0, fin.1, 0).unwrap())
        .execute(pool)
        .await
        .unwrap();
}

/// Próxima fecha con ese día de la semana, al menos `desde_dias` días hacia adelante (hora local).
fn fecha(dia: Weekday, desde_dias: i64) -> NaiveDate {
    let mut f = ahora_local().date() + Duration::days(desde_dias);
    while f.weekday() != dia {
        f += Duration::days(1);
    }
    f
}

/// Comercio con turnos: martes y miércoles 09:00-12:00 y 16:00-20:00, turnos de 30 min.
struct Escenario {
    app: Router,
    pool: PgPool,
    cliente: Usuario,
    dueno: Usuario,
    superadmin: Usuario,
    rubro: i32,
    empresa: i32,
}

async fn escenario(pool: PgPool) -> Escenario {
    let cliente = crear_usuario(&pool, "cliente@test.com", "cliente").await;
    let dueno = crear_usuario(&pool, "dueno@test.com", "admin").await;
    let superadmin = crear_usuario(&pool, "super@test.com", "superadmin").await;
    let rubro = crear_rubro(&pool, "Peluquerías").await;
    let empresa = crear_empresa(
        &pool,
        "Peluquería Central",
        rubro,
        Some(dueno.id),
        true,
        Some(30),
    )
    .await;
    for dia in ["Martes", "Miércoles"] {
        poner_horario(&pool, empresa, dia, (9, 0), (12, 0)).await;
        poner_horario(&pool, empresa, dia, (16, 0), (20, 0)).await;
    }
    Escenario {
        app: app(pool.clone()),
        pool,
        cliente,
        dueno,
        superadmin,
        rubro,
        empresa,
    }
}

fn reserva(empresa: i32, f: NaiveDate, hora: &str) -> Value {
    json!({ "empresa_id": empresa, "fecha": f.to_string(), "hora": hora })
}

// ------------------------------------------------------------------ autenticación

#[sqlx::test(migrations = "./migrations")]
async fn el_registro_publico_no_permite_autoasignarse_superadmin(pool: PgPool) {
    let app = app(pool.clone());
    let (st, _) = call(&app, "POST", "/api/auth/register", None, Some(json!({
        "nombre": "Intruso", "email": "intruso@test.com", "contraseña": "Clave1234", "rol": "superadmin"
    }))).await;
    assert_eq!(st, StatusCode::CREATED);
    let rol: String =
        sqlx::query_scalar("SELECT rol FROM usuario WHERE email = 'intruso@test.com'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(rol, "cliente");
}

#[sqlx::test(migrations = "./migrations")]
async fn el_registro_publico_crea_administradores_de_comercio(pool: PgPool) {
    let app = app(pool.clone());
    let (st, _) = call(&app, "POST", "/api/auth/register", None, Some(json!({
        "nombre": "Dueña", "email": "duena@test.com", "contraseña": "Clave1234", "rol": "comercio"
    }))).await;
    assert_eq!(st, StatusCode::CREATED);
    let (rol, verificado): (String, bool) =
        sqlx::query_as("SELECT rol, email_verificado FROM usuario WHERE email = 'duena@test.com'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(rol, "admin");
    assert!(
        !verificado,
        "la cuenta de comercio también verifica su correo"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn el_registro_rechaza_correos_repetidos_sin_importar_mayusculas(pool: PgPool) {
    let app = app(pool);
    let cuerpo = |e: &str| json!({ "nombre": "Ana", "email": e, "contraseña": "Clave1234" });
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/auth/register",
            None,
            Some(cuerpo("ana@test.com"))
        )
        .await
        .0,
        StatusCode::CREATED
    );
    let (st, v) = call(
        &app,
        "POST",
        "/api/auth/register",
        None,
        Some(cuerpo("ANA@Test.com")),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);
    assert!(v["error"].as_str().unwrap().contains("registrado"));
}

#[sqlx::test(migrations = "./migrations")]
async fn el_registro_valida_los_datos(pool: PgPool) {
    let app = app(pool);
    let larga = "x".repeat(100);
    for (nombre, email, clave) in [
        ("Ana", "ana@test.com", "123"),
        ("Ana", "ana@test.com", "clave1234"),
        ("Ana", "no-es-email", "Clave1234"),
        ("  ", "a@test.com", "clave123"),
        ("Ana", "a@test.com", larga.as_str()),
    ] {
        let (st, v) = call(
            &app,
            "POST",
            "/api/auth/register",
            None,
            Some(json!({ "nombre": nombre, "email": email, "contraseña": clave })),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{nombre}/{email}/{clave}");
        assert!(v["error"].is_string());
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn el_registro_crea_la_cuenta_sin_verificar_y_genera_token(pool: PgPool) {
    let app = app(pool.clone());
    let (st, v) = call(
        &app,
        "POST",
        "/api/auth/register",
        None,
        Some(json!({
            "nombre": "Nueva", "email": "nueva@test.com", "contraseña": "Clave1234"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(v["user"]["email_verificado"], false);
    let verificado: bool =
        sqlx::query_scalar("SELECT email_verificado FROM usuario WHERE email = 'nueva@test.com'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!verificado);
    let tokens: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM token_usuario WHERE tipo = 'verificar_email' AND usado_en IS NULL",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(tokens, 1);
    let minutos: f64 = sqlx::query_scalar(
        "SELECT EXTRACT(EPOCH FROM (expira_en - now()))::double precision / 60.0 \
         FROM token_usuario WHERE tipo = 'verificar_email' AND usado_en IS NULL",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        (55.0..=61.0).contains(&minutos),
        "el enlace de verificación vence en 1 hora, no en {minutos} minutos"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn verificar_email_confirma_la_cuenta_y_el_token_es_de_un_solo_uso(pool: PgPool) {
    let u = crear_usuario_verificacion(&pool, "nueva@test.com", "cliente", false).await;
    let app = app(pool.clone());
    use sha2::{Digest, Sha256};
    let token = "token-verificacion-1";
    let hash = hex::encode(Sha256::digest(token.as_bytes()));
    sqlx::query("INSERT INTO token_usuario (usuario_id, tipo, token_hash, expira_en) VALUES ($1, 'verificar_email', $2, now() + interval '24 hours')")
        .bind(u.id)
        .bind(&hash)
        .execute(&pool)
        .await
        .unwrap();

    let (st, _) = call(
        &app,
        "POST",
        "/api/auth/verificar-email",
        None,
        Some(json!({ "token": token })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let verificado: bool =
        sqlx::query_scalar("SELECT email_verificado FROM usuario WHERE id = $1")
            .bind(u.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(verificado);
    let (st, _) = call(
        &app,
        "POST",
        "/api/auth/verificar-email",
        None,
        Some(json!({ "token": token })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "el token se consume");
    let (st, _) = call(
        &app,
        "POST",
        "/api/auth/verificar-email",
        None,
        Some(json!({ "token": "no-existe" })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrations = "./migrations")]
async fn reenviar_verificacion_responde_igual_y_solo_genera_token_si_falta_verificar(pool: PgPool) {
    crear_usuario(&pool, "verificada@test.com", "cliente").await;
    let pendiente = crear_usuario_verificacion(&pool, "pendiente@test.com", "cliente", false).await;
    let app = app(pool.clone());
    let cuerpo = |e: &str| Some(json!({ "email": e }));
    let r1 = call(
        &app,
        "POST",
        "/api/auth/reenviar-verificacion",
        None,
        cuerpo("verificada@test.com"),
    )
    .await;
    let r2 = call(
        &app,
        "POST",
        "/api/auth/reenviar-verificacion",
        None,
        cuerpo("nadie@test.com"),
    )
    .await;
    let r3 = call(
        &app,
        "POST",
        "/api/auth/reenviar-verificacion",
        None,
        cuerpo("pendiente@test.com"),
    )
    .await;
    assert_eq!(r1, r2, "no revela qué correos existen");
    assert_eq!(r3.0, StatusCode::OK);
    let tokens: i64 =
        sqlx::query_scalar("SELECT count(*) FROM token_usuario WHERE tipo = 'verificar_email'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(tokens, 1, "solo se genera token para la cuenta pendiente");

    let (hash_previo, minutos): (String, f64) = sqlx::query_as(
        "SELECT token_hash, EXTRACT(EPOCH FROM (expira_en - now()))::double precision / 60.0 \
         FROM token_usuario WHERE usuario_id = $1 AND tipo = 'verificar_email' AND usado_en IS NULL",
    )
    .bind(pendiente.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        (55.0..=61.0).contains(&minutos),
        "el reenvío también vence en 1 hora"
    );

    let r4 = call(
        &app,
        "POST",
        "/api/auth/reenviar-verificacion",
        None,
        cuerpo("pendiente@test.com"),
    )
    .await;
    assert_eq!(r4.0, StatusCode::OK);
    let (hash_activo, usado): (String, bool) = sqlx::query_as(
        "SELECT token_hash, usado_en IS NOT NULL FROM token_usuario WHERE token_hash = $1",
    )
    .bind(&hash_previo)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(usado, "el enlace anterior deja de servir tras el reenvío");
    let hash_nuevo: String = sqlx::query_scalar(
        "SELECT token_hash FROM token_usuario \
         WHERE usuario_id = $1 AND tipo = 'verificar_email' AND usado_en IS NULL",
    )
    .bind(pendiente.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_ne!(hash_activo, hash_nuevo, "se genera un token distinto");
    let activos: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM token_usuario \
         WHERE usuario_id = $1 AND tipo = 'verificar_email' AND usado_en IS NULL",
    )
    .bind(pendiente.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(activos, 1, "queda un solo enlace válido");
}

#[sqlx::test(migrations = "./migrations")]
async fn reservar_y_cancelar_exigen_cuenta_verificada(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    sqlx::query("UPDATE usuario SET email_verificado = false WHERE id = $1")
        .bind(e.cliente.id)
        .execute(&pool)
        .await
        .unwrap();
    let f = fecha(Weekday::Tue, 2);
    let (st, v) = call(
        &e.app,
        "POST",
        "/api/turnos/reservar",
        Some(&e.cliente.token),
        Some(reserva(e.empresa, f, "10:00")),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert!(v["error"].as_str().unwrap().contains("verificar"));

    // Tras verificar con el enlace, la misma reserva pasa.
    use sha2::{Digest, Sha256};
    let token = "token-verificacion-2";
    let hash = hex::encode(Sha256::digest(token.as_bytes()));
    sqlx::query("INSERT INTO token_usuario (usuario_id, tipo, token_hash, expira_en) VALUES ($1, 'verificar_email', $2, now() + interval '24 hours')")
        .bind(e.cliente.id)
        .bind(&hash)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        call(
            &e.app,
            "POST",
            "/api/auth/verificar-email",
            None,
            Some(json!({ "token": token }))
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &e.app,
            "POST",
            "/api/turnos/reservar",
            Some(&e.cliente.token),
            Some(reserva(e.empresa, f, "10:00"))
        )
        .await
        .0,
        StatusCode::OK
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn el_login_devuelve_un_token_valido(pool: PgPool) {
    let u = crear_usuario(&pool, "ana@test.com", "admin").await;
    let app = app(pool);
    let (st, v) = call(
        &app,
        "POST",
        "/api/auth/login",
        None,
        Some(json!({ "email": "ANA@test.com", "contraseña": "clave123" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let claims = decode_token(v["token"].as_str().unwrap(), SECRETO).unwrap();
    assert_eq!((claims.id, claims.rol.as_str()), (u.id, "admin"));
    assert_eq!(v["user"]["email"], "ana@test.com");
    assert_eq!(v["user"]["email_verificado"], true);
    assert!(
        v["user"].get("contraseña").is_none(),
        "no debe filtrar el hash"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn el_login_no_revela_si_el_correo_existe(pool: PgPool) {
    crear_usuario(&pool, "ana@test.com", "cliente").await;
    let app = app(pool);
    let a = call(
        &app,
        "POST",
        "/api/auth/login",
        None,
        Some(json!({ "email": "ana@test.com", "contraseña": "mala" })),
    )
    .await;
    let b = call(
        &app,
        "POST",
        "/api/auth/login",
        None,
        Some(json!({ "email": "nadie@test.com", "contraseña": "mala" })),
    )
    .await;
    assert_eq!(a.0, StatusCode::UNAUTHORIZED);
    assert_eq!(a, b);
}

#[sqlx::test(migrations = "./migrations")]
async fn el_login_se_bloquea_tras_5_fallos_y_un_acierto_reinicia_la_cuenta(pool: PgPool) {
    crear_usuario(&pool, "ana@test.com", "cliente").await;
    let app = app(pool);
    let login = |clave: &'static str| {
        let app = app.clone();
        async move {
            call(
                &app,
                "POST",
                "/api/auth/login",
                None,
                Some(json!({ "email": "ana@test.com", "contraseña": clave })),
            )
            .await
            .0
        }
    };
    for _ in 0..4 {
        assert_eq!(login("mala").await, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(
        login("clave123").await,
        StatusCode::OK,
        "un acierto reinicia el contador"
    );
    for _ in 0..5 {
        assert_eq!(login("mala").await, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(login("mala").await, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        login("clave123").await,
        StatusCode::TOO_MANY_REQUESTS,
        "bloqueada aunque la clave sea correcta"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn el_bloqueo_de_un_correo_no_afecta_a_otro(pool: PgPool) {
    crear_usuario(&pool, "ana@test.com", "cliente").await;
    crear_usuario(&pool, "beto@test.com", "cliente").await;
    let app = app(pool);
    for _ in 0..6 {
        call(
            &app,
            "POST",
            "/api/auth/login",
            None,
            Some(json!({ "email": "ana@test.com", "contraseña": "mala" })),
        )
        .await;
    }
    let (st, _) = call(
        &app,
        "POST",
        "/api/auth/login",
        None,
        Some(json!({ "email": "beto@test.com", "contraseña": "clave123" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
}

#[sqlx::test(migrations = "./migrations")]
async fn recuperar_la_contrasena_funciona_una_sola_vez(pool: PgPool) {
    let u = crear_usuario(&pool, "ana@test.com", "cliente").await;
    let app = app(pool.clone());
    // El token real solo viaja por correo (simulado en el log): se inserta su hash directamente.
    let token = "token-de-prueba-123";
    use sha2::{Digest, Sha256};
    let hash = hex::encode(Sha256::digest(token.as_bytes()));
    sqlx::query("INSERT INTO token_usuario (usuario_id, tipo, token_hash, expira_en) VALUES ($1, 'reset_password', $2, now() + interval '1 hour')")
        .bind(u.id)
        .bind(&hash)
        .execute(&pool)
        .await
        .unwrap();

    let (st, _) = call(
        &app,
        "POST",
        "/api/auth/reset-password",
        None,
        Some(json!({ "token": token, "contraseña": "NuevaClave1" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = call(
        &app,
        "POST",
        "/api/auth/login",
        None,
        Some(json!({ "email": "ana@test.com", "contraseña": "NuevaClave1" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = call(
        &app,
        "POST",
        "/api/auth/login",
        None,
        Some(json!({ "email": "ana@test.com", "contraseña": "clave123" })),
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "la clave vieja ya no sirve");
    let (st, _) = call(
        &app,
        "POST",
        "/api/auth/reset-password",
        None,
        Some(json!({ "token": token, "contraseña": "OtraClave1" })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "el token se consume");
}

#[sqlx::test(migrations = "./migrations")]
async fn recuperar_la_contrasena_rechaza_tokens_vencidos_o_desconocidos(pool: PgPool) {
    let u = crear_usuario(&pool, "ana@test.com", "cliente").await;
    let app = app(pool.clone());
    use sha2::{Digest, Sha256};
    sqlx::query("INSERT INTO token_usuario (usuario_id, tipo, token_hash, expira_en) VALUES ($1, 'reset_password', $2, now() - interval '1 minute')")
        .bind(u.id)
        .bind(hex::encode(Sha256::digest(b"vencido")))
        .execute(&pool)
        .await
        .unwrap();
    for token in ["vencido", "no-existe"] {
        let (st, _) = call(
            &app,
            "POST",
            "/api/auth/reset-password",
            None,
            Some(json!({ "token": token, "contraseña": "NuevaClave1" })),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{token}");
    }
    let (st, _) = call(
        &app,
        "POST",
        "/api/auth/reset-password",
        None,
        Some(json!({ "token": "x", "contraseña": "123" })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "contraseña corta");
}

#[sqlx::test(migrations = "./migrations")]
async fn olvide_mi_contrasena_responde_igual_y_limita_los_pedidos(pool: PgPool) {
    crear_usuario(&pool, "ana@test.com", "cliente").await;
    let app = app(pool.clone());
    let existe = call(
        &app,
        "POST",
        "/api/auth/forgot-password",
        None,
        Some(json!({ "email": "ana@test.com" })),
    )
    .await;
    let no_existe = call(
        &app,
        "POST",
        "/api/auth/forgot-password",
        None,
        Some(json!({ "email": "nadie@test.com" })),
    )
    .await;
    assert_eq!(existe, no_existe, "no revela qué correos existen");
    let tokens: i64 = sqlx::query_scalar("SELECT count(*) FROM token_usuario")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(tokens, 1, "solo se genera token para el correo real");
    call(
        &app,
        "POST",
        "/api/auth/forgot-password",
        None,
        Some(json!({ "email": "ana@test.com" })),
    )
    .await;
    call(
        &app,
        "POST",
        "/api/auth/forgot-password",
        None,
        Some(json!({ "email": "ana@test.com" })),
    )
    .await;
    let (st, _) = call(
        &app,
        "POST",
        "/api/auth/forgot-password",
        None,
        Some(json!({ "email": "ana@test.com" })),
    )
    .await;
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS);
}

#[sqlx::test(migrations = "./migrations")]
async fn un_nuevo_pedido_de_recuperacion_invalida_el_enlace_anterior(pool: PgPool) {
    let u = crear_usuario(&pool, "ana@test.com", "cliente").await;
    let app = app(pool.clone());
    let cuerpo = || Some(json!({ "email": "ana@test.com" }));
    let (st, _) = call(&app, "POST", "/api/auth/forgot-password", None, cuerpo()).await;
    assert_eq!(st, StatusCode::OK);
    let (hash_previo, minutos): (String, f64) = sqlx::query_as(
        "SELECT token_hash, EXTRACT(EPOCH FROM (expira_en - now()))::double precision / 60.0 \
         FROM token_usuario WHERE usuario_id = $1 AND tipo = 'reset_password' AND usado_en IS NULL",
    )
    .bind(u.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!((55.0..=61.0).contains(&minutos));

    let (st, _) = call(&app, "POST", "/api/auth/forgot-password", None, cuerpo()).await;
    assert_eq!(st, StatusCode::OK);
    let usado: bool =
        sqlx::query_scalar("SELECT usado_en IS NOT NULL FROM token_usuario WHERE token_hash = $1")
            .bind(&hash_previo)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(usado, "el enlace anterior deja de servir");
    let activos: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM token_usuario \
         WHERE usuario_id = $1 AND tipo = 'reset_password' AND usado_en IS NULL",
    )
    .bind(u.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(activos, 1, "queda un solo enlace válido");
}

#[sqlx::test(migrations = "./migrations")]
async fn un_json_invalido_responde_con_un_error_en_json(pool: PgPool) {
    let app = app(pool);
    let (st, v) = enviar(&app, "POST", "/api/auth/login", None, Some("{malo".into())).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(v["error"].as_str().unwrap().contains("JSON"));
}

#[sqlx::test(migrations = "./migrations")]
async fn las_rutas_protegidas_exigen_un_token_autentico(pool: PgPool) {
    let app = app(pool);
    assert_eq!(
        call(&app, "GET", "/api/usuarios", None, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&app, "GET", "/api/favoritos", None, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    let forjado = token_de(1, "superadmin", "supersecreto");
    assert_eq!(
        call(&app, "GET", "/api/usuarios", Some(&forjado), None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&app, "GET", "/api/usuarios", Some("basura"), None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
}

// ------------------------------------------------------------------ usuarios y rubros

#[sqlx::test(migrations = "./migrations")]
async fn solo_el_superadmin_lista_usuarios_y_asigna_roles(pool: PgPool) {
    let e = escenario(pool).await;
    assert_eq!(
        call(&e.app, "GET", "/api/usuarios", Some(&e.cliente.token), None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&e.app, "GET", "/api/usuarios", Some(&e.dueno.token), None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let (st, v) = call(
        &e.app,
        "GET",
        "/api/usuarios",
        Some(&e.superadmin.token),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v.as_array().unwrap().len(), 3);

    let nuevo = json!({ "nombre": "Nuevo", "email": "nuevo@test.com", "contraseña": "Clave1234", "rol": "admin" });
    assert_eq!(
        call(&e.app, "POST", "/api/usuarios", None, Some(nuevo.clone()))
            .await
            .0,
        StatusCode::UNAUTHORIZED,
        "sin sesión: 401, no 403"
    );
    assert_eq!(
        call(
            &e.app,
            "POST",
            "/api/usuarios",
            Some(&e.cliente.token),
            Some(nuevo.clone())
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &e.app,
            "POST",
            "/api/usuarios",
            Some(&e.superadmin.token),
            Some(nuevo)
        )
        .await
        .0,
        StatusCode::CREATED
    );
    let malo =
        json!({ "nombre": "X", "email": "x@test.com", "contraseña": "Clave1234", "rol": "dios" });
    assert_eq!(
        call(
            &e.app,
            "POST",
            "/api/usuarios",
            Some(&e.superadmin.token),
            Some(malo)
        )
        .await
        .0,
        StatusCode::BAD_REQUEST,
        "un rol inventado se rechaza"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn los_rubros_se_gestionan_solo_como_superadmin(pool: PgPool) {
    let e = escenario(pool).await;
    let nuevo = json!({ "nombre": "Gimnasios" });
    assert_eq!(
        call(
            &e.app,
            "POST",
            "/api/rubros",
            Some(&e.dueno.token),
            Some(nuevo.clone())
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (st, v) = call(
        &e.app,
        "POST",
        "/api/rubros",
        Some(&e.superadmin.token),
        Some(nuevo.clone()),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let id = v["id"].as_i64().unwrap();
    assert_eq!(
        call(
            &e.app,
            "POST",
            "/api/rubros",
            Some(&e.superadmin.token),
            Some(nuevo)
        )
        .await
        .0,
        StatusCode::CONFLICT,
        "nombre repetido"
    );
    assert_eq!(
        call(
            &e.app,
            "POST",
            "/api/rubros",
            Some(&e.superadmin.token),
            Some(json!({ "nombre": "  " }))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let (st, v) = call(
        &e.app,
        "PUT",
        &format!("/api/rubros/{id}"),
        Some(&e.superadmin.token),
        Some(json!({ "nombre": "Gimnasios y pileta" })),
    )
    .await;
    assert_eq!(
        (st, v["nombre"].as_str()),
        (StatusCode::OK, Some("Gimnasios y pileta"))
    );
    assert_eq!(
        call(
            &e.app,
            "PUT",
            "/api/rubros/99999",
            Some(&e.superadmin.token),
            Some(json!({ "nombre": "x" }))
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &e.app,
            "DELETE",
            &format!("/api/rubros/{id}"),
            Some(&e.superadmin.token),
            None
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &e.app,
            "DELETE",
            &format!("/api/rubros/{}", e.rubro),
            Some(&e.superadmin.token),
            None
        )
        .await
        .0,
        StatusCode::CONFLICT,
        "no se borra un rubro con comercios"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn el_listado_de_rubros_informa_cantidad_y_si_toman_turnos(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    let sin_turnos = crear_rubro(&pool, "Kioscos").await;
    crear_empresa(&pool, "Kiosco 1", sin_turnos, None, false, None).await;
    crear_rubro(&pool, "Vacío").await;
    let (_, v) = call(&e.app, "GET", "/api/rubros", None, None).await;
    let por_nombre = |n: &str| {
        v.as_array()
            .unwrap()
            .iter()
            .find(|c| c["nombre"] == n)
            .unwrap()
            .clone()
    };
    assert_eq!(
        (
            por_nombre("Peluquerías")["cantidad"].as_i64(),
            por_nombre("Peluquerías")["turnos_habilitados"].as_bool()
        ),
        (Some(1), Some(true))
    );
    assert_eq!(
        (
            por_nombre("Kioscos")["cantidad"].as_i64(),
            por_nombre("Kioscos")["turnos_habilitados"].as_bool()
        ),
        (Some(1), Some(false))
    );
    assert_eq!(por_nombre("Vacío")["cantidad"], 0);
}

// ------------------------------------------------------------------ comercios

#[sqlx::test(migrations = "./migrations")]
async fn el_listado_pagina_y_acota_los_limites(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    for i in 0..5 {
        crear_empresa(
            &pool,
            &format!("Comercio {i}"),
            e.rubro,
            None,
            false,
            None,
        )
        .await;
    }
    let total = |v: &Value| v.as_array().unwrap().len();
    assert_eq!(
        total(&call(&e.app, "GET", "/api/empresas", None, None).await.1),
        6
    );
    assert_eq!(
        total(
            &call(&e.app, "GET", "/api/empresas?limit=2", None, None)
                .await
                .1
        ),
        2
    );
    assert_eq!(
        total(
            &call(&e.app, "GET", "/api/empresas?limit=2&offset=5", None, None)
                .await
                .1
        ),
        1
    );
    let (st, v) = call(
        &e.app,
        "GET",
        "/api/empresas?limit=-5&offset=-3",
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "límites negativos no rompen");
    assert_eq!(total(&v), 1, "un límite negativo se acota a 1");
    assert_eq!(
        total(
            &call(&e.app, "GET", "/api/empresas?limit=99999999", None, None)
                .await
                .1
        ),
        6
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn buscar_comida_no_trae_barberias_ni_comercios_por_coincidencias_parciales(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    let barberias = crear_rubro(&pool, "Barberías").await;
    let barberia = crear_empresa(&pool, "Barber Shop", barberias, None, true, Some(30)).await;
    sqlx::query("UPDATE empresa SET descripcion = 'El mejor comercio del barrio' WHERE id = $1")
        .bind(barberia)
        .execute(&pool)
        .await
        .unwrap();
    let bares = crear_rubro(&pool, "Bares").await;
    crear_empresa(&pool, "La Esquina", bares, None, false, None).await;
    let comidas = crear_rubro(&pool, "Casas de Comidas").await;
    crear_empresa(&pool, "Rotisería Don Pepe", comidas, None, false, None).await;
    let herrerias = crear_rubro(&pool, "Herrerías").await;
    let herreria = crear_empresa(&pool, "Herrería Jeremías", herrerias, None, false, None).await;
    let hospedajes = crear_rubro(&pool, "Hospedajes").await;
    let cabana = crear_empresa(&pool, "Cabañas del Lago", hospedajes, None, false, None).await;
    for (id, texto) in [
        (herreria, "Fabricamos parrillas y portones"),
        (cabana, "Desayuno y comida casera"),
    ] {
        sqlx::query("UPDATE empresa SET descripcion = $2 WHERE id = $1")
            .bind(id)
            .bind(texto)
            .execute(&pool)
            .await
            .unwrap();
    }
    let (_, v) = call(&e.app, "GET", "/api/empresas?q=comida", None, None).await;
    let en_orden: Vec<_> = v.as_array().unwrap().iter().map(|x| x["nombre"].as_str().unwrap()).collect();
    assert_eq!(
        en_orden.first(),
        Some(&"Rotisería Don Pepe"),
        "primero lo que coincide con lo escrito (Casas de Comidas), después los sinónimos (Bares)"
    );
    assert_eq!(
        en_orden.last(),
        Some(&"Cabañas del Lago"),
        "lo que solo menciona 'comida' en la descripción va después de los rubros de comida"
    );
    assert!(
        !en_orden.contains(&"Herrería Jeremías"),
        "un sinónimo ('parrilla') no se busca en la descripción"
    );
    let nombres = |v: &Value| {
        let mut n = v
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["nombre"].as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        n.sort();
        n
    };
    let (_, v) = call(&e.app, "GET", "/api/empresas?q=comida", None, None).await;
    assert_eq!(
        nombres(&v),
        ["Cabañas del Lago", "La Esquina", "Rotisería Don Pepe"],
        "'bar' y 'comer' se buscan como palabra completa: ni barberías ni 'comercio'"
    );
    let (_, v) = call(&e.app, "GET", "/api/empresas?q=corte%20de%20pelo", None, None).await;
    assert_eq!(
        nombres(&v),
        ["Barber Shop", "Peluquería Central"],
        "'de' no se busca en todos los comercios"
    );
}

/// Devolución 6/11 (segunda pasada): lo escrito se busca como comienzo de palabra y los sinónimos
/// no se buscan en las etiquetas.
#[sqlx::test(migrations = "./migrations")]
async fn la_busqueda_no_mezcla_rubros_por_etiquetas_ni_por_pedazos_de_palabras(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    let veterinarias = crear_rubro(&pool, "Veterinaria").await;
    let vete = crear_empresa(&pool, "Veterinaria San Roque", veterinarias, None, false, None).await;
    let gimnasios = crear_rubro(&pool, "Gimnasio").await;
    let gym = crear_empresa(&pool, "PowerFit", gimnasios, None, false, None).await;
    let carnicerias = crear_rubro(&pool, "Carnicería").await;
    crear_empresa(&pool, "El Novillo", carnicerias, None, false, None).await;
    let manicurias = crear_rubro(&pool, "Manicuría").await;
    crear_empresa(&pool, "Uñas & Detalles", manicurias, None, false, None).await;
    for (empresa, nombre) in [
        (vete, "Vacunas"),
        (vete, "Peluquería canina"),
        (gym, "Acepta mascotas"),
        (gym, "Wi-Fi gratis"),
    ] {
        let etiqueta: i32 = sqlx::query_scalar(
            "INSERT INTO etiqueta (nombre) VALUES ($1) \
             ON CONFLICT (nombre) DO UPDATE SET nombre = EXCLUDED.nombre RETURNING id",
        )
        .bind(nombre)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO empresa_etiqueta VALUES ($1, $2) ON CONFLICT DO NOTHING")
            .bind(empresa)
            .bind(etiqueta)
            .execute(&pool)
            .await
            .unwrap();
    }
    let nombres = |v: &Value| {
        let mut n = v
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["nombre"].as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        n.sort();
        n
    };
    let buscar = |q: &'static str| {
        let app = e.app.clone();
        async move { call(&app, "GET", &format!("/api/empresas?q={q}"), None, None).await.1 }
    };
    assert_eq!(nombres(&buscar("u%C3%B1as").await), ["Uñas & Detalles"], "'uñas' no está dentro de 'vacunas'");
    assert_eq!(nombres(&buscar("perro").await), ["Veterinaria San Roque"], "el sinónimo no se busca en 'Acepta mascotas'");
    assert!(
        !nombres(&buscar("pelo").await).contains(&"Veterinaria San Roque".to_string()),
        "el sinónimo 'peluquería' no se busca en la etiqueta 'Peluquería canina'"
    );
    assert_eq!(nombres(&buscar("carne").await), ["El Novillo"]);
    assert_eq!(nombres(&buscar("mascotas").await), ["PowerFit"], "lo escrito sí se busca en las etiquetas");
    assert_eq!(nombres(&buscar("vete").await), ["Veterinaria San Roque"], "lo escrito a medias encuentra el comienzo");
    assert_eq!(nombres(&buscar("wifi").await), ["PowerFit"], "las etiquetas también se comparan sin signos");
    assert_eq!(nombres(&buscar("wi-fi").await), ["PowerFit"]);

    // Palabras conocidas del diccionario: se buscan enteras ("comer" ≠ "Comercio").
    let oficinas = crear_rubro(&pool, "Trámites y Oficinas").await;
    crear_empresa(&pool, "Cámara de Comercio", oficinas, None, false, None).await;
    let arquitectos = crear_rubro(&pool, "Arquitectos").await;
    crear_empresa(&pool, "Estudio Perroud", arquitectos, None, false, None).await;
    let tramite = crear_empresa(&pool, "Gestoría Sur", oficinas, None, false, None).await;
    sqlx::query("UPDATE empresa SET descripcion = 'Renovación de carnet de conducir' WHERE id = $1")
        .bind(tramite)
        .execute(&pool)
        .await
        .unwrap();
    assert!(!nombres(&buscar("comer").await).contains(&"Cámara de Comercio".to_string()));
    assert_eq!(nombres(&buscar("perro").await), ["Veterinaria San Roque"], "ni 'Perroud'");
    assert_eq!(nombres(&buscar("carne").await), ["El Novillo"], "ni 'carnet'");

    // "bar" es ambiguo: primero el rubro que se llama así (Bares), después lo que empieza igual.
    let bares = crear_rubro(&pool, "Bares").await;
    crear_empresa(&pool, "La Esquina", bares, None, false, None).await;
    let barberias = crear_rubro(&pool, "Barberías").await;
    crear_empresa(&pool, "Peluquería Champú", barberias, None, false, None).await;
    let (_, v) = call(&e.app, "GET", "/api/empresas?q=bar", None, None).await;
    let en_orden: Vec<_> = v.as_array().unwrap().iter().map(|x| x["nombre"].as_str().unwrap()).collect();
    assert_eq!(en_orden.first(), Some(&"La Esquina"), "{en_orden:?}");
    assert!(en_orden.contains(&"Peluquería Champú"), "lo que empieza con 'bar' también aparece, después");
}

#[sqlx::test(migrations = "./migrations")]
async fn la_busqueda_encuentra_por_nombre_o_por_rubro_y_resiste_inyeccion(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    let otro = crear_rubro(&pool, "Ferreterías").await;
    crear_empresa(&pool, "El Tornillo", otro, None, false, None).await;
    let comida = crear_rubro(&pool, "Gastronomía").await;
    crear_empresa(&pool, "Lo de Juan", comida, None, false, None).await;
    let nombres = |v: &Value| {
        v.as_array()
            .unwrap()
            .iter()
            .map(|x| x["nombre"].as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        nombres(
            &call(&e.app, "GET", "/api/empresas?q=tornillo", None, None)
                .await
                .1
        ),
        ["El Tornillo"]
    );
    assert_eq!(
        nombres(
            &call(&e.app, "GET", "/api/empresas?q=peluquer", None, None)
                .await
                .1
        ),
        ["Peluquería Central"],
        "por rubro"
    );
    assert_eq!(
        nombres(
            &call(&e.app, "GET", "/api/empresas?q=peluqueria", None, None)
                .await
                .1
        ),
        ["Peluquería Central"],
        "tolera tildes en el rubro"
    );
    assert_eq!(
        nombres(
            &call(&e.app, "GET", "/api/empresas?q=comida", None, None)
                .await
                .1
        ),
        ["Lo de Juan"],
        "RF-07: 'comida' se asocia al rubro gastronomía"
    );
    assert_eq!(
        nombres(
            &call(&e.app, "GET", "/api/empresas?q=corte", None, None)
                .await
                .1
        ),
        ["Peluquería Central"],
        "RF-07: 'corte' se asocia a peluquería"
    );
    assert_eq!(
        nombres(
            &call(
                &e.app,
                "GET",
                &format!("/api/empresas?rubro_id={otro}"),
                None,
                None
            )
            .await
            .1
        ),
        ["El Tornillo"]
    );
    assert_eq!(
        nombres(
            &call(
                &e.app,
                "GET",
                &format!("/api/empresas/rubro/{otro}"),
                None,
                None
            )
            .await
            .1
        ),
        ["El Tornillo"]
    );
    let (st, v) = call(
        &e.app,
        "GET",
        "/api/empresas?q=%27%3B%20DROP%20TABLE%20empresas%3B--",
        None,
        None,
    )
    .await;
    assert_eq!((st, total_de(&v)), (StatusCode::OK, 0));
    assert_eq!(
        total_de(&call(&e.app, "GET", "/api/empresas", None, None).await.1),
        3,
        "la tabla sigue intacta"
    );
}

fn total_de(v: &Value) -> usize {
    v.as_array().unwrap().len()
}

#[sqlx::test(migrations = "./migrations")]
async fn filtrar_por_dueno_devuelve_solo_sus_comercios(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    crear_empresa(&pool, "Ajeno", e.rubro, None, false, None).await;
    let (_, v) = call(
        &e.app,
        "GET",
        &format!("/api/empresas?usuario_id={}", e.dueno.id),
        None,
        None,
    )
    .await;
    assert_eq!(total_de(&v), 1);
    assert_eq!(v[0]["id"], e.empresa);
}

#[sqlx::test(migrations = "./migrations")]
async fn la_ficha_trae_todo_pero_el_listado_no_trae_la_descripcion(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    sqlx::query("INSERT INTO telefono_empresa (empresa_id, numero) VALUES ($1, '2392111111')")
        .bind(e.empresa)
        .execute(&pool)
        .await
        .unwrap();
    let et: i32 =
        sqlx::query_scalar("INSERT INTO etiqueta (nombre) VALUES ('Wi-Fi') RETURNING id")
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query("INSERT INTO empresa_etiqueta VALUES ($1, $2)")
        .bind(e.empresa)
        .bind(et)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE empresa SET slug = 'peluqueria-central' WHERE id = $1")
        .bind(e.empresa)
        .execute(&pool)
        .await
        .unwrap();

    let (st, ficha) = call(
        &e.app,
        "GET",
        &format!("/api/empresas/{}", e.empresa),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(ficha["descripcion"], "Descripción larga de prueba");
    assert_eq!(ficha["telefonos"][0]["numero"], "2392111111");
    assert_eq!(ficha["etiquetas"], json!(["Wi-Fi"]));
    assert_eq!(ficha["rubro"], "Peluquerías");
    assert_eq!(ficha["verificado"], false);

    // La URL pública usa el slug: la ficha debe resolverlo igual que el ID.
    let (st, por_slug) = call(
        &e.app,
        "GET",
        "/api/empresas/peluqueria-central",
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(por_slug["id"], e.empresa);

    let (_, lista) = call(&e.app, "GET", "/api/empresas", None, None).await;
    assert!(lista[0].get("descripcion").is_none());
    assert_eq!(lista[0]["etiquetas"], json!(["Wi-Fi"]));
    assert_eq!(
        call(&e.app, "GET", "/api/empresas/999999", None, None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    // Un slug inexistente también es "no encontrado", no un error de parseo.
    assert_eq!(
        call(&e.app, "GET", "/api/empresas/abc", None, None).await.0,
        StatusCode::NOT_FOUND
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn solo_el_dueno_o_un_superadmin_edita_un_comercio(pool: PgPool) {
    let e = escenario(pool).await;
    let cuerpo = json!({ "nombre": "Nuevo nombre", "direccion": "Otra calle 1", "rubro_id": e.rubro });
    let uri = format!("/api/empresas/{}", e.empresa);
    assert_eq!(
        call(&e.app, "PUT", &uri, None, Some(cuerpo.clone()))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &e.app,
            "PUT",
            &uri,
            Some(&e.cliente.token),
            Some(cuerpo.clone())
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&e.app, "DELETE", &uri, Some(&e.cliente.token), None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let (st, v) = call(
        &e.app,
        "PUT",
        &uri,
        Some(&e.dueno.token),
        Some(cuerpo.clone()),
    )
    .await;
    assert_eq!(
        (st, v["nombre"].as_str()),
        (StatusCode::OK, Some("Nuevo nombre"))
    );
    assert_eq!(
        call(&e.app, "PUT", &uri, Some(&e.superadmin.token), Some(cuerpo))
            .await
            .0,
        StatusCode::OK
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn editar_la_ficha_conserva_lo_que_no_se_envia(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    sqlx::query("INSERT INTO foto_empresa (empresa_id, url) VALUES ($1, 'https://x.com/a.jpg')")
        .bind(e.empresa)
        .execute(&pool)
        .await
        .unwrap();
    let cuerpo = json!({ "nombre": "Solo cambio el nombre", "direccion": "Calle 123", "rubro_id": e.rubro, "usuario_id": e.cliente.id });
    let (st, v) = call(
        &e.app,
        "PUT",
        &format!("/api/empresas/{}", e.empresa),
        Some(&e.dueno.token),
        Some(cuerpo),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        v["usuario_id"], e.dueno.id,
        "un dueño no puede ceder ni cambiar la propiedad"
    );
    assert_eq!(v["fotos"], json!(["https://x.com/a.jpg"]));
    assert_eq!(v["descripcion"], "Descripción larga de prueba");
    assert_eq!(v["turnos_habilitados"], true);
}

#[sqlx::test(migrations = "./migrations")]
async fn filtrar_por_etiquetas_exige_todas_y_acepta_alias(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    let otra = crear_empresa(&pool, "Gimnasio Norte", e.rubro, None, false, None).await;
    for (empresa, nombres) in [
        (e.empresa, vec!["Wi-Fi gratis", "Rampa de acceso"]),
        (otra, vec!["Apto celíacos"]),
    ] {
        for nombre in nombres {
            let etiqueta: i32 = sqlx::query_scalar(
                "INSERT INTO etiqueta (nombre) VALUES ($1) \
                 ON CONFLICT (nombre) DO UPDATE SET nombre = EXCLUDED.nombre RETURNING id",
            )
            .bind(nombre)
            .fetch_one(&pool)
            .await
            .unwrap();
            sqlx::query("INSERT INTO empresa_etiqueta VALUES ($1, $2) ON CONFLICT DO NOTHING")
                .bind(empresa)
                .bind(etiqueta)
                .execute(&pool)
                .await
                .unwrap();
        }
    }
    let nombres_de = |v: &Value| {
        v.as_array()
            .unwrap()
            .iter()
            .map(|x| x["nombre"].as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    };

    let (st, v) = call(&e.app, "GET", "/api/empresas?etiquetas=wifi", None, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(nombres_de(&v), ["Peluquería Central"], "alias sin acento");

    let (_, v) = call(
        &e.app,
        "GET",
        "/api/empresas?etiquetas=rampa,wifi",
        None,
        None,
    )
    .await;
    assert_eq!(nombres_de(&v), ["Peluquería Central"], "se exigen todas");

    let (_, v) = call(
        &e.app,
        "GET",
        "/api/empresas?etiquetas=rampa,apto%20celiacos",
        None,
        None,
    )
    .await;
    assert_eq!(total_de(&v), 0, "no alcanza con una sola etiqueta");

    let (_, v) = call(
        &e.app,
        "GET",
        "/api/empresas?etiquetas=Acceso%20para%20sillas%20de%20ruedas",
        None,
        None,
    )
    .await;
    assert_eq!(nombres_de(&v), ["Peluquería Central"], "alias histórico");
}

#[sqlx::test(migrations = "./migrations")]
async fn el_dueno_reemplaza_las_etiquetas_y_puede_vaciarlas(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    let uri = format!("/api/empresas/{}", e.empresa);
    let base = json!({
        "nombre": "Peluquería Central",
        "direccion": "Calle 123",
        "rubro_id": e.rubro
    });

    let mut con_etiquetas = base.clone();
    con_etiquetas["etiquetas"] =
        json!(["wifi", "Acceso para sillas de ruedas", "wifi", " Terraza "]);
    let (st, v) = call(
        &e.app,
        "PUT",
        &uri,
        Some(&e.dueno.token),
        Some(con_etiquetas),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        v["etiquetas"],
        json!(["Rampa de acceso", "Terraza", "Wi-Fi gratis"]),
        "canonicaliza alias y deduplica"
    );

    let (_, v) = call(
        &e.app,
        "PUT",
        &uri,
        Some(&e.dueno.token),
        Some(base.clone()),
    )
    .await;
    assert_eq!(
        v["etiquetas"],
        json!(["Rampa de acceso", "Terraza", "Wi-Fi gratis"]),
        "omitirlas conserva las actuales"
    );

    let mut vacias = base;
    vacias["etiquetas"] = json!([]);
    let (st, v) = call(&e.app, "PUT", &uri, Some(&e.dueno.token), Some(vacias)).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["etiquetas"], json!([]));
    let cantidad: i64 =
        sqlx::query_scalar("SELECT count(*) FROM empresa_etiqueta WHERE empresa_id = $1")
            .bind(e.empresa)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(cantidad, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn crear_comercio_guarda_las_etiquetas_canonicas(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    let cuerpo = json!({
        "nombre": "Nuevo local",
        "direccion": "Calle 1",
        "rubro_id": e.rubro,
        "etiquetas": ["wifi", "baño adaptado", "wifi"]
    });
    let (st, v) = call(
        &e.app,
        "POST",
        "/api/empresas",
        Some(&e.dueno.token),
        Some(cuerpo),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(v["etiquetas"], json!(["Baño adaptado", "Wi-Fi gratis"]));
    let id = v["id"].as_i64().unwrap();
    let (_, ficha) = call(&e.app, "GET", &format!("/api/empresas/{id}"), None, None).await;
    assert_eq!(ficha["etiquetas"], json!(["Baño adaptado", "Wi-Fi gratis"]));
}

#[sqlx::test(migrations = "./migrations")]
async fn las_urls_peligrosas_se_rechazan(pool: PgPool) {
    let e = escenario(pool).await;
    let uri = format!("/api/empresas/{}", e.empresa);
    let base = |extra: Value| {
        let mut b = json!({ "nombre": "X", "direccion": "Y", "rubro_id": e.rubro });
        b.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        b
    };
    for malo in [
        json!({ "redes_sociales": [{ "tipo": "instagram", "url": "javascript:alert(1)" }] }),
        json!({ "redes_sociales": [{ "tipo": "facebook", "url": "data:text/html,x" }] }),
        json!({ "fotos": ["https://ok.com/a.jpg", "javascript:1"] }),
    ] {
        assert_eq!(
            call(&e.app, "PUT", &uri, Some(&e.dueno.token), Some(base(malo)))
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    let bueno = base(json!({
        "redes_sociales": [{ "tipo": "instagram", "url": "https://instagram.com/negocio" }],
        "fotos": ["https://ok.com/a.jpg"]
    }));
    assert_eq!(
        call(&e.app, "PUT", &uri, Some(&e.dueno.token), Some(bueno))
            .await
            .0,
        StatusCode::OK
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn la_ficha_respeta_los_largos_del_modelo(pool: PgPool) {
    let e = escenario(pool).await;
    let uri = format!("/api/empresas/{}", e.empresa);
    // Los mismos N que las columnas VARCHAR(N): la API responde 400 en vez de un error de la base.
    for (campo, largo) in [("nombre", 101), ("direccion", 151), ("slogan", 151), ("descripcion", 2001)] {
        let mut cuerpo = json!({ "nombre": "Peluquería Central", "direccion": "Calle 123", "rubro_id": e.rubro });
        cuerpo[campo] = json!("x".repeat(largo));
        let (st, v) = call(&e.app, "PUT", &uri, Some(&e.dueno.token), Some(cuerpo)).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{campo}: {v}");
        assert!(v["error"].as_str().unwrap().contains("caracteres"), "{v}");
    }
    // telefono_empresa: numero VARCHAR(20) y descripcion VARCHAR(50).
    for tel in [json!({ "numero": "1".repeat(21) }), json!({ "numero": "2392111111", "descripcion": "x".repeat(51) })] {
        let cuerpo = json!({ "nombre": "P", "direccion": "C", "rubro_id": e.rubro, "telefonos": [tel] });
        let (st, v) = call(&e.app, "PUT", &uri, Some(&e.dueno.token), Some(cuerpo)).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    }
}

/// Modelo v2: teléfonos, redes y fotos viven en sus tablas y la API los expone como listas
/// (`telefonos`, `redes_sociales`, `fotos`), sin los campos sueltos del modelo anterior.
#[sqlx::test(migrations = "./migrations")]
async fn telefonos_redes_y_fotos_se_guardan_en_sus_tablas(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    let uri = format!("/api/empresas/{}", e.empresa);
    let ficha = |extra: Value| {
        let mut b = json!({ "nombre": "Peluquería Central", "direccion": "Calle 123", "rubro_id": e.rubro });
        b.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        b
    };
    let (st, v) = call(&e.app, "PUT", &uri, Some(&e.dueno.token), Some(ficha(json!({
        "telefonos": [
            { "numero": "2392111111" },
            { "numero": "2392999999", "descripcion": "Turnos", "canal": "mensaje", "es_whatsapp": true }
        ],
        "redes_sociales": [
            { "tipo": "instagram", "url": "https://instagram.com/central" },
            { "tipo": "tiktok", "url": "https://tiktok.com/@central" }
        ],
        "fotos": ["https://x.com/1.jpg", "https://x.com/2.jpg"]
    })))).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    for viejo in ["telefono", "instagram", "facebook"] {
        assert!(v.get(viejo).is_none(), "la respuesta no usa el campo viejo '{viejo}': {v}");
    }
    assert_eq!(v["telefonos"][0]["numero"], "2392111111");
    assert_eq!(v["telefonos"][0]["principal"], true, "sin marcar, el principal es el primero");
    assert_eq!(v["telefonos"][1]["descripcion"], "Turnos");
    assert_eq!(v["telefonos"][1]["canal"], "mensaje");
    assert_eq!(v["telefonos"][1]["es_whatsapp"], true);
    assert_eq!(v["telefonos"][1]["principal"], false);
    assert_eq!(
        v["redes_sociales"],
        json!([
            { "tipo": "instagram", "url": "https://instagram.com/central" },
            { "tipo": "tiktok", "url": "https://tiktok.com/@central" }
        ])
    );
    assert_eq!(v["fotos"], json!(["https://x.com/1.jpg", "https://x.com/2.jpg"]));
    let cuenta = |sql: &'static str| {
        let pool = pool.clone();
        let id = e.empresa;
        async move { sqlx::query_scalar::<_, i64>(sql).bind(id).fetch_one(&pool).await.unwrap() }
    };
    assert_eq!(cuenta("SELECT count(*) FROM telefono_empresa WHERE empresa_id = $1").await, 2);
    assert_eq!(cuenta("SELECT count(*) FROM telefono_empresa WHERE empresa_id = $1 AND principal").await, 1);
    assert_eq!(cuenta("SELECT count(*) FROM red_social WHERE empresa_id = $1").await, 2);
    assert_eq!(cuenta("SELECT count(*) FROM foto_empresa WHERE empresa_id = $1").await, 2);

    // La ficha individual y el listado devuelven lo mismo.
    let (_, detalle) = call(&e.app, "GET", &uri, None, None).await;
    assert_eq!(detalle["telefonos"], v["telefonos"]);
    assert_eq!(detalle["redes_sociales"], v["redes_sociales"]);

    // Lo que no viene en el cuerpo se conserva; una lista vacía borra.
    let (_, v) = call(&e.app, "PUT", &uri, Some(&e.dueno.token), Some(ficha(json!({ "redes_sociales": [] })))).await;
    assert_eq!(v["redes_sociales"], json!([]));
    assert_eq!(v["telefonos"].as_array().unwrap().len(), 2, "sin teléfonos en el cuerpo se conservan");
    assert_eq!(v["fotos"].as_array().unwrap().len(), 2, "sin fotos en el cuerpo se conservan");

    // PUT /telefonos reemplaza la lista entera y respeta el principal marcado.
    let tels = format!("/api/empresas/{}/telefonos", e.empresa);
    let (st, lista) = call(&e.app, "PUT", &tels, Some(&e.dueno.token), Some(json!({ "telefonos": [
        { "numero": "2392777777", "descripcion": "Consultas" },
        { "numero": "2392555555", "principal": true }
    ] }))).await;
    assert_eq!(st, StatusCode::OK, "{lista}");
    assert_eq!(lista.as_array().unwrap().len(), 2);
    assert_eq!(lista[0]["numero"], "2392555555", "el principal va primero");
    assert_eq!(lista[0]["principal"], true);
    assert_eq!(cuenta("SELECT count(*) FROM telefono_empresa WHERE empresa_id = $1 AND numero = '2392111111'").await, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn telefonos_y_redes_invalidos_se_rechazan(pool: PgPool) {
    let e = escenario(pool).await;
    let uri = format!("/api/empresas/{}", e.empresa);
    let tels = format!("/api/empresas/{}/telefonos", e.empresa);
    let ficha = |extra: Value| {
        let mut b = json!({ "nombre": "P", "direccion": "C", "rubro_id": e.rubro });
        b.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        b
    };
    for (caso, cuerpo) in [
        ("letras", json!({ "telefonos": [{ "numero": "23921abc11" }] })),
        ("muy corto", json!({ "telefonos": [{ "numero": "12345" }] })),
        ("repetido", json!({ "telefonos": [{ "numero": "2392111111" }, { "numero": "2392111111" }] })),
        ("dos principales", json!({ "telefonos": [
            { "numero": "2392111111", "principal": true }, { "numero": "2392222222", "principal": true }
        ] })),
        ("canal inválido", json!({ "telefonos": [{ "numero": "2392111111", "canal": "fax" }] })),
        ("red desconocida", json!({ "redes_sociales": [{ "tipo": "myspace", "url": "https://myspace.com/x" }] })),
        ("red repetida", json!({ "redes_sociales": [
            { "tipo": "instagram", "url": "https://instagram.com/a" },
            { "tipo": "instagram", "url": "https://instagram.com/b" }
        ] })),
        ("red sin url", json!({ "redes_sociales": [{ "tipo": "web", "url": " " }] })),
    ] {
        let (st, v) = call(&e.app, "PUT", &uri, Some(&e.dueno.token), Some(ficha(cuerpo))).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{caso}: {v}");
    }
    let (st, _) = call(&e.app, "PUT", &tels, Some(&e.dueno.token), Some(json!({ "telefonos": [{ "numero": "abc" }] }))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    // Los formatos habituales con separadores se aceptan.
    let (st, v) = call(&e.app, "PUT", &tels, Some(&e.dueno.token), Some(json!({ "telefonos": [{ "numero": "+54 9 2392 41-1234" }] }))).await;
    assert_eq!(st, StatusCode::OK, "{v}");
}

#[sqlx::test(migrations = "./migrations")]
async fn crear_un_comercio_lo_asigna_al_usuario_y_valida_el_rubro(pool: PgPool) {
    let e = escenario(pool).await;
    let cuerpo = json!({ "nombre": "Mi local", "direccion": "Calle 1", "rubro_id": e.rubro, "usuario_id": e.superadmin.id });
    assert_eq!(
        call(&e.app, "POST", "/api/empresas", None, Some(cuerpo.clone()))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &e.app,
            "POST",
            "/api/empresas",
            Some(&e.cliente.token),
            Some(cuerpo.clone())
        )
        .await
        .0,
        StatusCode::FORBIDDEN,
        "un cliente no puede registrar un comercio"
    );
    let (st, v) = call(
        &e.app,
        "POST",
        "/api/empresas",
        Some(&e.dueno.token),
        Some(cuerpo.clone()),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(v["usuario_id"], e.dueno.id, "usuario_id ajeno se ignora");
    let (_, v) = call(
        &e.app,
        "POST",
        "/api/empresas",
        Some(&e.superadmin.token),
        Some(cuerpo),
    )
    .await;
    assert_eq!(
        v["usuario_id"], e.superadmin.id,
        "el superadmin sí puede asignar"
    );
    let sin_rubro = json!({ "nombre": "X", "direccion": "Y", "rubro_id": 424242 });
    assert_eq!(
        call(
            &e.app,
            "POST",
            "/api/empresas",
            Some(&e.dueno.token),
            Some(sin_rubro)
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let vacio = json!({ "nombre": " ", "direccion": "Y", "rubro_id": e.rubro });
    assert_eq!(
        call(
            &e.app,
            "POST",
            "/api/empresas",
            Some(&e.dueno.token),
            Some(vacio)
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn los_horarios_se_editan_validan_y_se_leen(pool: PgPool) {
    let e = escenario(pool).await;
    sqlx::query("UPDATE empresa SET slug = 'peluqueria-central' WHERE id = $1")
        .bind(e.empresa)
        .execute(&e.pool)
        .await
        .unwrap();
    let uri = format!("/api/empresas/{}/horarios", e.empresa);
    let nuevos = json!({ "horarios": { "Lunes": ["09:00-12:00", "16:00-20:00"], "Sábado": ["09:00-13:00"] } });
    assert_eq!(
        call(
            &e.app,
            "PUT",
            &uri,
            Some(&e.cliente.token),
            Some(nuevos.clone())
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (st, v) = call(&e.app, "PUT", &uri, Some(&e.dueno.token), Some(nuevos)).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["Lunes"], json!(["09:00-12:00", "16:00-20:00"]));
    assert!(
        v.get("Martes").is_none(),
        "reemplaza los horarios anteriores"
    );
    let (_, leidos) = call(
        &e.app,
        "GET",
        &format!("/api/empresas/horarios/{}", e.empresa),
        None,
        None,
    )
    .await;
    assert_eq!(leidos["Sábado"], json!(["09:00-13:00"]));

    // La tarjeta pide los horarios por la URL pública (slug).
    let (st, por_slug) = call(
        &e.app,
        "GET",
        "/api/empresas/horarios/peluqueria-central",
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(por_slug, leidos);

    for malo in [
        json!({ "Lunes": ["20:00-10:00"] }),
        json!({ "Lunez": ["09:00-10:00"] }),
        json!({ "Lunes": ["9 a 10"] }),
        json!({ "Lunes": ["09:00-13:00", "12:00-16:00"] }),
    ] {
        assert_eq!(
            call(
                &e.app,
                "PUT",
                &uri,
                Some(&e.dueno.token),
                Some(json!({ "horarios": malo }))
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    let (_, tras_error) = call(
        &e.app,
        "GET",
        &format!("/api/empresas/horarios/{}", e.empresa),
        None,
        None,
    )
    .await;
    assert_eq!(
        tras_error["Sábado"],
        json!(["09:00-13:00"]),
        "un pedido inválido no deja los horarios a medias"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn los_telefonos_se_reemplazan_y_un_repetido_se_informa(pool: PgPool) {
    let e = escenario(pool).await;
    let uri = format!("/api/empresas/{}/telefonos", e.empresa);
    let repetido = json!({ "telefonos": [
        { "numero": "2392111111", "descripcion": "Turnos", "canal": "mensaje" },
        { "numero": "2392111111" }
    ] });
    let (st, v) = call(&e.app, "PUT", &uri, Some(&e.dueno.token), Some(repetido)).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    assert!(v["error"].as_str().unwrap().contains("repetido"), "{v}");
    let cuerpo = json!({ "telefonos": [
        { "numero": "2392111111", "descripcion": "Turnos", "canal": "mensaje" },
        { "numero": " 2392222222 " }
    ] });
    let (st, v) = call(&e.app, "PUT", &uri, Some(&e.dueno.token), Some(cuerpo)).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v[1]["numero"], "2392222222", "se guarda sin espacios sobrantes");
    assert_eq!(v.as_array().unwrap().len(), 2);
    assert_eq!(v[0]["canal"], "mensaje");
    assert!(
        v[1]["canal"].is_null(),
        "sin modo hereda el del comercio"
    );
    assert_eq!(
        call(
            &e.app,
            "PUT",
            &uri,
            Some(&e.dueno.token),
            Some(json!({ "telefonos": [{ "numero": "2392333333", "canal": "paloma" }] }))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &e.app,
            "PUT",
            &uri,
            Some(&e.dueno.token),
            Some(json!({ "telefonos": [{ "numero": " " }] }))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &e.app,
            "PUT",
            &uri,
            Some(&e.cliente.token),
            Some(json!({ "telefonos": [] }))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn no_se_elimina_un_comercio_con_turnos(pool: PgPool) {
    let e = escenario(pool).await;
    let f = fecha(Weekday::Tue, 3);
    assert_eq!(
        call(
            &e.app,
            "POST",
            "/api/turnos/reservar",
            Some(&e.cliente.token),
            Some(reserva(e.empresa, f, "09:00"))
        )
        .await
        .0,
        StatusCode::OK
    );
    let uri = format!("/api/empresas/{}", e.empresa);
    assert_eq!(
        call(&e.app, "DELETE", &uri, Some(&e.dueno.token), None)
            .await
            .0,
        StatusCode::CONFLICT
    );
    let vacia = crear_empresa(
        &e.pool,
        "Sin turnos",
        e.rubro,
        Some(e.dueno.id),
        false,
        None,
    )
    .await;
    assert_eq!(
        call(
            &e.app,
            "DELETE",
            &format!("/api/empresas/{vacia}"),
            Some(&e.dueno.token),
            None
        )
        .await
        .0,
        StatusCode::OK
    );
}

// ------------------------------------------------------------------ turnos

#[sqlx::test(migrations = "./migrations")]
async fn reservar_un_turno_lo_deja_ocupado_y_en_mis_turnos(pool: PgPool) {
    let e = escenario(pool).await;
    let f = fecha(Weekday::Tue, 3);
    let (st, reservado) = call(
        &e.app,
        "POST",
        "/api/turnos/reservar",
        Some(&e.cliente.token),
        Some(reserva(e.empresa, f, "10:30")),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (_, ocupados) = call(
        &e.app,
        "GET",
        &format!("/api/turnos/ocupados?empresa_id={}&fecha={f}", e.empresa),
        None,
        None,
    )
    .await;
    assert_eq!(ocupados, json!([{ "hora": "10:30" }]));
    let (_, mios) = call(
        &e.app,
        "GET",
        "/api/turnos/mis-turnos",
        Some(&e.cliente.token),
        None,
    )
    .await;
    assert_eq!(mios[0]["estado"], "confirmado");
    assert_eq!(mios[0]["empresa_nombre"], "Peluquería Central");
    assert!(mios[0]["empresa_direccion"].is_string(), "el calendario necesita el lugar");
    assert_eq!(mios[0]["id"], reservado["id"], "la reserva devuelve el id del turno");
    let (_, ajenos) = call(
        &e.app,
        "GET",
        "/api/turnos/mis-turnos",
        Some(&e.dueno.token),
        None,
    )
    .await;
    assert_eq!(total_de(&ajenos), 0, "cada usuario ve solo sus turnos");
}

#[sqlx::test(migrations = "./migrations")]
async fn dos_reservas_simultaneas_del_mismo_horario_solo_confirman_una(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    let f = fecha(Weekday::Wed, 3);
    for ronda in 0..3 {
        let hora = ["09:00", "09:30", "10:00"][ronda];
        let mut tareas = Vec::new();
        for i in 0..8 {
            let u = crear_usuario(&pool, &format!("c{ronda}-{i}@test.com"), "cliente").await;
            let app = e.app.clone();
            let cuerpo = reserva(e.empresa, f, hora);
            tareas.push(tokio::spawn(async move {
                call(
                    &app,
                    "POST",
                    "/api/turnos/reservar",
                    Some(&u.token),
                    Some(cuerpo),
                )
                .await
                .0
            }));
        }
        let mut estados = Vec::new();
        for t in tareas {
            estados.push(t.await.unwrap());
        }
        assert_eq!(
            estados.iter().filter(|s| **s == StatusCode::OK).count(),
            1,
            "ronda {ronda}: {estados:?}"
        );
        assert_eq!(
            estados
                .iter()
                .filter(|s| **s == StatusCode::CONFLICT)
                .count(),
            7,
            "ronda {ronda}: {estados:?}"
        );
    }
    let confirmados: i64 =
        sqlx::query_scalar("SELECT count(*) FROM turno WHERE estado_turno_id = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(confirmados, 3);
}

#[sqlx::test(migrations = "./migrations")]
async fn una_reserva_debe_respetar_los_horarios_del_comercio(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    let martes = fecha(Weekday::Tue, 3);
    let domingo = fecha(Weekday::Sun, 3);
    let reservar = |empresa: i32, f: NaiveDate, hora: &'static str, token: Option<String>| {
        let app = e.app.clone();
        async move {
            call(
                &app,
                "POST",
                "/api/turnos/reservar",
                token.as_deref(),
                Some(reserva(empresa, f, hora)),
            )
            .await
        }
    };
    let tok = Some(e.cliente.token.clone());

    assert_eq!(
        reservar(e.empresa, martes, "10:00", None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        reservar(999_999, martes, "10:00", tok.clone()).await.0,
        StatusCode::NOT_FOUND
    );
    let (st, v) = reservar(e.empresa, domingo, "10:00", tok.clone()).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(v["error"].as_str().unwrap().contains("no atiende"), "{v}");
    for (hora, motivo) in [
        ("03:00", "madrugada"),
        ("12:00", "hora de cierre"),
        ("13:00", "entre franjas"),
        ("20:00", "cierre de la tarde"),
        ("09:07", "fuera de la grilla"),
        ("09:15", "fuera de la grilla"),
    ] {
        assert_eq!(
            reservar(e.empresa, martes, hora, tok.clone()).await.0,
            StatusCode::BAD_REQUEST,
            "{motivo}: {hora}"
        );
    }
    assert_eq!(
        reservar(e.empresa, martes, "abc", tok.clone()).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        reservar(
            e.empresa,
            NaiveDate::from_ymd_opt(2020, 1, 7).unwrap(),
            "10:00",
            tok.clone()
        )
        .await
        .0,
        StatusCode::BAD_REQUEST,
        "fecha pasada"
    );
    assert_eq!(
        reservar(e.empresa, martes, "19:30", tok.clone()).await.0,
        StatusCode::OK,
        "último turno de la tarde"
    );
    assert_eq!(
        reservar(e.empresa, martes, "16:00", tok.clone()).await.0,
        StatusCode::OK,
        "la grilla arranca en cada franja"
    );

    let sin_turnos = crear_empresa(&pool, "Solo ficha", e.rubro, None, false, None).await;
    let (st, v) = reservar(sin_turnos, martes, "10:00", tok).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(v["error"].as_str().unwrap().contains("no toma turnos"));
}

#[sqlx::test(migrations = "./migrations")]
async fn la_grilla_usa_la_duracion_del_turno_del_comercio(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    let f = fecha(Weekday::Tue, 3);
    sqlx::query("UPDATE empresa SET duracion_turno_min = 45 WHERE id = $1")
        .bind(e.empresa)
        .execute(&pool)
        .await
        .unwrap();
    let r = |h: &'static str| {
        let app = e.app.clone();
        let t = e.cliente.token.clone();
        let cuerpo = reserva(e.empresa, f, h);
        async move {
            call(&app, "POST", "/api/turnos/reservar", Some(&t), Some(cuerpo))
                .await
                .0
        }
    };
    assert_eq!(r("09:45").await, StatusCode::OK);
    assert_eq!(r("10:30").await, StatusCode::OK);
    assert_eq!(
        r("10:00").await,
        StatusCode::BAD_REQUEST,
        "no es múltiplo de 45 desde las 09:00"
    );
    assert_eq!(
        r("11:15").await,
        StatusCode::OK,
        "termina justo a las 12:00"
    );
    assert_eq!(
        r("11:30").await,
        StatusCode::BAD_REQUEST,
        "se pasaría del cierre"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn cancelar_conserva_el_historial_y_libera_el_horario(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    let f = fecha(Weekday::Tue, 3);
    let otro = crear_usuario(&pool, "otro@test.com", "cliente").await;
    call(
        &e.app,
        "POST",
        "/api/turnos/reservar",
        Some(&e.cliente.token),
        Some(reserva(e.empresa, f, "10:00")),
    )
    .await;
    let id: i32 = sqlx::query_scalar("SELECT id FROM turno WHERE usuario_id = $1")
        .bind(e.cliente.id)
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(
        call(
            &e.app,
            "DELETE",
            &format!("/api/turnos/cancelar/{id}"),
            Some(&otro.token),
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND,
        "turno ajeno"
    );
    assert_eq!(
        call(
            &e.app,
            "DELETE",
            &format!("/api/turnos/cancelar/{id}"),
            Some(&e.cliente.token),
            None
        )
        .await
        .0,
        StatusCode::OK
    );
    let estado: String = sqlx::query_scalar("SELECT et.nombre::text FROM turno t JOIN estado_turno et ON et.id = t.estado_turno_id WHERE t.id = $1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(estado, "cancelado", "no se borra");

    let (_, ocupados) = call(
        &e.app,
        "GET",
        &format!("/api/turnos/ocupados?empresa_id={}&fecha={f}", e.empresa),
        None,
        None,
    )
    .await;
    assert_eq!(ocupados, json!([]));
    let (_, activos) = call(
        &e.app,
        "GET",
        "/api/turnos/mis-turnos",
        Some(&e.cliente.token),
        None,
    )
    .await;
    assert_eq!(total_de(&activos), 0);
    let (_, todos) = call(
        &e.app,
        "GET",
        "/api/turnos/mis-turnos?incluir_cancelados=true",
        Some(&e.cliente.token),
        None,
    )
    .await;
    assert_eq!(todos[0]["estado"], "cancelado");

    assert_eq!(
        call(
            &e.app,
            "DELETE",
            &format!("/api/turnos/cancelar/{id}"),
            Some(&e.cliente.token),
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND,
        "ya cancelado"
    );
    assert_eq!(
        call(
            &e.app,
            "POST",
            "/api/turnos/reservar",
            Some(&otro.token),
            Some(reserva(e.empresa, f, "10:00"))
        )
        .await
        .0,
        StatusCode::OK,
        "otro cliente puede tomar el horario liberado"
    );
    assert_eq!(
        call(
            &e.app,
            "POST",
            "/api/turnos/reservar",
            Some(&e.cliente.token),
            Some(reserva(e.empresa, f, "10:00"))
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn solo_se_cancela_con_la_anticipacion_minima_del_comercio(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    let f = fecha(Weekday::Tue, 3);
    call(
        &e.app,
        "POST",
        "/api/turnos/reservar",
        Some(&e.cliente.token),
        Some(reserva(e.empresa, f, "10:00")),
    )
    .await;
    let id: i32 = sqlx::query_scalar("SELECT id FROM turno")
        .fetch_one(&pool)
        .await
        .unwrap();
    // 30 días de anticipación exigidos: el turno (a menos de 10 días) ya no se puede cancelar
    sqlx::query("UPDATE empresa SET anticipacion_cancelacion_min = 43200 WHERE id = $1")
        .bind(e.empresa)
        .execute(&pool)
        .await
        .unwrap();
    let (st, v) = call(
        &e.app,
        "DELETE",
        &format!("/api/turnos/cancelar/{id}"),
        Some(&e.cliente.token),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(v["error"].as_str().unwrap().contains("anticipación"), "{v}");
    let estado: String = sqlx::query_scalar("SELECT et.nombre::text FROM turno t JOIN estado_turno et ON et.id = t.estado_turno_id WHERE t.id = $1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(estado, "confirmado");
    // con la política por defecto (2 h) sí
    sqlx::query("UPDATE empresa SET anticipacion_cancelacion_min = 120 WHERE id = $1")
        .bind(e.empresa)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        call(
            &e.app,
            "DELETE",
            &format!("/api/turnos/cancelar/{id}"),
            Some(&e.cliente.token),
            None
        )
        .await
        .0,
        StatusCode::OK
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn el_dueno_cancela_sin_la_anticipacion_del_cliente(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    let f = fecha(Weekday::Tue, 3);
    call(
        &e.app,
        "POST",
        "/api/turnos/reservar",
        Some(&e.cliente.token),
        Some(reserva(e.empresa, f, "10:00")),
    )
    .await;
    let id: i32 = sqlx::query_scalar("SELECT id FROM turno")
        .fetch_one(&pool)
        .await
        .unwrap();
    // El cliente no podría cancelar con 30 días de anticipación exigidos, pero el dueño sí.
    sqlx::query("UPDATE empresa SET anticipacion_cancelacion_min = 43200 WHERE id = $1")
        .bind(e.empresa)
        .execute(&pool)
        .await
        .unwrap();
    let uri = format!("/api/turnos/cancelar/{id}");
    let otro_admin = crear_usuario(&pool, "otro-admin@test.com", "admin").await;

    for (token, caso) in [
        (otro_admin.token.as_str(), "admin de otro comercio"),
        (e.superadmin.token.as_str(), "superadmin"),
    ] {
        assert_eq!(
            call(&e.app, "DELETE", &uri, Some(token), None).await.0,
            StatusCode::NOT_FOUND,
            "{caso}"
        );
    }

    assert_eq!(
        call(&e.app, "DELETE", &uri, Some(&e.dueno.token), None)
            .await
            .0,
        StatusCode::OK
    );
    let estado: String = sqlx::query_scalar("SELECT et.nombre::text FROM turno t JOIN estado_turno et ON et.id = t.estado_turno_id WHERE t.id = $1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(estado, "cancelado");
    let (_, ocupados) = call(
        &e.app,
        "GET",
        &format!("/api/turnos/ocupados?empresa_id={}&fecha={f}", e.empresa),
        None,
        None,
    )
    .await;
    assert_eq!(ocupados, json!([]));
    let (_, todos) = call(
        &e.app,
        "GET",
        "/api/turnos/mis-turnos?incluir_cancelados=true",
        Some(&e.cliente.token),
        None,
    )
    .await;
    assert_eq!(todos[0]["estado"], "cancelado");
    let pasado: i32 = sqlx::query_scalar(
        "INSERT INTO turno (empresa_id, usuario_id, fecha, hora, estado_turno_id) \
         VALUES ($1, $2, CURRENT_DATE - 3, '11:00', 1) RETURNING id",
    )
    .bind(e.empresa)
    .bind(e.cliente.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        call(
            &e.app,
            "DELETE",
            &format!("/api/turnos/cancelar/{pasado}"),
            Some(&e.dueno.token),
            None
        )
        .await
        .0,
        StatusCode::BAD_REQUEST,
        "el dueño tampoco cancela turnos pasados"
    );
    assert_eq!(
        call(&e.app, "DELETE", &uri, Some(&e.dueno.token), None)
            .await
            .0,
        StatusCode::NOT_FOUND,
        "ya cancelado"
    );
}

// ------------------------------------------------------------------ calificaciones, favoritos, publicidades

#[sqlx::test(migrations = "./migrations")]
async fn las_calificaciones_son_por_turno_pasado_y_actualizan_en_vez_de_duplicar(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    let otro = crear_usuario(&pool, "otro@test.com", "cliente").await;
    let uri = format!("/api/empresas/{}/calificaciones", e.empresa);

    // Turnos del cliente: uno pasado, uno futuro y uno cancelado; más uno pasado de otro cliente.
    let pasado: i32 = sqlx::query_scalar(
        "INSERT INTO turno (empresa_id, usuario_id, fecha, hora, estado_turno_id) \
         VALUES ($1, $2, CURRENT_DATE - 7, '10:00', 1) RETURNING id",
    )
    .bind(e.empresa)
    .bind(e.cliente.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let futuro: i32 = sqlx::query_scalar(
        "INSERT INTO turno (empresa_id, usuario_id, fecha, hora, estado_turno_id) \
         VALUES ($1, $2, CURRENT_DATE + 7, '10:00', 1) RETURNING id",
    )
    .bind(e.empresa)
    .bind(e.cliente.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let cancelado: i32 = sqlx::query_scalar(
        "INSERT INTO turno (empresa_id, usuario_id, fecha, hora, estado_turno_id) \
         VALUES ($1, $2, CURRENT_DATE - 5, '11:00', 2) RETURNING id",
    )
    .bind(e.empresa)
    .bind(e.cliente.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let ajeno: i32 = sqlx::query_scalar(
        "INSERT INTO turno (empresa_id, usuario_id, fecha, hora, estado_turno_id) \
         VALUES ($1, $2, CURRENT_DATE - 6, '16:00', 1) RETURNING id",
    )
    .bind(e.empresa)
    .bind(otro.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let otra_empresa = crear_empresa(&pool, "Otro comercio", e.rubro, None, false, None).await;

    for malo in [0, 6, 99, -1] {
        assert_eq!(
            call(
                &e.app,
                "POST",
                &uri,
                Some(&e.cliente.token),
                Some(json!({ "turno_id": pasado, "puntaje": malo }))
            )
            .await
            .0,
            StatusCode::BAD_REQUEST,
            "{malo}"
        );
    }
    assert_eq!(
        call(
            &e.app,
            "POST",
            &uri,
            None,
            Some(json!({ "turno_id": pasado, "puntaje": 5 }))
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &e.app,
            "POST",
            &uri,
            Some(&e.dueno.token),
            Some(json!({ "turno_id": pasado, "puntaje": 5 }))
        )
        .await
        .0,
        StatusCode::FORBIDDEN,
        "no se califica el propio comercio"
    );
    assert_eq!(
        call(
            &e.app,
            "POST",
            &uri,
            Some(&e.cliente.token),
            Some(json!({ "puntaje": 5 }))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST,
        "el turno es obligatorio (RF-13)"
    );
    assert_eq!(
        call(
            &e.app,
            "POST",
            "/api/empresas/999999/calificaciones",
            Some(&e.cliente.token),
            Some(json!({ "turno_id": pasado, "puntaje": 5 }))
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let largo = "x".repeat(1001);
    assert_eq!(
        call(
            &e.app,
            "POST",
            &uri,
            Some(&e.cliente.token),
            Some(json!({ "turno_id": pasado, "puntaje": 5, "comentario": largo }))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &e.app,
            "POST",
            &uri,
            Some(&e.cliente.token),
            Some(json!({ "turno_id": ajeno, "puntaje": 5 }))
        )
        .await
        .0,
        StatusCode::NOT_FOUND,
        "turno de otro cliente"
    );
    assert_eq!(
        call(
            &e.app,
            "POST",
            &uri,
            Some(&e.cliente.token),
            Some(json!({ "turno_id": futuro, "puntaje": 5 }))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST,
        "todavía no pasó su horario"
    );
    assert_eq!(
        call(
            &e.app,
            "POST",
            &uri,
            Some(&e.cliente.token),
            Some(json!({ "turno_id": cancelado, "puntaje": 5 }))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST,
        "un turno cancelado no se califica"
    );
    assert_eq!(
        call(
            &e.app,
            "POST",
            &format!("/api/empresas/{otra_empresa}/calificaciones"),
            Some(&e.cliente.token),
            Some(json!({ "turno_id": pasado, "puntaje": 5 }))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST,
        "el turno es de otro comercio"
    );

    assert_eq!(
        call(
            &e.app,
            "POST",
            &uri,
            Some(&e.cliente.token),
            Some(json!({ "turno_id": pasado, "puntaje": 5, "comentario": "Excelente" }))
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &e.app,
            "POST",
            &uri,
            Some(&otro.token),
            Some(json!({ "turno_id": ajeno, "puntaje": 2 }))
        )
        .await
        .0,
        StatusCode::OK
    );
    let (_, ficha) = call(
        &e.app,
        "GET",
        &format!("/api/empresas/{}", e.empresa),
        None,
        None,
    )
    .await;
    assert_eq!(
        (
            ficha["calificacion_promedio"].as_f64(),
            ficha["calificacion_cantidad"].as_i64()
        ),
        (Some(3.5), Some(2))
    );

    // calificar de nuevo el mismo turno actualiza en vez de duplicar
    assert_eq!(
        call(
            &e.app,
            "POST",
            &uri,
            Some(&e.cliente.token),
            Some(json!({ "turno_id": pasado, "puntaje": 1 }))
        )
        .await
        .0,
        StatusCode::OK
    );
    let (_, ficha) = call(
        &e.app,
        "GET",
        &format!("/api/empresas/{}", e.empresa),
        None,
        None,
    )
    .await;
    assert_eq!(
        (
            ficha["calificacion_promedio"].as_f64(),
            ficha["calificacion_cantidad"].as_i64()
        ),
        (Some(1.5), Some(2))
    );
    let (_, resenas) = call(&e.app, "GET", &uri, None, None).await;
    assert_eq!(total_de(&resenas), 2);

    // el historial indica qué turnos ya se calificaron (lo usa el front para ofrecer "Calificar")
    let (_, mios) = call(
        &e.app,
        "GET",
        "/api/turnos/mis-turnos",
        Some(&e.cliente.token),
        None,
    )
    .await;
    let mio = |id: i32| {
        mios.as_array()
            .unwrap()
            .iter()
            .find(|t| t["id"] == id)
            .unwrap()
            .clone()
    };
    assert_eq!(mio(pasado)["calificado"], true);
    assert_eq!(mio(futuro)["calificado"], false);
    assert_eq!(mio(pasado)["empresa_id"], e.empresa);
}

#[sqlx::test(migrations = "./migrations")]
async fn los_favoritos_son_por_usuario_e_idempotentes(pool: PgPool) {
    let e = escenario(pool).await;
    let uri = format!("/api/favoritos/{}", e.empresa);
    assert_eq!(
        call(&e.app, "PUT", &uri, None, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&e.app, "PUT", &uri, Some(&e.cliente.token), None)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        call(&e.app, "PUT", &uri, Some(&e.cliente.token), None)
            .await
            .0,
        StatusCode::OK,
        "repetir no falla"
    );
    assert_eq!(
        call(
            &e.app,
            "GET",
            "/api/favoritos",
            Some(&e.cliente.token),
            None
        )
        .await
        .1,
        json!([e.empresa])
    );
    assert_eq!(
        call(&e.app, "GET", "/api/favoritos", Some(&e.dueno.token), None)
            .await
            .1,
        json!([])
    );
    // El dueño de un comercio (y el superadmin) no marcan favoritos.
    for token in [&e.dueno.token, &e.superadmin.token] {
        assert_eq!(
            call(&e.app, "PUT", &uri, Some(token), None).await.0,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        call(
            &e.app,
            "PUT",
            "/api/favoritos/999999",
            Some(&e.cliente.token),
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&e.app, "DELETE", &uri, Some(&e.cliente.token), None)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &e.app,
            "GET",
            "/api/favoritos",
            Some(&e.cliente.token),
            None
        )
        .await
        .1,
        json!([])
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn las_publicidades_las_carga_el_dueno_y_las_aprueba_el_superadmin(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    let otra = crear_empresa(&pool, "Otra", e.rubro, None, false, None).await;
    let nueva = json!({ "empresa_id": e.empresa, "titulo": "2x1", "descripcion": "Este mes", "url_imagen": "https://x.com/a.jpg" });
    let post = |token: Option<&str>, cuerpo: Value| {
        let app = e.app.clone();
        let token = token.map(str::to_string);
        async move { call(&app, "POST", "/api/publicidades", token.as_deref(), Some(cuerpo)).await }
    };
    assert_eq!(post(None, nueva.clone()).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(post(Some(&e.cliente.token), nueva.clone()).await.0, StatusCode::FORBIDDEN);
    let mut ajena = nueva.clone();
    ajena["empresa_id"] = json!(otra);
    assert_eq!(post(Some(&e.dueno.token), ajena).await.0, StatusCode::FORBIDDEN, "no publica en comercios ajenos");
    for (campo, valor) in [
        ("url_imagen", json!("javascript:alert(1)")),
        ("url_imagen", json!("")),
        ("titulo", json!(" ")),
        ("titulo", json!("x".repeat(101))),
        ("descripcion", json!(" ")),
        ("descripcion", json!("x".repeat(501))),
        ("fecha_fin", json!("2020-01-01")),
    ] {
        let mut mala = nueva.clone();
        mala[campo] = valor;
        if campo == "fecha_fin" {
            mala["fecha_inicio"] = json!("2020-02-01");
        }
        assert_eq!(post(Some(&e.dueno.token), mala).await.0, StatusCode::BAD_REQUEST, "{campo}");
    }

    // RF-06: la publicidad del dueño nace pendiente y no aparece en la portada.
    let (st, creada) = post(Some(&e.dueno.token), nueva.clone()).await;
    assert_eq!(st, StatusCode::CREATED);
    assert!(creada["id"].is_string(), "el front espera el id como texto");
    assert_eq!(creada["estado"], "pendiente");
    assert_eq!(creada["empresa_nombre"], "Peluquería Central");
    let id = creada["id"].as_str().unwrap().to_string();
    assert_eq!(total_de(&call(&e.app, "GET", "/api/publicidades", None, None).await.1), 0);
    // El dueño ve las suyas con su estado; un cliente no ve la bandeja.
    let (st, mias) = call(&e.app, "GET", &format!("/api/publicidades?empresa_id={}&estado=todas", e.empresa), Some(&e.dueno.token), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(mias[0]["estado"], "pendiente");
    assert_eq!(call(&e.app, "GET", "/api/publicidades?estado=pendiente", Some(&e.dueno.token), None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&e.app, "GET", "/api/publicidades?estado=pendiente", None, None).await.0, StatusCode::UNAUTHORIZED);
    let (_, bandeja) = call(&e.app, "GET", "/api/publicidades?estado=pendiente", Some(&e.superadmin.token), None).await;
    assert_eq!(total_de(&bandeja), 1);

    // Solo el superadmin aprueba; el dueño no puede aprobarse a sí mismo.
    let uri = format!("/api/publicidades/{id}/estado");
    assert_eq!(call(&e.app, "PUT", &uri, Some(&e.dueno.token), Some(json!({ "estado": "aprobada" }))).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&e.app, "PUT", &uri, Some(&e.superadmin.token), Some(json!({ "estado": "publicada" }))).await.0, StatusCode::BAD_REQUEST);
    let (st, aprobada) = call(&e.app, "PUT", &uri, Some(&e.superadmin.token), Some(json!({ "estado": "aprobada" }))).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(aprobada["estado"], "aprobada");
    let (_, portada) = call(&e.app, "GET", "/api/publicidades", None, None).await;
    assert_eq!(total_de(&portada), 1);
    assert_eq!(portada[0]["url_imagen"], "https://x.com/a.jpg");
    assert_eq!(total_de(&call(&e.app, "GET", &format!("/api/publicidades?empresa_id={otra}"), None, None).await.1), 0);

    // Una publicidad vencida no se muestra aunque esté aprobada.
    let mut vencida = nueva.clone();
    vencida["fecha_inicio"] = json!("2020-01-01");
    vencida["fecha_fin"] = json!("2020-01-31");
    let (st, v) = post(Some(&e.superadmin.token), vencida).await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(v["estado"], "aprobada", "la que carga el superadmin nace aprobada");
    assert_eq!(total_de(&call(&e.app, "GET", "/api/publicidades", None, None).await.1), 1);

    // El dueño puede darla de baja, pero no reabrirla si no fue rechazada.
    assert_eq!(call(&e.app, "PUT", &uri, Some(&e.dueno.token), Some(json!({ "estado": "pendiente" }))).await.0, StatusCode::FORBIDDEN);
    let (st, baja) = call(&e.app, "PUT", &uri, Some(&e.dueno.token), Some(json!({ "estado": "finalizada" }))).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(baja["estado"], "finalizada");
    assert_eq!(total_de(&call(&e.app, "GET", "/api/publicidades", None, None).await.1), 0);

    let borrar = format!("/api/publicidades/{id}");
    assert_eq!(call(&e.app, "DELETE", &borrar, Some(&e.cliente.token), None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&e.app, "DELETE", &borrar, Some(&e.dueno.token), None).await.0, StatusCode::OK);
    assert_eq!(call(&e.app, "DELETE", &borrar, Some(&e.dueno.token), None).await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "./migrations")]
async fn una_publicidad_rechazada_se_puede_corregir_y_reenviar(pool: PgPool) {
    let e = escenario(pool).await;
    let nueva = json!({ "empresa_id": e.empresa, "titulo": "Promo", "descripcion": "Corte y barba", "url_imagen": "https://x.com/p.jpg" });
    let (_, creada) = call(&e.app, "POST", "/api/publicidades", Some(&e.dueno.token), Some(nueva)).await;
    let uri = format!("/api/publicidades/{}/estado", creada["id"].as_str().unwrap());
    let (_, rechazada) = call(&e.app, "PUT", &uri, Some(&e.superadmin.token), Some(json!({ "estado": "rechazada" }))).await;
    assert_eq!(rechazada["estado"], "rechazada");
    let (st, otra_vez) = call(&e.app, "PUT", &uri, Some(&e.dueno.token), Some(json!({ "estado": "pendiente" }))).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(otra_vez["estado"], "pendiente");
}

#[sqlx::test(migrations = "./migrations")]
async fn el_canal_publico_pide_difusiones_y_el_superadmin_las_aprueba(pool: PgPool) {
    let e = escenario(pool).await;
    let pedido = json!({
        "tipo": "colecta", "titulo": "Colecta para el club", "descripcion": "Arreglamos el techo del vestuario",
        "solicitante_nombre": "Comisión del club", "solicitante_contacto": "club@test.com"
    });
    let post = |token: Option<&str>, cuerpo: Value| {
        let app = e.app.clone();
        let token = token.map(str::to_string);
        async move { call(&app, "POST", "/api/difusiones", token.as_deref(), Some(cuerpo)).await }
    };
    for (campo, valor) in [
        ("tipo", json!("rifa")),
        ("titulo", json!("x")),
        ("descripcion", json!("corta")),
        ("solicitante_nombre", json!("")),
        ("solicitante_contacto", json!("sin datos")),
        ("url_imagen", json!("javascript:alert(1)")),
    ] {
        let mut malo = pedido.clone();
        malo[campo] = valor;
        assert_eq!(post(None, malo).await.0, StatusCode::BAD_REQUEST, "{campo}");
    }
    // RF-06: se pide sin cuenta y queda pendiente, fuera de la portada.
    let (st, creada) = post(None, pedido.clone()).await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(creada["estado"], "pendiente");
    assert_eq!(total_de(&call(&e.app, "GET", "/api/difusiones", None, None).await.1), 0);
    let mut mayusculas = pedido.clone();
    mayusculas["solicitante_contacto"] = json!("CLUB@test.com");
    assert_eq!(post(None, mayusculas).await.0, StatusCode::CONFLICT, "un pendiente por contacto");

    assert_eq!(call(&e.app, "GET", "/api/difusiones?estado=pendiente", Some(&e.dueno.token), None).await.0, StatusCode::FORBIDDEN);
    let (_, bandeja) = call(&e.app, "GET", "/api/difusiones?estado=pendiente", Some(&e.superadmin.token), None).await;
    assert_eq!(total_de(&bandeja), 1);
    assert_eq!(bandeja[0]["solicitante_contacto"], "club@test.com");

    let uri = format!("/api/difusiones/{}/estado", creada["id"]);
    assert_eq!(call(&e.app, "PUT", &uri, Some(&e.dueno.token), Some(json!({ "estado": "aprobada" }))).await.0, StatusCode::FORBIDDEN);
    let (st, aprobada) = call(&e.app, "PUT", &uri, Some(&e.superadmin.token), Some(json!({ "estado": "aprobada" }))).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(aprobada["estado"], "aprobada");
    let (_, portada) = call(&e.app, "GET", "/api/difusiones", None, None).await;
    assert_eq!(total_de(&portada), 1);
    assert_eq!(portada[0]["titulo"], "Colecta para el club");
    // La portada es pública: no expone el nombre ni el contacto de quien pidió la difusión.
    assert!(portada[0].get("solicitante_nombre").is_none());
    assert!(portada[0].get("solicitante_contacto").is_none());
    // Ya no está pendiente: el mismo contacto puede pedir otra.
    assert_eq!(post(None, pedido).await.0, StatusCode::CREATED);
    assert_eq!(call(&e.app, "PUT", &uri, Some(&e.superadmin.token), Some(json!({ "estado": "pendiente" }))).await.0, StatusCode::CONFLICT);

    // La que carga el superadmin nace aprobada, con sus datos como solicitante.
    let (st, propia) = post(
        Some(&e.superadmin.token),
        json!({ "tipo": "evento", "titulo": "Maratón solidaria", "descripcion": "Domingo a las 9 en la plaza" }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(propia["estado"], "aprobada");
    assert_eq!(propia["solicitante_contacto"], "super@test.com");
    let borrar = format!("/api/difusiones/{}", propia["id"]);
    assert_eq!(call(&e.app, "DELETE", &borrar, Some(&e.dueno.token), None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&e.app, "DELETE", &borrar, Some(&e.superadmin.token), None).await.0, StatusCode::OK);
    assert_eq!(call(&e.app, "DELETE", &borrar, Some(&e.superadmin.token), None).await.0, StatusCode::NOT_FOUND);
}

// ------------------------------------------------------------------ esquema

#[sqlx::test(migrations = "./migrations")]
async fn el_esquema_impide_datos_incoherentes(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    let falla = |sql: &'static str| {
        let pool = pool.clone();
        async move { sqlx::query(sql).execute(&pool).await.is_err() }
    };
    assert!(falla("INSERT INTO calificacion (usuario_id, empresa_id, puntaje) SELECT u.id, e.id, 9 FROM usuario u, empresa e LIMIT 1").await, "puntaje fuera de 1-5");
    assert!(falla("INSERT INTO usuario (nombre, email, contrasena_hash, rol) VALUES ('x', 'x@x.com', 'h', 'dios')").await, "rol inválido");
    assert!(falla("INSERT INTO usuario (nombre, email, contrasena_hash) VALUES ('x', 'CLIENTE@test.com', 'h')").await, "email repetido sin importar mayúsculas");
    assert!(falla("INSERT INTO horario_empresa (empresa_id, dia_semana, hora_inicio, hora_fin) SELECT id, 'Lunes', '10:00', '09:00' FROM empresa").await, "franja al revés");
    assert!(falla("INSERT INTO horario_empresa (empresa_id, dia_semana, hora_inicio, hora_fin) SELECT id, 'Lunez', '09:00', '10:00' FROM empresa").await, "día inexistente");
    // 0015: un comercio tiene un solo rubro, que es obligatorio.
    assert!(falla("UPDATE empresa SET rubro_id = NULL").await, "comercio sin rubro");
    // VARCHAR(N): la base rechaza un texto más largo que su columna.
    assert!(falla("UPDATE empresa SET nombre = repeat('x', 101)").await, "nombre de más de 100");
    assert!(falla("UPDATE usuario SET email = repeat('x', 250) || '@x.com'").await, "email de más de 254");
    // Un solo teléfono principal y una sola red de cada tipo por comercio.
    sqlx::query("INSERT INTO telefono_empresa (empresa_id, numero, principal) VALUES ($1, '2392111111', true)")
        .bind(e.empresa)
        .execute(&pool)
        .await
        .unwrap();
    assert!(falla("INSERT INTO telefono_empresa (empresa_id, numero, principal) SELECT id, '2392222222', true FROM empresa").await, "dos principales");
    assert!(falla("INSERT INTO red_social (empresa_id, tipo, url) SELECT id, 'myspace', 'https://x.com' FROM empresa").await, "red desconocida");
    sqlx::query("INSERT INTO red_social (empresa_id, tipo, url) VALUES ($1, 'instagram', 'https://instagram.com/a')")
        .bind(e.empresa)
        .execute(&pool)
        .await
        .unwrap();
    assert!(falla("INSERT INTO red_social (empresa_id, tipo, url) SELECT id, 'instagram', 'https://instagram.com/b' FROM empresa").await, "dos instagram");
    // El estado del turno es una fila de estado_turno y el de la publicidad, de estado_aprobacion.
    assert!(falla("INSERT INTO turno (empresa_id, usuario_id, fecha, hora, estado_turno_id) SELECT e.id, u.id, '2031-01-07', '09:00', 9 FROM empresa e, usuario u LIMIT 1").await, "estado de turno inexistente");
    assert!(falla("INSERT INTO publicidad (empresa_id, titulo, descripcion, url_imagen, estado_aprobacion_id) SELECT id, 't', 'd', 'https://x.com/i.jpg', 9 FROM empresa").await, "estado de aprobación inexistente");
    assert!(falla("INSERT INTO publicidad (empresa_id, titulo, descripcion, url_imagen) VALUES (NULL, 't', 'd', 'https://x.com/i.jpg')").await, "publicidad sin comercio");

    // 0013: reglas de RF-13, RF-15 y RF-16 que antes solo cuidaba la API.
    assert!(falla("UPDATE empresa SET verificado = true, verificado_estado = 'pendiente'").await, "sello y estado contradictorios");
    assert!(falla("UPDATE empresa SET cuit = '20-12345678-6'").await, "CUIT sin normalizar");
    sqlx::query("INSERT INTO reporte (empresa_id, usuario_id, descripcion) VALUES ($1, $2, 'cerrado')")
        .bind(e.empresa)
        .bind(e.cliente.id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(falla("INSERT INTO reporte (empresa_id, usuario_id, descripcion) SELECT empresa_id, usuario_id, 'otra vez' FROM reporte").await, "dos reportes pendientes iguales");
    let turno: i32 = sqlx::query_scalar(
        "INSERT INTO turno (empresa_id, usuario_id, fecha, hora) VALUES ($1, $2, '2030-01-08', '10:00') RETURNING id",
    )
    .bind(e.empresa)
    .bind(e.cliente.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let empresa = e.empresa;
    let calificar = |usuario: i32| {
        let pool = pool.clone();
        async move {
            sqlx::query("INSERT INTO calificacion (usuario_id, empresa_id, turno_id, puntaje) VALUES ($1, $2, $3, 5)")
                .bind(usuario)
                .bind(empresa)
                .bind(turno)
                .execute(&pool)
                .await
                .is_ok()
        }
    };
    assert!(!calificar(e.dueno.id).await, "la calificación debe ser del cliente del turno");
    assert!(calificar(e.cliente.id).await);
}

/// Modelo v2: no queda ninguna tabla, columna, restricción ni índice con nombres del modelo
/// anterior, y las coordenadas son DECIMAL(9,6) como en el modelo lógico.
#[sqlx::test(migrations = "./migrations")]
async fn el_esquema_no_conserva_nombres_del_modelo_anterior(pool: PgPool) {
    const VIEJOS: &str = "(^|_)(usuarios|empresas|categorias|categoria|turnos|calificaciones|reportes|\
        favoritos|etiquetas|tokens_usuario|push_suscripciones|horarios|telefonos_empresa|\
        publicaciones_destacadas|super_rubros|empresa_categorias|atenciones_mostrador|\
        resumenes_semanales|solicitudes_publicidad|admin_id|categoria_id|contraseña|\
        constancia_cuit_url|imagen_url)(_|$)";
    let nombres: Vec<String> = sqlx::query_scalar(
        "SELECT kind || ' ' || name FROM ( \
           SELECT 'tabla' AS kind, table_name::text AS name FROM information_schema.tables \
             WHERE table_schema = 'public' AND table_name NOT LIKE '\\_sqlx%' \
           UNION ALL SELECT 'columna', table_name || '.' || column_name FROM information_schema.columns \
             WHERE table_schema = 'public' AND table_name NOT LIKE '\\_sqlx%' \
           UNION ALL SELECT 'restricción', conname::text FROM pg_constraint \
             WHERE connamespace = 'public'::regnamespace AND conrelid::regclass::text NOT LIKE '\\_sqlx%' \
           UNION ALL SELECT 'índice', indexname::text FROM pg_indexes \
             WHERE schemaname = 'public' AND tablename NOT LIKE '\\_sqlx%' \
           UNION ALL SELECT 'secuencia', sequencename::text FROM pg_sequences WHERE schemaname = 'public' \
         ) n WHERE (split_part(name, '.', 2) ~ $1 OR (name NOT LIKE '%.%' AND name ~ $1)) \
           AND name NOT LIKE '%turnos_habilitados%' \
         ORDER BY 1",
    )
    .bind(VIEJOS.replace(' ', ""))
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(nombres.is_empty(), "quedan nombres del modelo anterior: {nombres:?}");

    let (precision, escala): (i32, i32) = sqlx::query_as(
        "SELECT numeric_precision, numeric_scale FROM information_schema.columns \
         WHERE table_name = 'empresa' AND column_name = 'lat'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((precision, escala), (9, 6));
}

#[sqlx::test(migrations = "./migrations")]
async fn el_dueno_ve_los_turnos_de_su_comercio_y_nadie_mas(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    let otro_dueno = crear_usuario(&pool, "otro-dueno@test.com", "admin").await;
    let f = fecha(Weekday::Tue, 3);
    call(
        &e.app,
        "POST",
        "/api/turnos/reservar",
        Some(&e.cliente.token),
        Some(reserva(e.empresa, f, "10:00")),
    )
    .await;
    call(
        &e.app,
        "POST",
        "/api/turnos/reservar",
        Some(&e.cliente.token),
        Some(reserva(e.empresa, f, "09:00")),
    )
    .await;
    call(
        &e.app,
        "POST",
        "/api/turnos/reservar",
        Some(&e.cliente.token),
        Some(reserva(e.empresa, f, "11:00")),
    )
    .await;
    // uno cancelado no se lista
    let id: i32 = sqlx::query_scalar("SELECT id FROM turno WHERE hora = '11:00'")
        .fetch_one(&pool)
        .await
        .unwrap();
    call(
        &e.app,
        "DELETE",
        &format!("/api/turnos/cancelar/{id}"),
        Some(&e.cliente.token),
        None,
    )
    .await;

    let uri = format!("/api/empresas/{}/turnos", e.empresa);
    let (st, v) = call(&e.app, "GET", &uri, Some(&e.dueno.token), None).await;
    assert_eq!(st, StatusCode::OK);
    let horas: Vec<&str> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["hora"].as_str().unwrap())
        .collect();
    assert_eq!(
        horas,
        ["09:00:00", "10:00:00"],
        "ordenados y sin los cancelados"
    );
    assert_eq!(v[0]["cliente_nombre"], "Usuario cliente@test.com");
    assert_eq!(v[0]["estado"], "confirmado");

    assert_eq!(
        call(&e.app, "GET", &uri, Some(&e.superadmin.token), None)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        call(&e.app, "GET", &uri, Some(&e.cliente.token), None)
            .await
            .0,
        StatusCode::FORBIDDEN,
        "un cliente no ve los turnos del comercio"
    );
    assert_eq!(
        call(&e.app, "GET", &uri, Some(&otro_dueno.token), None)
            .await
            .0,
        StatusCode::FORBIDDEN,
        "otro dueño tampoco"
    );
    assert_eq!(
        call(&e.app, "GET", &uri, None, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &e.app,
            "GET",
            "/api/empresas/999999/turnos",
            Some(&e.superadmin.token),
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn el_sello_se_otorga_por_verificacion_y_solo_el_superadmin_lo_retira(pool: PgPool) {
    let e = escenario(pool).await;
    let uri = format!("/api/empresas/{}/verificado", e.empresa);
    for t in [&e.cliente.token, &e.dueno.token] {
        assert_eq!(
            call(
                &e.app,
                "PUT",
                &uri,
                Some(t),
                Some(json!({ "verificado": true }))
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        call(
            &e.app,
            "PUT",
            &uri,
            None,
            Some(json!({ "verificado": true }))
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &e.app,
            "PUT",
            "/api/empresas/999999/verificado",
            Some(&e.superadmin.token),
            Some(json!({ "verificado": true }))
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );

    assert_eq!(
        call(
            &e.app,
            "PUT",
            &uri,
            Some(&e.superadmin.token),
            Some(json!({ "verificado": true }))
        )
        .await
        .0,
        StatusCode::CONFLICT,
        "el sello no se otorga por el endpoint legacy: exige solicitud con constancia"
    );

    // El flujo real otorga el sello y el endpoint legacy solo lo retira.
    let verificacion = format!("/api/empresas/{}/verificacion", e.empresa);
    assert_eq!(
        call(
            &e.app,
            "PUT",
            &verificacion,
            Some(&e.dueno.token),
            Some(json!({
                "accion": "solicitar", "cuit": "20123456786",
                "url_cuit": "https://ejemplo.com/constancia.pdf"
            }))
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &e.app,
            "PUT",
            &verificacion,
            Some(&e.superadmin.token),
            Some(json!({ "accion": "aprobar" }))
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &e.app,
            "PUT",
            &uri,
            Some(&e.superadmin.token),
            Some(json!({ "verificado": false }))
        )
        .await
        .0,
        StatusCode::OK
    );
    let (_, ficha) = call(
        &e.app,
        "GET",
        &format!("/api/empresas/{}", e.empresa),
        None,
        None,
    )
    .await;
    assert_eq!(ficha["verificado"], json!(false));
    assert_eq!(ficha["verificado_estado"], json!("sin_solicitar"));
}

#[sqlx::test(migrations = "./migrations")]
async fn una_nueva_solicitud_apaga_el_sello_hasta_que_se_apruebe(pool: PgPool) {
    let e = escenario(pool).await;
    let uri = format!("/api/empresas/{}/verificacion", e.empresa);
    let solicitud = json!({
        "accion": "solicitar", "cuit": "20-12345678-6",
        "url_cuit": "https://ejemplo.com/constancia.pdf"
    });
    for (token, cuerpo) in [
        (e.dueno.token.as_str(), solicitud.clone()),
        (e.superadmin.token.as_str(), json!({ "accion": "aprobar" })),
    ] {
        assert_eq!(
            call(&e.app, "PUT", &uri, Some(token), Some(cuerpo)).await.0,
            StatusCode::OK
        );
    }
    // Antes quedaba verificado = true con estado 'pendiente'; ahora la base no lo admite.
    let (st, v) = call(&e.app, "PUT", &uri, Some(&e.dueno.token), Some(solicitud)).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["verificado"], json!(false), "el sello no convive con una solicitud pendiente");
    assert_eq!(v["verificado_estado"], json!("pendiente"));
    let cuit: Option<String> = sqlx::query_scalar("SELECT cuit FROM empresa WHERE id = $1")
        .bind(e.empresa)
        .fetch_one(&e.pool)
        .await
        .unwrap();
    assert_eq!(cuit.as_deref(), Some("20123456786"), "el CUIT se guarda normalizado");
}

#[sqlx::test(migrations = "./migrations")]
async fn el_dueno_solicita_la_verificacion_con_cuit_y_el_superadmin_la_resuelve(pool: PgPool) {
    let e = escenario(pool).await;
    let uri = format!("/api/empresas/{}/verificacion", e.empresa);

    assert_eq!(
        call(
            &e.app,
            "PUT",
            &uri,
            None,
            Some(json!({ "accion": "solicitar" }))
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &e.app,
            "PUT",
            &uri,
            Some(&e.dueno.token),
            Some(json!({ "accion": "solicitar" }))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST,
        "sin CUIT no hay solicitud"
    );
    assert_eq!(
        call(
            &e.app,
            "PUT",
            &uri,
            Some(&e.dueno.token),
            Some(json!({ "accion": "solicitar", "cuit": "123" }))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST,
        "el CUIT debe tener 11 dígitos"
    );
    assert_eq!(call(&e.app, "PUT", &uri, Some(&e.dueno.token), Some(json!({ "accion": "solicitar", "cuit": "20123456786", "url_cuit": "no-es-url" }))).await.0, StatusCode::BAD_REQUEST, "la constancia debe ser una URL http(s)");
    assert_eq!(
        call(
            &e.app,
            "PUT",
            &uri,
            Some(&e.dueno.token),
            Some(json!({ "accion": "solicitar", "cuit": "20123456786" }))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST,
        "sin constancia no hay nada que el admin pueda validar"
    );
    assert_eq!(
        call(
            &e.app,
            "PUT",
            &uri,
            Some(&e.superadmin.token),
            Some(json!({ "accion": "aprobar" }))
        )
        .await
        .0,
        StatusCode::CONFLICT,
        "no se aprueba una solicitud que no existe"
    );
    assert_eq!(
        call(
            &e.app,
            "PUT",
            &uri,
            Some(&e.dueno.token),
            Some(json!({ "accion": "licuar" }))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST,
        "acción inventada"
    );

    let intruso = crear_usuario(&e.pool, "intruso@test.com", "admin").await;
    assert_eq!(
        call(
            &e.app,
            "PUT",
            &uri,
            Some(&intruso.token),
            Some(json!({ "accion": "solicitar", "cuit": "20123456786" }))
        )
        .await
        .0,
        StatusCode::FORBIDDEN,
        "no se puede solicitar por un comercio ajeno"
    );

    // El dueño solicita: queda pendiente y el CUIT se normaliza (se guardan solo los dígitos).
    let (st, v) = call(
        &e.app,
        "PUT",
        &uri,
        Some(&e.dueno.token),
        Some(json!({
            "accion": "solicitar", "cuit": "20-12345678-6",
            "url_cuit": "https://ejemplo.com/constancia.pdf"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["verificado"], json!(false));
    assert_eq!(v["verificado_estado"], json!("pendiente"));

    // Aparece en la bandeja de pendientes del admin, con CUIT y constancia.
    let (_, pendientes) = call(
        &e.app,
        "GET",
        "/api/empresas?verificado_estado=pendiente",
        Some(&e.superadmin.token),
        None,
    )
    .await;
    assert_eq!(total_de(&pendientes), 1);
    assert_eq!(pendientes[0]["cuit"], json!("20123456786"));
    assert_eq!(
        pendientes[0]["url_cuit"],
        json!("https://ejemplo.com/constancia.pdf")
    );

    // Aprobar/rechazar es solo del superadmin.
    assert_eq!(
        call(
            &e.app,
            "PUT",
            &uri,
            Some(&e.dueno.token),
            Some(json!({ "accion": "aprobar" }))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (st, v) = call(
        &e.app,
        "PUT",
        &uri,
        Some(&e.superadmin.token),
        Some(json!({ "accion": "aprobar" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["verificado"], json!(true));
    assert_eq!(v["verificado_estado"], json!("verificado"));

    let (_, ficha) = call(
        &e.app,
        "GET",
        &format!("/api/empresas/{}", e.empresa),
        None,
        None,
    )
    .await;
    assert_eq!(ficha["verificado"], json!(true));
    assert_eq!(ficha["verificado_estado"], json!("verificado"));
    // El público ve el sello, no el CUIT ni la constancia (solo el dueño y el superadmin).
    assert!(ficha.get("cuit").is_none() && ficha.get("url_cuit").is_none());
    let (_, propia) = call(
        &e.app,
        "GET",
        &format!("/api/empresas/{}", e.empresa),
        Some(&e.dueno.token),
        None,
    )
    .await;
    assert_eq!(propia["cuit"], json!("20123456786"));
    assert_eq!(
        call(
            &e.app,
            "PUT",
            &uri,
            Some(&e.superadmin.token),
            Some(json!({ "accion": "aprobar" }))
        )
        .await
        .0,
        StatusCode::CONFLICT,
        "una solicitud ya resuelta no se vuelve a aprobar"
    );

    // Re-solicitar y rechazar: el sello se apaga y el comercio sale de la bandeja.
    assert_eq!(
        call(
            &e.app,
            "PUT",
            &uri,
            Some(&e.dueno.token),
            Some(json!({ "accion": "solicitar", "cuit": "20123456786" }))
        )
        .await
        .0,
        StatusCode::OK
    );
    // Rechazar sin motivo (o con uno vacío) no se acepta: el dueño tiene que saber qué corregir.
    for cuerpo in [json!({ "accion": "rechazar" }), json!({ "accion": "rechazar", "motivo": "  no " })] {
        assert_eq!(
            call(&e.app, "PUT", &uri, Some(&e.superadmin.token), Some(cuerpo)).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    let (st, v) = call(
        &e.app,
        "PUT",
        &uri,
        Some(&e.superadmin.token),
        Some(json!({ "accion": "rechazar", "motivo": "La constancia está vencida" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["verificado"], json!(false));
    assert_eq!(v["verificado_estado"], json!("rechazado"));
    let (_, ficha) = call(&e.app, "GET", &format!("/api/empresas/{}", e.empresa), Some(&e.dueno.token), None).await;
    assert_eq!(ficha["motivo_rechazo"], json!("La constancia está vencida"));
    let (_, pendientes) = call(
        &e.app,
        "GET",
        "/api/empresas?verificado_estado=pendiente",
        Some(&e.superadmin.token),
        None,
    )
    .await;
    assert_eq!(total_de(&pendientes), 0);

    assert_eq!(
        call(
            &e.app,
            "PUT",
            "/api/empresas/999999/verificacion",
            Some(&e.superadmin.token),
            Some(json!({ "accion": "aprobar" }))
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn los_reportes_se_crean_sin_duplicar_y_los_modera_el_superadmin(pool: PgPool) {
    let e = escenario(pool).await;
    let uri = format!("/api/empresas/{}/reportes", e.empresa);
    assert_eq!(
        call(
            &e.app,
            "POST",
            &uri,
            None,
            Some(json!({ "descripcion": "Datos falsos" }))
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &e.app,
            "POST",
            &uri,
            Some(&e.cliente.token),
            Some(json!({ "descripcion": "mal" }))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST,
        "motivo muy corto"
    );
    assert_eq!(
        call(
            &e.app,
            "POST",
            "/api/empresas/999999/reportes",
            Some(&e.cliente.token),
            Some(json!({ "descripcion": "Datos falsos" }))
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );

    assert_eq!(
        call(
            &e.app,
            "POST",
            &uri,
            Some(&e.cliente.token),
            Some(json!({ "descripcion": "Los datos son falsos" }))
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &e.app,
            "POST",
            &uri,
            Some(&e.cliente.token),
            Some(json!({ "descripcion": "Los datos son falsos" }))
        )
        .await
        .0,
        StatusCode::CONFLICT,
        "no se repite mientras está pendiente"
    );

    // la bandeja es solo del superadmin
    assert_eq!(
        call(&e.app, "GET", "/api/reportes", Some(&e.cliente.token), None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&e.app, "GET", "/api/reportes", Some(&e.dueno.token), None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let (st, lista) = call(
        &e.app,
        "GET",
        "/api/reportes",
        Some(&e.superadmin.token),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(total_de(&lista), 1);
    let id = lista[0]["id"].as_i64().unwrap();
    assert_eq!(lista[0]["empresa_nombre"], json!("Peluquería Central"));

    assert_eq!(
        call(
            &e.app,
            "PUT",
            &format!("/api/reportes/{id}/resolver"),
            Some(&e.cliente.token),
            None
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &e.app,
            "PUT",
            "/api/reportes/999999/resolver",
            Some(&e.superadmin.token),
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &e.app,
            "PUT",
            &format!("/api/reportes/{id}/resolver"),
            Some(&e.superadmin.token),
            None
        )
        .await
        .0,
        StatusCode::OK
    );

    let (_, pendientes) = call(
        &e.app,
        "GET",
        "/api/reportes",
        Some(&e.superadmin.token),
        None,
    )
    .await;
    assert_eq!(total_de(&pendientes), 0);
    let (_, todos) = call(
        &e.app,
        "GET",
        "/api/reportes?resueltos=true",
        Some(&e.superadmin.token),
        None,
    )
    .await;
    assert_eq!(total_de(&todos), 1);
    // ya resuelto, puede volver a reportar
    assert_eq!(
        call(
            &e.app,
            "POST",
            &uri,
            Some(&e.cliente.token),
            Some(json!({ "descripcion": "Sigue mal" }))
        )
        .await
        .0,
        StatusCode::OK
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn la_api_expone_las_coordenadas_para_el_mapa(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    sqlx::query("UPDATE empresa SET lat = -35.9711, lng = -62.7333 WHERE id = $1")
        .bind(e.empresa)
        .execute(&pool)
        .await
        .unwrap();
    let (_, ficha) = call(
        &e.app,
        "GET",
        &format!("/api/empresas/{}", e.empresa),
        None,
        None,
    )
    .await;
    assert_eq!(
        (ficha["lat"].as_f64(), ficha["lng"].as_f64()),
        (Some(-35.9711), Some(-62.7333))
    );
    let (_, lista) = call(&e.app, "GET", "/api/empresas", None, None).await;
    assert_eq!(lista[0]["lat"].as_f64(), Some(-35.9711));
}

#[sqlx::test(migrations = "./migrations")]
async fn el_comercio_persiste_ubicacion_y_modo_de_contacto(pool: PgPool) {
    let e = escenario(pool).await;
    let uri = format!("/api/empresas/{}", e.empresa);
    let cuerpo = json!({
        "nombre": "Peluquería Central",
        "direccion": "Calle 123",
        "rubro_id": e.rubro,
        "lat": -35.9711,
        "lng": -62.7333,
        "modo_contacto": "mensaje"
    });
    let (st, v) = call(&e.app, "PUT", &uri, Some(&e.dueno.token), Some(cuerpo)).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        (v["lat"].as_f64(), v["lng"].as_f64()),
        (Some(-35.9711), Some(-62.7333))
    );
    assert_eq!(v["modo_contacto"], "mensaje");

    // Sin los campos, la edición conserva lo ya guardado.
    let solo_texto = json!({
        "nombre": "Peluquería Central",
        "direccion": "Calle 123",
        "rubro_id": e.rubro
    });
    let (_, v) = call(&e.app, "PUT", &uri, Some(&e.dueno.token), Some(solo_texto)).await;
    assert_eq!(v["lat"].as_f64(), Some(-35.9711));
    assert_eq!(v["modo_contacto"], "mensaje");

    let (st, nueva) = call(
        &e.app,
        "POST",
        "/api/empresas",
        Some(&e.dueno.token),
        Some(json!({
            "nombre": "Con mapa",
            "direccion": "Calle 2",
            "rubro_id": e.rubro,
            "lat": -35.97,
            "lng": -62.73,
            "modo_contacto": "llamada"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(nueva["modo_contacto"], "llamada");
    assert_eq!(nueva["lat"].as_f64(), Some(-35.97));

    for (lat, lng) in [
        (Some(-35.97), None),
        (None, Some(-62.73)),
        (Some(91.0), Some(-62.73)),
        (Some(-35.97), Some(-181.0)),
    ] {
        let mut malo = json!({
            "nombre": "Mal",
            "direccion": "Calle 3",
            "rubro_id": e.rubro
        });
        if let Some(v) = lat {
            malo["lat"] = json!(v);
        }
        if let Some(v) = lng {
            malo["lng"] = json!(v);
        }
        assert_eq!(
            call(
                &e.app,
                "POST",
                "/api/empresas",
                Some(&e.dueno.token),
                Some(malo)
            )
            .await
            .0,
            StatusCode::BAD_REQUEST,
            "coordenadas inválidas: {lat:?}, {lng:?}"
        );
    }

    let mut modo_malo = json!({
        "nombre": "Mal",
        "direccion": "Calle 3",
        "rubro_id": e.rubro
    });
    modo_malo["modo_contacto"] = json!("fax");
    assert_eq!(
        call(
            &e.app,
            "POST",
            "/api/empresas",
            Some(&e.dueno.token),
            Some(modo_malo)
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn el_listado_ordena_por_cercania_y_abiertos_primero(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    let base = (-35.9711, -62.7333);
    let cerca = crear_empresa(&pool, "Cerca pero cerrado", e.rubro, None, false, None).await;
    let lejos = crear_empresa(&pool, "Lejos pero abierto", e.rubro, None, false, None).await;
    crear_empresa(&pool, "Sin ubicación", e.rubro, None, false, None).await;
    for (id, lat, lng) in [
        (cerca, base.0 + 0.0001, base.1),
        (lejos, base.0 + 0.01, base.1),
    ] {
        sqlx::query("UPDATE empresa SET lat = $1, lng = $2 WHERE id = $3")
            .bind(lat)
            .bind(lng)
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
    }
    sqlx::query(
        "INSERT INTO horario_empresa (empresa_id, dia_semana, hora_inicio, hora_fin) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(lejos)
    .bind(nombre_dia(ahora_local().date()))
    .bind(NaiveTime::from_hms_opt(0, 0, 0).unwrap())
    .bind(NaiveTime::from_hms_nano_opt(23, 59, 59, 999_999_000).unwrap())
    .execute(&pool)
    .await
    .unwrap();

    let uri = format!("/api/empresas?lat={}&lng={}&limit=10", base.0, base.1);
    let (st, v) = call(&e.app, "GET", &uri, None, None).await;
    assert_eq!(st, StatusCode::OK);
    let filas = v.as_array().unwrap();
    let indice = |nombre: &str| {
        filas
            .iter()
            .position(|x| x["nombre"].as_str() == Some(nombre))
            .unwrap()
    };
    assert!(
        indice("Lejos pero abierto") < indice("Cerca pero cerrado"),
        "los abiertos van primero"
    );
    assert!(
        indice("Cerca pero cerrado") < indice("Sin ubicación"),
        "los geolocalizados van antes que los que no tienen coordenadas"
    );
    let fila = |nombre: &str| {
        filas
            .iter()
            .find(|x| x["nombre"].as_str() == Some(nombre))
            .unwrap()
    };
    let cerca_km = fila("Cerca pero cerrado")["distancia_km"].as_f64().unwrap();
    let lejos_km = fila("Lejos pero abierto")["distancia_km"].as_f64().unwrap();
    assert!(cerca_km < 0.05, "la distancia cercana da {cerca_km}");
    assert!(
        (1.0..1.2).contains(&lejos_km),
        "un centésimo de latitud son ~1,1 km, dio {lejos_km}"
    );
    assert_eq!(
        (
            fila("Lejos pero abierto")["abierto_ahora"].as_bool(),
            fila("Cerca pero cerrado")["abierto_ahora"].as_bool(),
        ),
        (Some(true), Some(false))
    );
    assert!(fila("Sin ubicación").get("distancia_km").is_none());
}

#[sqlx::test(migrations = "./migrations")]
async fn la_cercania_exige_las_dos_coordenadas_y_un_rango_valido(pool: PgPool) {
    let e = escenario(pool).await;
    for uri in [
        "/api/empresas?lat=-35.97",
        "/api/empresas?lng=-62.73",
        "/api/empresas?lat=91&lng=-62.73",
        "/api/empresas?lat=-35.97&lng=181",
    ] {
        let (st, v) = call(&e.app, "GET", uri, None, None).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{uri}");
        assert!(v["error"].as_str().unwrap().contains("coordenada"), "{v}");
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn el_mapa_devuelve_solo_comercios_geolocalizados_y_filtra_por_rubro(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    let otra = crear_rubro(&pool, "Gimnasios").await;
    let sin_geo = crear_empresa(&pool, "Sin coordenadas", e.rubro, None, false, None).await;
    let en_gim = crear_empresa(&pool, "Gimnasio Norte", otra, None, false, None).await;
    for id in [e.empresa, en_gim] {
        sqlx::query("UPDATE empresa SET lat = -35.97, lng = -62.73 WHERE id = $1")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
    }
    let (st, todos) = call(&e.app, "GET", "/api/empresas/mapa", None, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(total_de(&todos), 2);
    assert!(todos
        .as_array()
        .unwrap()
        .iter()
        .all(|p| p["id"].as_i64() != Some(sin_geo as i64)));
    assert_eq!(todos[0]["lat"].as_f64(), Some(-35.97));

    let (_, solo_gim) = call(
        &e.app,
        "GET",
        &format!("/api/empresas/mapa?rubro_id={otra}"),
        None,
        None,
    )
    .await;
    assert_eq!(total_de(&solo_gim), 1);
    assert_eq!(solo_gim[0]["nombre"], json!("Gimnasio Norte"));
}

#[sqlx::test(migrations = "./migrations")]
async fn el_resumen_del_comercio_se_calcula_para_el_periodo_pedido(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    let hoy = ahora_local().date();
    // Dos turnos online (uno cancelado), uno de mostrador ya atendido y uno fuera del período.
    for (fecha, hora, usuario, origen, estado, nombre) in [
        (hoy - Duration::days(2), "10:00", Some(e.cliente.id), "online", 1, None),
        (hoy - Duration::days(2), "11:00", Some(e.cliente.id), "online", 2, None),
        (hoy - Duration::days(1), "09:30", None, "mostrador", 3, Some("Juan Pérez")),
        (hoy - Duration::days(60), "10:00", Some(e.cliente.id), "online", 1, None),
    ] {
        sqlx::query(
            "INSERT INTO turno (empresa_id, usuario_id, fecha, hora, origen, estado_turno_id, cliente_nombre) \
             VALUES ($1, $2, $3, $4::time, $5, $6, $7)",
        )
        .bind(e.empresa)
        .bind(usuario)
        .bind(fecha)
        .bind(hora)
        .bind(origen)
        .bind(estado as i16)
        .bind(nombre)
        .execute(&pool)
        .await
        .unwrap();
    }
    sqlx::query("INSERT INTO favorito (usuario_id, empresa_id) VALUES ($1, $2)")
        .bind(e.cliente.id)
        .bind(e.empresa)
        .execute(&pool)
        .await
        .unwrap();

    let uri = format!("/api/empresas/{}/resumen", e.empresa);
    assert_eq!(call(&e.app, "GET", &uri, None, None).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(call(&e.app, "GET", &uri, Some(&e.cliente.token), None).await.0, StatusCode::FORBIDDEN);
    // Sin fechas: los últimos 30 días.
    let (st, r) = call(&e.app, "GET", &uri, Some(&e.dueno.token), None).await;
    assert_eq!(st, StatusCode::OK, "{r}");
    assert_eq!(r["empresa_nombre"], "Peluquería Central");
    assert_eq!(r["hasta"], hoy.to_string());
    assert_eq!(r["turnos"]["turnos"], 2, "no cuenta los cancelados");
    assert_eq!(r["turnos"]["online"], 1);
    assert_eq!(r["turnos"]["mostrador"], 1);
    assert_eq!(r["turnos"]["cancelados"], 1);
    assert_eq!(r["turnos"]["completados"], 1);
    assert_eq!(r["favoritos_nuevos"], 1);
    assert_eq!(total_de(&r["por_dia"]), 2);
    // Un período elegido por el dueño incluye el turno viejo.
    let desde = hoy - Duration::days(90);
    let (_, r) = call(&e.app, "GET", &format!("{uri}?desde={desde}&hasta={hoy}"), Some(&e.dueno.token), None).await;
    assert_eq!(r["turnos"]["turnos"], 3);
    let (st, _) = call(&e.app, "GET", &format!("{uri}?desde={hoy}&hasta={desde}"), Some(&e.dueno.token), None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "período invertido");
    let viejo = hoy - Duration::days(400);
    let (st, _) = call(&e.app, "GET", &format!("{uri}?desde={viejo}&hasta={hoy}"), Some(&e.dueno.token), None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "más de un año");
}

#[sqlx::test(migrations = "./migrations")]
async fn las_suscripciones_push_se_guardan_por_usuario_y_se_dan_de_baja(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    let uri = "/api/push/suscripciones";
    let endpoint = "https://fcm.googleapis.com/fcm/send/prueba-integracion";
    let cuerpo = json!({
        "endpoint": endpoint,
        "keys": { "p256dh": PUSH_P256DH, "auth": PUSH_AUTH }
    });

    // Sin VAPID configurado la clave pública no existe, pero suscribirse sigue siendo válido:
    // la app simplemente no ofrece activar notificaciones hasta que el servidor esté configurado.
    assert_eq!(
        call(&e.app, "GET", "/api/push/clave-publica", None, None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&e.app, "POST", uri, None, Some(cuerpo.clone()))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &e.app,
            "POST",
            uri,
            Some(&e.cliente.token),
            Some(cuerpo.clone())
        )
        .await
        .0,
        StatusCode::CREATED
    );
    let dueno: i32 =
        sqlx::query_scalar("SELECT usuario_id FROM push_suscripcion WHERE endpoint = $1")
            .bind(endpoint)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(dueno, e.cliente.id);

    // Repetir la suscripción (mismo navegador) actualiza, no duplica.
    assert_eq!(
        call(
            &e.app,
            "POST",
            uri,
            Some(&e.cliente.token),
            Some(cuerpo.clone())
        )
        .await
        .0,
        StatusCode::CREATED
    );
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM push_suscripcion")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(total, 1);

    // Un endpoint fuera de la allowlist evitaría SSRF; las claves se validan por largo.
    let malo = json!({
        "endpoint": "https://127.0.0.1/push",
        "keys": { "p256dh": PUSH_P256DH, "auth": PUSH_AUTH }
    });
    assert_eq!(
        call(&e.app, "POST", uri, Some(&e.cliente.token), Some(malo))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let corta = json!({
        "endpoint": endpoint,
        "keys": { "p256dh": "abc", "auth": PUSH_AUTH }
    });
    assert_eq!(
        call(&e.app, "POST", uri, Some(&e.cliente.token), Some(corta))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );

    // La baja es idempotente y solo afecta al usuario dueño de la suscripción.
    assert_eq!(
        call(
            &e.app,
            "DELETE",
            uri,
            Some(&e.dueno.token),
            Some(json!({ "endpoint": endpoint }))
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM push_suscripcion")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(total, 1);
    assert_eq!(
        call(
            &e.app,
            "DELETE",
            uri,
            Some(&e.cliente.token),
            Some(json!({ "endpoint": endpoint }))
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM push_suscripcion")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(total, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn los_recordatorios_toman_solo_turnos_futuros_con_push_y_no_se_repiten(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    let endpoint = "https://updates.push.services.mozilla.com/wpush/v2/prueba";
    sqlx::query("INSERT INTO push_suscripcion (usuario_id, endpoint, p256dh, auth) VALUES ($1, $2, $3, $4)")
        .bind(e.cliente.id)
        .bind(endpoint)
        .bind(PUSH_P256DH)
        .bind(PUSH_AUTH)
        .execute(&pool)
        .await
        .unwrap();

    let ahora = ahora_local();
    let cercano = ahora + Duration::hours(2);
    let turno: i32 = sqlx::query_scalar(
        "INSERT INTO turno (empresa_id, usuario_id, fecha, hora, estado_turno_id) \
         VALUES ($1, $2, $3, $4, 1) RETURNING id",
    )
    .bind(e.empresa)
    .bind(e.cliente.id)
    .bind(cercano.date())
    .bind(cercano.time())
    .fetch_one(&pool)
    .await
    .unwrap();
    let lejano = ahora + Duration::days(2);
    let otro: i32 = sqlx::query_scalar(
        "INSERT INTO turno (empresa_id, usuario_id, fecha, hora, estado_turno_id) \
         VALUES ($1, $2, $3, $4, 1) RETURNING id",
    )
    .bind(e.empresa)
    .bind(e.cliente.id)
    .bind(lejano.date())
    .bind(lejano.time())
    .fetch_one(&pool)
    .await
    .unwrap();

    let pendientes = recordatorios::pendientes(&pool, ahora).await.unwrap();
    assert!(pendientes.iter().any(|t| t.turno_id == turno));
    assert!(!pendientes.iter().any(|t| t.turno_id == otro));

    recordatorios::marcar_enviado(&pool, turno).await.unwrap();
    let pendientes = recordatorios::pendientes(&pool, ahora).await.unwrap();
    assert!(
        !pendientes.iter().any(|t| t.turno_id == turno),
        "un recordatorio ya enviado no se repite"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn el_cliente_configura_la_anticipacion_del_recordatorio(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    let uri = "/api/usuarios/yo/recordatorio";

    let (st, body) = call(&e.app, "GET", uri, Some(&e.cliente.token), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["horas"], 24);

    let (st, _) = call(
        &e.app,
        "PUT",
        uri,
        Some(&e.cliente.token),
        Some(json!({ "horas": 0 })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = call(
        &e.app,
        "PUT",
        uri,
        Some(&e.cliente.token),
        Some(json!({ "horas": 169 })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    let (st, body) = call(
        &e.app,
        "PUT",
        uri,
        Some(&e.cliente.token),
        Some(json!({ "horas": 1 })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["horas"], 1);

    let (st, body) = call(&e.app, "GET", uri, Some(&e.cliente.token), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["horas"], 1);
    let horas: i32 =
        sqlx::query_scalar("SELECT recordatorio_horas FROM usuario WHERE id = $1")
            .bind(e.cliente.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(horas, 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn el_recordatorio_respeta_la_anticipacion_de_cada_cliente(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    let otro = crear_usuario(&pool, "otro@test.com", "cliente").await;

    for (usuario, sufijo) in [(&e.cliente, "cliente"), (&otro, "otro")] {
        sqlx::query(
            "INSERT INTO push_suscripcion (usuario_id, endpoint, p256dh, auth) VALUES ($1, $2, $3, $4)",
        )
        .bind(usuario.id)
        .bind(format!(
            "https://updates.push.services.mozilla.com/wpush/v2/{sufijo}"
        ))
        .bind(PUSH_P256DH)
        .bind(PUSH_AUTH)
        .execute(&pool)
        .await
        .unwrap();
    }

    sqlx::query("UPDATE usuario SET recordatorio_horas = 1 WHERE id = $1")
        .bind(e.cliente.id)
        .execute(&pool)
        .await
        .unwrap();

    let ahora = ahora_local();
    let programados = [
        (e.cliente.id, ahora + Duration::minutes(45)),
        (e.cliente.id, ahora + Duration::hours(2)),
        (otro.id, ahora + Duration::hours(3)),
    ];
    let mut ids_turnos = Vec::new();
    for (usuario_id, cuando) in programados {
        let id: i32 = sqlx::query_scalar(
            "INSERT INTO turno (empresa_id, usuario_id, fecha, hora, estado_turno_id) \
             VALUES ($1, $2, $3, $4, 1) RETURNING id",
        )
        .bind(e.empresa)
        .bind(usuario_id)
        .bind(cuando.date())
        .bind(cuando.time())
        .fetch_one(&pool)
        .await
        .unwrap();
        ids_turnos.push(id);
    }
    let (cliente_45, cliente_2h, otro_3h) = (ids_turnos[0], ids_turnos[1], ids_turnos[2]);

    let pendientes = recordatorios::pendientes(&pool, ahora).await.unwrap();
    let ids: Vec<i32> = pendientes.iter().map(|t| t.turno_id).collect();
    assert!(
        ids.contains(&cliente_45),
        "el turno dentro de 1 h entra en la ventana configurada"
    );
    assert!(
        !ids.contains(&cliente_2h),
        "un turno a 2 h queda afuera para quien pidió 1 h"
    );
    assert!(
        ids.contains(&otro_3h),
        "sin configuración se mantiene la ventana por defecto de 24 h"
    );
}

#[sqlx::test]
async fn health_verifica_la_base_y_degradacion(pool: PgPool) {
    let app = app(pool.clone());

    let (status, body) = call(&app, "GET", "/health", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert_eq!(body["db"], "ok");

    let (status, _) = enviar(&app, "GET", "/health/live", None, None).await;
    assert_eq!(status, StatusCode::OK);

    pool.close().await;

    let (status, body) = call(&app, "GET", "/health", None, None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["status"], "degraded");
    assert_eq!(body["db"], "error");
}

// ------------------------------------------------------------------ mis datos (PUT /usuarios/yo)

#[sqlx::test(migrations = "./migrations")]
async fn mis_datos_cambia_el_nombre_sin_tocar_la_verificacion(pool: PgPool) {
    let e = escenario(pool).await;
    let (st, v) = call(
        &e.app,
        "PUT",
        "/api/usuarios/yo",
        Some(&e.cliente.token),
        Some(json!({ "nombre": "  Martina Gómez  ", "email": "CLIENTE@test.com" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["nombre"], "Martina Gómez");
    assert_eq!(v["email_verificado"], json!(true), "el mismo correo (otra capitalización) no se re-verifica");
    let (st, _) = call(&e.app, "PUT", "/api/usuarios/yo", None, Some(json!({ "nombre": "x" }))).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let (st, _) = call(&e.app, "PUT", "/api/usuarios/yo", Some(&e.cliente.token), Some(json!({}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrations = "./migrations")]
async fn mis_datos_un_correo_nuevo_vuelve_a_exigir_verificacion(pool: PgPool) {
    let e = escenario(pool).await;
    let (st, v) = call(
        &e.app,
        "PUT",
        "/api/usuarios/yo",
        Some(&e.dueno.token),
        Some(json!({ "email": "nuevo@test.com" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "el dueño también edita sus datos");
    assert_eq!(v["email"], "nuevo@test.com");
    assert_eq!(v["email_verificado"], json!(false));
    let tokens: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM token_usuario WHERE usuario_id = $1 AND tipo = 'verificar_email' AND usado_en IS NULL",
    )
    .bind(e.dueno.id)
    .fetch_one(&e.pool)
    .await
    .unwrap();
    assert_eq!(tokens, 1, "se emitió el enlace para verificar el correo nuevo");

    let (st, v) = call(
        &e.app,
        "PUT",
        "/api/usuarios/yo",
        Some(&e.dueno.token),
        Some(json!({ "email": "SUPER@test.com" })),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT, "{v}");
    let (st, _) = call(&e.app, "PUT", "/api/usuarios/yo", Some(&e.dueno.token), Some(json!({ "email": "sin-arroba" }))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrations = "./migrations")]
async fn mis_datos_cambiar_la_contrasena_exige_la_actual(pool: PgPool) {
    let e = escenario(pool).await;
    let cambiar = |actual: &'static str, nueva: &'static str| {
        let app = e.app.clone();
        let token = e.cliente.token.clone();
        async move {
            call(&app, "PUT", "/api/usuarios/yo", Some(&token), Some(json!({ "password_actual": actual, "password_nueva": nueva })))
                .await
        }
    };
    let (st, v) = cambiar("otra", "NuevaClave1").await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"], "La contraseña actual no es correcta");
    assert_eq!(cambiar("clave123", "123").await.0, StatusCode::BAD_REQUEST, "la nueva debe ser válida");
    let (st, _) = call(&e.app, "PUT", "/api/usuarios/yo", Some(&e.cliente.token), Some(json!({ "password_nueva": "NuevaClave1" }))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "sin la actual no se cambia");

    assert_eq!(cambiar("clave123", "NuevaClave1").await.0, StatusCode::OK);
    let (st, _) = call(&e.app, "POST", "/api/auth/login", None, Some(json!({ "email": "cliente@test.com", "contraseña": "NuevaClave1" }))).await;
    assert_eq!(st, StatusCode::OK, "la clave nueva sirve para ingresar");
    let (st, _) = call(&e.app, "POST", "/api/auth/login", None, Some(json!({ "email": "cliente@test.com", "contraseña": "clave123" }))).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "la anterior ya no");
}

// ------------------------------------------------------------------ turnos cargados por el comercio (0014)

#[sqlx::test(migrations = "./migrations")]
async fn el_dueno_agenda_un_turno_para_alguien_sin_cuenta(pool: PgPool) {
    let e = escenario(pool).await;
    let uri = format!("/api/empresas/{}/turnos", e.empresa);
    let f = fecha(Weekday::Tue, 3);
    let turno = |nombre: &str, email: Option<&str>, hora: &str| {
        json!({ "fecha": f.to_string(), "hora": hora, "cliente_nombre": nombre, "cliente_email": email })
    };

    // Solo el dueño (o un superadmin) agenda en su comercio.
    assert_eq!(
        call(&e.app, "POST", &uri, Some(&e.cliente.token), Some(turno("Ana", None, "09:00"))).await.0,
        StatusCode::FORBIDDEN
    );
    // Validaciones: nombre obligatorio, correo válido y la misma grilla que la reserva online.
    assert_eq!(call(&e.app, "POST", &uri, Some(&e.dueno.token), Some(turno("  ", None, "09:00"))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&e.app, "POST", &uri, Some(&e.dueno.token), Some(turno("Ana", Some("ana@"), "09:00"))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&e.app, "POST", &uri, Some(&e.dueno.token), Some(turno("Ana", None, "09:10"))).await.0, StatusCode::BAD_REQUEST, "fuera de la grilla");
    assert_eq!(call(&e.app, "POST", &uri, Some(&e.dueno.token), Some(turno("Ana", None, "13:00"))).await.0, StatusCode::BAD_REQUEST, "fuera del horario");

    let mut nota_larga = turno("Ana", None, "09:00");
    nota_larga["nota"] = json!("x".repeat(301));
    assert_eq!(call(&e.app, "POST", &uri, Some(&e.dueno.token), Some(nota_larga)).await.0, StatusCode::BAD_REQUEST, "nota de más de 300");

    let mut con_nota = turno(" Carlos Medina ", Some("carlos@ejemplo.com"), "09:00");
    con_nota["nota"] = json!("Pidió por teléfono");
    let (st, v) = call(&e.app, "POST", &uri, Some(&e.dueno.token), Some(con_nota)).await;
    assert_eq!(st, StatusCode::CREATED, "{v}");
    // Comparte el horario con la reserva online: nadie puede tomar el mismo slot.
    let (st, _) = call(&e.app, "POST", "/api/turnos/reservar", Some(&e.cliente.token), Some(reserva(e.empresa, f, "09:00"))).await;
    assert_eq!(st, StatusCode::CONFLICT);
    assert_eq!(
        call(&e.app, "POST", &uri, Some(&e.dueno.token), Some(turno("Otra", None, "09:00"))).await.0,
        StatusCode::CONFLICT
    );

    // En "Turnos recibidos" figura con su nombre, su correo y la marca de que no tiene cuenta.
    call(&e.app, "POST", "/api/turnos/reservar", Some(&e.cliente.token), Some(reserva(e.empresa, f, "09:30"))).await;
    let (_, recibidos) = call(&e.app, "GET", &uri, Some(&e.dueno.token), None).await;
    let lista = recibidos.as_array().unwrap();
    assert_eq!(lista.len(), 2);
    assert_eq!(lista[0]["cliente_nombre"], "Carlos Medina");
    assert_eq!(lista[0]["cliente_email"], "carlos@ejemplo.com");
    assert_eq!(lista[0]["sin_cuenta"], json!(true));
    assert_eq!(lista[0]["origen"], "mostrador");
    assert_eq!(lista[0]["nota"], "Pidió por teléfono");
    assert_eq!(lista[1]["cliente_email"], "cliente@test.com", "el canal de contacto del cliente registrado");
    assert_eq!(lista[1]["sin_cuenta"], json!(false));
    assert_eq!(lista[1]["origen"], "online");

    // El dueño lo puede cancelar como cualquier otro turno, y el horario se libera.
    let id = lista[0]["id"].as_i64().unwrap();
    let (st, _) = call(&e.app, "DELETE", &format!("/api/turnos/cancelar/{id}"), Some(&e.dueno.token), None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = call(&e.app, "POST", "/api/turnos/reservar", Some(&e.cliente.token), Some(reserva(e.empresa, f, "09:00"))).await;
    assert_eq!(st, StatusCode::OK, "el slot cancelado vuelve a estar libre");
}

#[sqlx::test(migrations = "./migrations")]
async fn un_turno_sin_cuenta_no_aparece_en_mis_turnos_de_nadie(pool: PgPool) {
    let e = escenario(pool).await;
    let f = fecha(Weekday::Wed, 3);
    let (st, _) = call(
        &e.app,
        "POST",
        &format!("/api/empresas/{}/turnos", e.empresa),
        Some(&e.dueno.token),
        Some(json!({ "fecha": f.to_string(), "hora": "16:00", "cliente_nombre": "Lucía" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let (_, mios) = call(&e.app, "GET", "/api/turnos/mis-turnos", Some(&e.cliente.token), None).await;
    assert_eq!(mios.as_array().unwrap().len(), 0);
}

// ------------------------------------------------------------------ esquema 0014 y 0015

#[sqlx::test(migrations = "./migrations")]
async fn el_esquema_exige_motivo_al_rechazar_y_un_titular_por_turno(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    let falla = |sql: &'static str| {
        let pool = pool.clone();
        async move { sqlx::query(sql).execute(&pool).await.is_err() }
    };
    assert!(falla("UPDATE empresa SET verificado_estado = 'rechazado'").await, "rechazado sin motivo");
    assert!(falla("UPDATE empresa SET motivo_rechazo = 'No corresponde' WHERE verificado_estado <> 'rechazado'").await, "motivo sin rechazo");
    assert!(falla("UPDATE empresa SET verificado_estado = 'rechazado', motivo_rechazo = 'no'").await, "motivo demasiado corto");
    assert!(!falla("UPDATE empresa SET verificado_estado = 'rechazado', motivo_rechazo = 'Constancia vencida'").await);

    let empresa = e.empresa;
    let insertar = |sql: &'static str| {
        let pool = pool.clone();
        async move { sqlx::query(sql).bind(empresa).execute(&pool).await.is_ok() }
    };
    // El origen define el titular: 'online' es de un cliente registrado; 'mostrador', de alguien sin cuenta.
    assert!(!insertar("INSERT INTO turno (empresa_id, fecha, hora) VALUES ($1, '2031-01-07', '09:00')").await, "online sin titular");
    assert!(!insertar("INSERT INTO turno (empresa_id, fecha, hora, origen) VALUES ($1, '2031-01-07', '09:00', 'mostrador')").await, "mostrador sin nombre");
    assert!(!insertar("INSERT INTO turno (empresa_id, fecha, hora, origen, cliente_nombre) VALUES ($1, '2031-01-07', '09:00', 'telefono', 'Ana')").await, "origen inválido");
    assert!(!insertar("INSERT INTO turno (empresa_id, fecha, hora, origen, cliente_nombre, cliente_email) VALUES ($1, '2031-01-07', '09:00', 'mostrador', 'Ana', 'sin-arroba')").await, "correo inválido");
    assert!(!insertar("INSERT INTO turno (empresa_id, usuario_id, fecha, hora, cliente_nombre) SELECT $1, id, '2031-01-07', '09:00', 'Ana' FROM usuario LIMIT 1").await, "cuenta y nombre a la vez");
    assert!(!insertar("INSERT INTO turno (empresa_id, usuario_id, fecha, hora, origen) SELECT $1, id, '2031-01-07', '09:00', 'mostrador' FROM usuario LIMIT 1").await, "mostrador con cuenta");
    assert!(!insertar("INSERT INTO turno (empresa_id, fecha, hora, origen, cliente_nombre, nota) VALUES ($1, '2031-01-07', '09:00', 'mostrador', 'Ana', repeat('x', 301))").await, "nota de más de 300");
    assert!(insertar("INSERT INTO turno (empresa_id, fecha, hora, origen, cliente_nombre, nota) VALUES ($1, '2031-01-07', '09:00', 'mostrador', 'Ana', 'Pidió por teléfono')").await);
}

#[sqlx::test(migrations = "./migrations")]
async fn la_edicion_de_la_ficha_no_permite_rechazar_sin_motivo(pool: PgPool) {
    let e = escenario(pool).await;
    let (st, v) = call(
        &e.app,
        "PUT",
        &format!("/api/empresas/{}", e.empresa),
        Some(&e.superadmin.token),
        Some(json!({ "nombre": "Peluquería Central", "direccion": "Calle 123", "rubro_id": e.rubro, "verificado_estado": "rechazado" })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    assert!(v["error"].as_str().unwrap().contains("motivo"), "{v}");
}

// ------------------------------------------------------------------ panel del superadmin: buscadores

#[sqlx::test(migrations = "./migrations")]
async fn el_superadmin_busca_y_pagina_usuarios(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    for i in 0..5 {
        crear_usuario(&pool, &format!("vecino{i}@test.com"), "cliente").await;
    }
    sqlx::query("UPDATE usuario SET nombre = 'José Martínez' WHERE email = 'vecino3@test.com'")
        .execute(&pool)
        .await
        .unwrap();
    let token = Some(e.superadmin.token.as_str());
    let largo = |v: &Value| v.as_array().unwrap().len();
    let (st, v) = call(&e.app, "GET", "/api/usuarios?limit=3", token, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(largo(&v), 3, "respeta el tamaño de página");
    let (_, v2) = call(&e.app, "GET", "/api/usuarios?limit=3&offset=3", token, None).await;
    assert_eq!(largo(&v2), 3, "8 usuarios: la segunda página trae 3 más");
    assert_ne!(v[0]["id"], v2[0]["id"]);
    let (_, v) = call(&e.app, "GET", "/api/usuarios?q=martinez", token, None).await;
    assert_eq!(largo(&v), 1, "busca por nombre sin tildes");
    assert_eq!(v[0]["nombre"], "José Martínez");
    let (_, v) = call(&e.app, "GET", "/api/usuarios?q=VECINO1@", token, None).await;
    assert_eq!(largo(&v), 1, "busca por correo");
    let (_, v) = call(&e.app, "GET", "/api/usuarios?rol=admin", token, None).await;
    assert_eq!(largo(&v), 1, "filtra por rol");
    let pedido = format!("/api/usuarios?ids={},{}", e.dueno.id, e.cliente.id);
    let (_, v) = call(&e.app, "GET", &pedido, token, None).await;
    assert_eq!(largo(&v), 2, "trae los pedidos por id (nombres de los dueños)");
    let (st, _) = call(&e.app, "GET", "/api/usuarios?ids=1,x", token, None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrations = "./migrations")]
async fn la_bandeja_de_publicidades_se_filtra_por_nombre_de_comercio(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    let otra = crear_empresa(&pool, "Ferretería Tornado", e.rubro, None, false, None).await;
    for (empresa, titulo) in [(e.empresa, "Corte + barba"), (otra, "Liquidación")] {
        sqlx::query("INSERT INTO publicidad (empresa_id, titulo, descripcion, url_imagen) VALUES ($1, $2, 'd', 'https://x.com/i.jpg')")
            .bind(empresa)
            .bind(titulo)
            .execute(&pool)
            .await
            .unwrap();
    }
    let token = Some(e.superadmin.token.as_str());
    let (st, v) = call(&e.app, "GET", "/api/publicidades?estado=todas&q=peluqueria", token, None).await;
    assert_eq!(st, StatusCode::OK);
    let titulos: Vec<_> = v.as_array().unwrap().iter().map(|p| p["titulo"].as_str().unwrap()).collect();
    assert_eq!(titulos, ["Corte + barba"], "por nombre del comercio, sin tildes");
    let (_, v) = call(&e.app, "GET", "/api/publicidades?estado=todas&q=liquid", token, None).await;
    assert_eq!(v.as_array().unwrap().len(), 1, "o por título");
}

#[sqlx::test(migrations = "./migrations")]
async fn el_mapa_respeta_los_filtros_y_el_listado_trae_las_fichas_pedidas_por_id(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    let bares = crear_rubro(&pool, "Bares").await;
    let bar = crear_empresa(&pool, "La Esquina", bares, None, false, None).await;
    let sin_geo = crear_empresa(&pool, "Bar Sin Mapa", bares, None, false, None).await;
    for id in [e.empresa, bar] {
        sqlx::query("UPDATE empresa SET lat = -35.97, lng = -62.73 WHERE id = $1")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
    }
    let ids = |v: &Value| -> Vec<i64> {
        v.as_array().unwrap().iter().map(|x| x["id"].as_i64().unwrap()).collect()
    };
    let (st, v) = call(&e.app, "GET", "/api/empresas/mapa", None, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(ids(&v).len(), 2, "solo los geolocalizados");
    let (_, v) = call(&e.app, "GET", "/api/empresas/mapa?q=comida", None, None).await;
    assert_eq!(ids(&v), [bar as i64], "la búsqueda también filtra el mapa");
    let (_, v) = call(&e.app, "GET", &format!("/api/empresas/mapa?rubro_id={bares}"), None, None).await;
    assert_eq!(ids(&v), [bar as i64]);

    let pedido = format!("/api/empresas?ids={sin_geo},{},{bar}", e.empresa);
    let (st, v) = call(&e.app, "GET", &pedido, None, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(ids(&v), [sin_geo as i64, e.empresa as i64, bar as i64], "en el orden pedido");
    let (st, _) = call(&e.app, "GET", "/api/empresas?ids=1,abc", None, None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, v) = call(&e.app, "GET", &format!("{pedido}&lat=-35.97&lng=-62.73"), None, None).await;
    assert_eq!(st, StatusCode::OK, "combina con la cercanía");
    assert_eq!(ids(&v).len(), 3);
}

/// Pedido del usuario (12/11): una cuenta de comercio nueva tiene que poder llegar a su panel y
/// crear su comercio. Sin SMTP (desarrollo) el enlace de verificación vuelve en la respuesta.
#[sqlx::test(migrations = "./migrations")]
async fn una_cuenta_de_comercio_nueva_se_verifica_y_crea_su_comercio(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    let (st, v) = call(
        &e.app,
        "POST",
        "/api/auth/register",
        None,
        Some(json!({ "nombre": "Ana Duena", "email": "ana@comercio.com", "contraseña": "Clave1234", "rol": "comercio" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{v}");
    assert_eq!(v["user"]["rol"], "admin");
    assert_eq!(v["user"]["email_verificado"], false);
    let token = v["token_verificacion"]
        .as_str()
        .expect("en desarrollo sin SMTP vuelve el enlace")
        .to_string();

    let (st, _) = call(&e.app, "POST", "/api/auth/verificar-email", None, Some(json!({ "token": token }))).await;
    assert_eq!(st, StatusCode::OK);
    let (st, login) = call(
        &e.app,
        "POST",
        "/api/auth/login",
        None,
        Some(json!({ "email": "ana@comercio.com", "contraseña": "Clave1234" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(login["user"]["rol"], "admin");
    assert_eq!(login["user"]["email_verificado"], true);
    let jwt = login["token"].as_str().unwrap();

    let (st, creada) = call(
        &e.app,
        "POST",
        "/api/empresas",
        Some(jwt),
        Some(json!({ "nombre": "Estetica Ana", "direccion": "Roca 100", "rubro_id": e.rubro })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{creada}");
    assert_eq!(creada["usuario_id"], login["user"]["id"]);

    // El reenvío también trae el enlace en desarrollo (para quien perdió el primero).
    call(
        &e.app,
        "POST",
        "/api/auth/register",
        None,
        Some(json!({ "nombre": "Beto", "email": "beto@comercio.com", "contraseña": "Clave1234", "rol": "comercio" })),
    )
    .await;
    let (_, r) = call(&e.app, "POST", "/api/auth/reenviar-verificacion", None, Some(json!({ "email": "beto@comercio.com" }))).await;
    assert!(r["token_verificacion"].is_string());
    let (_, r) = call(&e.app, "POST", "/api/auth/reenviar-verificacion", None, Some(json!({ "email": "nadie@x.com" }))).await;
    assert!(r.get("token_verificacion").is_none(), "no revela si un correo existe");
}

#[sqlx::test(migrations = "./migrations")]
async fn el_superadmin_verifica_correos_y_cambia_roles_con_reglas(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    let pendiente = crear_usuario_verificacion(&pool, "pendiente@test.com", "cliente", false).await;
    let uri = format!("/api/usuarios/{}", pendiente.id);

    for token in [&e.cliente.token, &e.dueno.token] {
        assert_eq!(
            call(&e.app, "PUT", &uri, Some(token), Some(json!({ "rol": "admin" }))).await.0,
            StatusCode::FORBIDDEN
        );
    }
    let (st, v) = call(&e.app, "PUT", &uri, Some(&e.superadmin.token), Some(json!({ "rol": "admin", "email_verificado": true }))).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["rol"], "admin");
    assert_eq!(v["email_verificado"], true);

    let (_, lista) = call(&e.app, "GET", "/api/usuarios?q=pendiente", Some(&e.superadmin.token), None).await;
    assert_eq!(lista[0]["email_verificado"], true, "el listado muestra si verificó el correo");

    // Reglas: no desverificar, no quitarse el propio rol, no dejar sin rol a un dueño con comercios.
    assert_eq!(call(&e.app, "PUT", &uri, Some(&e.superadmin.token), Some(json!({ "email_verificado": false }))).await.0, StatusCode::BAD_REQUEST);
    let propio = format!("/api/usuarios/{}", e.superadmin.id);
    assert_eq!(call(&e.app, "PUT", &propio, Some(&e.superadmin.token), Some(json!({ "rol": "cliente" }))).await.0, StatusCode::BAD_REQUEST);
    let dueno = format!("/api/usuarios/{}", e.dueno.id);
    assert_eq!(call(&e.app, "PUT", &dueno, Some(&e.superadmin.token), Some(json!({ "rol": "cliente" }))).await.0, StatusCode::CONFLICT);
    assert_eq!(call(&e.app, "PUT", &uri, Some(&e.superadmin.token), Some(json!({ "rol": "jefe" }))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&e.app, "PUT", "/api/usuarios/999999", Some(&e.superadmin.token), Some(json!({ "rol": "admin" }))).await.0, StatusCode::NOT_FOUND);

    // Un correo repetido no es un error interno.
    let (st, v) = call(
        &e.app,
        "POST",
        "/api/usuarios",
        Some(&e.superadmin.token),
        Some(json!({ "nombre": "Otro", "email": " PENDIENTE@test.com ", "contraseña": "Clave1234", "rol": "admin" })),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT, "{v}");

    // El cambio de rol rige al instante, aunque el token viejo diga otra cosa.
    let sesion_vieja = token_de(pendiente.id, "admin", SECRETO);
    call(&e.app, "PUT", &uri, Some(&e.superadmin.token), Some(json!({ "rol": "cliente" }))).await;
    let (st, _) = call(
        &e.app,
        "POST",
        "/api/empresas",
        Some(&sesion_vieja),
        Some(json!({ "nombre": "No", "direccion": "X 1", "rubro_id": e.rubro })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "ya no es dueño de comercio");
    let fantasma = token_de(424242, "superadmin", SECRETO);
    assert_eq!(call(&e.app, "GET", "/api/usuarios", Some(&fantasma), None).await.0, StatusCode::UNAUTHORIZED);
}

/// Matriz de permisos (12/11): ni un cliente ni el dueño de OTRO comercio pueden ver o tocar la
/// gestión de un comercio ajeno ni la de la plataforma.
#[sqlx::test(migrations = "./migrations")]
async fn ni_clientes_ni_duenos_ajenos_gestionan_lo_que_no_es_suyo(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    let otro = crear_usuario(&pool, "otro-dueno@test.com", "admin").await;
    let id = e.empresa;
    let manana = (chrono::Local::now() + chrono::Duration::days(1)).date_naive();
    let acciones: Vec<(&str, String, Option<Value>)> = vec![
        ("GET", format!("/api/empresas/{id}/turnos"), None),
        ("POST", format!("/api/empresas/{id}/turnos"), Some(json!({ "fecha": manana.to_string(), "hora": "09:00", "cliente_nombre": "X" }))),
        ("GET", format!("/api/empresas/{id}/resumen"), None),
        ("PUT", format!("/api/empresas/{id}"), Some(json!({ "nombre": "Robado", "direccion": "X 1", "rubro_id": e.rubro }))),
        ("DELETE", format!("/api/empresas/{id}"), None),
        ("PUT", format!("/api/empresas/{id}/horarios"), Some(json!({ "horarios": {} }))),
        ("PUT", format!("/api/empresas/{id}/telefonos"), Some(json!({ "telefonos": [] }))),
        ("PUT", format!("/api/empresas/{id}/verificado"), Some(json!({ "verificado": true }))),
        ("PUT", format!("/api/empresas/{id}/verificacion"), Some(json!({ "accion": "aprobar" }))),
        ("POST", "/api/publicidades".into(), Some(json!({ "empresa_id": id, "titulo": "T", "descripcion": "D", "url_imagen": "https://x.com/a.png" }))),
        ("GET", "/api/usuarios".into(), None),
        ("POST", "/api/usuarios".into(), Some(json!({ "nombre": "Z", "email": "z@z.com", "contraseña": "Clave1234", "rol": "superadmin" }))),
        ("PUT", format!("/api/usuarios/{}", e.cliente.id), Some(json!({ "rol": "superadmin" }))),
        ("POST", "/api/rubros".into(), Some(json!({ "nombre": "Nuevo" }))),
        ("PUT", format!("/api/rubros/{}", e.rubro), Some(json!({ "nombre": "Otro" }))),
        ("DELETE", format!("/api/rubros/{}", e.rubro), None),
        ("GET", "/api/reportes".into(), None),
        ("GET", "/api/publicidades?estado=pendiente".into(), None),
        ("GET", "/api/difusiones?estado=pendiente".into(), None),
    ];
    for (quien, token) in [("cliente", &e.cliente.token), ("dueno ajeno", &otro.token)] {
        for (metodo, uri, cuerpo) in &acciones {
            let (st, v) = call(&e.app, metodo, uri, Some(token), cuerpo.clone()).await;
            assert!(
                st == StatusCode::FORBIDDEN || st == StatusCode::NOT_FOUND,
                "{quien} pudo {metodo} {uri}: {st} {v}"
            );
            if st == StatusCode::FORBIDDEN {
                // Un solo mensaje, que habla de permisos (no de quién es dueño de qué).
                assert!(
                    v["error"].as_str().unwrap_or("").starts_with("No tenés los permisos para realizar esta acción"),
                    "{quien} {metodo} {uri}: {v}"
                );
            }
        }
    }
    // Sin sesión, todas piden iniciar sesión (401) antes que cualquier otra cosa.
    for (metodo, uri, cuerpo) in &acciones {
        let es_publica = *metodo == "GET" && (uri.contains("/publicidades") || uri.contains("/difusiones"));
        if es_publica {
            continue;
        }
        let (st, v) = call(&e.app, metodo, uri, None, cuerpo.clone()).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "anónimo {metodo} {uri}: {v}");
    }
    // El comercio sigue intacto.
    let (_, ficha) = call(&e.app, "GET", &format!("/api/empresas/{id}"), None, None).await;
    assert_eq!(ficha["nombre"], "Peluquería Central");

    // Reservar online es de clientes: ni el dueño (en su comercio o en otro) ni el superadmin.
    let martes = fecha(Weekday::Tue, 2);
    for (quien, token) in [("dueño", &e.dueno.token), ("dueño ajeno", &otro.token), ("superadmin", &e.superadmin.token)] {
        let (st, v) = call(&e.app, "POST", "/api/turnos/reservar", Some(token), Some(reserva(id, martes, "09:00"))).await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{quien}: {v}");
    }
    let (st, v) = call(&e.app, "POST", "/api/turnos/reservar", Some(&e.cliente.token), Some(reserva(id, martes, "09:00"))).await;
    assert_eq!(st, StatusCode::OK, "{v}");
}

/// Auditoría por rol (12/11): el CUIT, la constancia fiscal, el motivo de rechazo y el estado de la
/// solicitud son datos de la verificación comercial; solo los ven el dueño y el superadmin.
#[sqlx::test(migrations = "./migrations")]
async fn los_datos_de_la_verificacion_comercial_solo_los_ven_el_dueno_y_el_superadmin(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    let otro = crear_usuario(&pool, "otro-dueno@test.com", "admin").await;
    let solicitud = json!({
        "accion": "solicitar", "cuit": "20123456786",
        "url_cuit": "https://ejemplo.com/constancia.pdf"
    });
    let uri = format!("/api/empresas/{}/verificacion", e.empresa);
    assert_eq!(call(&e.app, "PUT", &uri, Some(&e.dueno.token), Some(solicitud)).await.0, StatusCode::OK);
    call(&e.app, "PUT", &uri, Some(&e.superadmin.token), Some(json!({ "accion": "rechazar", "motivo": "La constancia está vencida" }))).await;

    let ficha = format!("/api/empresas/{}", e.empresa);
    for (quien, token) in [("visitante", None), ("cliente", Some(&e.cliente.token)), ("otro dueño", Some(&otro.token))] {
        let (_, v) = call(&e.app, "GET", &ficha, token.map(|t| t.as_str()), None).await;
        assert!(v.get("cuit").is_none() && v.get("url_cuit").is_none() && v.get("motivo_rechazo").is_none(), "{quien} ve datos fiscales: {v}");
        assert_eq!(v["verificado_estado"], "sin_solicitar", "{quien} no se entera del rechazo");
        let (_, lista) = call(&e.app, "GET", "/api/empresas", token.map(|t| t.as_str()), None).await;
        let propia = lista.as_array().unwrap().iter().find(|x| x["id"] == e.empresa).unwrap();
        assert!(propia.get("cuit").is_none() && propia.get("motivo_rechazo").is_none(), "{quien} (listado)");
        let (st, _) = call(&e.app, "GET", "/api/empresas?verificado_estado=rechazado", token.map(|t| t.as_str()), None).await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{quien} no puede listar la bandeja de moderación");
    }
    for token in [&e.dueno.token, &e.superadmin.token] {
        let (_, v) = call(&e.app, "GET", &ficha, Some(token), None).await;
        assert_eq!(v["cuit"], "20123456786");
        assert_eq!(v["motivo_rechazo"], "La constancia está vencida");
        assert_eq!(v["verificado_estado"], "rechazado");
    }
    let (st, v) = call(&e.app, "GET", "/api/empresas?verificado_estado=rechazado", Some(&e.superadmin.token), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v[0]["cuit"], "20123456786");
}
