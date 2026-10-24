//! RF-14: resumen semanal por push al comercio. Cada lunes desde las 09:00 (hora local) se
//! envía al dueño un resumen de los turnos de esa semana. La tabla `resumenes_semanales` evita
//! repetir el aviso si el proceso vuelve a evaluar la misma semana. Solo corre con push activo.

use std::time::Duration;

use chrono::{Datelike, NaiveDate, NaiveDateTime, NaiveTime, Weekday};
use sqlx::{FromRow, PgPool};

use crate::agenda::ahora_local;
use crate::push::Notificacion;
use crate::state::AppState;

const HORA_ENVIO: (u32, u32) = (9, 0);

#[derive(FromRow)]
pub struct ResumenSemanal {
    pub empresa_id: i32,
    pub usuario_id: i32,
    pub empresa_nombre: String,
    pub confirmados: i64,
    pub cancelados: i64,
    pub proximo: Option<NaiveDateTime>,
}

pub fn bucle(state: AppState) {
    tokio::spawn(async move {
        let mut intervalo = tokio::time::interval(Duration::from_secs(15 * 60));
        loop {
            intervalo.tick().await;
            if !toca_enviar(ahora_local()) {
                continue;
            }
            let enviados = enviar_pendientes(&state).await;
            if enviados > 0 {
                tracing::info!("resúmenes semanales push enviados: {enviados}");
            }
        }
    });
}

async fn enviar_pendientes(state: &AppState) -> usize {
    let ahora = ahora_local();
    let inicio = inicio_semana(ahora.date());
    let fin = inicio + chrono::Duration::days(7);
    let resumenes = match pendientes(&state.db, inicio, fin, ahora).await {
        Ok(resumenes) => resumenes,
        Err(e) => {
            tracing::warn!("no se pudieron buscar resúmenes semanales: {e}");
            return 0;
        }
    };
    let mut enviados = 0;
    for resumen in resumenes {
        let titulo = format!("Resumen semanal: {}", resumen.empresa_nombre);
        let cuerpo = cuerpo_resumen(resumen.confirmados, resumen.cancelados, resumen.proximo);
        let notificacion = Notificacion {
            titulo: &titulo,
            cuerpo: &cuerpo,
            url: "/negocio",
        };
        let entregados = state
            .push
            .enviar_a_usuario(&state.db, resumen.usuario_id, &notificacion)
            .await;
        // RF-14: si ningún dispositivo recibió el push, la semana queda pendiente para reintentar.
        if entregados == 0 {
            tracing::warn!(
                "resumen semanal del comercio {} no entregado; queda pendiente",
                resumen.empresa_id
            );
            continue;
        }
        if let Err(e) = marcar_enviado(&state.db, resumen.empresa_id, inicio).await {
            tracing::warn!(
                "no se pudo marcar el resumen semanal del comercio {}: {e}",
                resumen.empresa_id
            );
        }
        enviados += 1;
    }
    enviados
}

pub fn inicio_semana(fecha: NaiveDate) -> NaiveDate {
    fecha - chrono::Duration::days(fecha.weekday().num_days_from_monday() as i64)
}

pub fn toca_enviar(ahora: NaiveDateTime) -> bool {
    ahora.weekday() == Weekday::Mon
        && ahora.time() >= NaiveTime::from_hms_opt(HORA_ENVIO.0, HORA_ENVIO.1, 0).unwrap()
}

fn plural(n: i64) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

fn cuerpo_resumen(confirmados: i64, cancelados: i64, proximo: Option<NaiveDateTime>) -> String {
    let mut texto = match (confirmados, cancelados) {
        (0, 0) => "Esta semana no tenés turnos reservados.".to_string(),
        (0, cancelados) => format!(
            "Esta semana no tenés turnos confirmados y {cancelados} cancelado{}.",
            plural(cancelados)
        ),
        (confirmados, 0) => format!(
            "Tenés {confirmados} turno{} confirmado{} esta semana.",
            plural(confirmados),
            plural(confirmados)
        ),
        (confirmados, cancelados) => format!(
            "Tenés {confirmados} turno{} confirmado{} y {cancelados} cancelado{} esta semana.",
            plural(confirmados),
            plural(confirmados),
            plural(cancelados)
        ),
    };
    if let Some(proximo) = proximo {
        texto.push_str(&format!(
            " Próximo: {}.",
            proximo.format("%d/%m a las %H:%M")
        ));
    }
    texto
}

/// Comercios con push suscripto, con el resumen de la semana indicada. Incluye los que no
/// tienen turnos para avisar una sola vez que la semana está libre.
pub async fn pendientes(
    db: &PgPool,
    inicio: NaiveDate,
    fin: NaiveDate,
    ahora: NaiveDateTime,
) -> Result<Vec<ResumenSemanal>, sqlx::Error> {
    sqlx::query_as::<_, ResumenSemanal>(
        "SELECT e.id AS empresa_id, e.admin_id AS usuario_id, e.nombre AS empresa_nombre, \
           COUNT(t.id) FILTER (WHERE t.estado IN ('confirmado', 'completado')) AS confirmados, \
           COUNT(t.id) FILTER (WHERE t.estado = 'cancelado') AS cancelados, \
           MIN(t.fecha + t.hora) FILTER (WHERE t.estado = 'confirmado' AND t.fecha + t.hora >= $2) AS proximo \
         FROM empresas e \
         LEFT JOIN turnos t ON t.empresa_id = e.id AND t.fecha >= $1 AND t.fecha < $3 \
           AND t.estado IN ('confirmado', 'completado', 'cancelado') \
         WHERE e.admin_id IS NOT NULL \
           AND EXISTS (SELECT 1 FROM push_suscripciones p WHERE p.usuario_id = e.admin_id) \
           AND NOT EXISTS (SELECT 1 FROM resumenes_semanales r \
                           WHERE r.empresa_id = e.id AND r.semana_inicio = $1) \
         GROUP BY e.id, e.admin_id, e.nombre \
         ORDER BY e.id",
    )
    .bind(inicio)
    .bind(ahora)
    .bind(fin)
    .fetch_all(db)
    .await
}

pub async fn marcar_enviado(
    db: &PgPool,
    empresa_id: i32,
    semana_inicio: NaiveDate,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO resumenes_semanales (empresa_id, semana_inicio) VALUES ($1, $2) \
         ON CONFLICT (empresa_id, semana_inicio) DO NOTHING",
    )
    .bind(empresa_id)
    .bind(semana_inicio)
    .execute(db)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, dia: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, dia).unwrap()
    }

    fn t(h: u32, m: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, m, 0).unwrap()
    }

    #[test]
    fn la_semana_empieza_el_lunes() {
        assert_eq!(inicio_semana(d(2026, 10, 19)), d(2026, 10, 19));
        assert_eq!(inicio_semana(d(2026, 10, 25)), d(2026, 10, 19));
    }

    #[test]
    fn envia_los_lunes_desde_las_nueve() {
        assert!(!toca_enviar(d(2026, 10, 19).and_time(t(8, 59))));
        assert!(toca_enviar(d(2026, 10, 19).and_time(t(9, 0))));
        assert!(!toca_enviar(d(2026, 10, 20).and_time(t(9, 0))));
    }

    #[test]
    fn arma_el_resumen_con_plurales_y_proximo_turno() {
        let proximo = d(2026, 10, 20).and_time(t(10, 30));
        assert_eq!(
            cuerpo_resumen(2, 1, Some(proximo)),
            "Tenés 2 turnos confirmados y 1 cancelado esta semana. Próximo: 20/10 a las 10:30."
        );
        assert_eq!(
            cuerpo_resumen(1, 0, None),
            "Tenés 1 turno confirmado esta semana."
        );
        assert_eq!(
            cuerpo_resumen(0, 0, None),
            "Esta semana no tenés turnos reservados."
        );
    }
}
