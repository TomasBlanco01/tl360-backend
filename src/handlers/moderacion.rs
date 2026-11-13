//! RF-15 (sello de verificado) y RF-16 (reportes de comercios). Solo el superadmin modera.
use axum::{
    extract::{Path, Query, State},
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;

use crate::auth_extractor::CurrentUser;
use crate::errors::{AppError, AppJson, AppResult};
use crate::handlers::empresas::{normalizar_cuit, verificar_dueno};
use crate::state::AppState;
use crate::validation::url_http_opt;

pub(crate) fn exigir_superadmin(user: &CurrentUser) -> AppResult<()> {
    if user.rol != "superadmin" {
        return Err(AppError::sin_permisos());
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct VerificadoRequest {
    verificado: bool,
}

/// Otorga o retira el sello de verificado (teléfono validado + constancia de CUIT/habilitación).
/// Sincroniza `verificado_estado` para que ambos campos cuenten siempre la misma historia.
pub async fn set_verificado(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i32>,
    AppJson(body): AppJson<VerificadoRequest>,
) -> AppResult<Json<Value>> {
    exigir_superadmin(&user)?;
    // RF-15: el sello solo se otorga aprobando una solicitud con constancia. Este endpoint
    // queda para retirarlo (por ejemplo, ante un reporte válido).
    if body.verificado {
        let empresa = sqlx::query_scalar::<_, String>(
            "SELECT COALESCE(verificado_estado, 'sin_solicitar') FROM empresa WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound("Empresa no encontrada".into()))?;
        return Err(AppError::Conflict(format!(
            "El sello se otorga con 'aprobar' desde la verificación (estado actual: {empresa})"
        )));
    }
    let (verificado, estado) = sqlx::query_as::<_, (bool, String)>(
        "UPDATE empresa SET verificado = false, \
         verificado_estado = CASE WHEN verificado_estado = 'verificado' THEN 'sin_solicitar' ELSE verificado_estado END \
         WHERE id = $1 RETURNING verificado, verificado_estado",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound("Empresa no encontrada".into()))?;
    Ok(Json(
        json!({ "verificado": verificado, "verificado_estado": estado }),
    ))
}

#[derive(Deserialize)]
pub struct VerificacionRequest {
    accion: String,
    cuit: Option<String>,
    url_cuit: Option<String>,
    motivo: Option<String>,
}

/// RF-15: rechazar exige explicar el motivo (5 a 500 caracteres), igual que la base.
fn motivo_de_rechazo(motivo: &Option<String>) -> AppResult<String> {
    let motivo = motivo.as_deref().map(str::trim).unwrap_or_default();
    let largo = motivo.chars().count();
    if !(5..=500).contains(&largo) {
        return Err(AppError::BadRequest(
            "Indicá el motivo del rechazo (entre 5 y 500 caracteres)".into(),
        ));
    }
    Ok(motivo.to_string())
}

/// RF-15: el dueño (o un superadmin) **solicita** con CUIT + constancia y queda `pendiente`;
/// solo el superadmin **aprueba** (enciende el sello) o **rechaza** (lo apaga) con un motivo.
pub async fn set_verificacion(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i32>,
    AppJson(body): AppJson<VerificacionRequest>,
) -> AppResult<Json<Value>> {
    let (verificado, estado) = match body.accion.as_str() {
        "solicitar" => {
            verificar_dueno(&state, &user, id).await?;
            let cuit = normalizar_cuit(&body.cuit)?.ok_or_else(|| {
                AppError::BadRequest("El CUIT es obligatorio para solicitar la verificación".into())
            })?;
            url_http_opt("url_cuit", &body.url_cuit)?;
            let actual = sqlx::query_scalar::<_, Option<String>>(
                "SELECT url_cuit FROM empresa WHERE id = $1",
            )
            .bind(id)
            .fetch_optional(&state.db)
            .await?
            .ok_or_else(|| AppError::NotFound("Empresa no encontrada".into()))?;
            // RF-15: sin constancia no hay nada que el admin pueda validar. Se puede reutilizar
            // la ya presentada al reenviar una solicitud.
            let constancia = body
                .url_cuit
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_string)
                .or(actual)
                .ok_or_else(|| {
                    AppError::BadRequest(
                        "La constancia de CUIT o habilitación es obligatoria".into(),
                    )
                })?;
            // Una nueva solicitud vuelve a revisión: el sello se apaga hasta que se apruebe
            // (la base exige verificado = (verificado_estado = 'verificado')).
            let resultado = sqlx::query_as::<_, (bool, String)>(
                "UPDATE empresa SET cuit = $2, url_cuit = $3, \
                 verificado_estado = 'pendiente', verificado = false, motivo_rechazo = NULL \
                 WHERE id = $1 RETURNING verificado, verificado_estado",
            )
            .bind(id)
            .bind(&cuit)
            .bind(&constancia)
            .fetch_optional(&state.db)
            .await?
            .ok_or_else(|| AppError::NotFound("Empresa no encontrada".into()))?;
            avisar_solicitud_verificacion(&state, id).await;
            resultado
        }
        "aprobar" | "rechazar" => {
            exigir_superadmin(&user)?;
            let aprobar = body.accion == "aprobar";
            let motivo = if aprobar {
                None
            } else {
                Some(motivo_de_rechazo(&body.motivo)?)
            };
            // RF-15: solo se resuelve una solicitud pendiente y, para aprobar, debe existir
            // la constancia que el superadmin valida.
            let resultado = sqlx::query_as::<_, (bool, String)>(
                "UPDATE empresa SET verificado = $2, verificado_estado = $3, motivo_rechazo = $4 \
                 WHERE id = $1 AND verificado_estado = 'pendiente' \
                 AND (NOT $2 OR url_cuit IS NOT NULL) \
                 RETURNING verificado, verificado_estado",
            )
            .bind(id)
            .bind(aprobar)
            .bind(if aprobar { "verificado" } else { "rechazado" })
            .bind(motivo.as_deref())
            .fetch_optional(&state.db)
            .await?;
            let (verificado, estado) = match resultado {
                Some(fila) => fila,
                None => {
                    let existe = sqlx::query_scalar::<_, bool>(
                        "SELECT EXISTS (SELECT 1 FROM empresa WHERE id = $1)",
                    )
                    .bind(id)
                    .fetch_one(&state.db)
                    .await?;
                    if !existe {
                        return Err(AppError::NotFound("Empresa no encontrada".into()));
                    }
                    return Err(AppError::Conflict(
                        "No hay una solicitud pendiente con constancia para resolver".into(),
                    ));
                }
            };
            avisar_resolucion(&state, id, motivo.as_deref()).await;
            (verificado, estado)
        }
        otra => {
            return Err(AppError::BadRequest(format!(
                "Acción inválida '{otra}': se espera 'solicitar', 'aprobar' o 'rechazar'"
            )))
        }
    };
    Ok(Json(
        json!({ "verificado": verificado, "verificado_estado": estado }),
    ))
}

/// RF-15: avisa al dueño la resolución de la solicitud (con el motivo si se rechazó). Si el correo
/// falla, la moderación ya quedó guardada igual.
async fn avisar_resolucion(state: &AppState, empresa_id: i32, motivo_rechazo: Option<&str>) {
    let aprobado = motivo_rechazo.is_none();
    let datos = sqlx::query_as::<_, (String, String, String)>(
        "SELECT u.email, u.nombre, e.nombre \
         FROM empresa e JOIN usuario u ON u.id = e.usuario_id WHERE e.id = $1",
    )
    .bind(empresa_id)
    .fetch_optional(&state.db)
    .await;
    match datos {
        Ok(Some((email, nombre, comercio))) => {
            let cuerpo =
                crate::mailer::cuerpo_verificacion_comercio(&nombre, &comercio, motivo_rechazo);
            let asunto = if aprobado {
                "Tu comercio fue verificado en TL360"
            } else {
                "Solicitud de verificación rechazada"
            };
            state.mailer.enviar_o_loggear(&email, asunto, cuerpo).await;
        }
        Ok(None) => {}
        Err(e) => tracing::warn!("no se pudo preparar el aviso de verificación: {e}"),
    }
}

/// RF-15: avisa a los superadmins que entró una solicitud de verificación.
async fn avisar_solicitud_verificacion(state: &AppState, empresa_id: i32) {
    let nombre = sqlx::query_scalar::<_, String>("SELECT nombre FROM empresa WHERE id = $1")
        .bind(empresa_id)
        .fetch_optional(&state.db)
        .await;
    if let Ok(Some(nombre)) = nombre {
        avisar_superadmins(
            state,
            "Nueva solicitud de verificación",
            &format!("{nombre} pidió el sello de verificado"),
            "/admin",
        )
        .await;
    }
}

async fn avisar_superadmins(state: &AppState, titulo: &str, cuerpo: &str, url: &str) {
    state
        .push
        .enviar_a_rol(
            &state.db,
            "superadmin",
            &crate::push::Notificacion {
                titulo,
                cuerpo,
                url,
            },
        )
        .await;
}

#[derive(Deserialize)]
pub struct ReporteRequest {
    descripcion: String,
}
/// Cualquier usuario con sesión puede reportar una ficha (datos falsos, cerrada, ofensiva, etc.).
pub async fn crear_reporte(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i32>,
    AppJson(body): AppJson<ReporteRequest>,
) -> AppResult<Json<Value>> {
    let descripcion = body.descripcion.trim();
    let largo = descripcion.chars().count();
    if !(5..=500).contains(&largo) {
        return Err(AppError::BadRequest(
            "La descripción debe tener entre 5 y 500 caracteres".into(),
        ));
    }
    let empresa = sqlx::query_scalar::<_, String>("SELECT nombre FROM empresa WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound("Empresa no encontrada".into()))?;
    // Evita que una misma persona llene la bandeja con el mismo comercio.
    let pendiente: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM reporte WHERE empresa_id = $1 AND usuario_id = $2 AND NOT resuelto)",
    )
    .bind(id)
    .bind(user.id)
    .fetch_one(&state.db)
    .await?;
    if pendiente {
        return Err(AppError::Conflict(
            "Ya reportaste este comercio y el reporte está en revisión".into(),
        ));
    }
    // reporte_pendiente_uq cubre la carrera entre dos envíos simultáneos que pasaron la consulta.
    sqlx::query("INSERT INTO reporte (empresa_id, usuario_id, descripcion) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(user.id)
        .bind(descripcion)
        .execute(&state.db)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(d) if d.code().as_deref() == Some("23505") => AppError::Conflict(
                "Ya reportaste este comercio y el reporte está en revisión".into(),
            ),
            _ => AppError::Db(e),
        })?;
    avisar_superadmins(
        &state,
        "Nuevo reporte de comercio",
        &format!("{empresa}: {descripcion}"),
        "/admin",
    )
    .await;
    Ok(Json(
        json!({ "message": "Reporte enviado. Gracias por avisarnos." }),
    ))
}

#[derive(FromRow, Serialize)]
pub struct ReporteAdmin {
    id: i32,
    empresa_id: i32,
    empresa_nombre: String,
    usuario_nombre: Option<String>,
    descripcion: String,
    resuelto: bool,
    creado_en: DateTime<Utc>,
}

#[derive(Deserialize)]
pub struct FiltroReportes {
    /// `true` para ver también los ya resueltos; por defecto solo los pendientes.
    resueltos: Option<bool>,
}

pub async fn listar_reportes(
    user: CurrentUser,
    State(state): State<AppState>,
    Query(filtro): Query<FiltroReportes>,
) -> AppResult<Json<Vec<ReporteAdmin>>> {
    exigir_superadmin(&user)?;
    let rows = sqlx::query_as::<_, ReporteAdmin>(
        "SELECT r.id, r.empresa_id, e.nombre AS empresa_nombre, u.nombre AS usuario_nombre, r.descripcion, r.resuelto, r.creado_en \
         FROM reporte r JOIN empresa e ON e.id = r.empresa_id LEFT JOIN usuario u ON u.id = r.usuario_id \
         WHERE ($1 OR NOT r.resuelto) ORDER BY r.resuelto, r.creado_en DESC LIMIT 200",
    )
    .bind(filtro.resueltos.unwrap_or(false))
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

pub async fn resolver_reporte(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> AppResult<Json<Value>> {
    exigir_superadmin(&user)?;
    let filas = sqlx::query("UPDATE reporte SET resuelto = true WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await?
        .rows_affected();
    if filas == 0 {
        return Err(AppError::NotFound("Reporte no encontrado".into()));
    }
    Ok(Json(json!({ "message": "Reporte resuelto" })))
}
