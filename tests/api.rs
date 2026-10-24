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
use tl360_backend::{recordatorios, resumen_semanal, routes, state::AppState};

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
        r#"INSERT INTO usuarios (nombre, email, "contraseña", rol, email_verificado) VALUES ($1, $2, $3, $4, $5) RETURNING id"#,
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

async fn crear_categoria(pool: &PgPool, nombre: &str) -> i32 {
    sqlx::query_scalar("INSERT INTO categorias (nombre) VALUES ($1) RETURNING id")
        .bind(nombre)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn crear_empresa(
    pool: &PgPool,
    nombre: &str,
    categoria: i32,
    admin: Option<i32>,
    turnos: bool,
    duracion: Option<i32>,
) -> i32 {
    sqlx::query_scalar(
        "INSERT INTO empresas (nombre, direccion, categoria_id, admin_id, turnos_habilitados, duracion_turno_min, descripcion) \
         VALUES ($1, 'Calle 123', $2, $3, $4, $5, 'Descripción larga de prueba') RETURNING id",
    )
    .bind(nombre)
    .bind(categoria)
    .bind(admin)
    .bind(turnos)
    .bind(duracion)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn poner_horario(pool: &PgPool, empresa: i32, dia: &str, ini: (u32, u32), fin: (u32, u32)) {
    sqlx::query("INSERT INTO horarios_empresa (empresa_id, dia_semana, hora_inicio, hora_fin) VALUES ($1, $2, $3, $4)")
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
    categoria: i32,
    empresa: i32,
}

async fn escenario(pool: PgPool) -> Escenario {
    let cliente = crear_usuario(&pool, "cliente@test.com", "cliente").await;
    let dueno = crear_usuario(&pool, "dueno@test.com", "admin").await;
    let superadmin = crear_usuario(&pool, "super@test.com", "superadmin").await;
    let categoria = crear_categoria(&pool, "Peluquerías").await;
    let empresa = crear_empresa(
        &pool,
        "Peluquería Central",
        categoria,
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
        categoria,
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
        "nombre": "Intruso", "email": "intruso@test.com", "contraseña": "clave123", "rol": "superadmin"
    }))).await;
    assert_eq!(st, StatusCode::CREATED);
    let rol: String =
        sqlx::query_scalar("SELECT rol FROM usuarios WHERE email = 'intruso@test.com'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(rol, "cliente");
}

#[sqlx::test(migrations = "./migrations")]
async fn el_registro_publico_crea_administradores_de_comercio(pool: PgPool) {
    let app = app(pool.clone());
    let (st, _) = call(&app, "POST", "/api/auth/register", None, Some(json!({
        "nombre": "Dueña", "email": "duena@test.com", "contraseña": "clave123", "rol": "comercio"
    }))).await;
    assert_eq!(st, StatusCode::CREATED);
    let (rol, verificado): (String, bool) =
        sqlx::query_as("SELECT rol, email_verificado FROM usuarios WHERE email = 'duena@test.com'")
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
    let cuerpo = |e: &str| json!({ "nombre": "Ana", "email": e, "contraseña": "clave123" });
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
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(v["error"].as_str().unwrap().contains("registrado"));
}

#[sqlx::test(migrations = "./migrations")]
async fn el_registro_valida_los_datos(pool: PgPool) {
    let app = app(pool);
    let larga = "x".repeat(100);
    for (nombre, email, clave) in [
        ("Ana", "ana@test.com", "123"),
        ("Ana", "no-es-email", "clave123"),
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
            "nombre": "Nueva", "email": "nueva@test.com", "contraseña": "clave123"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(v["user"]["email_verificado"], false);
    let verificado: bool =
        sqlx::query_scalar("SELECT email_verificado FROM usuarios WHERE email = 'nueva@test.com'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!verificado);
    let tokens: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM tokens_usuario WHERE tipo = 'verificar_email' AND usado_en IS NULL",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(tokens, 1);
    let minutos: f64 = sqlx::query_scalar(
        "SELECT EXTRACT(EPOCH FROM (expira_en - now()))::double precision / 60.0 \
         FROM tokens_usuario WHERE tipo = 'verificar_email' AND usado_en IS NULL",
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
    sqlx::query("INSERT INTO tokens_usuario (usuario_id, tipo, token_hash, expira_en) VALUES ($1, 'verificar_email', $2, now() + interval '24 hours')")
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
        sqlx::query_scalar("SELECT email_verificado FROM usuarios WHERE id = $1")
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
        sqlx::query_scalar("SELECT count(*) FROM tokens_usuario WHERE tipo = 'verificar_email'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(tokens, 1, "solo se genera token para la cuenta pendiente");

    let (hash_previo, minutos): (String, f64) = sqlx::query_as(
        "SELECT token_hash, EXTRACT(EPOCH FROM (expira_en - now()))::double precision / 60.0 \
         FROM tokens_usuario WHERE usuario_id = $1 AND tipo = 'verificar_email' AND usado_en IS NULL",
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
        "SELECT token_hash, usado_en IS NOT NULL FROM tokens_usuario WHERE token_hash = $1",
    )
    .bind(&hash_previo)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(usado, "el enlace anterior deja de servir tras el reenvío");
    let hash_nuevo: String = sqlx::query_scalar(
        "SELECT token_hash FROM tokens_usuario \
         WHERE usuario_id = $1 AND tipo = 'verificar_email' AND usado_en IS NULL",
    )
    .bind(pendiente.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_ne!(hash_activo, hash_nuevo, "se genera un token distinto");
    let activos: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM tokens_usuario \
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
    sqlx::query("UPDATE usuarios SET email_verificado = false WHERE id = $1")
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
    sqlx::query("INSERT INTO tokens_usuario (usuario_id, tipo, token_hash, expira_en) VALUES ($1, 'verificar_email', $2, now() + interval '24 hours')")
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
    sqlx::query("INSERT INTO tokens_usuario (usuario_id, tipo, token_hash, expira_en) VALUES ($1, 'reset_password', $2, now() + interval '1 hour')")
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
        Some(json!({ "token": token, "contraseña": "nueva-clave" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = call(
        &app,
        "POST",
        "/api/auth/login",
        None,
        Some(json!({ "email": "ana@test.com", "contraseña": "nueva-clave" })),
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
        Some(json!({ "token": token, "contraseña": "otra-clave" })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "el token se consume");
}

#[sqlx::test(migrations = "./migrations")]
async fn recuperar_la_contrasena_rechaza_tokens_vencidos_o_desconocidos(pool: PgPool) {
    let u = crear_usuario(&pool, "ana@test.com", "cliente").await;
    let app = app(pool.clone());
    use sha2::{Digest, Sha256};
    sqlx::query("INSERT INTO tokens_usuario (usuario_id, tipo, token_hash, expira_en) VALUES ($1, 'reset_password', $2, now() - interval '1 minute')")
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
            Some(json!({ "token": token, "contraseña": "nueva-clave" })),
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
    let tokens: i64 = sqlx::query_scalar("SELECT count(*) FROM tokens_usuario")
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
         FROM tokens_usuario WHERE usuario_id = $1 AND tipo = 'reset_password' AND usado_en IS NULL",
    )
    .bind(u.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!((55.0..=61.0).contains(&minutos));

    let (st, _) = call(&app, "POST", "/api/auth/forgot-password", None, cuerpo()).await;
    assert_eq!(st, StatusCode::OK);
    let usado: bool =
        sqlx::query_scalar("SELECT usado_en IS NOT NULL FROM tokens_usuario WHERE token_hash = $1")
            .bind(&hash_previo)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(usado, "el enlace anterior deja de servir");
    let activos: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM tokens_usuario \
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

    let nuevo = json!({ "nombre": "Nuevo", "email": "nuevo@test.com", "contraseña": "clave123", "rol": "admin" });
    assert_eq!(
        call(&e.app, "POST", "/api/usuarios", None, Some(nuevo.clone()))
            .await
            .0,
        StatusCode::FORBIDDEN
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
        json!({ "nombre": "X", "email": "x@test.com", "contraseña": "clave123", "rol": "dios" });
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
            "/api/categorias",
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
        "/api/categorias",
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
            "/api/categorias",
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
            "/api/categorias",
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
        &format!("/api/categorias/{id}"),
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
            "/api/categorias/99999",
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
            &format!("/api/categorias/{id}"),
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
            &format!("/api/categorias/{}", e.categoria),
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
    let sin_turnos = crear_categoria(&pool, "Kioscos").await;
    crear_empresa(&pool, "Kiosco 1", sin_turnos, None, false, None).await;
    crear_categoria(&pool, "Vacío").await;
    let (_, v) = call(&e.app, "GET", "/api/categorias", None, None).await;
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
            e.categoria,
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
async fn la_busqueda_encuentra_por_nombre_o_por_rubro_y_resiste_inyeccion(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    let otro = crear_categoria(&pool, "Ferreterías").await;
    crear_empresa(&pool, "El Tornillo", otro, None, false, None).await;
    let comida = crear_categoria(&pool, "Gastronomía").await;
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
                &format!("/api/empresas?categoria_id={otro}"),
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
                &format!("/api/empresas/categoria/{otro}"),
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
    crear_empresa(&pool, "Ajeno", e.categoria, None, false, None).await;
    let (_, v) = call(
        &e.app,
        "GET",
        &format!("/api/empresas?admin_id={}", e.dueno.id),
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
    sqlx::query("INSERT INTO telefonos_empresa (empresa_id, numero) VALUES ($1, '2392111111')")
        .bind(e.empresa)
        .execute(&pool)
        .await
        .unwrap();
    let et: i32 =
        sqlx::query_scalar("INSERT INTO etiquetas (nombre) VALUES ('Wi-Fi') RETURNING id")
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query("INSERT INTO empresa_etiquetas VALUES ($1, $2)")
        .bind(e.empresa)
        .bind(et)
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
    assert_eq!(ficha["categoria"], "Peluquerías");
    assert_eq!(ficha["verificado"], false);

    let (_, lista) = call(&e.app, "GET", "/api/empresas", None, None).await;
    assert!(lista[0].get("descripcion").is_none());
    assert_eq!(lista[0]["etiquetas"], json!(["Wi-Fi"]));
    assert_eq!(
        call(&e.app, "GET", "/api/empresas/999999", None, None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&e.app, "GET", "/api/empresas/abc", None, None).await.0,
        StatusCode::BAD_REQUEST
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn solo_el_dueno_o_un_superadmin_edita_un_comercio(pool: PgPool) {
    let e = escenario(pool).await;
    let cuerpo = json!({ "nombre": "Nuevo nombre", "direccion": "Otra calle 1", "categoria_id": e.categoria });
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
    sqlx::query("UPDATE empresas SET fotos = ARRAY['https://x.com/a.jpg'] WHERE id = $1")
        .bind(e.empresa)
        .execute(&pool)
        .await
        .unwrap();
    let cuerpo = json!({ "nombre": "Solo cambio el nombre", "direccion": "Calle 123", "categoria_id": e.categoria, "admin_id": e.cliente.id });
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
        v["admin_id"], e.dueno.id,
        "un dueño no puede ceder ni cambiar la propiedad"
    );
    assert_eq!(v["fotos"], json!(["https://x.com/a.jpg"]));
    assert_eq!(v["descripcion"], "Descripción larga de prueba");
    assert_eq!(v["turnos_habilitados"], true);
}

#[sqlx::test(migrations = "./migrations")]
async fn filtrar_por_etiquetas_exige_todas_y_acepta_alias(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    let otra = crear_empresa(&pool, "Gimnasio Norte", e.categoria, None, false, None).await;
    for (empresa, nombres) in [
        (e.empresa, vec!["Wi-Fi gratis", "Rampa de acceso"]),
        (otra, vec!["Apto celíacos"]),
    ] {
        for nombre in nombres {
            let etiqueta: i32 = sqlx::query_scalar(
                "INSERT INTO etiquetas (nombre) VALUES ($1) \
                 ON CONFLICT (nombre) DO UPDATE SET nombre = EXCLUDED.nombre RETURNING id",
            )
            .bind(nombre)
            .fetch_one(&pool)
            .await
            .unwrap();
            sqlx::query("INSERT INTO empresa_etiquetas VALUES ($1, $2) ON CONFLICT DO NOTHING")
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
        "categoria_id": e.categoria
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
        sqlx::query_scalar("SELECT count(*) FROM empresa_etiquetas WHERE empresa_id = $1")
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
        "categoria_id": e.categoria,
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
        let mut b = json!({ "nombre": "X", "direccion": "Y", "categoria_id": e.categoria });
        b.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        b
    };
    for malo in [
        json!({ "instagram": "javascript:alert(1)" }),
        json!({ "facebook": "data:text/html,x" }),
        json!({ "fotos": ["https://ok.com/a.jpg", "javascript:1"] }),
    ] {
        assert_eq!(
            call(&e.app, "PUT", &uri, Some(&e.dueno.token), Some(base(malo)))
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    let bueno = base(
        json!({ "instagram": "https://instagram.com/negocio", "fotos": ["https://ok.com/a.jpg"] }),
    );
    assert_eq!(
        call(&e.app, "PUT", &uri, Some(&e.dueno.token), Some(bueno))
            .await
            .0,
        StatusCode::OK
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn crear_un_comercio_lo_asigna_al_usuario_y_valida_el_rubro(pool: PgPool) {
    let e = escenario(pool).await;
    let cuerpo = json!({ "nombre": "Mi local", "direccion": "Calle 1", "categoria_id": e.categoria, "admin_id": e.superadmin.id });
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
    assert_eq!(v["admin_id"], e.dueno.id, "admin_id ajeno se ignora");
    let (_, v) = call(
        &e.app,
        "POST",
        "/api/empresas",
        Some(&e.superadmin.token),
        Some(cuerpo),
    )
    .await;
    assert_eq!(
        v["admin_id"], e.superadmin.id,
        "el superadmin sí puede asignar"
    );
    let sin_rubro = json!({ "nombre": "X", "direccion": "Y", "categoria_id": 424242 });
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
    let vacio = json!({ "nombre": " ", "direccion": "Y", "categoria_id": e.categoria });
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

    for malo in [
        json!({ "Lunes": ["20:00-10:00"] }),
        json!({ "Lunez": ["09:00-10:00"] }),
        json!({ "Lunes": ["9 a 10"] }),
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
async fn los_telefonos_se_reemplazan_sin_romper_con_repetidos(pool: PgPool) {
    let e = escenario(pool).await;
    let uri = format!("/api/empresas/{}/telefonos", e.empresa);
    let cuerpo = json!({ "telefonos": [
        { "numero": "2392111111", "etiqueta": "Turnos", "modo_contacto": "mensaje" },
        { "numero": "2392111111" },
        { "numero": " 2392222222 " }
    ] });
    let (st, v) = call(&e.app, "PUT", &uri, Some(&e.dueno.token), Some(cuerpo)).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v.as_array().unwrap().len(), 2);
    assert_eq!(v[0]["modo_contacto"], "mensaje");
    assert!(
        v[1]["modo_contacto"].is_null(),
        "sin modo hereda el del comercio"
    );
    assert_eq!(
        call(
            &e.app,
            "PUT",
            &uri,
            Some(&e.dueno.token),
            Some(json!({ "telefonos": [{ "numero": "2392333333", "modo_contacto": "paloma" }] }))
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
        e.categoria,
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
    let (st, _) = call(
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
        sqlx::query_scalar("SELECT count(*) FROM turnos WHERE estado = 'confirmado'")
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

    let sin_turnos = crear_empresa(&pool, "Solo ficha", e.categoria, None, false, None).await;
    let (st, v) = reservar(sin_turnos, martes, "10:00", tok).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(v["error"].as_str().unwrap().contains("no toma turnos"));
}

#[sqlx::test(migrations = "./migrations")]
async fn la_grilla_usa_la_duracion_del_turno_del_comercio(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    let f = fecha(Weekday::Tue, 3);
    sqlx::query("UPDATE empresas SET duracion_turno_min = 45 WHERE id = $1")
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
    let id: i32 = sqlx::query_scalar("SELECT id FROM turnos WHERE usuario_id = $1")
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
    let estado: String = sqlx::query_scalar("SELECT estado FROM turnos WHERE id = $1")
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
    let id: i32 = sqlx::query_scalar("SELECT id FROM turnos")
        .fetch_one(&pool)
        .await
        .unwrap();
    // 30 días de anticipación exigidos: el turno (a menos de 10 días) ya no se puede cancelar
    sqlx::query("UPDATE empresas SET anticipacion_cancelacion_min = 43200 WHERE id = $1")
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
    let estado: String = sqlx::query_scalar("SELECT estado FROM turnos WHERE id = $1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(estado, "confirmado");
    // con la política por defecto (2 h) sí
    sqlx::query("UPDATE empresas SET anticipacion_cancelacion_min = 120 WHERE id = $1")
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
    let id: i32 = sqlx::query_scalar("SELECT id FROM turnos")
        .fetch_one(&pool)
        .await
        .unwrap();
    // El cliente no podría cancelar con 30 días de anticipación exigidos, pero el dueño sí.
    sqlx::query("UPDATE empresas SET anticipacion_cancelacion_min = 43200 WHERE id = $1")
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
    let estado: String = sqlx::query_scalar("SELECT estado FROM turnos WHERE id = $1")
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
        "INSERT INTO turnos (empresa_id, usuario_id, fecha, hora, estado) \
         VALUES ($1, $2, CURRENT_DATE - 3, '11:00', 'confirmado') RETURNING id",
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
        "INSERT INTO turnos (empresa_id, usuario_id, fecha, hora, estado) \
         VALUES ($1, $2, CURRENT_DATE - 7, '10:00', 'confirmado') RETURNING id",
    )
    .bind(e.empresa)
    .bind(e.cliente.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let futuro: i32 = sqlx::query_scalar(
        "INSERT INTO turnos (empresa_id, usuario_id, fecha, hora, estado) \
         VALUES ($1, $2, CURRENT_DATE + 7, '10:00', 'confirmado') RETURNING id",
    )
    .bind(e.empresa)
    .bind(e.cliente.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let cancelado: i32 = sqlx::query_scalar(
        "INSERT INTO turnos (empresa_id, usuario_id, fecha, hora, estado) \
         VALUES ($1, $2, CURRENT_DATE - 5, '11:00', 'cancelado') RETURNING id",
    )
    .bind(e.empresa)
    .bind(e.cliente.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let ajeno: i32 = sqlx::query_scalar(
        "INSERT INTO turnos (empresa_id, usuario_id, fecha, hora, estado) \
         VALUES ($1, $2, CURRENT_DATE - 6, '16:00', 'confirmado') RETURNING id",
    )
    .bind(e.empresa)
    .bind(otro.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let otra_empresa = crear_empresa(&pool, "Otro comercio", e.categoria, None, false, None).await;

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
async fn las_publicidades_las_gestiona_solo_el_dueno(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    let otra = crear_empresa(&pool, "Otra", e.categoria, None, false, None).await;
    let nueva = json!({ "empresa_id": e.empresa, "titulo": "2x1", "descripcion": "Este mes", "imagen_url": "https://x.com/a.jpg" });
    assert_eq!(
        call(
            &e.app,
            "POST",
            "/api/publicidades",
            None,
            Some(nueva.clone())
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &e.app,
            "POST",
            "/api/publicidades",
            Some(&e.cliente.token),
            Some(nueva.clone())
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let mut ajena = nueva.clone();
    ajena["empresa_id"] = json!(otra);
    assert_eq!(
        call(
            &e.app,
            "POST",
            "/api/publicidades",
            Some(&e.dueno.token),
            Some(ajena)
        )
        .await
        .0,
        StatusCode::FORBIDDEN,
        "no publica en comercios ajenos"
    );
    let mut peligrosa = nueva.clone();
    peligrosa["imagen_url"] = json!("javascript:alert(1)");
    assert_eq!(
        call(
            &e.app,
            "POST",
            "/api/publicidades",
            Some(&e.dueno.token),
            Some(peligrosa)
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let mut sin_titulo = nueva.clone();
    sin_titulo["titulo"] = json!(" ");
    assert_eq!(
        call(
            &e.app,
            "POST",
            "/api/publicidades",
            Some(&e.dueno.token),
            Some(sin_titulo)
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let mut sin_descripcion = nueva.clone();
    sin_descripcion["descripcion"] = json!(" ");
    assert_eq!(
        call(
            &e.app,
            "POST",
            "/api/publicidades",
            Some(&e.dueno.token),
            Some(sin_descripcion)
        )
        .await
        .0,
        StatusCode::BAD_REQUEST,
        "la descripción breve es parte de la publicación"
    );
    let mut sin_imagen = nueva.clone();
    sin_imagen["imagen_url"] = json!("");
    assert_eq!(
        call(
            &e.app,
            "POST",
            "/api/publicidades",
            Some(&e.dueno.token),
            Some(sin_imagen)
        )
        .await
        .0,
        StatusCode::BAD_REQUEST,
        "sin imagen no hay publicación destacada"
    );

    let (st, creada) = call(
        &e.app,
        "POST",
        "/api/publicidades",
        Some(&e.dueno.token),
        Some(nueva),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    assert!(creada["id"].is_string(), "el front espera el id como texto");
    let id = creada["id"].as_str().unwrap();
    assert_eq!(
        total_de(&call(&e.app, "GET", "/api/publicidades", None, None).await.1),
        1
    );
    assert_eq!(
        total_de(
            &call(
                &e.app,
                "GET",
                &format!("/api/publicidades?empresa_id={otra}"),
                None,
                None
            )
            .await
            .1
        ),
        0
    );
    // RF-06: las colectas/eventos de la plataforma no tienen comercio y solo las crea el superadmin.
    let colecta = json!({ "empresa_id": null, "titulo": "Colecta para el club", "descripcion": "Arreglamos el techo", "imagen_url": "https://x.com/colecta.jpg" });
    assert_eq!(
        call(
            &e.app,
            "POST",
            "/api/publicidades",
            Some(&e.cliente.token),
            Some(colecta.clone())
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &e.app,
            "POST",
            "/api/publicidades",
            Some(&e.dueno.token),
            Some(colecta.clone())
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (st, colecta_creada) = call(
        &e.app,
        "POST",
        "/api/publicidades",
        Some(&e.superadmin.token),
        Some(colecta),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    assert!(
        colecta_creada["empresa_id"].is_null(),
        "la colecta no pertenece a un comercio"
    );
    assert_eq!(
        total_de(&call(&e.app, "GET", "/api/publicidades", None, None).await.1),
        2
    );
    let colecta_id = colecta_creada["id"].as_str().unwrap();
    assert_eq!(
        call(
            &e.app,
            "DELETE",
            &format!("/api/publicidades/{colecta_id}"),
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
            "DELETE",
            &format!("/api/publicidades/{id}"),
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
            "DELETE",
            &format!("/api/publicidades/{id}"),
            Some(&e.dueno.token),
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
            &format!("/api/publicidades/{id}"),
            Some(&e.dueno.token),
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &e.app,
            "DELETE",
            &format!("/api/publicidades/{colecta_id}"),
            Some(&e.superadmin.token),
            None
        )
        .await
        .0,
        StatusCode::OK
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn el_canal_publico_recibe_solicitudes_y_el_superadmin_las_gestiona(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    let solicitud = json!({
        "nombre": "Vecinos por el club",
        "contacto": "vecinos@demo.com",
        "tipo": "colecta",
        "mensaje": "Recaudamos para arreglar el techo del club del barrio."
    });
    let (st, v) = call(
        &e.app,
        "POST",
        "/api/solicitudes-publicidad",
        None,
        Some(solicitud.clone()),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "el canal público no pide sesión");
    assert!(v["message"].as_str().unwrap().contains("Recibimos"));
    assert_eq!(
        call(
            &e.app,
            "POST",
            "/api/solicitudes-publicidad",
            None,
            Some(solicitud)
        )
        .await
        .0,
        StatusCode::CONFLICT,
        "no se duplica una solicitud pendiente del mismo contacto"
    );

    let mut corta = json!({ "nombre": "Ana", "contacto": "11 5555 1234", "mensaje": "hola" });
    assert_eq!(
        call(
            &e.app,
            "POST",
            "/api/solicitudes-publicidad",
            None,
            Some(corta.clone())
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    corta["mensaje"] = json!("Quiero difundir un evento solidario del barrio");
    corta["contacto"] = json!("sin datos");
    assert_eq!(
        call(
            &e.app,
            "POST",
            "/api/solicitudes-publicidad",
            None,
            Some(corta.clone())
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    corta["contacto"] = json!("11 5555-1234");
    corta["tipo"] = json!("sorteo");
    assert_eq!(
        call(
            &e.app,
            "POST",
            "/api/solicitudes-publicidad",
            None,
            Some(corta.clone())
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    corta["tipo"] = json!("publicidad");
    assert_eq!(
        call(
            &e.app,
            "POST",
            "/api/solicitudes-publicidad",
            None,
            Some(corta)
        )
        .await
        .0,
        StatusCode::CREATED,
        "un teléfono también sirve de contacto"
    );

    assert_eq!(
        call(&e.app, "GET", "/api/solicitudes-publicidad", None, None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &e.app,
            "GET",
            "/api/solicitudes-publicidad",
            Some(&e.cliente.token),
            None
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (st, lista) = call(
        &e.app,
        "GET",
        "/api/solicitudes-publicidad",
        Some(&e.superadmin.token),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(lista.as_array().unwrap().len(), 2);
    let id = lista[0]["id"].as_i64().unwrap();
    let uri = format!("/api/solicitudes-publicidad/{id}");

    assert_eq!(
        call(
            &e.app,
            "PUT",
            &uri,
            Some(&e.cliente.token),
            Some(json!({ "estado": "atendida" }))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &e.app,
            "PUT",
            &uri,
            Some(&e.superadmin.token),
            Some(json!({ "estado": "en_curso" }))
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
            Some(&e.superadmin.token),
            Some(json!({ "estado": "atendida" }))
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &e.app,
            "PUT",
            "/api/solicitudes-publicidad/999999",
            Some(&e.superadmin.token),
            Some(json!({ "estado": "atendida" }))
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );

    let (_, pendientes) = call(
        &e.app,
        "GET",
        "/api/solicitudes-publicidad",
        Some(&e.superadmin.token),
        None,
    )
    .await;
    assert_eq!(
        pendientes.as_array().unwrap().len(),
        1,
        "la atendida sale de la bandeja"
    );
    let (_, todas) = call(
        &e.app,
        "GET",
        "/api/solicitudes-publicidad?todas=true",
        Some(&e.superadmin.token),
        None,
    )
    .await;
    assert_eq!(todas.as_array().unwrap().len(), 2);
}

// ------------------------------------------------------------------ esquema

#[sqlx::test(migrations = "./migrations")]
async fn el_esquema_impide_datos_incoherentes(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    let falla = |sql: &'static str| {
        let pool = pool.clone();
        async move { sqlx::query(sql).execute(&pool).await.is_err() }
    };
    assert!(falla("INSERT INTO calificaciones (usuario_id, empresa_id, puntaje) SELECT u.id, e.id, 9 FROM usuarios u, empresas e LIMIT 1").await, "puntaje fuera de 1-5");
    assert!(falla("INSERT INTO usuarios (nombre, email, \"contraseña\", rol) VALUES ('x', 'x@x.com', 'h', 'dios')").await, "rol inválido");
    assert!(falla("INSERT INTO usuarios (nombre, email, \"contraseña\") VALUES ('x', 'CLIENTE@test.com', 'h')").await, "email repetido sin importar mayúsculas");
    assert!(falla("INSERT INTO horarios_empresa (empresa_id, dia_semana, hora_inicio, hora_fin) SELECT id, 'Lunes', '10:00', '09:00' FROM empresas").await, "franja al revés");
    assert!(falla("INSERT INTO horarios_empresa (empresa_id, dia_semana, hora_inicio, hora_fin) SELECT id, 'Lunez', '09:00', '10:00' FROM empresas").await, "día inexistente");
    // el rubro principal figura siempre en la relación N:M (trigger)
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM empresa_categorias WHERE empresa_id = $1 AND categoria_id = $2",
    )
    .bind(e.empresa)
    .bind(e.categoria)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 1);
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
    let id: i32 = sqlx::query_scalar("SELECT id FROM turnos WHERE hora = '11:00'")
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
                "constancia_cuit_url": "https://ejemplo.com/constancia.pdf"
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
    assert_eq!(call(&e.app, "PUT", &uri, Some(&e.dueno.token), Some(json!({ "accion": "solicitar", "cuit": "20123456786", "constancia_cuit_url": "no-es-url" }))).await.0, StatusCode::BAD_REQUEST, "la constancia debe ser una URL http(s)");
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
            "constancia_cuit_url": "https://ejemplo.com/constancia.pdf"
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
        None,
        None,
    )
    .await;
    assert_eq!(total_de(&pendientes), 1);
    assert_eq!(pendientes[0]["cuit"], json!("20123456786"));
    assert_eq!(
        pendientes[0]["constancia_cuit_url"],
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
    assert_eq!(ficha["cuit"], json!("20123456786"));
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
    let (st, v) = call(
        &e.app,
        "PUT",
        &uri,
        Some(&e.superadmin.token),
        Some(json!({ "accion": "rechazar" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["verificado"], json!(false));
    assert_eq!(v["verificado_estado"], json!("rechazado"));
    let (_, pendientes) = call(
        &e.app,
        "GET",
        "/api/empresas?verificado_estado=pendiente",
        None,
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
            Some(json!({ "motivo": "Datos falsos" }))
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
            Some(json!({ "motivo": "mal" }))
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
            Some(json!({ "motivo": "Datos falsos" }))
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
            Some(json!({ "motivo": "Los datos son falsos" }))
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
            Some(json!({ "motivo": "Los datos son falsos" }))
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
            Some(json!({ "motivo": "Sigue mal" }))
        )
        .await
        .0,
        StatusCode::OK
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn la_api_expone_las_coordenadas_para_el_mapa(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    sqlx::query("UPDATE empresas SET lat = -35.9711, lng = -62.7333 WHERE id = $1")
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
        "categoria_id": e.categoria,
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
        "categoria_id": e.categoria
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
            "categoria_id": e.categoria,
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
            "categoria_id": e.categoria
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
        "categoria_id": e.categoria
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
    let cerca = crear_empresa(&pool, "Cerca pero cerrado", e.categoria, None, false, None).await;
    let lejos = crear_empresa(&pool, "Lejos pero abierto", e.categoria, None, false, None).await;
    crear_empresa(&pool, "Sin ubicación", e.categoria, None, false, None).await;
    for (id, lat, lng) in [
        (cerca, base.0 + 0.0001, base.1),
        (lejos, base.0 + 0.01, base.1),
    ] {
        sqlx::query("UPDATE empresas SET lat = $1, lng = $2 WHERE id = $3")
            .bind(lat)
            .bind(lng)
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
    }
    sqlx::query(
        "INSERT INTO horarios_empresa (empresa_id, dia_semana, hora_inicio, hora_fin) \
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
    let otra = crear_categoria(&pool, "Gimnasios").await;
    let sin_geo = crear_empresa(&pool, "Sin coordenadas", e.categoria, None, false, None).await;
    let en_gim = crear_empresa(&pool, "Gimnasio Norte", otra, None, false, None).await;
    for id in [e.empresa, en_gim] {
        sqlx::query("UPDATE empresas SET lat = -35.97, lng = -62.73 WHERE id = $1")
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
        &format!("/api/empresas/mapa?categoria_id={otra}"),
        None,
        None,
    )
    .await;
    assert_eq!(total_de(&solo_gim), 1);
    assert_eq!(solo_gim[0]["nombre"], json!("Gimnasio Norte"));
}

#[sqlx::test(migrations = "./migrations")]
async fn los_walk_ins_se_registran_y_alimentan_las_metricas_del_comercio(pool: PgPool) {
    let e = escenario(pool).await;
    let uri = format!("/api/empresas/{}/atenciones", e.empresa);
    let hoy = ahora_local().date();
    let viejo = hoy - Duration::days(40);
    let intruso = crear_usuario(&e.pool, "otro-dueno@test.com", "admin").await;

    assert_eq!(
        call(&e.app, "GET", &uri, None, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&e.app, "GET", &uri, Some(&e.cliente.token), None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&e.app, "GET", &uri, Some(&intruso.token), None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &e.app,
            "POST",
            &uri,
            Some(&e.cliente.token),
            Some(json!({ "cantidad": 1 }))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );

    for cuerpo in [
        json!({ "cantidad": 0 }),
        json!({ "cantidad": 1001 }),
        json!({ "cantidad": 1, "importe": -10 }),
        json!({ "cantidad": 1, "importe": 0 }),
        json!({ "cantidad": 1, "fecha": (hoy + Duration::days(1)).to_string() }),
        json!({ "cantidad": 1, "nota": "x".repeat(301) }),
    ] {
        assert_eq!(
            call(&e.app, "POST", &uri, Some(&e.dueno.token), Some(cuerpo))
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }

    let (st, creada) = call(
        &e.app,
        "POST",
        &uri,
        Some(&e.dueno.token),
        Some(json!({
            "cantidad": 2, "importe": 1500.5, "nota": "  Dos cortes  "
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(creada["cantidad"], json!(2));
    assert_eq!(creada["importe"].as_f64(), Some(1500.5));
    assert_eq!(creada["nota"], json!("Dos cortes"));
    assert_eq!(creada["fecha"], json!(hoy.to_string()));
    let id = creada["id"].as_i64().unwrap();

    let (st, _) = call(
        &e.app,
        "POST",
        &uri,
        Some(&e.dueno.token),
        Some(json!({
            "cantidad": 5, "fecha": viejo.to_string()
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);

    let (st, v) = call(&e.app, "GET", &uri, Some(&e.dueno.token), None).await;
    assert_eq!(st, StatusCode::OK);
    let lista = v["atenciones"].as_array().unwrap();
    assert_eq!(lista.len(), 2);
    assert_eq!(lista[0]["fecha"], json!(hoy.to_string()));
    assert!(
        lista[1]["importe"].is_null(),
        "sin importe no se inventa un monto"
    );
    assert_eq!(v["resumen"]["hoy_cantidad"], json!(2));
    assert_eq!(v["resumen"]["hoy_importe"].as_f64(), Some(1500.5));
    assert_eq!(v["resumen"]["mes_cantidad"], json!(2));
    assert_eq!(v["resumen"]["total_cantidad"], json!(7));
    assert_eq!(v["resumen"]["total_importe"].as_f64(), Some(1500.5));

    assert_eq!(
        call(&e.app, "GET", &uri, Some(&e.superadmin.token), None)
            .await
            .0,
        StatusCode::OK
    );

    let borrar = format!("{uri}/{id}");
    assert_eq!(
        call(&e.app, "DELETE", &borrar, Some(&intruso.token), None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&e.app, "DELETE", &borrar, Some(&e.dueno.token), None)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        call(&e.app, "DELETE", &borrar, Some(&e.dueno.token), None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );

    let (_, v) = call(&e.app, "GET", &uri, Some(&e.dueno.token), None).await;
    assert_eq!(v["resumen"]["total_cantidad"], json!(5));
    assert_eq!(v["resumen"]["hoy_cantidad"], json!(0));
    assert_eq!(v["resumen"]["hoy_importe"].as_f64(), Some(0.0));
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
        sqlx::query_scalar("SELECT usuario_id FROM push_suscripciones WHERE endpoint = $1")
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
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM push_suscripciones")
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
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM push_suscripciones")
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
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM push_suscripciones")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(total, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn los_recordatorios_toman_solo_turnos_futuros_con_push_y_no_se_repiten(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    let endpoint = "https://updates.push.services.mozilla.com/wpush/v2/prueba";
    sqlx::query("INSERT INTO push_suscripciones (usuario_id, endpoint, p256dh, auth) VALUES ($1, $2, $3, $4)")
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
        "INSERT INTO turnos (empresa_id, usuario_id, fecha, hora, estado) \
         VALUES ($1, $2, $3, $4, 'confirmado') RETURNING id",
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
        "INSERT INTO turnos (empresa_id, usuario_id, fecha, hora, estado) \
         VALUES ($1, $2, $3, $4, 'confirmado') RETURNING id",
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
        sqlx::query_scalar("SELECT recordatorio_horas_anticipacion FROM usuarios WHERE id = $1")
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
            "INSERT INTO push_suscripciones (usuario_id, endpoint, p256dh, auth) VALUES ($1, $2, $3, $4)",
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

    sqlx::query("UPDATE usuarios SET recordatorio_horas_anticipacion = 1 WHERE id = $1")
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
            "INSERT INTO turnos (empresa_id, usuario_id, fecha, hora, estado) \
             VALUES ($1, $2, $3, $4, 'confirmado') RETURNING id",
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

#[sqlx::test(migrations = "./migrations")]
async fn el_resumen_semanal_cuenta_la_semana_y_no_se_repite(pool: PgPool) {
    let e = escenario(pool.clone()).await;
    let inicio = NaiveDate::from_ymd_opt(2026, 10, 19).unwrap();
    let fin = inicio + Duration::days(7);
    let ahora = inicio.and_hms_opt(9, 0, 0).unwrap();

    sqlx::query(
        "INSERT INTO push_suscripciones (usuario_id, endpoint, p256dh, auth) VALUES ($1, $2, $3, $4)",
    )
    .bind(e.dueno.id)
    .bind("https://updates.push.services.mozilla.com/wpush/v2/resumen")
    .bind(PUSH_P256DH)
    .bind(PUSH_AUTH)
    .execute(&pool)
    .await
    .unwrap();

    for (fecha, hora, estado) in [
        (inicio + Duration::days(1), "10:00", "confirmado"),
        (inicio + Duration::days(2), "11:00", "cancelado"),
        (inicio + Duration::days(3), "12:00", "completado"),
        (fin, "10:00", "confirmado"),
    ] {
        let hora = NaiveTime::parse_from_str(hora, "%H:%M").unwrap();
        sqlx::query(
            "INSERT INTO turnos (empresa_id, usuario_id, fecha, hora, estado) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(e.empresa)
        .bind(e.cliente.id)
        .bind(fecha)
        .bind(hora)
        .bind(estado)
        .execute(&pool)
        .await
        .unwrap();
    }

    let resumenes = resumen_semanal::pendientes(&pool, inicio, fin, ahora)
        .await
        .unwrap();
    assert_eq!(resumenes.len(), 1);
    assert_eq!(resumenes[0].empresa_id, e.empresa);
    assert_eq!(resumenes[0].confirmados, 2);
    assert_eq!(resumenes[0].cancelados, 1);
    assert_eq!(
        resumenes[0].proximo,
        Some((inicio + Duration::days(1)).and_hms_opt(10, 0, 0).unwrap())
    );

    resumen_semanal::marcar_enviado(&pool, e.empresa, inicio)
        .await
        .unwrap();
    let resumenes = resumen_semanal::pendientes(&pool, inicio, fin, ahora)
        .await
        .unwrap();
    assert!(
        resumenes.is_empty(),
        "el resumen de una semana ya enviada no se repite"
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
