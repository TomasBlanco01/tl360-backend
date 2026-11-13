use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;

use crate::auth_extractor::{require_role, CurrentUser};
use crate::errors::{AppError, AppJson, AppResult};
use crate::state::AppState;
use crate::validation::{email_valido, nombre_valido, password_valida};

#[derive(FromRow, Serialize)]
pub struct UsuarioPublico {
    id: i32,
    nombre: String,
    email: String,
    rol: String,
    /// El superadmin ve quién no confirmó el correo (y puede confirmarlo él).
    email_verificado: bool,
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
        "SELECT recordatorio_horas FROM usuario WHERE id = $1",
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
    sqlx::query("UPDATE usuario SET recordatorio_horas = $1 WHERE id = $2")
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
        r#"SELECT email, contrasena_hash FROM usuario WHERE id = $1"#,
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
        sqlx::query(r#"UPDATE usuario SET contrasena_hash = $1 WHERE id = $2"#)
            .bind(hash)
            .bind(user.id)
            .execute(&mut *tx)
            .await?;
    }

    let mut correo_nuevo = None;
    if let Some(nombre) = &body.nombre {
        nombre_valido(nombre)?;
        sqlx::query("UPDATE usuario SET nombre = $1 WHERE id = $2")
            .bind(nombre.trim())
            .bind(user.id)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(email) = &body.email {
        email_valido(email)?;
        let email = email.trim();
        if !email.eq_ignore_ascii_case(&email_actual) {
            sqlx::query("UPDATE usuario SET email = $1, email_verificado = false WHERE id = $2")
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
        "SELECT id, nombre, email, rol, email_verificado FROM usuario WHERE id = $1",
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

#[derive(Deserialize)]
pub struct UsuariosQuery {
    /// Busca por nombre o correo (sin distinguir mayúsculas ni tildes).
    q: Option<String>,
    rol: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
    /// Ids separados por coma: el panel muestra el nombre del dueño de cada comercio listado.
    ids: Option<String>,
}

/// Listado del superadmin, paginado y con buscador: la tabla crece con cada registro. Sin `limit`
/// devuelve hasta 1000 (compatibilidad); el panel pide de a una página.
pub async fn get_usuarios(
    user: CurrentUser,
    State(state): State<AppState>,
    Query(f): Query<UsuariosQuery>,
) -> AppResult<Json<Vec<UsuarioPublico>>> {
    require_role(&user, &["superadmin"])?;
    let q = f
        .q
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase);
    let rol = f.rol.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let ids = match f.ids.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        None => None,
        Some(raw) => Some(
            raw.split(',')
                .map(|s| s.trim().parse::<i32>())
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| AppError::BadRequest("ids debe ser una lista de números".into()))?,
        ),
    };
    let rows = sqlx::query_as::<_, UsuarioPublico>(
        "SELECT id, nombre, email, rol, email_verificado FROM usuario \
         WHERE ($1::text IS NULL \
                OR translate(lower(nombre), 'áéíóúüñ', 'aeiouun') LIKE '%' || translate($1, 'áéíóúüñ', 'aeiouun') || '%' \
                OR lower(email) LIKE '%' || $1 || '%') \
           AND ($2::text IS NULL OR rol = $2) \
           AND ($5::int4[] IS NULL OR id = ANY($5)) \
         ORDER BY nombre, id LIMIT $3 OFFSET $4",
    )
    .bind(q)
    .bind(rol)
    .bind(f.limit.map_or(1000, |l| l.clamp(1, 1000)))
    .bind(f.offset.unwrap_or(0).max(0))
    .bind(ids)
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
    user: CurrentUser,
    State(state): State<AppState>,
    AppJson(body): AppJson<CrearUsuarioRequest>,
) -> AppResult<(StatusCode, Json<Value>)> {
    // Primero los permisos (el rol vigente sale de la base, ver CurrentUser) y después los datos.
    require_role(&user, &["superadmin"])?;
    nombre_valido(&body.nombre)?;
    email_valido(&body.email)?;
    password_valida(&body.contrasena)?;
    let rol = body.rol.as_deref().unwrap_or("cliente");
    if !matches!(rol, "cliente" | "admin" | "superadmin") {
        return Err(AppError::BadRequest("Rol inválido".into()));
    }

    let existe: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM usuario WHERE lower(email) = lower($1))")
            .bind(body.email.trim())
            .fetch_one(&state.db)
            .await?;
    if existe {
        return Err(AppError::Conflict("Ese correo ya está registrado".into()));
    }

    let hash = bcrypt::hash(&body.contrasena, bcrypt::DEFAULT_COST)?;
    // Cuentas creadas por un superadmin: el superadmin avala la cuenta y nace verificada.
    // Solo el registro público crea cuentas pendientes de verificación (RF-01).
    sqlx::query(r#"INSERT INTO usuario (nombre, email, contrasena_hash, rol, email_verificado) VALUES ($1, $2, $3, $4, true)"#)
        .bind(body.nombre.trim())
        .bind(body.email.trim())
        .bind(&hash)
        .bind(rol)
        .execute(&state.db)
        .await?;

    Ok((
        StatusCode::CREATED,
        Json(json!({ "mensaje": "Usuario creado" })),
    ))
}

#[derive(Deserialize)]
pub struct GestionUsuarioRequest {
    rol: Option<String>,
    email_verificado: Option<bool>,
}

/// Gestión de cuentas por el superadmin: cambiar el rol (p. ej. un cliente que pasa a ser dueño
/// de comercio) y confirmar el correo de alguien que no recibió el enlace. No puede quitarse a sí
/// mismo el rol de superadmin (la plataforma quedaría sin administrador) ni dejar sin rol de
/// comercio a un dueño que todavía tiene comercios a su nombre.
pub async fn gestionar_usuario(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i32>,
    AppJson(body): AppJson<GestionUsuarioRequest>,
) -> AppResult<Json<UsuarioPublico>> {
    require_role(&user, &["superadmin"])?;
    let actual = sqlx::query_as::<_, UsuarioPublico>(
        "SELECT id, nombre, email, rol, email_verificado FROM usuario WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound("Usuario no encontrado".into()))?;

    let rol = body.rol.as_deref().map(str::trim).filter(|r| !r.is_empty());
    if let Some(rol) = rol {
        if !matches!(rol, "cliente" | "admin" | "superadmin") {
            return Err(AppError::BadRequest("Rol inválido".into()));
        }
        if id == user.id && rol != "superadmin" {
            return Err(AppError::BadRequest(
                "No podés quitarte a vos mismo el rol de superadmin".into(),
            ));
        }
        if actual.rol == "admin" && rol != "admin" {
            let comercios: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM empresa WHERE usuario_id = $1")
                    .bind(id)
                    .fetch_one(&state.db)
                    .await?;
            if comercios > 0 {
                return Err(AppError::Conflict(format!(
                    "Tiene {comercios} comercio(s) a su nombre: reasignalos a otro dueño antes de cambiarle el rol"
                )));
            }
        }
    }
    if body.email_verificado == Some(false) {
        return Err(AppError::BadRequest(
            "Solo se puede confirmar un correo, no desverificarlo".into(),
        ));
    }

    let actualizado = sqlx::query_as::<_, UsuarioPublico>(
        "UPDATE usuario SET rol = COALESCE($2, rol), \
                email_verificado = email_verificado OR COALESCE($3, false) \
         WHERE id = $1 RETURNING id, nombre, email, rol, email_verificado",
    )
    .bind(id)
    .bind(rol)
    .bind(body.email_verificado)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(actualizado))
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
