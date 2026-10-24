//! Reglas de agenda de los turnos, como funciones puras (sin base de datos) para poder probarlas.
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, Timelike, Utc, Weekday};

/// Paso de la grilla cuando el comercio no definió `duracion_turno_min` (el front usa el mismo).
pub const PASO_POR_DEFECTO_MIN: i32 = 30;

/// Hora local de Argentina (UTC-3 todo el año).
pub fn ahora_local() -> NaiveDateTime {
    (Utc::now() - Duration::hours(3)).naive_utc()
}

pub fn nombre_dia(fecha: NaiveDate) -> &'static str {
    match fecha.weekday() {
        Weekday::Mon => "Lunes",
        Weekday::Tue => "Martes",
        Weekday::Wed => "Miércoles",
        Weekday::Thu => "Jueves",
        Weekday::Fri => "Viernes",
        Weekday::Sat => "Sábado",
        Weekday::Sun => "Domingo",
    }
}

fn minutos(t: NaiveTime) -> i32 {
    (t.hour() * 60 + t.minute()) as i32
}

/// Un turno es válido si el comercio atiende ese día, el horario cae dentro de una franja,
/// entra completo antes del cierre y coincide con la grilla (inicio de franja + múltiplos del paso).
pub fn validar_slot(
    fecha: NaiveDate,
    hora: NaiveTime,
    paso_min: Option<i32>,
    franjas_del_dia: &[(NaiveTime, NaiveTime)],
    ahora: NaiveDateTime,
) -> Result<(), String> {
    if fecha.and_time(hora) <= ahora {
        return Err("El horario del turno ya pasó".into());
    }
    if franjas_del_dia.is_empty() {
        return Err(format!(
            "El comercio no atiende los {}",
            nombre_dia(fecha).to_lowercase()
        ));
    }
    let paso = paso_min.filter(|p| *p > 0).unwrap_or(PASO_POR_DEFECTO_MIN);
    let m = minutos(hora);
    let encaja = hora.second() == 0
        && franjas_del_dia.iter().any(|(ini, fin)| {
            let (i, f) = (minutos(*ini), minutos(*fin));
            m >= i && m + paso <= f && (m - i) % paso == 0
        });
    if encaja {
        Ok(())
    } else {
        Err(
            "Ese horario no está disponible: elegí uno de los horarios de atención del comercio"
                .into(),
        )
    }
}

/// RF-05: solo se puede cancelar con una anticipación mínima respecto del horario del turno.
pub fn puede_cancelar(
    ahora: NaiveDateTime,
    fecha: NaiveDate,
    hora: NaiveTime,
    anticipacion_min: i32,
) -> bool {
    fecha.and_time(hora) - ahora >= Duration::minutes(anticipacion_min as i64)
}

pub fn texto_anticipacion(min: i32) -> String {
    if min > 0 && min % 60 == 0 {
        let h = min / 60;
        format!("{h} hora{}", if h == 1 { "" } else { "s" })
    } else {
        format!("{min} minutos")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(h: u32, m: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, m, 0).unwrap()
    }
    fn d(y: i32, m: u32, dia: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, dia).unwrap()
    }
    // "Ahora" de las pruebas: lunes 2026-10-05 a las 08:00
    fn ahora() -> NaiveDateTime {
        d(2026, 10, 5).and_time(t(8, 0))
    }
    // Lunes de la semana siguiente
    fn lunes() -> NaiveDate {
        d(2026, 10, 12)
    }
    fn franjas() -> Vec<(NaiveTime, NaiveTime)> {
        vec![(t(9, 0), t(12, 0)), (t(16, 0), t(20, 0))]
    }

    #[test]
    fn nombra_los_dias_en_espaniol() {
        assert_eq!(nombre_dia(d(2026, 10, 5)), "Lunes");
        assert_eq!(nombre_dia(d(2026, 10, 7)), "Miércoles");
        assert_eq!(nombre_dia(d(2026, 10, 10)), "Sábado");
        assert_eq!(nombre_dia(d(2026, 10, 11)), "Domingo");
    }

    #[test]
    fn acepta_horarios_de_la_grilla() {
        for h in [t(9, 0), t(9, 30), t(11, 30), t(16, 0), t(19, 30)] {
            assert!(
                validar_slot(lunes(), h, Some(30), &franjas(), ahora()).is_ok(),
                "{h}"
            );
        }
    }

    #[test]
    fn rechaza_dias_sin_atencion() {
        let e = validar_slot(lunes(), t(10, 0), Some(30), &[], ahora()).unwrap_err();
        assert!(e.contains("no atiende"), "{e}");
    }

    #[test]
    fn rechaza_fuera_de_franja() {
        for h in [t(3, 0), t(8, 30), t(12, 0), t(13, 0), t(20, 0), t(23, 30)] {
            assert!(
                validar_slot(lunes(), h, Some(30), &franjas(), ahora()).is_err(),
                "{h}"
            );
        }
    }

    #[test]
    fn rechaza_horarios_desalineados() {
        assert!(validar_slot(lunes(), t(9, 7), Some(30), &franjas(), ahora()).is_err());
        assert!(validar_slot(lunes(), t(9, 15), Some(30), &franjas(), ahora()).is_err());
        assert!(validar_slot(lunes(), t(9, 15), Some(15), &franjas(), ahora()).is_ok());
    }

    #[test]
    fn el_turno_debe_terminar_antes_del_cierre() {
        // turnos de 45 min desde las 09:00: 09:00, 09:45, 10:30, 11:15 (termina 12:00); 11:30 no entra
        assert!(validar_slot(lunes(), t(11, 15), Some(45), &franjas(), ahora()).is_ok());
        assert!(validar_slot(lunes(), t(11, 30), Some(45), &franjas(), ahora()).is_err());
        // la grilla arranca en el inicio de CADA franja, no a las 00:00
        assert!(validar_slot(lunes(), t(16, 45), Some(45), &franjas(), ahora()).is_ok());
    }

    #[test]
    fn sin_duracion_usa_el_paso_por_defecto() {
        assert!(validar_slot(lunes(), t(9, 30), None, &franjas(), ahora()).is_ok());
        assert!(validar_slot(lunes(), t(9, 15), None, &franjas(), ahora()).is_err());
        assert!(validar_slot(lunes(), t(9, 30), Some(0), &franjas(), ahora()).is_ok());
    }

    #[test]
    fn rechaza_el_pasado() {
        let hoy = d(2026, 10, 5);
        assert!(validar_slot(hoy, t(9, 0), Some(30), &franjas(), ahora()).is_ok());
        assert!(validar_slot(hoy, t(8, 0), Some(30), &[(t(8, 0), t(12, 0))], ahora()).is_err());
        assert!(validar_slot(d(2020, 1, 6), t(9, 0), Some(30), &franjas(), ahora()).is_err());
    }

    #[test]
    fn rechaza_segundos_sueltos() {
        let raro = NaiveTime::from_hms_opt(9, 30, 15).unwrap();
        assert!(validar_slot(lunes(), raro, Some(30), &franjas(), ahora()).is_err());
    }

    #[test]
    fn cancelacion_exige_la_anticipacion() {
        let dia = d(2026, 10, 5);
        let hora = t(12, 0);
        assert!(puede_cancelar(ahora(), dia, hora, 120)); // faltan 4 h
        assert!(puede_cancelar(dia.and_time(t(10, 0)), dia, hora, 120)); // faltan justo 2 h
        assert!(!puede_cancelar(dia.and_time(t(10, 1)), dia, hora, 120)); // faltan 1 h 59 min
        assert!(!puede_cancelar(dia.and_time(t(13, 0)), dia, hora, 120)); // ya pasó
        assert!(puede_cancelar(dia.and_time(t(11, 59)), dia, hora, 0)); // sin anticipación configurada
    }

    #[test]
    fn textos_de_anticipacion() {
        assert_eq!(texto_anticipacion(120), "2 horas");
        assert_eq!(texto_anticipacion(60), "1 hora");
        assert_eq!(texto_anticipacion(90), "90 minutos");
    }
}
