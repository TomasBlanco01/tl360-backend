use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;

use crate::auth_extractor::{decode_token, require_role, CurrentUser};
use crate::errors::{AppError, AppJson, AppResult};
use crate::state::AppState;
use crate::validation::{email_valido, nombre_valido, password_valida};

#[derive(FromRow, Serialize)]
pub struct UsuarioPublico {
    id: i32,
    nombre: String,
    email: String,
    rol: String,
}

const HORAS_RECORDATORIO_MIN: i32 = 1;
const HORAS_RECORDATORIO_MAX: i32 = 168;

fn validar_horas_recordatorio(horas: i32) -> AppResult<()> {
    if (HORAS_RECORDATORIO_MIN..=HORAS_RECORDATORIO_MAX).contains(&horas) {
        Ok(())
    } else {
        Err(AppError::BadRequest(format!(
            "La anticipación del recordatorio debe ser entre {HORAS_RECORDATORIO_MIN} y {HORAS_RECORDATORIO_MAX} horas"
        )))
    }
}

#[derive(Deserialize)]
pub struct RecordatorioRequest {
    horas: i32,
}

/// RF-03: preferencia del cliente para el recordatorio push de sus turnos.
pub async fn get_recordatorio(
    user: CurrentUser,
    State(state): State<AppState>,
) -> AppResult<Json<Value>> {
    let horas = sqlx::query_scalar::<_, i32>(
        "SELECT recordatorio_horas_anticipacion FROM usuarios WHERE id = $1",
    )
    .bind(user.id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound("Usuario no encontrado".into()))?;
    Ok(Json(json!({ "horas": horas })))
}

pub async fn set_recordatorio(
    user: CurrentUser,
    State(state): State<AppState>,
    AppJson(body): AppJson<RecordatorioRequest>,
) -> AppResult<Json<Value>> {
    validar_horas_recordatorio(body.horas)?;
    sqlx::query("UPDATE usuarios SET recordatorio_horas_anticipacion = $1 WHERE id = $2")
        .bind(body.horas)
        .bind(user.id)
        .execute(&state.db)
        .await?;
    Ok(Json(json!({ "horas": body.horas })))
}

#[derive(Deserialize)]
pub struct PerfilRequest {
    nombre: Option<String>,
    email: Option<String>,
    password_actual: Option<String>,
    password_nueva: Option<String>,
}

#[derive(FromRow, Serialize)]
pub struct Perfil {
    id: i32,
    nombre: String,
    email: String,
    rol: String,
    email_verificado: bool,
}

/// Mis datos (clientes y dueños): cambia nombre y correo, o la contraseña con la actual.
/// Un correo nuevo vuelve a exigir verificación (RF-01) y se le manda el enlace.
pub async fn actualizar_perfil(
    user: CurrentUser,
    State(state): State<AppState>,
    AppJson(body): AppJson<PerfilRequest>,
) -> AppResult<Json<Perfil>> {
    let cambia_clave = body.password_actual.is_some() || body.password_nueva.is_some();
    let cambia_datos = body.nombre.is_some() || body.email.is_some();
    if !cambia_clave && !cambia_datos {
        return Err(AppError::BadRequest("No hay datos para actualizar".into()));
    }
    let (email_actual, hash_actual) = sqlx::query_as::<_, (String, String)>(
        r#"SELECT email, "contraseña" FROM usuarios WHERE id = $1"#,
    )
    .bind(user.id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound("Usuario no encontrado".into()))?;

    let mut tx = state.db.begin().await?;
    if cambia_clave {
        let (Some(actual), Some(nueva)) = (&body.password_actual, &body.password_nueva) else {
            return Err(AppError::BadRequest(
                "Para cambiar la contraseña indicá la actual y la nueva".into(),
            ));
        };
        if !bcrypt::verify(actual, &hash_actual)? {
            return Err(AppError::BadRequest(
                "La contraseña actual no es correcta".into(),
            ));
        }
        password_valida(nueva)?;
        let hash = bcrypt::hash(nueva, bcrypt::DEFAULT_COST)?;
        sqlx::query(r#"UPDATE usuarios SET "contraseña" = $1 WHERE id = $2"#)
            .bind(hash)
            .bind(user.id)
            .execute(&mut *tx)
            .await?;
    }

    let mut correo_nuevo = None;
    if let Some(nombre) = &body.nombre {
        nombre_valido(nombre)?;
        sqlx::query("UPDATE usuarios SET nombre = $1 WHERE id = $2")
            .bind(nombre.trim())
            .bind(user.id)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(email) = &body.email {
        email_valido(email)?;
        let email = email.trim();
        if !email.eq_ignore_ascii_case(&email_actual) {
            sqlx::query("UPDATE usuarios SET email = $1, email_verificado = false WHERE id = $2")
                .bind(email)
                .bind(user.id)
                .execute(&mut *tx)
                .await
                .map_err(|e| match &e {
                    sqlx::Error::Database(d) if d.code().as_deref() == Some("23505") => {
                        AppError::Conflict("Ya existe una cuenta con ese correo".into())
                    }
                    _ => AppError::Db(e),
                })?;
            correo_nuevo = Some(email.to_string());
        }
    }
    tx.commit().await?;

    let perfil = sqlx::query_as::<_, Perfil>(
        "SELECT id, nombre, email, rol, email_verificado FROM usuarios WHERE id = $1",
    )
    .bind(user.id)
    .fetch_one(&state.db)
    .await?;
    if let Some(email) = correo_nuevo {
        let token = crate::handlers::auth::emitir_token_verificacion(&state, user.id).await?;
        crate::handlers::auth::enviar_correo_verificacion(&state, &perfil.nombre, &email, &token)
            .await;
    }
    Ok(Json(perfil))
}

pub async fn get_usuarios(
    user: CurrentUser,
    State(state): State<AppState>,
) -> AppResult<Json<Vec<UsuarioPublico>>> {
    require_role(&user, &["superadmin"])?;
    let rows = sqlx::query_as::<_, UsuarioPublico>("SELECT id, nombre, email, rol FROM usuarios")
        .fetch_all(&state.db)
        .await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
pub struct CrearUsuarioRequest {
    nombre: String,
    email: String,
    #[serde(rename = "contraseña")]
    contrasena: String,
    rol: Option<String>,
}

/// RF-01: el alta administrativa de cuentas (incluidos clientes) exige superadmin. El registro
/// público es el único camino abierto y siempre nace sin verificar; esta ruta no debe servir
/// para crear cuentas ya verificadas sin control.
pub async fn crear_usuario(
    State(state): State<AppState>,
    headers: HeaderMap,
    AppJson(body): AppJson<CrearUsuarioRequest>,
) -> AppResult<(StatusCode, Json<Value>)> {
    nombre_valido(&body.nombre)?;
    email_valido(&body.email)?;
    password_valida(&body.contrasena)?;
    let rol = body.rol.as_deref().unwrap_or("cliente");
    if !matches!(rol, "cliente" | "admin" | "superadmin") {
        return Err(AppError::BadRequest("Rol inválido".into()));
    }
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let es_superadmin = token
        .and_then(|t| decode_token(t, &state.jwt_secret).ok())
        .map(|c| c.rol == "superadmin")
        .unwrap_or(false);
    if !es_superadmin {
        return Err(AppError::Forbidden(
            "Solo un superadmin puede crear usuarios".into(),
        ));
    }

    let hash = bcrypt::hash(&body.contrasena, bcrypt::DEFAULT_COST)?;
    // Cuentas creadas por un superadmin: el superadmin avala la cuenta y nace verificada.
    // Solo el registro público crea cuentas pendientes de verificación (RF-01).
    sqlx::query(r#"INSERT INTO usuarios (nombre, email, "contraseña", rol, email_verificado) VALUES ($1, $2, $3, $4, true)"#)
        .bind(&body.nombre)
        .bind(&body.email)
        .bind(&hash)
        .bind(rol)
        .execute(&state.db)
        .await?;

    Ok((
        StatusCode::CREATED,
        Json(json!({ "mensaje": "Usuario creado" })),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valida_el_rango_de_anticipacion_del_recordatorio() {
        assert!(validar_horas_recordatorio(1).is_ok());
        assert!(validar_horas_recordatorio(24).is_ok());
        assert!(validar_horas_recordatorio(168).is_ok());
        assert!(validar_horas_recordatorio(0).is_err());
        assert!(validar_horas_recordatorio(169).is_err());
    }
}
