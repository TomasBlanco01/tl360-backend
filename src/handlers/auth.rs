use axum::{extract::State, http::StatusCode, Json};
use chrono::{Duration, Utc};
use jsonwebtoken::{encode, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;

use crate::auth_extractor::Claims;
use crate::errors::{AppError, AppJson, AppResult};
use crate::rate_limit::{espera_legible, ClientIp};
use crate::state::AppState;
use crate::validation::{email_valido, nombre_valido, password_valida};

// Hash bcrypt (costo 12) de una clave cualquiera: se usa para gastar el mismo tiempo cuando el correo no existe.
/// Ventana y topes de intentos. Los fallos de login se cuentan por (IP, correo) y también por IP sola.
const VENTANA_LOGIN: std::time::Duration = std::time::Duration::from_secs(15 * 60);
const MAX_FALLOS_POR_CUENTA: usize = 5;
const MAX_FALLOS_POR_IP: usize = 30;
const VENTANA_HORA: std::time::Duration = std::time::Duration::from_secs(60 * 60);

fn demasiados(espera: std::time::Duration) -> AppError {
    AppError::TooManyRequests(format!(
        "Demasiados intentos. Probá de nuevo en {}.",
        espera_legible(espera)
    ))
}

const HASH_DESCARTABLE: &str = "$2b$12$RmKPy2/VQx0PY2uWsgsVL.77xbiM0Y3COSHVZwyNX0GuPI2n/1sb.";

#[derive(Deserialize)]
pub struct RegisterRequest {
    nombre: String,
    email: String,
    #[serde(rename = "contraseña")]
    contrasena: String,
    /// RF-01: `comercio` crea una cuenta de administrador de comercio; el resto, cliente.
    rol: Option<String>,
}

#[derive(Deserialize)]
pub struct LoginRequest {
    email: String,
    #[serde(rename = "contraseña")]
    contrasena: String,
}

#[derive(FromRow, Serialize)]
struct UsuarioPublico {
    id: i32,
    nombre: String,
    email: String,
    rol: String,
    email_verificado: bool,
}

#[derive(FromRow)]
struct UsuarioConHash {
    id: i32,
    nombre: String,
    email: String,
    #[sqlx(rename = "contraseña")]
    contrasena: String,
    rol: String,
    email_verificado: bool,
}

pub async fn register(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    AppJson(body): AppJson<RegisterRequest>,
) -> AppResult<(StatusCode, Json<Value>)> {
    let clave = format!("register:{ip}");
    if let Some(espera) = state.limiter.bloqueado(&clave, 20, VENTANA_HORA) {
        return Err(demasiados(espera));
    }
    state.limiter.registrar(&clave);
    nombre_valido(&body.nombre)?;
    email_valido(&body.email)?;
    password_valida(&body.contrasena)?;
    // RF-01: el registro público crea clientes o administradores de comercio. El rol de
    // plataforma (superadmin) nunca se puede autoasignar: solo lo otorga otro superadmin.
    let rol = match body.rol.as_deref().map(str::trim) {
        Some("comercio") | Some("admin") => "admin",
        _ => "cliente",
    };

    let existente =
        sqlx::query_scalar::<_, i32>("SELECT id FROM usuarios WHERE lower(email) = lower($1)")
            .bind(&body.email)
            .fetch_optional(&state.db)
            .await?;
    if existente.is_some() {
        return Err(AppError::BadRequest("El correo ya está registrado".into()));
    }

    let hash = bcrypt::hash(&body.contrasena, bcrypt::DEFAULT_COST)?;

    let user = sqlx::query_as::<_, UsuarioPublico>(
        r#"INSERT INTO usuarios (nombre, email, "contraseña", rol)
           VALUES ($1, $2, $3, $4)
           RETURNING id, nombre, email, rol, email_verificado"#,
    )
    .bind(&body.nombre)
    .bind(body.email.trim())
    .bind(&hash)
    .bind(rol)
    .fetch_one(&state.db)
    .await?;

    // RF-01: la cuenta nace sin verificar. El enlace viaja por correo y es obligatorio confirmarlo
    // antes de reservar/cancelar turnos.
    let token = emitir_token_verificacion(&state, user.id).await?;
    enviar_correo_verificacion(&state, &user.nombre, &user.email, &token).await;

    Ok((
        StatusCode::CREATED,
        Json(
            json!({ "message": "Usuario registrado correctamente. Te enviamos un correo para verificar tu cuenta.", "user": user }),
        ),
    ))
}

/// Token opaco de 256 bits y su hash SHA-256 (lo que se guarda en la base).
fn generar_token() -> (String, String) {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let token = hex::encode(bytes);
    (token.clone(), hash_token(&token))
}

/// Emite un enlace de verificación nuevo e invalida los anteriores sin usar: pedir un reenvío
/// deja un solo enlace válido, con vencimiento de 1 hora.
async fn emitir_token_verificacion(state: &AppState, usuario_id: i32) -> AppResult<String> {
    let (token, token_hash) = generar_token();
    let mut tx = state.db.begin().await?;
    sqlx::query(
        "UPDATE tokens_usuario SET usado_en = now() \
         WHERE usuario_id = $1 AND tipo = 'verificar_email' AND usado_en IS NULL",
    )
    .bind(usuario_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO tokens_usuario (usuario_id, tipo, token_hash, expira_en) \
         VALUES ($1, 'verificar_email', $2, now() + interval '1 hour')",
    )
    .bind(usuario_id)
    .bind(&token_hash)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(token)
}

async fn enviar_correo_verificacion(state: &AppState, nombre: &str, email: &str, token: &str) {
    let enlace = format!(
        "{}/auth/verificar?token={token}",
        state.frontend_url.trim_end_matches('/')
    );
    let cuerpo = crate::mailer::cuerpo_verificacion(nombre, &enlace);
    state
        .mailer
        .enviar_o_loggear(email, "Verificá tu cuenta de TL360", cuerpo)
        .await;
}

/// Mismo criterio que la verificación: al pedir un recupero nuevo, los enlaces anteriores dejan de
/// servir y el nuevo vence en 1 hora.
async fn emitir_token_reset(state: &AppState, usuario_id: i32) -> AppResult<String> {
    let (token, token_hash) = generar_token();
    let mut tx = state.db.begin().await?;
    sqlx::query(
        "UPDATE tokens_usuario SET usado_en = now() \
         WHERE usuario_id = $1 AND tipo = 'reset_password' AND usado_en IS NULL",
    )
    .bind(usuario_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO tokens_usuario (usuario_id, tipo, token_hash, expira_en) \
         VALUES ($1, 'reset_password', $2, now() + interval '1 hour')",
    )
    .bind(usuario_id)
    .bind(&token_hash)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(token)
}

async fn enviar_correo_reset(state: &AppState, nombre: &str, email: &str, token: &str) {
    let enlace = format!(
        "{}/auth/reset?token={token}",
        state.frontend_url.trim_end_matches('/')
    );
    let cuerpo = crate::mailer::cuerpo_reset(nombre, &enlace);
    state
        .mailer
        .enviar_o_loggear(email, "Recuperá tu contraseña de TL360", cuerpo)
        .await;
}

pub async fn login(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    AppJson(body): AppJson<LoginRequest>,
) -> AppResult<Json<Value>> {
    let clave_cuenta = format!("login:{ip}:{}", body.email.trim().to_lowercase());
    let clave_ip = format!("login-ip:{ip}");
    for (clave, max) in [
        (&clave_cuenta, MAX_FALLOS_POR_CUENTA),
        (&clave_ip, MAX_FALLOS_POR_IP),
    ] {
        if let Some(espera) = state.limiter.bloqueado(clave, max, VENTANA_LOGIN) {
            return Err(demasiados(espera));
        }
    }
    let user = sqlx::query_as::<_, UsuarioConHash>(
        r#"SELECT id, nombre, email, "contraseña", rol, email_verificado FROM usuarios WHERE lower(email) = lower($1)"#,
    )
    .bind(&body.email)
    .fetch_optional(&state.db)
    .await?;

    // Mismo mensaje y mismo costo de CPU exista o no el usuario: no se puede averiguar qué correos
    // están registrados ni por la respuesta ni por el tiempo. Además bcrypt es CPU-pesado, así que
    // corre fuera de los hilos async.
    let hash = user
        .as_ref()
        .map(|u| u.contrasena.clone())
        .unwrap_or_else(|| HASH_DESCARTABLE.to_string());
    let clave = body.contrasena.clone();
    let valido = tokio::task::spawn_blocking(move || bcrypt::verify(clave, &hash))
        .await
        .map_err(|_| AppError::BadRequest("No se pudo verificar la contraseña".into()))??;
    let user = match user {
        Some(u) if valido => u,
        _ => {
            state.limiter.registrar(&clave_cuenta);
            state.limiter.registrar(&clave_ip);
            return Err(AppError::AuthFailed(
                "Correo o contraseña incorrectos".into(),
            ));
        }
    };
    state.limiter.limpiar(&clave_cuenta);

    let exp = (Utc::now() + Duration::hours(6)).timestamp() as usize;
    let claims = Claims {
        id: user.id,
        nombre: user.nombre.clone(),
        rol: user.rol.clone(),
        exp,
    };
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(state.jwt_secret.as_bytes()),
    )
    .map_err(|_| AppError::BadRequest("No se pudo generar el token".into()))?;

    Ok(Json(json!({
        "message": "Login exitoso",
        "token": token,
        "user": { "id": user.id, "nombre": user.nombre, "email": user.email, "rol": user.rol, "email_verificado": user.email_verificado }
    })))
}

#[derive(Deserialize)]
pub struct VerificarEmailRequest {
    token: String,
}

/// RF-01: confirma la cuenta con el token de un solo uso (24 h). Dos pedidos simultáneos con
/// el mismo token no pueden consumirlo ambos (UPDATE ... RETURNING atómico, igual que el reset).
pub async fn verificar_email(
    State(state): State<AppState>,
    AppJson(body): AppJson<VerificarEmailRequest>,
) -> AppResult<Json<Value>> {
    let mut tx = state.db.begin().await?;

    let usuario_id = sqlx::query_scalar::<_, i32>(
        "UPDATE tokens_usuario SET usado_en = now() \
         WHERE token_hash = $1 AND tipo = 'verificar_email' AND usado_en IS NULL AND expira_en > now() \
         RETURNING usuario_id",
    )
    .bind(hash_token(&body.token))
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::BadRequest("El enlace de verificación es inválido o venció".into()))?;

    sqlx::query("UPDATE usuarios SET email_verificado = true WHERE id = $1")
        .bind(usuario_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    Ok(Json(
        json!({ "message": "Cuenta verificada correctamente. Ya podés reservar turnos." }),
    ))
}

#[derive(Deserialize)]
pub struct ReenviarVerificacionRequest {
    email: String,
}

/// Reenvía el enlace de verificación. Responde siempre igual (exista o no el correo, esté o no
/// verificado) para no revelar qué cuentas existen, con el mismo rate limit que forgot-password.
pub async fn reenviar_verificacion(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    AppJson(body): AppJson<ReenviarVerificacionRequest>,
) -> AppResult<Json<Value>> {
    for (clave, max) in [
        (format!("reenviar:{ip}"), 10usize),
        (
            format!("reenviar-mail:{}", body.email.trim().to_lowercase()),
            3usize,
        ),
    ] {
        if let Some(espera) = state.limiter.bloqueado(&clave, max, VENTANA_HORA) {
            return Err(demasiados(espera));
        }
        state.limiter.registrar(&clave);
    }
    let pendiente = sqlx::query_as::<_, (i32, String, String)>(
        "SELECT id, nombre, email FROM usuarios WHERE lower(email) = lower($1) AND NOT email_verificado",
    )
    .bind(body.email.trim())
    .fetch_optional(&state.db)
    .await?;

    if let Some((usuario_id, nombre, email)) = pendiente {
        let token = emitir_token_verificacion(&state, usuario_id).await?;
        enviar_correo_verificacion(&state, &nombre, &email, &token).await;
    }

    Ok(Json(
        json!({ "message": "Si el correo está registrado y pendiente de verificación, te enviamos un nuevo enlace." }),
    ))
}

#[derive(Deserialize)]
pub struct ForgotRequest {
    email: String,
}

fn hash_token(token: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// El enlace de recuperación se envía por correo (o se escribe en el log si no hay SMTP); la
/// respuesta es siempre la misma, exista o no el correo, para no revelar qué emails están registrados.
pub async fn forgot_password(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    AppJson(body): AppJson<ForgotRequest>,
) -> AppResult<Json<Value>> {
    // Cada pedido cuenta (exista o no el correo) para no poder inundar de correos a un usuario ni
    // usar el endpoint para averiguar qué cuentas existen.
    for (clave, max) in [
        (format!("forgot:{ip}"), 10usize),
        (
            format!("forgot-mail:{}", body.email.trim().to_lowercase()),
            3usize,
        ),
    ] {
        if let Some(espera) = state.limiter.bloqueado(&clave, max, VENTANA_HORA) {
            return Err(demasiados(espera));
        }
        state.limiter.registrar(&clave);
    }
    let usuario = sqlx::query_as::<_, (i32, String, String)>(
        "SELECT id, nombre, email FROM usuarios WHERE lower(email) = lower($1)",
    )
    .bind(body.email.trim())
    .fetch_optional(&state.db)
    .await?;

    if let Some((usuario_id, nombre, email)) = usuario {
        let token = emitir_token_reset(&state, usuario_id).await?;
        enviar_correo_reset(&state, &nombre, &email, &token).await;
    }

    Ok(Json(
        json!({ "message": "Si el correo existe, te enviamos instrucciones para recuperar tu contraseña." }),
    ))
}

#[derive(Deserialize)]
pub struct ResetRequest {
    token: String,
    #[serde(rename = "contraseña")]
    contrasena: String,
}

pub async fn reset_password(
    State(state): State<AppState>,
    AppJson(body): AppJson<ResetRequest>,
) -> AppResult<Json<Value>> {
    password_valida(&body.contrasena)?;
    let mut tx = state.db.begin().await?;

    // El UPDATE ... RETURNING marca el token como usado y lo devuelve en un solo paso: dos pedidos
    // simultáneos con el mismo token no pueden consumirlo ambos.
    let usuario_id = sqlx::query_scalar::<_, i32>(
        "UPDATE tokens_usuario SET usado_en = now() \
         WHERE token_hash = $1 AND tipo = 'reset_password' AND usado_en IS NULL AND expira_en > now() \
         RETURNING usuario_id",
    )
    .bind(hash_token(&body.token))
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::BadRequest("El enlace de recuperación es inválido o venció".into()))?;

    let hash = bcrypt::hash(&body.contrasena, bcrypt::DEFAULT_COST)?;
    sqlx::query(r#"UPDATE usuarios SET "contraseña" = $1 WHERE id = $2"#)
        .bind(hash)
        .bind(usuario_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    Ok(Json(
        json!({ "message": "Contraseña actualizada correctamente" }),
    ))
}
